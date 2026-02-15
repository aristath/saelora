use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::util::{now_ms, random_token_hex, sha256_hex, verify_password};
use super::{looks_like_email, DbError};

pub struct UsersStore {
    conn: Connection,
}

impl UsersStore {
    pub(crate) fn open(path: &Path) -> Result<Self, DbError> {
        let mut conn = open_sqlite(path)?;
        init_schema(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn user_id_status_by_email(
        &self,
        email: &str,
    ) -> Result<Option<(String, String)>, DbError> {
        let e = email.trim();
        if e.is_empty() {
            return Ok(None);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id, status FROM users WHERE email = ?1 LIMIT 1")?;
        let row = stmt.query_row(params![e], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        });
        match row {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Sql(e)),
        }
    }

    pub fn user_auth_row_by_email(
        &self,
        email: &str,
    ) -> Result<Option<(String, String, String)>, DbError> {
        let e = email.trim();
        if e.is_empty() {
            return Ok(None);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id, status, password_hash FROM users WHERE email = ?1 LIMIT 1")?;
        let row = stmt.query_row(params![e], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        });
        match row {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Sql(e)),
        }
    }

    pub fn create_user(
        &self,
        email: &str,
        password_hash: &str,
        status: &str,
    ) -> Result<String, DbError> {
        let e = email.trim();
        if !looks_like_email(e) {
            return Err(DbError::InvalidEmail);
        }
        let st = if status.trim().is_empty() {
            "pending"
        } else {
            status.trim()
        };

        let id = Uuid::new_v4().to_string();
        let now = now_ms();

        let res = self.conn.execute(
            "INSERT INTO users(id,email,password_hash,status,created_at) VALUES (?,?,?,?,?)",
            params![id.as_str(), e, password_hash, st, now],
        );
        match res {
            Ok(_) => Ok(id),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
            {
                Err(DbError::UserExists)
            }
            Err(e) => Err(DbError::Sql(e)),
        }
    }

    pub fn has_user(&self, email: &str) -> Result<bool, DbError> {
        let e = email.trim();
        if e.is_empty() {
            return Ok(false);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM users WHERE email = ?1 LIMIT 1")?;
        let mut rows = stmt.query(params![e])?;
        Ok(rows.next()?.is_some())
    }

    pub fn count_users(&self) -> Result<usize, DbError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(1) FROM users", params![], |r| r.get(0))?;
        Ok(n.max(0) as usize)
    }

