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

#[derive(Debug, Clone)]
pub struct SemanticThreadMembershipRecord {
    pub thread_id: i64,
    pub score: f64,
    pub is_primary: bool,
}

#[derive(Debug, Clone)]
pub struct SemanticThreadRecord {
    pub id: i64,
    pub message_count: i64,
    pub centroid: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct SemanticPendingAnchorRecord {
    pub message_id: i64,
    pub embedding: Vec<f32>,
}

pub struct UserDataStore {
    conn: Connection,
}

pub struct MemoryPatch<'a> {
    pub statement_id: i64,
    pub model: &'a str,
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

    pub fn create_semantic_thread(&self, model: &str, centroid: &[f32]) -> Result<i64, DbError> {
        if centroid.is_empty() {
            return Err(DbError::InvalidData(
                "semantic thread centroid is empty".to_string(),
            ));
        }
        let model = normalize_model(model)?;
        let dims = i64::try_from(centroid.len()).unwrap_or(i64::MAX);
        let now = now_ms();
        self.conn.execute(
            r#"INSERT INTO semantic_threads(
                   model, dims, centroid, message_count, created_at, updated_at
               ) VALUES (?1, ?2, ?3, 0, ?4, ?4)"#,
            params![model.as_str(), dims, encode_embedding_blob(centroid), now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn nearest_semantic_threads(
        &self,
        model: &str,
        query_embedding: &[f32],
        top_k: usize,
        min_similarity: f64,
    ) -> Result<Vec<(i64, f64)>, DbError> {
        if query_embedding.is_empty() || top_k == 0 {
            return Ok(Vec::new());
        }
        let model = normalize_model(model)?;
        let dims = i64::try_from(query_embedding.len()).unwrap_or(i64::MAX);
        let lim = i64::try_from(top_k.saturating_mul(80))
            .unwrap_or(i64::MAX)
            .max(400);
        let mut stmt = self.conn.prepare(
            r#"SELECT id, centroid
               FROM semantic_threads
               WHERE model = ?1 AND dims = ?2
               ORDER BY updated_at DESC, id DESC
               LIMIT ?3"#,
        )?;
        let rows = stmt.query_map(params![model.as_str(), dims, lim], |r| {
            let id = r.get::<_, i64>(0)?;
            let blob = r.get::<_, Vec<u8>>(1)?;
            let centroid = decode_embedding_blob(&blob, query_embedding.len())?;
            Ok((id, centroid))
        })?;
        let mut scored = Vec::new();
        for row in rows {
            let (id, centroid) = row?;
            let sim = cosine_similarity(query_embedding, &centroid)
                .clamp(-1.0, 1.0)
                .max(0.0);
            if sim >= min_similarity {
                scored.push((id, sim));
            }
        }
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(top_k);
        Ok(scored)
    }

    pub fn list_semantic_threads(
        &self,
        model: &str,
        dims: usize,
    ) -> Result<Vec<SemanticThreadRecord>, DbError> {
        if dims == 0 {
            return Ok(Vec::new());
        }
        let model = normalize_model(model)?;
        let dims_i64 = i64::try_from(dims).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            r#"SELECT id, message_count, centroid
               FROM semantic_threads
               WHERE model = ?1 AND dims = ?2
               ORDER BY id ASC"#,
        )?;
        let rows = stmt.query_map(params![model.as_str(), dims_i64], |r| {
            let id = r.get::<_, i64>(0)?;
            let message_count = r.get::<_, i64>(1)?;
            let blob = r.get::<_, Vec<u8>>(2)?;
            let centroid = decode_embedding_blob(&blob, dims)?;
            Ok(SemanticThreadRecord {
                id,
                message_count,
                centroid,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn upsert_semantic_pending_anchor(
        &self,
        message_id: i64,
        model: &str,
        embedding: &[f32],
    ) -> Result<(), DbError> {
        if message_id <= 0 || embedding.is_empty() {
            return Ok(());
        }
        let model = normalize_model(model)?;
        let dims = i64::try_from(embedding.len()).unwrap_or(i64::MAX);
        let now = now_ms();
        self.conn.execute(
            r#"INSERT INTO semantic_pending_anchors(
                   message_id, model, dims, embedding, created_at, updated_at
               ) VALUES (?1, ?2, ?3, ?4, ?5, ?5)
               ON CONFLICT(message_id) DO UPDATE SET
                   model = excluded.model,
                   dims = excluded.dims,
                   embedding = excluded.embedding,
                   updated_at = excluded.updated_at"#,
            params![
                message_id,
                model.as_str(),
                dims,
                encode_embedding_blob(embedding),
                now
            ],
        )?;
        Ok(())
    }

    pub fn remove_semantic_pending_anchor(&self, message_id: i64) -> Result<(), DbError> {
        if message_id <= 0 {
            return Ok(());
        }
        self.conn.execute(
            "DELETE FROM semantic_pending_anchors WHERE message_id = ?1",
            params![message_id],
        )?;
        Ok(())
    }

    pub fn nearest_semantic_pending_anchor(
        &self,
        model: &str,
        query_embedding: &[f32],
        min_similarity: f64,
    ) -> Result<Option<(SemanticPendingAnchorRecord, f64)>, DbError> {
        if query_embedding.is_empty() {
            return Ok(None);
        }
        let model = normalize_model(model)?;
        let dims = i64::try_from(query_embedding.len()).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            r#"SELECT message_id, embedding
               FROM semantic_pending_anchors
               WHERE model = ?1 AND dims = ?2
               ORDER BY message_id ASC"#,
        )?;
        let rows = stmt.query_map(params![model.as_str(), dims], |r| {
            let message_id = r.get::<_, i64>(0)?;
            let blob = r.get::<_, Vec<u8>>(1)?;
            let embedding = decode_embedding_blob(&blob, query_embedding.len())?;
            Ok(SemanticPendingAnchorRecord {
                message_id,
                embedding,
            })
        })?;
        let mut best: Option<(SemanticPendingAnchorRecord, f64)> = None;
        for row in rows {
            let rec = row?;
            let sim = cosine_similarity(query_embedding, &rec.embedding)
                .clamp(-1.0, 1.0)
                .max(0.0);
            if sim < min_similarity {
                continue;
            }
            match best.as_ref() {
                Some((best_rec, best_score)) => {
                    if sim > *best_score
                        || ((sim - *best_score).abs() < 1e-9
                            && rec.message_id < best_rec.message_id)
                    {
                        best = Some((rec, sim));
                    }
                }
                None => best = Some((rec, sim)),
            }
        }
        Ok(best)
    }

    pub fn semantic_thread_memberships_for_message(
        &self,
        message_id: i64,
        limit: usize,
    ) -> Result<Vec<SemanticThreadMembershipRecord>, DbError> {
        if message_id <= 0 {
            return Ok(Vec::new());
        }
        let lim = if limit == 0 {
            20_i64
        } else {
            i64::try_from(limit).unwrap_or(i64::MAX)
        };
        let mut stmt = self.conn.prepare(
            r#"SELECT thread_id, score, is_primary
               FROM semantic_thread_memberships
               WHERE message_id = ?1
               ORDER BY is_primary DESC, score DESC, thread_id ASC
               LIMIT ?2"#,
        )?;
        let rows = stmt.query_map(params![message_id, lim], |r| {
            Ok(SemanticThreadMembershipRecord {
                thread_id: r.get::<_, i64>(0)?,
                score: r.get::<_, f64>(1)?,
                is_primary: r.get::<_, i64>(2)? != 0,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn upsert_semantic_thread_membership(
        &self,
        thread_id: i64,
        message_id: i64,
        score: f64,
        is_primary: bool,
    ) -> Result<bool, DbError> {
        if thread_id <= 0 || message_id <= 0 {
            return Ok(false);
        }
        let now = now_ms();
        let is_primary = if is_primary { 1_i64 } else { 0_i64 };
        let score = clamp01(score);
        let inserted = self.conn.execute(
            r#"INSERT OR IGNORE INTO semantic_thread_memberships(
                   thread_id, message_id, score, is_primary, created_at, updated_at
               ) VALUES (?1, ?2, ?3, ?4, ?5, ?5)"#,
            params![thread_id, message_id, score, is_primary, now],
        )?;
        self.conn.execute(
            r#"UPDATE semantic_thread_memberships
               SET score = ?3,
                   is_primary = ?4,
                   updated_at = ?5
               WHERE thread_id = ?1 AND message_id = ?2"#,
            params![thread_id, message_id, score, is_primary, now],
        )?;
        Ok(inserted > 0)
    }

    pub fn increment_semantic_thread_centroid(
        &self,
        thread_id: i64,
        embedding: &[f32],
    ) -> Result<(), DbError> {
        if thread_id <= 0 {
            return Ok(());
        }
        if embedding.is_empty() {
            return Err(DbError::InvalidData(
                "semantic thread embedding is empty".to_string(),
            ));
        }
        let mut stmt = self.conn.prepare(
            r#"SELECT model, dims, centroid, message_count
               FROM semantic_threads
               WHERE id = ?1
               LIMIT 1"#,
        )?;
        let row = stmt.query_row(params![thread_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, i64>(3)?,
            ))
        });
        let (_model, dims, blob, count) = match row {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Err(DbError::NotFound),
            Err(e) => return Err(DbError::Sql(e)),
        };
        let dims_usize = usize::try_from(dims).unwrap_or(0);
        if dims_usize == 0 || dims_usize != embedding.len() {
            return Err(DbError::InvalidData(
                "semantic thread dims mismatch".to_string(),
            ));
        }
        let old = decode_embedding_blob(&blob, dims_usize).map_err(DbError::Sql)?;
        let base = if count <= 0 { 0.0 } else { count as f64 };
        let denom = base + 1.0;
        let mut next = Vec::with_capacity(dims_usize);
        for (a, b) in old.iter().zip(embedding.iter()) {
            let v = ((f64::from(*a) * base) + f64::from(*b)) / denom;
            next.push(v as f32);
        }
        let now = now_ms();
        self.conn.execute(
            r#"UPDATE semantic_threads
               SET centroid = ?2,
                   message_count = message_count + 1,
                   updated_at = ?3
               WHERE id = ?1"#,
            params![thread_id, encode_embedding_blob(&next), now],
        )?;
        Ok(())
    }

    pub fn update_semantic_thread_centroids_batch(
        &self,
        updates: &[(i64, Vec<f32>, i64)],
    ) -> Result<(), DbError> {
        if updates.is_empty() {
            return Ok(());
        }
        let now = now_ms();
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                r#"UPDATE semantic_threads
                   SET centroid = ?2,
                       message_count = ?3,
                       updated_at = ?4
                   WHERE id = ?1"#,
            )?;
            for (thread_id, centroid, message_count) in updates {
                if *thread_id <= 0 || centroid.is_empty() {
                    continue;
                }
                stmt.execute(params![
                    *thread_id,
                    encode_embedding_blob(centroid),
                    (*message_count).max(0),
                    now
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_semantic_thread_edge(
        &self,
        thread_a: i64,
        thread_b: i64,
        kind: &str,
        strength: f64,
    ) -> Result<(), DbError> {
        if thread_a <= 0 || thread_b <= 0 || thread_a == thread_b {
            return Ok(());
        }
        let (a, b) = if thread_a < thread_b {
            (thread_a, thread_b)
        } else {
            (thread_b, thread_a)
        };
        let kind = normalize_semantic_edge_kind(kind);
        let now = now_ms();
        self.conn.execute(
            r#"INSERT INTO semantic_thread_edges(
                   thread_a_id, thread_b_id, kind, strength, created_at, updated_at
               ) VALUES (?1, ?2, ?3, ?4, ?5, ?5)
               ON CONFLICT(thread_a_id, thread_b_id, kind) DO UPDATE SET
                   strength = excluded.strength,
                   updated_at = excluded.updated_at"#,
            params![a, b, kind, clamp01(strength), now],
        )?;
        Ok(())
    }

    pub fn merge_semantic_threads(
        &self,
        target_thread_id: i64,
        source_thread_id: i64,
    ) -> Result<bool, DbError> {
        if target_thread_id <= 0 || source_thread_id <= 0 || target_thread_id == source_thread_id {
            return Ok(false);
        }

        let now = now_ms();
        let tx = self.conn.unchecked_transaction()?;
        let merged = (|| -> Result<bool, DbError> {
            let mut thread_stmt = tx.prepare(
                r#"SELECT model, dims, message_count, centroid
                   FROM semantic_threads
                   WHERE id = ?1
                   LIMIT 1"#,
            )?;
            let target_row = thread_stmt
                .query_row(params![target_thread_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Vec<u8>>(3)?,
                    ))
                })
                .optional()?;
            let source_row = thread_stmt
                .query_row(params![source_thread_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Vec<u8>>(3)?,
                    ))
                })
                .optional()?;
            drop(thread_stmt);

            let Some((target_model, target_dims, target_count, target_blob)) = target_row else {
                return Ok(false);
            };
            let Some((source_model, source_dims, source_count, source_blob)) = source_row else {
                return Ok(false);
            };
            if target_model != source_model || target_dims != source_dims || target_dims <= 0 {
                return Ok(false);
            }
            let dims = usize::try_from(target_dims).unwrap_or(0);
            if dims == 0 {
                return Ok(false);
            }
            let target_centroid = decode_embedding_blob(&target_blob, dims)?;
            let source_centroid = decode_embedding_blob(&source_blob, dims)?;

            let target_base = target_count.max(0) as f64;
            let source_base = source_count.max(0) as f64;
            let total = (target_base + source_base).max(1.0);
            let mut merged_centroid = Vec::with_capacity(dims);
            for (a, b) in target_centroid.iter().zip(source_centroid.iter()) {
                let v = ((f64::from(*a) * target_base) + (f64::from(*b) * source_base)) / total;
                merged_centroid.push(v as f32);
            }

            let mut membership_stmt = tx.prepare(
                r#"SELECT message_id, score, is_primary
                   FROM semantic_thread_memberships
                   WHERE thread_id = ?1
                   ORDER BY message_id ASC"#,
            )?;
            let rows = membership_stmt.query_map(params![source_thread_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, i64>(2)? != 0,
                ))
            })?;
            let mut source_memberships: Vec<(i64, f64, bool)> = Vec::new();
            for row in rows {
                source_memberships.push(row?);
            }
            drop(membership_stmt);

            for (message_id, source_score, source_primary) in source_memberships {
                let existing = tx
                    .query_row(
                        r#"SELECT score, is_primary
                           FROM semantic_thread_memberships
                           WHERE thread_id = ?1 AND message_id = ?2
                           LIMIT 1"#,
                        params![target_thread_id, message_id],
                        |r| Ok((r.get::<_, f64>(0)?, r.get::<_, i64>(1)? != 0)),
                    )
                    .optional()?;
                match existing {
                    Some((target_score, target_primary)) => {
                        let choose_source = source_score > target_score;
                        let next_score = if choose_source {
                            source_score
                        } else {
                            target_score
                        };
                        let next_primary = if choose_source {
                            source_primary
                        } else if (source_score - target_score).abs() < 1e-9 {
                            source_primary || target_primary
                        } else {
                            target_primary
                        };
                        tx.execute(
                            r#"UPDATE semantic_thread_memberships
                               SET score = ?3,
                                   is_primary = ?4,
                                   updated_at = ?5
                               WHERE thread_id = ?1 AND message_id = ?2"#,
                            params![
                                target_thread_id,
                                message_id,
                                clamp01(next_score),
                                if next_primary { 1_i64 } else { 0_i64 },
                                now
                            ],
                        )?;
                    }
                    None => {
                        tx.execute(
                            r#"INSERT INTO semantic_thread_memberships(
                                   thread_id, message_id, score, is_primary, created_at, updated_at
                               ) VALUES (?1, ?2, ?3, ?4, ?5, ?5)"#,
                            params![
                                target_thread_id,
                                message_id,
                                clamp01(source_score),
                                if source_primary { 1_i64 } else { 0_i64 },
                                now
                            ],
                        )?;
                    }
                }
            }

            tx.execute(
                "DELETE FROM semantic_thread_memberships WHERE thread_id = ?1",
                params![source_thread_id],
            )?;

            let mut edge_stmt = tx.prepare(
                r#"SELECT thread_a_id, thread_b_id, kind, strength
                   FROM semantic_thread_edges
                   WHERE thread_a_id = ?1 OR thread_b_id = ?1"#,
            )?;
            let edge_rows = edge_stmt.query_map(params![source_thread_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, f64>(3)?,
                ))
            })?;
            let mut source_edges: Vec<(i64, i64, String, f64)> = Vec::new();
            for row in edge_rows {
                source_edges.push(row?);
            }
            drop(edge_stmt);

            for (a, b, kind, strength) in source_edges {
                let other = if a == source_thread_id { b } else { a };
                if other <= 0 || other == target_thread_id {
                    continue;
                }
                let (na, nb) = if target_thread_id < other {
                    (target_thread_id, other)
                } else {
                    (other, target_thread_id)
                };
                tx.execute(
                    r#"INSERT INTO semantic_thread_edges(
                           thread_a_id, thread_b_id, kind, strength, created_at, updated_at
                       ) VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                       ON CONFLICT(thread_a_id, thread_b_id, kind) DO UPDATE SET
                           strength = CASE
                               WHEN excluded.strength > semantic_thread_edges.strength
                               THEN excluded.strength
                               ELSE semantic_thread_edges.strength
                           END,
                           updated_at = excluded.updated_at"#,
                    params![na, nb, normalize_semantic_edge_kind(&kind), clamp01(strength), now],
                )?;
            }

            tx.execute(
                "DELETE FROM semantic_thread_edges WHERE thread_a_id = ?1 OR thread_b_id = ?1",
                params![source_thread_id],
            )?;

            let merged_count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM semantic_thread_memberships WHERE thread_id = ?1 AND is_primary = 1",
                params![target_thread_id],
                |r| r.get(0),
            )?;
            tx.execute(
                r#"UPDATE semantic_threads
                   SET centroid = ?2,
                       message_count = ?3,
                       updated_at = ?4
                   WHERE id = ?1"#,
                params![
                    target_thread_id,
                    encode_embedding_blob(&merged_centroid),
                    merged_count.max(0),
                    now
                ],
            )?;
            tx.execute(
                "DELETE FROM semantic_threads WHERE id = ?1",
                params![source_thread_id],
            )?;
            Ok(true)
        })()?;

        if merged {
            tx.commit()?;
        }
        Ok(merged)
    }

    pub fn list_memory_statements(
        &self,
        limit: usize,
        model: Option<&str>,
        conversation_id: Option<i64>,
    ) -> Result<Vec<MemoryStatementRecord>, DbError> {
        let lim = if limit == 0 {
            500_i64
        } else {
            i64::try_from(limit).unwrap_or(i64::MAX)
        };
        let mut out = Vec::new();
        let model = model.map(str::trim).filter(|m| !m.is_empty());
        match (model, conversation_id) {
            (Some(model), Some(cid)) => {
                let mut stmt = self.conn.prepare(
                    r#"SELECT
                           id, text, belief_score, salience,
                           first_seen_message_id, last_seen_message_id,
                           evidence_count, created_at, updated_at
                       FROM memory_statements AS s
                       WHERE s.model = ?1
                         AND EXISTS (
                           SELECT 1
                           FROM memory_statement_evidence AS e
                           JOIN messages AS m ON m.id = e.message_id
                           WHERE e.statement_id = s.id AND m.conversation_id = ?2
                           LIMIT 1
                         )
                       ORDER BY updated_at DESC, id DESC
                       LIMIT ?3"#,
                )?;
                let rows = stmt.query_map(params![model, cid, lim], map_memory_statement_row)?;
                for row in rows {
                    out.push(row?);
                }
            }
            (Some(model), None) => {
                let mut stmt = self.conn.prepare(
                    r#"SELECT
                           id, text, belief_score, salience,
                           first_seen_message_id, last_seen_message_id,
                           evidence_count, created_at, updated_at
                       FROM memory_statements
                       WHERE model = ?1
                       ORDER BY updated_at DESC, id DESC
                       LIMIT ?2"#,
                )?;
                let rows = stmt.query_map(params![model, lim], map_memory_statement_row)?;
                for row in rows {
                    out.push(row?);
                }
            }
            (None, Some(cid)) => {
                let mut stmt = self.conn.prepare(
                    r#"SELECT
                           id, text, belief_score, salience,
                           first_seen_message_id, last_seen_message_id,
                           evidence_count, created_at, updated_at
                       FROM memory_statements AS s
                       WHERE EXISTS (
                           SELECT 1
                           FROM memory_statement_evidence AS e
                           JOIN messages AS m ON m.id = e.message_id
                           WHERE e.statement_id = s.id AND m.conversation_id = ?1
                           LIMIT 1
                       )
                       ORDER BY updated_at DESC, id DESC
                       LIMIT ?2"#,
                )?;
                let rows = stmt.query_map(params![cid, lim], map_memory_statement_row)?;
                for row in rows {
                    out.push(row?);
                }
            }
            (None, None) => {
                let mut stmt = self.conn.prepare(
                    r#"SELECT
                           id, text, belief_score, salience,
                           first_seen_message_id, last_seen_message_id,
                           evidence_count, created_at, updated_at
                       FROM memory_statements
                       ORDER BY updated_at DESC, id DESC
                       LIMIT ?1"#,
                )?;
                let rows = stmt.query_map(params![lim], map_memory_statement_row)?;
                for row in rows {
                    out.push(row?);
                }
            }
        }
        Ok(out)
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
                let has_membership = self.conn.query_row(
                    "SELECT 1 FROM semantic_thread_memberships WHERE message_id = ?1 LIMIT 1",
                    params![message_id],
                    |_| Ok(()),
                );
                let has_pending_anchor = self.conn.query_row(
                    "SELECT 1 FROM semantic_pending_anchors WHERE message_id = ?1 LIMIT 1",
                    params![message_id],
                    |_| Ok(()),
                );
                let has_embedding = match has_embedding {
                    Ok(()) => true,
                    Err(rusqlite::Error::QueryReturnedNoRows) => false,
                    Err(e) => return Err(DbError::Sql(e)),
                };
                let has_membership = match has_membership {
                    Ok(()) => true,
                    Err(rusqlite::Error::QueryReturnedNoRows) => false,
                    Err(e) => return Err(DbError::Sql(e)),
                };
                let has_pending_anchor = match has_pending_anchor {
                    Ok(()) => true,
                    Err(rusqlite::Error::QueryReturnedNoRows) => false,
                    Err(e) => return Err(DbError::Sql(e)),
                };
                if has_embedding && (has_membership || has_pending_anchor) {
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
                   OR (
                     m.embed_status = 'done'
                     AND (
                       m.embedding IS NULL
                       OR NOT EXISTS (
                         SELECT 1
                         FROM semantic_thread_memberships AS st
                         WHERE st.message_id = m.id
                         LIMIT 1
                       )
                       AND NOT EXISTS (
                         SELECT 1
                         FROM semantic_pending_anchors AS pa
                         WHERE pa.message_id = m.id
                         LIMIT 1
                       )
                     )
                   )
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

    pub fn nearest_memory_statements(
        &self,
        model: &str,
        query_embedding: &[f32],
        top_k: usize,
        conversation_id: Option<i64>,
    ) -> Result<Vec<(MemoryStatementRecord, f64)>, DbError> {
        if query_embedding.is_empty() || top_k == 0 {
            return Ok(Vec::new());
        }
        let model = normalize_model(model)?;
        let dims = i64::try_from(query_embedding.len()).unwrap_or(i64::MAX);
        let lim = i64::try_from(top_k.saturating_mul(80))
            .unwrap_or(i64::MAX)
            .max(400);
        let mut scored = Vec::new();
        if let Some(cid) = conversation_id {
            let mut stmt = self.conn.prepare(
                r#"SELECT
                       s.id, s.text, s.belief_score, s.salience,
                       s.first_seen_message_id, s.last_seen_message_id,
                       s.evidence_count, s.embedding, s.created_at, s.updated_at
                   FROM memory_statements AS s
                   WHERE s.model = ?1
                     AND s.dims = ?2
                     AND EXISTS (
                         SELECT 1
                         FROM memory_statement_evidence AS e
                         JOIN messages AS m ON m.id = e.message_id
                         WHERE e.statement_id = s.id AND m.conversation_id = ?3
                         LIMIT 1
                     )
                   ORDER BY s.updated_at DESC, s.id DESC
                   LIMIT ?4"#,
            )?;
            let rows = stmt.query_map(params![model.as_str(), dims, cid, lim], |r| {
                let blob = r.get::<_, Vec<u8>>(7)?;
                let emb = decode_embedding_blob(&blob, query_embedding.len())?;
                let rec = MemoryStatementRecord {
                    id: r.get::<_, i64>(0)?,
                    text: r.get::<_, String>(1)?,
                    belief_score: r.get::<_, f64>(2)?,
                    salience: r.get::<_, f64>(3)?,
                    first_seen_message_id: r.get::<_, i64>(4)?,
                    last_seen_message_id: r.get::<_, i64>(5)?,
                    evidence_count: r.get::<_, i64>(6)?,
                    created_at: r.get::<_, i64>(8)?,
                    updated_at: r.get::<_, i64>(9)?,
                };
                Ok((rec, emb))
            })?;
            for row in rows {
                let (rec, emb) = row?;
                let sim = cosine_similarity(query_embedding, &emb).clamp(-1.0, 1.0);
                let distance = 1.0 - sim.max(0.0);
                scored.push((rec, distance));
            }
        } else {
            let mut stmt = self.conn.prepare(
                r#"SELECT
                       id, text, belief_score, salience,
                       first_seen_message_id, last_seen_message_id,
                       evidence_count, embedding, created_at, updated_at
                   FROM memory_statements
                   WHERE model = ?1 AND dims = ?2
                   ORDER BY updated_at DESC, id DESC
                   LIMIT ?3"#,
            )?;
            let rows = stmt.query_map(params![model.as_str(), dims, lim], |r| {
                let blob = r.get::<_, Vec<u8>>(7)?;
                let emb = decode_embedding_blob(&blob, query_embedding.len())?;
                let rec = MemoryStatementRecord {
                    id: r.get::<_, i64>(0)?,
                    text: r.get::<_, String>(1)?,
                    belief_score: r.get::<_, f64>(2)?,
                    salience: r.get::<_, f64>(3)?,
                    first_seen_message_id: r.get::<_, i64>(4)?,
                    last_seen_message_id: r.get::<_, i64>(5)?,
                    evidence_count: r.get::<_, i64>(6)?,
                    created_at: r.get::<_, i64>(8)?,
                    updated_at: r.get::<_, i64>(9)?,
                };
                Ok((rec, emb))
            })?;
            for row in rows {
                let (rec, emb) = row?;
                let sim = cosine_similarity(query_embedding, &emb).clamp(-1.0, 1.0);
                let distance = 1.0 - sim.max(0.0);
                scored.push((rec, distance));
            }
        }
        scored.sort_by(|a, b| a.1.total_cmp(&b.1));
        scored.truncate(top_k);
        Ok(scored)
    }

    pub fn apply_memory_patch(&self, patch: MemoryPatch<'_>) -> Result<i64, DbError> {
        let model = normalize_model(patch.model)?;
        if patch.embedding.is_empty() {
            return Err(DbError::InvalidData("empty embedding".to_string()));
        }
        let tx = self.conn.unchecked_transaction()?;
        let now = now_ms();
        let mut statement_id = patch.statement_id;
        let dims = i64::try_from(patch.embedding.len()).unwrap_or(i64::MAX);
        if statement_id <= 0 {
            let text = normalize_memory_text(patch.text)?;
            let sal = clamp01(patch.salience);
            let score = clamp_score(patch.delta);
            tx.execute(
                r#"INSERT INTO memory_statements(
                       model, text, belief_score, salience, first_seen_message_id, last_seen_message_id,
                       evidence_count, dims, embedding, created_at, updated_at
                   ) VALUES (?1, ?2, ?3, ?4, ?5, ?5, 1, ?6, ?7, ?8, ?8)"#,
                params![
                    model.as_str(),
                    text,
                    score,
                    sal,
                    patch.message_id,
                    dims,
                    encode_embedding_blob(patch.embedding),
                    now
                ],
            )?;
            statement_id = tx.last_insert_rowid();
        } else {
            tx.execute(
                r#"UPDATE memory_statements
                   SET model = ?1,
                       belief_score = MIN(50.0, MAX(-50.0, belief_score + ?2)),
                       salience = MIN(1.0, MAX(0.0, ?3)),
                       last_seen_message_id = ?4,
                       evidence_count = evidence_count + 1,
                       dims = ?5,
                       embedding = ?6,
                       updated_at = ?7
                   WHERE id = ?8"#,
                params![
                    model.as_str(),
                    patch.delta,
                    clamp01(patch.salience),
                    patch.message_id,
                    dims,
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

    #[cfg(test)]
    pub fn count_semantic_threads(&self) -> Result<i64, DbError> {
        let n = self
            .conn
            .query_row("SELECT COUNT(*) FROM semantic_threads", [], |r| r.get(0))?;
        Ok(n)
    }

    #[cfg(test)]
    pub fn count_semantic_thread_memberships(&self) -> Result<i64, DbError> {
        let n = self.conn.query_row(
            "SELECT COUNT(*) FROM semantic_thread_memberships",
            [],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    #[cfg(test)]
    pub fn count_semantic_thread_edges(&self) -> Result<i64, DbError> {
        let n = self
            .conn
            .query_row("SELECT COUNT(*) FROM semantic_thread_edges", [], |r| {
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
        r#"CREATE TABLE IF NOT EXISTS semantic_threads (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               model TEXT NOT NULL,
               dims INTEGER NOT NULL,
               centroid BLOB NOT NULL,
               message_count INTEGER NOT NULL DEFAULT 0,
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL
           );"#,
        r#"CREATE INDEX IF NOT EXISTS semantic_threads_model_dims_updated_idx
           ON semantic_threads(model, dims, updated_at DESC, id DESC);"#,
        r#"CREATE TABLE IF NOT EXISTS semantic_thread_memberships (
               thread_id INTEGER NOT NULL,
               message_id INTEGER NOT NULL,
               score REAL NOT NULL,
               is_primary INTEGER NOT NULL DEFAULT 0 CHECK(is_primary IN (0,1)),
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL,
               PRIMARY KEY(thread_id, message_id),
               FOREIGN KEY(thread_id) REFERENCES semantic_threads(id) ON DELETE CASCADE,
               FOREIGN KEY(message_id) REFERENCES messages(id) ON DELETE CASCADE
           );"#,
        r#"CREATE INDEX IF NOT EXISTS semantic_thread_memberships_message_idx
           ON semantic_thread_memberships(message_id, is_primary DESC, score DESC, thread_id);"#,
        r#"CREATE INDEX IF NOT EXISTS semantic_thread_memberships_thread_idx
           ON semantic_thread_memberships(thread_id, score DESC, message_id);"#,
        r#"CREATE TABLE IF NOT EXISTS semantic_thread_edges (
               thread_a_id INTEGER NOT NULL,
               thread_b_id INTEGER NOT NULL,
               kind TEXT NOT NULL CHECK(kind IN ('supports','conflicts','related')),
               strength REAL NOT NULL,
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL,
               PRIMARY KEY(thread_a_id, thread_b_id, kind),
               CHECK(thread_a_id < thread_b_id),
               FOREIGN KEY(thread_a_id) REFERENCES semantic_threads(id) ON DELETE CASCADE,
               FOREIGN KEY(thread_b_id) REFERENCES semantic_threads(id) ON DELETE CASCADE
           );"#,
        r#"CREATE INDEX IF NOT EXISTS semantic_thread_edges_strength_idx
           ON semantic_thread_edges(strength DESC, updated_at DESC, thread_a_id, thread_b_id);"#,
        r#"CREATE TABLE IF NOT EXISTS semantic_pending_anchors (
               message_id INTEGER PRIMARY KEY,
               model TEXT NOT NULL,
               dims INTEGER NOT NULL,
               embedding BLOB NOT NULL,
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL,
               FOREIGN KEY(message_id) REFERENCES messages(id) ON DELETE CASCADE
           );"#,
        r#"CREATE INDEX IF NOT EXISTS semantic_pending_anchors_model_dims_idx
           ON semantic_pending_anchors(model, dims, updated_at DESC, message_id);"#,
        r#"CREATE TABLE IF NOT EXISTS memory_statements (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               model TEXT NOT NULL,
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
        r#"CREATE INDEX IF NOT EXISTS memory_statements_model_updated_idx
           ON memory_statements(model, updated_at DESC, id DESC);"#,
        r#"CREATE INDEX IF NOT EXISTS memory_statements_model_dims_updated_idx
           ON memory_statements(model, dims, updated_at DESC, id DESC);"#,
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

fn map_memory_statement_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryStatementRecord> {
    Ok(MemoryStatementRecord {
        id: r.get::<_, i64>(0)?,
        text: r.get::<_, String>(1)?,
        belief_score: r.get::<_, f64>(2)?,
        salience: r.get::<_, f64>(3)?,
        first_seen_message_id: r.get::<_, i64>(4)?,
        last_seen_message_id: r.get::<_, i64>(5)?,
        evidence_count: r.get::<_, i64>(6)?,
        created_at: r.get::<_, i64>(7)?,
        updated_at: r.get::<_, i64>(8)?,
    })
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

fn normalize_semantic_edge_kind(kind: &str) -> &'static str {
    match kind.trim().to_ascii_lowercase().as_str() {
        "supports" => "supports",
        "conflicts" => "conflicts",
        _ => "related",
    }
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
