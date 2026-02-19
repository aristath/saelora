use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::util::now_ms;
use super::DbError;

const DEFAULT_CONVERSATION_ID: i64 = 1;
const MODE_INSTANT: &str = "instant";
const MODE_HOURLY: &str = "hourly";
const MODE_DAILY: &str = "daily";
const MODE_WEEKLY: &str = "weekly";
const MEMORY_INGEST_MAX_ATTEMPTS: i64 = 10;

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
    pub model: Option<String>,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct ConversationSummaryRecord {
    pub id: i64,
    pub conversation_id: i64,
    pub start_message_id: i64,
    pub end_message_id: i64,
    pub next_start_message_id: i64,
    pub message_count: i64,
    pub token_estimate: i64,
    pub model: String,
    pub content: String,
}

pub struct UserDataStore {
    conn: Connection,
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

    pub fn list_recent_messages_for_conversation_up_to(
        &self,
        conversation_id: &str,
        limit: usize,
        max_message_id: Option<i64>,
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
                 AND (?2 IS NULL OR id <= ?2)
               ORDER BY id DESC
               LIMIT ?3"#,
        )?;
        let rows = stmt.query_map(params![cid, max_message_id, lim], |r| {
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

    pub fn list_messages_for_conversation_from(
        &self,
        conversation_id: &str,
        start_message_id: i64,
        limit: usize,
    ) -> Result<Vec<ChatMessageRecord>, DbError> {
        let cid = normalize_or_default_conversation_id(conversation_id)?;
        let start_id = if start_message_id <= 0 {
            1_i64
        } else {
            start_message_id
        };
        let lim = if limit == 0 {
            5000_i64
        } else {
            i64::try_from(limit).unwrap_or(i64::MAX)
        };
        let mut stmt = self.conn.prepare(
            r#"SELECT id, conversation_id, role, content, created_at
               FROM messages
               WHERE conversation_id = ?1
                 AND id >= ?2
               ORDER BY id ASC
               LIMIT ?3"#,
        )?;
        let rows = stmt.query_map(params![cid, start_id, lim], |r| {
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

    pub fn upsert_message_embedding(
        &self,
        message_id: i64,
        model: &str,
        embedding: &[f32],
    ) -> Result<(), DbError> {
        if embedding.is_empty() {
            return Err(DbError::InvalidData("empty embedding".to_string()));
        }
        let model = normalize_model(model)?;
        let _msg = self.message_by_id(message_id)?;
        let blob = encode_embedding_blob(embedding);
        let now = now_ms();
        self.conn.execute(
            r#"UPDATE messages
               SET embedding_model = ?2,
                   embedding_dims = ?3,
                   embedding = ?4,
                   embed_status = 'done',
                   embed_last_error = '',
                   embed_updated_at = ?5,
                   embed_finished_at = ?5
               WHERE id = ?1"#,
            params![
                message_id,
                model.as_str(),
                i64::try_from(embedding.len()).unwrap_or(i64::MAX),
                blob,
                now
            ],
        )?;
        Ok(())
    }

    pub fn message_embedding(
        &self,
        message_id: i64,
    ) -> Result<Option<MessageEmbeddingRecord>, DbError> {
        let mut stmt = self.conn.prepare(
            r#"SELECT embedding_model, embedding_dims, embedding
               FROM messages
               WHERE id = ?1
                 AND embedding IS NOT NULL
                 AND embedding_dims IS NOT NULL
               LIMIT 1"#,
        )?;
        let out = stmt.query_row(params![message_id], |r| {
            let model = r.get::<_, Option<String>>(0)?;
            let dims = r.get::<_, i64>(1)?;
            let blob = r.get::<_, Vec<u8>>(2)?;
            let embedding = decode_embedding_blob(&blob, dims as usize)?;
            Ok(MessageEmbeddingRecord { model, embedding })
        });
        match out {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Sql(e)),
        }
    }

    pub fn latest_conversation_summary(
        &self,
        conversation_id: i64,
    ) -> Result<Option<ConversationSummaryRecord>, DbError> {
        if conversation_id <= 0 {
            return Ok(None);
        }
        let mut stmt = self.conn.prepare(
            r#"SELECT
                   id, conversation_id, start_message_id, end_message_id,
                   next_start_message_id, message_count, token_estimate, model, content
               FROM conversation_summaries
               WHERE conversation_id = ?1
               ORDER BY end_message_id DESC, id DESC
               LIMIT 1"#,
        )?;
        let row = stmt
            .query_row(params![conversation_id], |r| {
                Ok(ConversationSummaryRecord {
                    id: r.get::<_, i64>(0)?,
                    conversation_id: r.get::<_, i64>(1)?,
                    start_message_id: r.get::<_, i64>(2)?,
                    end_message_id: r.get::<_, i64>(3)?,
                    next_start_message_id: r.get::<_, i64>(4)?,
                    message_count: r.get::<_, i64>(5)?,
                    token_estimate: r.get::<_, i64>(6)?,
                    model: r.get::<_, String>(7)?,
                    content: r.get::<_, String>(8)?,
                })
            })
            .optional()?;
        Ok(row)
    }

    pub fn create_conversation_summary(
        &self,
        conversation_id: i64,
        start_message_id: i64,
        end_message_id: i64,
        next_start_message_id: i64,
        message_count: i64,
        token_estimate: i64,
        model: &str,
        content: &str,
    ) -> Result<i64, DbError> {
        if conversation_id <= 0
            || start_message_id <= 0
            || end_message_id < start_message_id
            || next_start_message_id <= start_message_id
            || message_count <= 0
            || token_estimate <= 0
        {
            return Err(DbError::InvalidData(
                "invalid conversation summary bounds".to_string(),
            ));
        }
        let model = normalize_model(model)?;
        let content = normalize_conversation_summary_content(content)?;
        let now = now_ms();

        let tx = self.conn.unchecked_transaction()?;
        let conv_exists = tx
            .query_row(
                "SELECT 1 FROM conversations WHERE id = ?1 LIMIT 1",
                params![conversation_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !conv_exists {
            return Err(DbError::NotFound);
        }
        let changed = tx.execute(
            r#"INSERT OR IGNORE INTO conversation_summaries(
                   conversation_id, start_message_id, end_message_id, next_start_message_id,
                   message_count, token_estimate, model, content, created_at, updated_at
               ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)"#,
            params![
                conversation_id,
                start_message_id,
                end_message_id,
                next_start_message_id,
                message_count,
                token_estimate,
                model.as_str(),
                content.as_str(),
                now
            ],
        )?;
        let summary_id = if changed > 0 {
            tx.last_insert_rowid()
        } else {
            let id = tx.query_row(
                r#"SELECT id
                   FROM conversation_summaries
                   WHERE conversation_id = ?1
                     AND start_message_id = ?2
                     AND end_message_id = ?3
                   LIMIT 1"#,
                params![conversation_id, start_message_id, end_message_id],
                |r| r.get::<_, i64>(0),
            )?;
            tx.execute(
                r#"UPDATE conversation_summaries
                   SET next_start_message_id = ?2,
                       message_count = ?3,
                       token_estimate = ?4,
                       model = ?5,
                       content = ?6,
                       updated_at = ?7
                   WHERE id = ?1"#,
                params![
                    id,
                    next_start_message_id,
                    message_count,
                    token_estimate,
                    model.as_str(),
                    content.as_str(),
                    now
                ],
            )?;
            id
        };
        tx.commit()?;
        Ok(summary_id)
    }

    pub fn list_conversation_summaries_without_embedding(
        &self,
        limit: usize,
    ) -> Result<Vec<(i64, String)>, DbError> {
        let lim = if limit == 0 {
            100_i64
        } else {
            i64::try_from(limit).unwrap_or(i64::MAX)
        };
        let mut stmt = self.conn.prepare(
            r#"SELECT id, content
               FROM conversation_summaries
               WHERE embedding IS NULL
                 AND length(trim(content)) > 0
               ORDER BY id ASC
               LIMIT ?1"#,
        )?;
        let rows = stmt.query_map(params![lim], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn upsert_conversation_summary_embedding(
        &self,
        summary_id: i64,
        model: &str,
        embedding: &[f32],
    ) -> Result<(), DbError> {
        if summary_id <= 0 {
            return Err(DbError::InvalidData("invalid summary id".to_string()));
        }
        if embedding.is_empty() {
            return Err(DbError::InvalidData(
                "conversation summary embedding is empty".to_string(),
            ));
        }
        let model = normalize_model(model)?;
        let dims = i64::try_from(embedding.len()).unwrap_or(i64::MAX);
        let now = now_ms();
        let changed = self.conn.execute(
            r#"UPDATE conversation_summaries
               SET embedding_model = ?2,
                   embedding_dims = ?3,
                   embedding = ?4,
                   updated_at = CASE
                       WHEN updated_at >= ?5 THEN updated_at
                       ELSE ?5
                   END
               WHERE id = ?1"#,
            params![
                summary_id,
                model.as_str(),
                dims,
                encode_embedding_blob(embedding),
                now
            ],
        )?;
        if changed == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    pub fn nearest_conversation_summaries_by_embedding(
        &self,
        model: &str,
        query_embedding: &[f32],
        conversation_id: i64,
        top_k: usize,
        min_similarity: f64,
    ) -> Result<Vec<(ConversationSummaryRecord, f64)>, DbError> {
        if query_embedding.is_empty() || conversation_id <= 0 {
            return Ok(Vec::new());
        }
        let model = normalize_model(model)?;
        let dims = i64::try_from(query_embedding.len()).unwrap_or(i64::MAX);
        let lim = i64::try_from(top_k.max(1).saturating_mul(80))
            .unwrap_or(i64::MAX)
            .max(400);
        let mut stmt = self.conn.prepare(
            r#"SELECT
                   id, conversation_id, start_message_id, end_message_id,
                   next_start_message_id, message_count, token_estimate, model, content, embedding
               FROM conversation_summaries
               WHERE conversation_id = ?1
                 AND embedding_model = ?2
                 AND embedding_dims = ?3
                 AND embedding IS NOT NULL
               ORDER BY end_message_id DESC, id DESC
               LIMIT ?4"#,
        )?;
        let rows = stmt.query_map(params![conversation_id, model.as_str(), dims, lim], |r| {
            let blob = r.get::<_, Vec<u8>>(9)?;
            let emb = decode_embedding_blob(&blob, query_embedding.len())?;
            Ok((
                ConversationSummaryRecord {
                    id: r.get::<_, i64>(0)?,
                    conversation_id: r.get::<_, i64>(1)?,
                    start_message_id: r.get::<_, i64>(2)?,
                    end_message_id: r.get::<_, i64>(3)?,
                    next_start_message_id: r.get::<_, i64>(4)?,
                    message_count: r.get::<_, i64>(5)?,
                    token_estimate: r.get::<_, i64>(6)?,
                    model: r.get::<_, String>(7)?,
                    content: r.get::<_, String>(8)?,
                },
                emb,
            ))
        })?;
        let mut scored = Vec::new();
        for row in rows {
            let (summary, emb) = row?;
            let sim = cosine_similarity(query_embedding, &emb);
            if sim >= min_similarity {
                scored.push((summary, sim));
            }
        }
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(top_k.max(1));
        Ok(scored)
    }

    pub fn nearest_messages_by_embedding(
        &self,
        model: &str,
        query_embedding: &[f32],
        exclude_message_id: i64,
        conversation_id: Option<i64>,
        max_message_id: Option<i64>,
        top_k: usize,
        min_similarity: f64,
    ) -> Result<Vec<ChatMessageRecord>, DbError> {
        if query_embedding.is_empty() {
            return Ok(Vec::new());
        }
        let model = normalize_model(model)?;
        let dims = i64::try_from(query_embedding.len()).unwrap_or(i64::MAX);
        let lim = i64::try_from(top_k.max(1).saturating_mul(80))
            .unwrap_or(i64::MAX)
            .max(400);
        let mut stmt = self.conn.prepare(
            r#"SELECT
                   m.id, m.conversation_id, m.role, m.content, m.created_at, m.embedding
               FROM messages AS m
               WHERE m.embedding_model = ?1
                 AND m.embedding_dims = ?2
                 AND m.embedding IS NOT NULL
                 AND m.id != ?3
                 AND (?4 IS NULL OR m.conversation_id = ?4)
                 AND (?5 IS NULL OR m.id <= ?5)
               ORDER BY m.id DESC
               LIMIT ?6"#,
        )?;
        let rows = stmt.query_map(
            params![
                model.as_str(),
                dims,
                exclude_message_id,
                conversation_id,
                max_message_id,
                lim
            ],
            |r| {
                let blob = r.get::<_, Vec<u8>>(5)?;
                let emb = decode_embedding_blob(&blob, query_embedding.len())?;
                Ok((
                    ChatMessageRecord {
                        id: r.get::<_, i64>(0)?,
                        conversation_id: r.get::<_, i64>(1)?,
                        role: r.get::<_, String>(2)?,
                        content: r.get::<_, String>(3)?,
                        created_at: r.get::<_, i64>(4)?,
                    },
                    emb,
                ))
            },
        )?;
        let mut scored = Vec::new();
        for row in rows {
            let (msg, emb) = row?;
            let sim = cosine_similarity(query_embedding, &emb);
            if sim >= min_similarity {
                scored.push((msg, sim));
            }
        }
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(top_k.max(1));
        Ok(scored.into_iter().map(|(m, _)| m).collect())
    }

    pub fn queue_message_ingest(&self, message_id: i64) -> Result<(), DbError> {
        if message_id <= 0 {
            return Ok(());
        }
        let now = now_ms();
        let mut stmt = self
            .conn
            .prepare("SELECT embed_status FROM messages WHERE id = ?1 LIMIT 1")?;
        let status = stmt.query_row(params![message_id], |r| r.get::<_, Option<String>>(0));
        match status {
            Ok(Some(st)) if st == "done" || st == "running" => Ok(()),
            Ok(_) => {
                self.conn.execute(
                    r#"UPDATE messages
                       SET embed_status = 'queued',
                           embed_attempts = COALESCE(embed_attempts, 0),
                           embed_updated_at = ?1,
                           embed_finished_at = NULL,
                           embed_started_at = NULL,
                           embed_last_error = ''
                       WHERE id = ?2"#,
                    params![now, message_id],
                )?;
                Ok(())
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(()),
            Err(e) => Err(DbError::Sql(e)),
        }
    }

    pub fn claim_message_ingest(&self, message_id: i64) -> Result<bool, DbError> {
        if message_id <= 0 {
            return Ok(false);
        }
        let now = now_ms();
        let mut stmt = self
            .conn
            .prepare("SELECT embed_status FROM messages WHERE id = ?1 LIMIT 1")?;
        let status = stmt.query_row(params![message_id], |r| r.get::<_, Option<String>>(0));
        match status {
            Ok(Some(st)) if st == "running" => Ok(false),
            Ok(Some(st)) if st == "done" => {
                let has_embedding = self.conn.query_row(
                    "SELECT 1 FROM messages WHERE id = ?1 AND embedding IS NOT NULL LIMIT 1",
                    params![message_id],
                    |_| Ok(()),
                );
                let has_embedding = match has_embedding {
                    Ok(()) => true,
                    Err(rusqlite::Error::QueryReturnedNoRows) => false,
                    Err(e) => return Err(DbError::Sql(e)),
                };
                if has_embedding {
                    return Ok(false);
                }
                let changed = self.conn.execute(
                    r#"UPDATE messages
                       SET embed_status = 'running',
                           embed_attempts = COALESCE(embed_attempts, 0) + 1,
                           embed_updated_at = ?1,
                           embed_started_at = ?1,
                           embed_finished_at = NULL,
                           embed_last_error = ''
                       WHERE id = ?2"#,
                    params![now, message_id],
                )?;
                Ok(changed > 0)
            }
            Ok(_) => {
                let changed = self.conn.execute(
                    r#"UPDATE messages
                       SET embed_status = 'running',
                           embed_attempts = COALESCE(embed_attempts, 0) + 1,
                           embed_updated_at = ?1,
                           embed_started_at = ?1,
                           embed_finished_at = NULL,
                           embed_last_error = ''
                       WHERE id = ?2"#,
                    params![now, message_id],
                )?;
                Ok(changed > 0)
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(e) => Err(DbError::Sql(e)),
        }
    }

    pub fn complete_message_ingest(&self, message_id: i64) -> Result<(), DbError> {
        if message_id <= 0 {
            return Ok(());
        }
        let now = now_ms();
        self.conn.execute(
            r#"UPDATE messages
               SET embed_status = 'done',
                   embed_updated_at = ?1,
                   embed_finished_at = ?1,
                   embed_last_error = ''
               WHERE id = ?2"#,
            params![now, message_id],
        )?;
        Ok(())
    }

    pub fn fail_message_ingest(&self, message_id: i64, error: &str) -> Result<(), DbError> {
        if message_id <= 0 {
            return Ok(());
        }
        let now = now_ms();
        let mut err = error.trim().to_string();
        if err.len() > 1024 {
            err.truncate(1024);
        }
        self.conn.execute(
            r#"UPDATE messages
               SET embed_status = 'failed',
                   embed_updated_at = ?1,
                   embed_finished_at = ?1,
                   embed_last_error = ?2
               WHERE id = ?3"#,
            params![now, err.as_str(), message_id],
        )?;
        Ok(())
    }

    pub fn recover_running_ingest_to_queued(&self) -> Result<usize, DbError> {
        let now = now_ms();
        let changed = self.conn.execute(
            r#"UPDATE messages
               SET embed_status = 'queued',
                   embed_updated_at = ?1,
                   embed_started_at = NULL,
                   embed_finished_at = NULL,
                   embed_last_error = CASE
                       WHEN trim(COALESCE(embed_last_error, '')) = '' THEN 'recovered after restart'
                       ELSE embed_last_error
                   END
               WHERE embed_status = 'running'"#,
            params![now],
        )?;
        Ok(changed)
    }

    pub fn next_backfill_message(&self) -> Result<Option<(i64, i64)>, DbError> {
        let mut stmt = self.conn.prepare(
            r#"SELECT m.id, m.conversation_id
               FROM messages AS m
               WHERE m.role = 'user'
                 AND length(trim(m.content)) > 0
                 AND (
                   m.embed_status IS NULL
                   OR m.embed_status = 'queued'
                   OR (m.embed_status = 'failed' AND COALESCE(m.embed_attempts, 0) < ?1)
                   OR (m.embed_status = 'done' AND m.embedding IS NULL)
                 )
               ORDER BY m.id ASC
               LIMIT 1"#,
        )?;
        let row = stmt.query_row(params![MEMORY_INGEST_MAX_ATTEMPTS], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        });
        match row {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Sql(e)),
        }
    }

    pub fn list_backfill_messages_without_embedding_from(
        &self,
        start_message_id: i64,
        limit: usize,
    ) -> Result<Vec<ChatMessageRecord>, DbError> {
        if start_message_id <= 0 {
            return Ok(Vec::new());
        }
        let lim = if limit == 0 {
            12_i64
        } else {
            i64::try_from(limit).unwrap_or(i64::MAX)
        };
        let mut stmt = self.conn.prepare(
            r#"SELECT m.id, m.conversation_id, m.role, m.content, m.created_at
               FROM messages AS m
               WHERE m.id >= ?1
                 AND m.role = 'user'
                 AND length(trim(m.content)) > 0
                 AND m.embedding IS NULL
                AND (
                   m.embed_status IS NULL
                   OR m.embed_status = 'queued'
                   OR (m.embed_status = 'failed' AND COALESCE(m.embed_attempts, 0) < ?2)
                   OR m.embed_status = 'done'
                 )
               ORDER BY m.id ASC
               LIMIT ?3"#,
        )?;
        let rows = stmt.query_map(
            params![start_message_id, MEMORY_INGEST_MAX_ATTEMPTS, lim],
            |r| {
                Ok(ChatMessageRecord {
                    id: r.get::<_, i64>(0)?,
                    conversation_id: r.get::<_, i64>(1)?,
                    role: r.get::<_, String>(2)?,
                    content: r.get::<_, String>(3)?,
                    created_at: r.get::<_, i64>(4)?,
                })
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
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
               embedding_model TEXT,
               embedding_dims INTEGER,
               embedding BLOB,
               embed_status TEXT CHECK (embed_status IN ('queued','running','done','failed')),
               embed_attempts INTEGER NOT NULL DEFAULT 0,
               embed_last_error TEXT NOT NULL DEFAULT '',
               embed_updated_at INTEGER,
               embed_started_at INTEGER,
               embed_finished_at INTEGER,
               FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
           );"#,
        r#"CREATE INDEX IF NOT EXISTS messages_conversation_created_idx
           ON messages(conversation_id, created_at, id);"#,
        r#"CREATE INDEX IF NOT EXISTS conversations_updated_idx
           ON conversations(updated_at DESC);"#,
        r#"CREATE INDEX IF NOT EXISTS conversations_archived_idx
           ON conversations(archived_at);"#,
        r#"CREATE TABLE IF NOT EXISTS conversation_summaries (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               conversation_id INTEGER NOT NULL,
               start_message_id INTEGER NOT NULL,
               end_message_id INTEGER NOT NULL,
               next_start_message_id INTEGER NOT NULL,
               message_count INTEGER NOT NULL,
               token_estimate INTEGER NOT NULL,
               model TEXT NOT NULL,
               content TEXT NOT NULL,
               embedding_model TEXT,
               embedding_dims INTEGER,
               embedding BLOB,
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL,
               UNIQUE(conversation_id, start_message_id, end_message_id),
               FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
           );"#,
        r#"CREATE INDEX IF NOT EXISTS conversation_summaries_conv_end_idx
           ON conversation_summaries(conversation_id, end_message_id DESC, id DESC);"#,
        r#"CREATE INDEX IF NOT EXISTS conversation_summaries_embedding_lookup_idx
           ON conversation_summaries(conversation_id, embedding_model, embedding_dims, id DESC);"#,
    ];
    for s in stmts {
        conn.execute_batch(s)?;
    }
    ensure_message_inline_columns(conn)?;
    conn.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS messages_embedding_lookup_idx
          ON messages(embedding_model, embedding_dims, id DESC);
        CREATE INDEX IF NOT EXISTS messages_embed_status_idx
          ON messages(embed_status, embed_attempts, embed_updated_at, id);
        "#,
    )?;
    migrate_legacy_embedding_tables(conn)?;
    conn.execute_batch(
        r#"
        DROP TABLE IF EXISTS thread_tags;
        DROP TABLE IF EXISTS tag_pipeline_state;
        DROP TABLE IF EXISTS thread_summary_sources;
        DROP TABLE IF EXISTS thread_summaries;
        DROP TABLE IF EXISTS semantic_thread_edges;
        DROP TABLE IF EXISTS semantic_thread_memberships;
        DROP TABLE IF EXISTS semantic_pending_anchors;
        DROP TABLE IF EXISTS semantic_threads;
        DROP TABLE IF EXISTS memory_statement_links;
        DROP TABLE IF EXISTS memory_statement_evidence;
        DROP TABLE IF EXISTS memory_statements;
        DROP TABLE IF EXISTS message_embeddings;
        DROP TABLE IF EXISTS message_memory_ingest;
        "#,
    )?;
    Ok(())
}

fn ensure_message_inline_columns(conn: &Connection) -> Result<(), DbError> {
    let mut cols = std::collections::HashSet::new();
    let mut stmt = conn.prepare("PRAGMA table_info(messages)")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
    for row in rows {
        cols.insert(row?);
    }

    let mut add_column = |name: &str, ddl: &str| -> Result<(), DbError> {
        if cols.contains(name) {
            return Ok(());
        }
        conn.execute_batch(ddl)?;
        cols.insert(name.to_string());
        Ok(())
    };

    add_column(
        "embedding_model",
        "ALTER TABLE messages ADD COLUMN embedding_model TEXT;",
    )?;
    add_column(
        "embedding_dims",
        "ALTER TABLE messages ADD COLUMN embedding_dims INTEGER;",
    )?;
    add_column(
        "embedding",
        "ALTER TABLE messages ADD COLUMN embedding BLOB;",
    )?;
    add_column(
        "embed_status",
        "ALTER TABLE messages ADD COLUMN embed_status TEXT CHECK (embed_status IN ('queued','running','done','failed'));",
    )?;
    add_column(
        "embed_attempts",
        "ALTER TABLE messages ADD COLUMN embed_attempts INTEGER NOT NULL DEFAULT 0;",
    )?;
    add_column(
        "embed_last_error",
        "ALTER TABLE messages ADD COLUMN embed_last_error TEXT NOT NULL DEFAULT '';",
    )?;
    add_column(
        "embed_updated_at",
        "ALTER TABLE messages ADD COLUMN embed_updated_at INTEGER;",
    )?;
    add_column(
        "embed_started_at",
        "ALTER TABLE messages ADD COLUMN embed_started_at INTEGER;",
    )?;
    add_column(
        "embed_finished_at",
        "ALTER TABLE messages ADD COLUMN embed_finished_at INTEGER;",
    )?;
    Ok(())
}

fn migrate_legacy_embedding_tables(conn: &Connection) -> Result<(), DbError> {
    if table_exists(conn, "message_embeddings")? {
        conn.execute_batch(
            r#"
            UPDATE messages
            SET embedding_model = (
                    SELECT e.model
                    FROM message_embeddings AS e
                    WHERE e.message_id = messages.id
                    LIMIT 1
                ),
                embedding_dims = (
                    SELECT e.dims
                    FROM message_embeddings AS e
                    WHERE e.message_id = messages.id
                    LIMIT 1
                ),
                embedding = (
                    SELECT e.embedding
                    FROM message_embeddings AS e
                    WHERE e.message_id = messages.id
                    LIMIT 1
                ),
                embed_status = COALESCE(embed_status, 'done'),
                embed_attempts = CASE
                    WHEN COALESCE(embed_attempts, 0) = 0 THEN 1
                    ELSE embed_attempts
                END,
                embed_last_error = '',
                embed_updated_at = COALESCE(embed_updated_at, created_at),
                embed_finished_at = COALESCE(embed_finished_at, created_at)
            WHERE EXISTS (
                SELECT 1
                FROM message_embeddings AS e
                WHERE e.message_id = messages.id
            );
            "#,
        )?;
    }

    if table_exists(conn, "message_memory_ingest")? {
        conn.execute_batch(
            r#"
            UPDATE messages
            SET embed_status = COALESCE(
                    embed_status,
                    (
                        SELECT s.status
                        FROM message_memory_ingest AS s
                        WHERE s.message_id = messages.id
                        LIMIT 1
                    )
                ),
                embed_attempts = CASE
                    WHEN COALESCE(embed_attempts, 0) = 0 THEN COALESCE(
                        (
                            SELECT s.attempts
                            FROM message_memory_ingest AS s
                            WHERE s.message_id = messages.id
                            LIMIT 1
                        ),
                        0
                    )
                    ELSE embed_attempts
                END,
                embed_last_error = CASE
                    WHEN trim(COALESCE(embed_last_error, '')) = '' THEN COALESCE(
                        (
                            SELECT s.last_error
                            FROM message_memory_ingest AS s
                            WHERE s.message_id = messages.id
                            LIMIT 1
                        ),
                        ''
                    )
                    ELSE embed_last_error
                END,
                embed_updated_at = COALESCE(
                    embed_updated_at,
                    (
                        SELECT s.updated_at
                        FROM message_memory_ingest AS s
                        WHERE s.message_id = messages.id
                        LIMIT 1
                    )
                ),
                embed_started_at = COALESCE(
                    embed_started_at,
                    (
                        SELECT s.started_at
                        FROM message_memory_ingest AS s
                        WHERE s.message_id = messages.id
                        LIMIT 1
                    )
                ),
                embed_finished_at = COALESCE(
                    embed_finished_at,
                    (
                        SELECT s.finished_at
                        FROM message_memory_ingest AS s
                        WHERE s.message_id = messages.id
                        LIMIT 1
                    )
                )
            WHERE EXISTS (
                SELECT 1
                FROM message_memory_ingest AS s
                WHERE s.message_id = messages.id
            );
            "#,
        )?;
    }

    conn.execute_batch(
        r#"
        UPDATE messages
        SET embed_status = 'done',
            embed_attempts = CASE
                WHEN COALESCE(embed_attempts, 0) = 0 THEN 1
                ELSE embed_attempts
            END,
            embed_last_error = '',
            embed_updated_at = COALESCE(embed_updated_at, created_at),
            embed_finished_at = COALESCE(embed_finished_at, created_at)
        WHERE embedding IS NOT NULL
          AND (embed_status IS NULL OR trim(embed_status) = '');

        UPDATE messages
        SET embed_attempts = COALESCE(embed_attempts, 0),
            embed_last_error = COALESCE(embed_last_error, '')
        WHERE 1 = 1;
        "#,
    )?;

    Ok(())
}

fn table_exists(conn: &Connection, table_name: &str) -> Result<bool, DbError> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
            params![table_name],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    Ok(exists)
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

fn normalize_model(model: &str) -> Result<String, DbError> {
    let m = model.trim();
    if m.is_empty() {
        return Err(DbError::InvalidData("embedding model is empty".to_string()));
    }
    let out: String = m.chars().take(240).collect();
    if out.trim().is_empty() {
        return Err(DbError::InvalidData("embedding model is empty".to_string()));
    }
    Ok(out)
}

fn normalize_conversation_summary_content(content: &str) -> Result<String, DbError> {
    let c = content.trim();
    if c.is_empty() {
        return Err(DbError::InvalidData("summary content is empty".to_string()));
    }
    let out: String = c.chars().take(12_000).collect();
    if out.trim().is_empty() {
        return Err(DbError::InvalidData("summary content is empty".to_string()));
    }
    Ok(out)
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

fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0_f64;
    let mut na = 0.0_f64;
    let mut nb = 0.0_f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let xf = f64::from(*x);
        let yf = f64::from(*y);
        dot += xf * yf;
        na += xf * xf;
        nb += yf * yf;
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}
