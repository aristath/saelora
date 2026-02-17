use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::util::now_ms;
use super::DbError;

const DEFAULT_CONVERSATION_ID: i64 = 1;
const MODE_INSTANT: &str = "instant";
const MODE_HOURLY: &str = "hourly";
const MODE_DAILY: &str = "daily";
const MODE_WEEKLY: &str = "weekly";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationRecord {
    pub id: i64,
    pub title: String,
    pub mode: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub archived_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessageRecord {
    pub id: i64,
    pub conversation_id: i64,
    pub role: String,
    pub content: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct MessageEmbeddingRecord {
    pub message_id: i64,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStatementRecord {
    pub id: i64,
    pub text: String,
    pub belief_score: f64,
    pub salience: f64,
    pub first_seen_message_id: i64,
    pub last_seen_message_id: i64,
    pub evidence_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct UserDataStore {
    conn: Connection,
}

pub struct MemoryPatch<'a> {
    pub statement_id: i64,
    pub text: &'a str,
    pub delta: f64,
    pub salience: f64,
    pub confidence: f64,
    pub message_id: i64,
    pub note: &'a str,
    pub embedding: &'a [f32],
}

impl UserDataStore {
    pub(crate) fn open(path: &Path) -> Result<Self, DbError> {
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_millis(5000))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(st) = std::fs::metadata(path) {
                if st.is_file() {
                    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
                }
            }
        }

        init_schema(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn list_conversations(
        &self,
        include_archived: bool,
        limit: usize,
    ) -> Result<Vec<ConversationRecord>, DbError> {
        let lim = if limit == 0 {
            1000_i64
        } else {
            i64::try_from(limit).unwrap_or(i64::MAX)
        };
        let sql = if include_archived {
            r#"SELECT id, title, mode, created_at, updated_at, archived_at
               FROM conversations
               ORDER BY updated_at DESC, rowid DESC
               LIMIT ?1"#
        } else {
            r#"SELECT id, title, mode, created_at, updated_at, archived_at
               FROM conversations
               WHERE archived_at IS NULL
               ORDER BY updated_at DESC, rowid DESC
               LIMIT ?1"#
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![lim], |r| {
            Ok(ConversationRecord {
                id: r.get::<_, i64>(0)?,
                title: r.get::<_, String>(1)?,
                mode: normalize_mode(&r.get::<_, String>(2)?)
                    .unwrap_or_else(|_| MODE_INSTANT.to_string()),
                created_at: r.get::<_, i64>(3)?,
                updated_at: r.get::<_, i64>(4)?,
                archived_at: r.get::<_, Option<i64>>(5)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn create_conversation(&self, title: &str) -> Result<ConversationRecord, DbError> {
        self.create_conversation_with_mode(title, MODE_INSTANT)
    }

    pub fn create_conversation_with_mode(
        &self,
        title: &str,
        mode: &str,
    ) -> Result<ConversationRecord, DbError> {
        let now = now_ms();
        let title = normalize_title(title, "New thread");
        let mode = normalize_mode(mode)?;
        self.conn.execute(
            r#"INSERT INTO conversations(title, mode, created_at, updated_at, archived_at)
               VALUES (?1, ?2, ?3, ?3, NULL)"#,
            params![title.as_str(), mode.as_str(), now],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(ConversationRecord {
            id,
            title,
            mode,
            created_at: now,
            updated_at: now,
            archived_at: None,
        })
    }

    pub fn rename_conversation(&self, conversation_id: &str, title: &str) -> Result<(), DbError> {
        let cid = normalize_conversation_id(conversation_id)?;
        let next = normalize_title(title, "Untitled");
        let now = now_ms();
        let n = self.conn.execute(
            r#"UPDATE conversations
               SET title = ?1, updated_at = ?2
               WHERE id = ?3 AND archived_at IS NULL"#,
            params![next.as_str(), now, cid],
        )?;
        if n == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    pub fn archive_conversation(&self, conversation_id: &str) -> Result<(), DbError> {
        let cid = normalize_conversation_id(conversation_id)?;
        let now = now_ms();
        let n = self.conn.execute(
            r#"UPDATE conversations
               SET archived_at = ?1, updated_at = ?1
               WHERE id = ?2 AND archived_at IS NULL"#,
            params![now, cid],
        )?;
        if n == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    pub fn set_conversation_mode(
        &self,
        conversation_id: &str,
        mode: &str,
    ) -> Result<String, DbError> {
        let cid = normalize_conversation_id(conversation_id)?;
        let mode = normalize_mode(mode)?;
        let now = now_ms();
        let n = self.conn.execute(
            r#"UPDATE conversations
               SET mode = ?1, updated_at = ?2
               WHERE id = ?3 AND archived_at IS NULL"#,
            params![mode.as_str(), now, cid],
        )?;
        if n == 0 {
            return Err(DbError::NotFound);
        }
        Ok(mode)
    }

    pub fn conversation_mode(&self, conversation_id: &str) -> Result<String, DbError> {
        let cid = normalize_or_default_conversation_id(conversation_id)?;
        let mode_raw: String = self
            .conn
            .query_row(
                r#"SELECT mode
                   FROM conversations
                   WHERE id = ?1 AND archived_at IS NULL
                   LIMIT 1"#,
                params![cid],
                |r| r.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => DbError::NotFound,
                _ => DbError::Sql(e),
            })?;
        Ok(normalize_mode(&mode_raw).unwrap_or_else(|_| MODE_INSTANT.to_string()))
    }

    #[cfg(test)]
    pub fn append_user_message(&self, content: &str) -> Result<i64, DbError> {
        self.append_user_message_in(&DEFAULT_CONVERSATION_ID.to_string(), content)
    }

    #[cfg(test)]
    pub fn append_saelora_message(&self, content: &str) -> Result<i64, DbError> {
        self.append_saelora_message_in(&DEFAULT_CONVERSATION_ID.to_string(), content)
    }

    pub fn append_user_message_in(
        &self,
        conversation_id: &str,
        content: &str,
    ) -> Result<i64, DbError> {
        self.append_message(conversation_id, "user", content)
    }

    pub fn append_saelora_message_in(
        &self,
        conversation_id: &str,
        content: &str,
    ) -> Result<i64, DbError> {
        self.append_message(conversation_id, "saelora", content)
    }

    #[cfg(test)]
    pub fn list_messages(&self, limit: usize) -> Result<Vec<ChatMessageRecord>, DbError> {
        self.list_messages_in(&DEFAULT_CONVERSATION_ID.to_string(), limit)
    }

    pub fn list_messages_in(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> Result<Vec<ChatMessageRecord>, DbError> {
        let cid = normalize_or_default_conversation_id(conversation_id)?;
        let lim = if limit == 0 {
            1000_i64
        } else {
            i64::try_from(limit).unwrap_or(i64::MAX)
        };
        let mut stmt = self.conn.prepare(
            r#"SELECT id, conversation_id, role, content, created_at
               FROM messages
               WHERE conversation_id = ?1
               ORDER BY rowid ASC
               LIMIT ?2"#,
        )?;
        let rows = stmt.query_map(params![cid, lim], |r| {
            Ok(ChatMessageRecord {
                id: r.get::<_, i64>(0)?,
                conversation_id: r.get::<_, i64>(1)?,
                role: r.get::<_, String>(2)?,
                content: r.get::<_, String>(3)?,
                created_at: r.get::<_, i64>(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn message_by_id(&self, message_id: i64) -> Result<ChatMessageRecord, DbError> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, conversation_id, role, content, created_at
               FROM messages
               WHERE id = ?1
               LIMIT 1"#,
        )?;
        stmt.query_row(params![message_id], |r| {
            Ok(ChatMessageRecord {
                id: r.get::<_, i64>(0)?,
                conversation_id: r.get::<_, i64>(1)?,
                role: r.get::<_, String>(2)?,
                content: r.get::<_, String>(3)?,
                created_at: r.get::<_, i64>(4)?,
            })
        })
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => DbError::NotFound,
            _ => DbError::Sql(e),
        })
    }

    pub fn list_recent_messages_for_conversation(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> Result<Vec<ChatMessageRecord>, DbError> {
        let cid = normalize_or_default_conversation_id(conversation_id)?;
        let lim = if limit == 0 {
            100_i64
        } else {
            i64::try_from(limit).unwrap_or(i64::MAX)
        };
        let mut stmt = self.conn.prepare(
            r#"SELECT id, conversation_id, role, content, created_at
               FROM messages
               WHERE conversation_id = ?1
               ORDER BY id DESC
               LIMIT ?2"#,
        )?;
        let rows = stmt.query_map(params![cid, lim], |r| {
            Ok(ChatMessageRecord {
                id: r.get::<_, i64>(0)?,
                conversation_id: r.get::<_, i64>(1)?,
                role: r.get::<_, String>(2)?,
                content: r.get::<_, String>(3)?,
                created_at: r.get::<_, i64>(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        out.reverse();
        Ok(out)
    }

    pub fn upsert_message_embedding(
        &self,
        message_id: i64,
        model: &str,
        embedding: &[f32],
    ) -> Result<(), DbError> {
        if embedding.is_empty() {
            return Err(DbError::InvalidData("empty embedding".to_string()));
        }
        let msg = self.message_by_id(message_id)?;
        let blob = encode_embedding_blob(embedding);
        self.conn.execute(
            r#"INSERT INTO message_embeddings(
                   message_id, conversation_id, role, model, dims, embedding, created_at
               )
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
               ON CONFLICT(message_id) DO UPDATE SET
                   conversation_id = excluded.conversation_id,
                   role = excluded.role,
                   model = excluded.model,
                   dims = excluded.dims,
                   embedding = excluded.embedding,
                   created_at = excluded.created_at"#,
            params![
                message_id,
                msg.conversation_id,
                msg.role,
                model.trim(),
                i64::try_from(embedding.len()).unwrap_or(i64::MAX),
                blob,
                msg.created_at
            ],
        )?;
        Ok(())
    }

    pub fn message_embedding(
        &self,
        message_id: i64,
    ) -> Result<Option<MessageEmbeddingRecord>, DbError> {
        let mut stmt = self.conn.prepare(
            r#"SELECT message_id, dims, embedding
               FROM message_embeddings
               WHERE message_id = ?1
               LIMIT 1"#,
        )?;
        let out = stmt.query_row(params![message_id], |r| {
            let dims = r.get::<_, i64>(1)?;
            let blob = r.get::<_, Vec<u8>>(2)?;
            let embedding = decode_embedding_blob(&blob, dims as usize)?;
            Ok(MessageEmbeddingRecord {
                message_id: r.get::<_, i64>(0)?,
                embedding,
            })
        });
        match out {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Sql(e)),
        }
    }

    pub fn list_message_embeddings(
        &self,
        limit: usize,
    ) -> Result<Vec<MessageEmbeddingRecord>, DbError> {
        let lim = if limit == 0 {
            5_000_i64
        } else {
            i64::try_from(limit).unwrap_or(i64::MAX)
        };
        let mut stmt = self.conn.prepare(
            r#"SELECT message_id, dims, embedding
               FROM message_embeddings
               ORDER BY message_id DESC
               LIMIT ?1"#,
        )?;
        let rows = stmt.query_map(params![lim], |r| {
            let dims = r.get::<_, i64>(1)?;
            let blob = r.get::<_, Vec<u8>>(2)?;
            let embedding = decode_embedding_blob(&blob, dims as usize)?;
            Ok(MessageEmbeddingRecord {
                message_id: r.get::<_, i64>(0)?,
                embedding,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn list_memory_statements_with_embeddings(
        &self,
        limit: usize,
    ) -> Result<Vec<(MemoryStatementRecord, Vec<f32>)>, DbError> {
        let lim = if limit == 0 {
            500_i64
        } else {
            i64::try_from(limit).unwrap_or(i64::MAX)
        };
        let mut stmt = self.conn.prepare(
            r#"SELECT
                   id, text, belief_score, salience,
                   first_seen_message_id, last_seen_message_id,
                   evidence_count, dims, embedding, created_at, updated_at
               FROM memory_statements
               ORDER BY updated_at DESC, id DESC
               LIMIT ?1"#,
        )?;
        let rows = stmt.query_map(params![lim], |r| {
            let dims = r.get::<_, i64>(7)?;
            let blob = r.get::<_, Vec<u8>>(8)?;
            let embedding = decode_embedding_blob(&blob, dims as usize)?;
            let rec = MemoryStatementRecord {
                id: r.get::<_, i64>(0)?,
                text: r.get::<_, String>(1)?,
                belief_score: r.get::<_, f64>(2)?,
                salience: r.get::<_, f64>(3)?,
                first_seen_message_id: r.get::<_, i64>(4)?,
                last_seen_message_id: r.get::<_, i64>(5)?,
                evidence_count: r.get::<_, i64>(6)?,
                created_at: r.get::<_, i64>(9)?,
                updated_at: r.get::<_, i64>(10)?,
            };
            Ok((rec, embedding))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn apply_memory_patch(&self, patch: MemoryPatch<'_>) -> Result<i64, DbError> {
        let tx = self.conn.unchecked_transaction()?;
        let now = now_ms();
        let mut statement_id = patch.statement_id;
        if statement_id <= 0 {
            let text = normalize_memory_text(patch.text)?;
            let sal = clamp01(patch.salience);
            let score = clamp_score(patch.delta);
            tx.execute(
                r#"INSERT INTO memory_statements(
                       text, belief_score, salience, first_seen_message_id, last_seen_message_id,
                       evidence_count, dims, embedding, created_at, updated_at
                   ) VALUES (?1, ?2, ?3, ?4, ?4, 1, ?5, ?6, ?7, ?7)"#,
                params![
                    text,
                    score,
                    sal,
                    patch.message_id,
                    i64::try_from(patch.embedding.len()).unwrap_or(i64::MAX),
                    encode_embedding_blob(patch.embedding),
                    now
                ],
            )?;
            statement_id = tx.last_insert_rowid();
        } else {
            tx.execute(
                r#"UPDATE memory_statements
                   SET belief_score = MIN(50.0, MAX(-50.0, belief_score + ?1)),
                       salience = MIN(1.0, MAX(0.0, ?2)),
                       last_seen_message_id = ?3,
                       evidence_count = evidence_count + 1,
                       dims = ?4,
                       embedding = ?5,
                       updated_at = ?6
                   WHERE id = ?7"#,
                params![
                    patch.delta,
                    clamp01(patch.salience),
                    patch.message_id,
                    i64::try_from(patch.embedding.len()).unwrap_or(i64::MAX),
                    encode_embedding_blob(patch.embedding),
                    now,
                    statement_id,
                ],
            )?;
            if tx.changes() == 0 {
                return Err(DbError::NotFound);
            }
        }

        tx.execute(
            r#"INSERT INTO memory_statement_evidence(
                   statement_id, message_id, delta, confidence, note, created_at
               ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
            params![
                statement_id,
                patch.message_id,
                patch.delta,
                clamp01(patch.confidence),
                patch.note.trim(),
                now
            ],
        )?;
        tx.commit()?;
        Ok(statement_id)
    }

    pub fn upsert_memory_link(
        &self,
        statement_a: i64,
        statement_b: i64,
        kind: &str,
        strength: f64,
    ) -> Result<(), DbError> {
        if statement_a <= 0 || statement_b <= 0 || statement_a == statement_b {
            return Ok(());
        }
        let (a, b) = if statement_a < statement_b {
            (statement_a, statement_b)
        } else {
            (statement_b, statement_a)
        };
        let now = now_ms();
        self.conn.execute(
            r#"INSERT INTO memory_statement_links(
                   statement_a_id, statement_b_id, kind, strength, updated_at
               ) VALUES (?1, ?2, ?3, ?4, ?5)
               ON CONFLICT(statement_a_id, statement_b_id, kind) DO UPDATE SET
                   strength = excluded.strength,
                   updated_at = excluded.updated_at"#,
            params![a, b, kind.trim(), clamp01(strength), now],
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub fn count_memory_links(&self) -> Result<i64, DbError> {
        let n = self
            .conn
            .query_row("SELECT COUNT(*) FROM memory_statement_links", [], |r| {
                r.get(0)
            })?;
        Ok(n)
    }

    fn append_message(
        &self,
        conversation_id: &str,
        role: &str,
        content: &str,
    ) -> Result<i64, DbError> {
        let cid = normalize_or_default_conversation_id(conversation_id)?;
        let r = role.trim();
        if r != "user" && r != "saelora" {
            return Err(DbError::InvalidStatus);
        }
        let c = content.trim();
        if c.is_empty() {
            return Ok(0);
        }
        let now = now_ms();
        self.ensure_conversation(cid, now)?;
        self.conn.execute(
            r#"INSERT INTO messages(conversation_id, role, content, created_at)
               VALUES (?1, ?2, ?3, ?4)"#,
            params![cid, r, c, now],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            r#"UPDATE conversations SET updated_at = ?1 WHERE id = ?2"#,
            params![now, cid],
        )?;
        Ok(id)
    }

    fn ensure_conversation(&self, cid: i64, now: i64) -> Result<(), DbError> {
        let mut stmt = self
            .conn
            .prepare("SELECT archived_at FROM conversations WHERE id = ?1 LIMIT 1")?;
        let found = stmt.query_row(params![cid], |r| r.get::<_, Option<i64>>(0));
        match found {
            Ok(Some(_)) => return Err(DbError::NotFound),
            Ok(None) => {
                self.conn.execute(
                    "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
                    params![now, cid],
                )?;
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                let title = if cid == DEFAULT_CONVERSATION_ID {
                    "Main"
                } else {
                    "New thread"
                };
                self.conn.execute(
                    r#"INSERT INTO conversations(id, title, mode, created_at, updated_at, archived_at)
                       VALUES (?1, ?2, ?3, ?4, ?4, NULL)"#,
                    params![cid, title, MODE_INSTANT, now],
                )?;
            }
            Err(e) => return Err(DbError::Sql(e)),
        }
        Ok(())
    }
}

fn init_schema(conn: &mut Connection) -> Result<(), DbError> {
    let stmts = [
        r#"CREATE TABLE IF NOT EXISTS conversations (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               title TEXT NOT NULL DEFAULT 'New thread',
               mode TEXT NOT NULL DEFAULT 'instant' CHECK(mode IN ('weekly','daily','hourly','instant')),
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL,
               archived_at INTEGER
           );"#,
        r#"CREATE TABLE IF NOT EXISTS messages (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               conversation_id INTEGER NOT NULL,
               role TEXT NOT NULL CHECK (role IN ('user','saelora')),
               content TEXT NOT NULL,
               created_at INTEGER NOT NULL,
               FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
           );"#,
        r#"CREATE INDEX IF NOT EXISTS messages_conversation_created_idx
           ON messages(conversation_id, created_at, id);"#,
        r#"CREATE INDEX IF NOT EXISTS conversations_updated_idx
           ON conversations(updated_at DESC);"#,
        r#"CREATE INDEX IF NOT EXISTS conversations_archived_idx
           ON conversations(archived_at);"#,
        r#"CREATE TABLE IF NOT EXISTS message_embeddings (
               message_id INTEGER PRIMARY KEY,
               conversation_id INTEGER NOT NULL,
               role TEXT NOT NULL CHECK (role IN ('user','saelora')),
               model TEXT NOT NULL,
               dims INTEGER NOT NULL,
               embedding BLOB NOT NULL,
               created_at INTEGER NOT NULL,
               FOREIGN KEY(message_id) REFERENCES messages(id) ON DELETE CASCADE,
               FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
           );"#,
        r#"CREATE INDEX IF NOT EXISTS message_embeddings_conversation_idx
           ON message_embeddings(conversation_id, message_id DESC);"#,
        r#"CREATE TABLE IF NOT EXISTS memory_statements (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               text TEXT NOT NULL,
               belief_score REAL NOT NULL DEFAULT 0.0,
               salience REAL NOT NULL DEFAULT 0.5,
               first_seen_message_id INTEGER NOT NULL,
               last_seen_message_id INTEGER NOT NULL,
               evidence_count INTEGER NOT NULL DEFAULT 0,
               dims INTEGER NOT NULL,
               embedding BLOB NOT NULL,
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL,
               FOREIGN KEY(first_seen_message_id) REFERENCES messages(id) ON DELETE CASCADE,
               FOREIGN KEY(last_seen_message_id) REFERENCES messages(id) ON DELETE CASCADE
           );"#,
        r#"CREATE INDEX IF NOT EXISTS memory_statements_updated_idx
           ON memory_statements(updated_at DESC, id DESC);"#,
        r#"CREATE TABLE IF NOT EXISTS memory_statement_evidence (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               statement_id INTEGER NOT NULL,
               message_id INTEGER NOT NULL,
               delta REAL NOT NULL,
               confidence REAL NOT NULL,
               note TEXT NOT NULL DEFAULT '',
               created_at INTEGER NOT NULL,
               FOREIGN KEY(statement_id) REFERENCES memory_statements(id) ON DELETE CASCADE,
               FOREIGN KEY(message_id) REFERENCES messages(id) ON DELETE CASCADE
           );"#,
        r#"CREATE INDEX IF NOT EXISTS memory_evidence_statement_idx
           ON memory_statement_evidence(statement_id, id DESC);"#,
        r#"CREATE INDEX IF NOT EXISTS memory_evidence_message_idx
           ON memory_statement_evidence(message_id, id DESC);"#,
        r#"CREATE TABLE IF NOT EXISTS memory_statement_links (
               statement_a_id INTEGER NOT NULL,
               statement_b_id INTEGER NOT NULL,
               kind TEXT NOT NULL,
               strength REAL NOT NULL,
               updated_at INTEGER NOT NULL,
               PRIMARY KEY(statement_a_id, statement_b_id, kind),
               FOREIGN KEY(statement_a_id) REFERENCES memory_statements(id) ON DELETE CASCADE,
               FOREIGN KEY(statement_b_id) REFERENCES memory_statements(id) ON DELETE CASCADE
           );"#,
        r#"CREATE INDEX IF NOT EXISTS memory_links_strength_idx
           ON memory_statement_links(strength DESC, updated_at DESC);"#,
    ];
    for s in stmts {
        conn.execute_batch(s)?;
    }
    Ok(())
}

fn normalize_or_default_conversation_id(conversation_id: &str) -> Result<i64, DbError> {
    let cid = conversation_id.trim();
    if cid.is_empty() {
        return Ok(DEFAULT_CONVERSATION_ID);
    }
    normalize_conversation_id(cid)
}

fn normalize_conversation_id(conversation_id: &str) -> Result<i64, DbError> {
    let cid = conversation_id.trim();
    if cid.is_empty() {
        return Err(DbError::InvalidConversation);
    }
    if cid.len() > 20 {
        return Err(DbError::InvalidConversation);
    }
    if !cid.chars().all(|c| c.is_ascii_digit()) {
        return Err(DbError::InvalidConversation);
    }
    let n = cid
        .parse::<i64>()
        .map_err(|_| DbError::InvalidConversation)?;
    if n <= 0 {
        return Err(DbError::InvalidConversation);
    }
    Ok(n)
}

fn normalize_title(title: &str, fallback: &str) -> String {
    let t = title.trim();
    let next = if t.is_empty() { fallback } else { t };
    next.chars().take(120).collect()
}

fn normalize_mode(mode: &str) -> Result<String, DbError> {
    let m = mode.trim().to_ascii_lowercase();
    match m.as_str() {
        MODE_INSTANT | MODE_HOURLY | MODE_DAILY | MODE_WEEKLY => Ok(m),
        _ => Err(DbError::InvalidStatus),
    }
}

fn normalize_memory_text(text: &str) -> Result<String, DbError> {
    let t = text.trim();
    if t.is_empty() {
        return Err(DbError::InvalidData(
            "memory statement text is empty".to_string(),
        ));
    }
    let out: String = t.chars().take(400).collect();
    if out.trim().is_empty() {
        return Err(DbError::InvalidData(
            "memory statement text is empty".to_string(),
        ));
    }
    Ok(out)
}

fn clamp01(v: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    v.clamp(0.0, 1.0)
}

fn clamp_score(v: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    v.clamp(-50.0, 50.0)
}

fn encode_embedding_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

fn decode_embedding_blob(blob: &[u8], dims: usize) -> rusqlite::Result<Vec<f32>> {
    if dims == 0 {
        return Err(rusqlite::Error::InvalidParameterName(
            "dims must be > 0".to_string(),
        ));
    }
    if blob.len() != dims * 4 {
        return Err(rusqlite::Error::InvalidParameterName(
            "embedding blob length mismatch".to_string(),
        ));
    }
    let mut out = Vec::with_capacity(dims);
    for c in blob.chunks_exact(4) {
        out.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
    }
    Ok(out)
}