    #[cfg(test)]
    pub fn count_magic_tokens(&self) -> Result<usize, DbError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(1) FROM magic_tokens", params![], |r| r.get(0))?;
        Ok(n.max(0) as usize)
    }

    pub fn list_users(&self) -> Result<Vec<UserRecord>, DbError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, email, status, created_at FROM users ORDER BY created_at DESC")?;
        let rows = stmt.query_map(params![], |r| {
            Ok(UserRecord {
                id: r.get::<_, String>(0)?,
                email: r.get::<_, String>(1)?,
                status: r.get::<_, String>(2)?,
                created_at: r.get::<_, i64>(3)?,
            })
        })?;
        let mut out = Vec::new();
        for rr in rows {
            out.push(rr?);
        }
        Ok(out)
    }

    pub fn get_message_counts(&self) -> Result<(u64, u64), DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT key, value FROM stats WHERE key IN ('user_messages','saelora_messages')",
        )?;
        let mut user = 0i64;
        let mut saelora = 0i64;
        let rows = stmt.query_map(params![], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (k, v) = row?;
            if k == "user_messages" {
                user = v;
            } else if k == "saelora_messages" {
                saelora = v;
            }
        }
        Ok((user.max(0) as u64, saelora.max(0) as u64))
    }

    pub fn incr_message_counts(&self, user_delta: i64, saelora_delta: i64) -> Result<(), DbError> {
        if user_delta != 0 {
            self.conn.execute(
                "INSERT INTO stats(key,value) VALUES('user_messages',?1) ON CONFLICT(key) DO UPDATE SET value = value + excluded.value",
                params![user_delta],
            )?;
        }
        if saelora_delta != 0 {
            self.conn.execute(
                "INSERT INTO stats(key,value) VALUES('saelora_messages',?1) ON CONFLICT(key) DO UPDATE SET value = value + excluded.value",
                params![saelora_delta],
            )?;
        }
        Ok(())
    }

    pub fn set_user_status(&self, user_id: &str, status: &str) -> Result<(), DbError> {
        let uid = user_id.trim();
        if uid.is_empty() {
            return Err(DbError::Unauthorized);
        }
        let st = status.trim();
        if st.is_empty() {
            return Err(DbError::InvalidStatus);
        }
        if st != "active" && st != "disabled" && st != "pending" {
            return Err(DbError::InvalidStatus);
        }
        let now = now_ms();
        self.conn.execute(
            "UPDATE users SET status = ?1, created_at = created_at WHERE id = ?2",
            params![st, uid],
        )?;
        // Revoke sessions for disabled users (best-effort).
        if st == "disabled" {
            let _ = self.conn.execute(
                "UPDATE sessions SET revoked_at = ?1 WHERE user_id = ?2 AND revoked_at IS NULL",
                params![now, uid],
            );
        }
        Ok(())
    }

    pub fn create_magic_token(&self, email: &str, ttl_seconds: i64) -> Result<String, DbError> {
        let e = email.trim();
        if !looks_like_email(e) {
            return Err(DbError::InvalidEmail);
        }
        let ttl = if ttl_seconds <= 0 { 3600 } else { ttl_seconds };
        let now = now_ms();
        let exp = now.saturating_add(ttl.saturating_mul(1000));

        // Disabled users should not get tokens.
        if let Some((_id, status)) = self.user_id_status_by_email(e)? {
            if status == "disabled" {
                return Err(DbError::Unauthorized);
            }
        }

        let raw = random_token_hex(32);
        let token_hash = sha256_hex(raw.as_bytes());
        let id = Uuid::new_v4().to_string();

        self.conn.execute(
            "INSERT INTO magic_tokens(id, email, token_hash, kind, created_at, expires_at, used_at) VALUES (?1, ?2, ?3, 'setup', ?4, ?5, NULL)",
            params![id, e, token_hash, now, exp],
        )?;
        Ok(raw)
    }

    pub fn peek_magic_token_email(&self, raw_token: &str) -> Result<String, DbError> {
        let tok = raw_token.trim();
        if tok.is_empty() {
            return Err(DbError::TokenInvalid);
        }
        let th = sha256_hex(tok.as_bytes());
        let now = now_ms();
        let mut stmt = self.conn.prepare(
            "SELECT email, expires_at, used_at FROM magic_tokens WHERE token_hash = ?1 LIMIT 1",
        )?;
        let row = stmt.query_row(params![th], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        });
        let (email, expires_at, used_at) = match row {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Err(DbError::TokenInvalid),
            Err(e) => return Err(DbError::Sql(e)),
        };
        if used_at.is_some() {
            return Err(DbError::TokenInvalid);
        }
        if expires_at <= now {
            return Err(DbError::TokenInvalid);
        }
        Ok(email)
    }

    pub fn consume_magic_token_set_password(
        &self,
        raw_token: &str,
        password_hash: &str,
        allow_create_user: bool,
    ) -> Result<String, DbError> {
        let tok = raw_token.trim();
        if tok.is_empty() {
            return Err(DbError::TokenInvalid);
        }
        let th = sha256_hex(tok.as_bytes());
        let now = now_ms();

        let tx = self.conn.unchecked_transaction()?;

        let (tok_id, email, expires_at, used_at) = {
            let mut stmt = tx.prepare(
                "SELECT id, email, expires_at, used_at FROM magic_tokens WHERE token_hash = ?1 LIMIT 1",
            )?;
            let row = stmt.query_row(params![th], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                ))
            });
            match row {
                Ok(v) => v,
                Err(rusqlite::Error::QueryReturnedNoRows) => return Err(DbError::TokenInvalid),
                Err(e) => return Err(DbError::Sql(e)),
            }
        };

        if used_at.is_some() {
            return Err(DbError::TokenInvalid);
        }
        if expires_at <= now {
            return Err(DbError::TokenInvalid);
        }

        // User row (if any).
        let mut user_id: Option<String> = None;
        let mut user_status: Option<String> = None;
        {
            let mut stmt = tx.prepare("SELECT id, status FROM users WHERE email = ?1 LIMIT 1")?;
            let row = stmt.query_row(params![email.as_str()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            });
            match row {
                Ok((id, st)) => {
                    user_id = Some(id);
                    user_status = Some(st);
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => {}
                Err(e) => return Err(DbError::Sql(e)),
            }
        }

        let uid = if let (Some(uid), Some(st)) = (user_id, user_status) {
            if st == "disabled" {
                return Err(DbError::TokenInvalid);
            }
            tx.execute(
                "UPDATE users SET password_hash = ?1, status = 'active' WHERE id = ?2",
                params![password_hash, uid.as_str()],
            )?;
            uid
        } else {
            if !allow_create_user {
                return Err(DbError::TokenInvalid);
            }
            let uid = Uuid::new_v4().to_string();
            let res = tx.execute(
                "INSERT INTO users(id,email,password_hash,status,created_at) VALUES (?1,?2,?3,'active',?4)",
                params![uid.as_str(), email.as_str(), password_hash, now],
            );
            match res {
                Ok(_) => uid,
                Err(rusqlite::Error::SqliteFailure(err, _))
                    if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
                {
                    // Race: user was created concurrently; fall back to update.
                    let (uid2, st2): (String, String) = tx.query_row(
                        "SELECT id, status FROM users WHERE email = ?1 LIMIT 1",
                        params![email.as_str()],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?;
                    if st2 == "disabled" {
                        return Err(DbError::TokenInvalid);
                    }
                    tx.execute(
                        "UPDATE users SET password_hash = ?1, status = 'active' WHERE id = ?2",
                        params![password_hash, uid2.as_str()],
                    )?;
                    uid2
                }
                Err(e) => return Err(DbError::Sql(e)),
            }
        };

        // Record credential (best-effort).
        let _ = tx.execute(
            "INSERT INTO credentials(id, user_id, kind, password_hash, public_key, sign_count, created_at) VALUES (?1, ?2, 'password', ?3, NULL, NULL, ?4)",
            params![Uuid::new_v4().to_string(), uid.as_str(), password_hash, now],
        );

        tx.execute(
            "UPDATE magic_tokens SET used_at = ?1 WHERE id = ?2",
            params![now, tok_id],
        )?;
        tx.commit()?;
        Ok(uid)
    }

    pub fn verify_login(&self, email: &str, password: &str) -> Result<UserRecord, DbError> {
        let e = email.trim();
        if e.is_empty() {
            return Err(DbError::Unauthorized);
        }
        let mut stmt = self.conn.prepare(
            "SELECT id, email, status, created_at, password_hash FROM users WHERE email = ?1 LIMIT 1",
        )?;
        let row = stmt.query_row(params![e], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        });

        let (id, email, status, created_at, ph) = match row {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Err(DbError::Unauthorized),
            Err(e) => return Err(DbError::Sql(e)),
        };
        if status != "active" {
            return Err(DbError::Unauthorized);
        }
        if ph.trim().is_empty() {
            return Err(DbError::Unauthorized);
        }
        if !verify_password(password, &ph) {
            return Err(DbError::Unauthorized);
        }

        Ok(UserRecord {
            id,
            email,
            status,
            created_at,
        })
    }

    pub fn create_session_token(&self, user_id: &str, ttl_seconds: i64) -> Result<String, DbError> {
        let uid = user_id.trim();
        if uid.is_empty() {
            return Err(DbError::Unauthorized);
        }
        let ttl = if ttl_seconds <= 0 {
            30 * 24 * 3600
        } else {
            ttl_seconds
        };

        let raw = random_token_hex(32);
        let token_hash = sha256_hex(raw.as_bytes());
        let now = now_ms();
        let exp = now.saturating_add(ttl.saturating_mul(1000));
        let id = Uuid::new_v4().to_string();

        self.conn.execute(
            "INSERT INTO sessions(id, user_id, token_hash, created_at, expires_at, revoked_at) VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            params![id, uid, token_hash, now, exp],
        )?;
        Ok(raw)
    }

    pub fn revoke_session(&self, raw_token: &str) -> Result<(), DbError> {
        let tok = raw_token.trim();
        if tok.is_empty() {
            return Ok(());
        }
        let th = sha256_hex(tok.as_bytes());
        let now = now_ms();
        self.conn.execute(
            "UPDATE sessions SET revoked_at = ?1 WHERE token_hash = ?2",
            params![now, th],
        )?;
        Ok(())
    }

    pub fn auth_user_from_token(&self, raw_token: &str) -> Result<UserRecord, DbError> {
        let tok = raw_token.trim();
        if tok.is_empty() {
            return Err(DbError::Unauthorized);
        }
        let th = sha256_hex(tok.as_bytes());
        let now = now_ms();

        let mut stmt = self.conn.prepare(
            "SELECT u.id, u.email, u.status, u.created_at
             FROM sessions s
             JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = ?1
               AND (s.revoked_at IS NULL)
               AND (s.expires_at IS NULL OR s.expires_at > ?2)
             LIMIT 1",
        )?;
        let row = stmt.query_row(params![th, now], |r| {
            Ok(UserRecord {
                id: r.get::<_, String>(0)?,
                email: r.get::<_, String>(1)?,
                status: r.get::<_, String>(2)?,
                created_at: r.get::<_, i64>(3)?,
            })
        });
        let u = match row {
            Ok(u) => u,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Err(DbError::Unauthorized),
            Err(e) => return Err(DbError::Sql(e)),
        };
        if u.status != "active" {
            return Err(DbError::Unauthorized);
        }
        Ok(u)
    }

    #[cfg(test)]
    pub(crate) fn test_force_expire_magic_token(&self, raw_token: &str) -> Result<(), DbError> {
        let tok = raw_token.trim();
        if tok.is_empty() {
            return Ok(());
        }
        let th = sha256_hex(tok.as_bytes());
        self.conn.execute(
            "UPDATE magic_tokens SET expires_at = 0 WHERE token_hash = ?1",
            params![th],
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn test_force_expire_session_token(&self, raw_token: &str) -> Result<(), DbError> {
        let tok = raw_token.trim();
        if tok.is_empty() {
            return Ok(());
        }
        let th = sha256_hex(tok.as_bytes());
        self.conn.execute(
            "UPDATE sessions SET expires_at = 0 WHERE token_hash = ?1",
            params![th],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRecord {
    pub id: String,
    pub email: String,
    pub status: String,
    pub created_at: i64,
}

fn open_sqlite(path: &Path) -> Result<Connection, DbError> {
    let conn = Connection::open(path)?;
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

    Ok(conn)
}

fn init_schema(conn: &mut Connection) -> Result<(), DbError> {
    let stmts = [
        r#"CREATE TABLE IF NOT EXISTS users (
				id TEXT PRIMARY KEY,
				email TEXT NOT NULL UNIQUE COLLATE NOCASE,
				password_hash TEXT NOT NULL,
				status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','disabled','pending')),
				created_at INTEGER NOT NULL
			);"#,
        r#"CREATE TABLE IF NOT EXISTS sessions (
				id TEXT PRIMARY KEY,
				user_id TEXT NOT NULL,
				token_hash TEXT NOT NULL UNIQUE,
				created_at INTEGER NOT NULL,
				expires_at INTEGER,
				revoked_at INTEGER
			);"#,
        r#"CREATE INDEX IF NOT EXISTS sessions_user_id_idx ON sessions(user_id);"#,
        r#"CREATE TABLE IF NOT EXISTS credentials (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            kind TEXT NOT NULL CHECK (kind IN ('password','passkey')),
            password_hash TEXT,
            public_key BLOB,
            sign_count INTEGER,
            created_at INTEGER NOT NULL
        );"#,
        r#"CREATE INDEX IF NOT EXISTS credentials_user_id_idx ON credentials(user_id);"#,
        r#"CREATE TABLE IF NOT EXISTS magic_tokens (
            id TEXT PRIMARY KEY,
            email TEXT NOT NULL COLLATE NOCASE,
            token_hash TEXT NOT NULL UNIQUE,
            kind TEXT NOT NULL CHECK (kind IN ('setup')),
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            used_at INTEGER
        );"#,
        r#"CREATE INDEX IF NOT EXISTS magic_tokens_email_idx ON magic_tokens(email);"#,
        r#"CREATE INDEX IF NOT EXISTS magic_tokens_created_idx ON magic_tokens(created_at);"#,
        r#"CREATE TABLE IF NOT EXISTS stats (
            key TEXT PRIMARY KEY,
            value INTEGER NOT NULL
        );"#,
    ];
    for s in stmts {
        conn.execute_batch(s)?;
    }
    Ok(())
}
