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
    pub fn append_user_message(&self, content: &str) -> Result<(), DbError> {
        self.append_user_message_in(&DEFAULT_CONVERSATION_ID.to_string(), content)
    }

    #[cfg(test)]
    pub fn append_saelora_message(&self, content: &str) -> Result<(), DbError> {
        self.append_saelora_message_in(&DEFAULT_CONVERSATION_ID.to_string(), content)
    }

    pub fn append_user_message_in(
        &self,
        conversation_id: &str,
        content: &str,
    ) -> Result<(), DbError> {
        self.append_message(conversation_id, "user", content)
    }

    pub fn append_saelora_message_in(
        &self,
        conversation_id: &str,
        content: &str,
    ) -> Result<(), DbError> {
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

    fn append_message(
        &self,
        conversation_id: &str,
        role: &str,
        content: &str,
    ) -> Result<(), DbError> {
        let cid = normalize_or_default_conversation_id(conversation_id)?;
        let r = role.trim();
        if r != "user" && r != "saelora" {
            return Err(DbError::InvalidStatus);
        }
        let c = content.trim();
        if c.is_empty() {
            return Ok(());
        }
        let now = now_ms();
        self.ensure_conversation(cid, now)?;
        self.conn.execute(
            r#"INSERT INTO messages(conversation_id, role, content, created_at)
               VALUES (?1, ?2, ?3, ?4)"#,
            params![cid, r, c, now],
        )?;
        self.conn.execute(
            r#"UPDATE conversations SET updated_at = ?1 WHERE id = ?2"#,
            params![now, cid],
        )?;
        Ok(())
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
