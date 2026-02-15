use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("invalid email")]
    InvalidEmail,
    #[error("invalid status")]
    InvalidStatus,
    #[error("user already exists")]
    UserExists,
    #[error("unauthorized")]
    Unauthorized,
    #[error("token invalid or expired")]
    TokenInvalid,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub struct Manager {
    data_dir: PathBuf,
}

impl Manager {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }

    fn root(&self) -> &Path {
        if self.data_dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            &self.data_dir
        }
    }

    pub fn users_db_path(&self) -> PathBuf {
        self.root().join("users").join("users.db")
    }

    pub fn waitlist_path(&self) -> PathBuf {
        self.root().join("users").join("waitlist.jsonl")
    }

    pub fn whitelist_path(&self) -> PathBuf {
        self.root().join("users").join("whitelist.jsonl")
    }

    pub fn users(&self) -> Result<UsersStore, DbError> {
        let path = self.users_db_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
        }
        let mut conn = open_sqlite(&path)?;
        migrate_users(&mut conn)?;
        Ok(UsersStore { conn })
    }

    pub fn waitlist_add(&self, email: &str) -> Result<(), DbError> {
        let e = email.trim();
        if !looks_like_email(e) {
            return Err(DbError::InvalidEmail);
        }

        // Avoid duplicates.
        if self.waitlist_has(e)? {
            return Ok(());
        }

        let path = self.waitlist_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }

        let f = OpenOptions::new().create(true).append(true).open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }

        let mut w = BufWriter::new(f);
        let rec = WaitlistRecord {
            email: e.to_string(),
            created_at: now_ms(),
        };
        let mut b = serde_json::to_vec(&rec)?;
        b.push(b'\n');
        w.write_all(&b)?;
        w.flush()?;
        Ok(())
    }

    pub fn waitlist_list(&self) -> Result<Vec<WaitlistRecord>, DbError> {
        let path = self.waitlist_path();
        let f = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(DbError::Io(e)),
        };
        let br = BufReader::new(f);
        let mut by_email: HashMap<String, WaitlistRecord> = HashMap::new();
        for line in br.lines() {
            let line = line?;
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if let Ok(r) = serde_json::from_str::<WaitlistRecord>(t) {
                let e = r.email.trim();
                if !e.is_empty() {
                    let key = e.to_ascii_lowercase();
                    match by_email.get(&key) {
                        Some(existing) if existing.created_at <= r.created_at => {}
                        _ => {
                            by_email.insert(key, r);
                        }
                    }
                }
            }
        }
        let mut out = by_email.into_values().collect::<Vec<_>>();
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(out)
    }

    pub fn whitelist_list(&self) -> Result<Vec<WaitlistRecord>, DbError> {
        let path = self.whitelist_path();
        let f = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(DbError::Io(e)),
        };
        let br = BufReader::new(f);
        let mut by_email: HashMap<String, WaitlistRecord> = HashMap::new();
        for line in br.lines() {
            let line = line?;
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if let Ok(r) = serde_json::from_str::<WaitlistRecord>(t) {
                let e = r.email.trim();
                if !e.is_empty() {
                    let key = e.to_ascii_lowercase();
                    match by_email.get(&key) {
                        Some(existing) if existing.created_at <= r.created_at => {}
                        _ => {
                            by_email.insert(key, r);
                        }
                    }
                }
            }
        }
        let mut out = by_email.into_values().collect::<Vec<_>>();
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(out)
    }

    pub fn waitlist_count(&self) -> Result<usize, DbError> {
        Ok(self.waitlist_list()?.len())
    }

    pub fn waitlist_has(&self, email: &str) -> Result<bool, DbError> {
        let target = email.trim();
        if target.is_empty() {
            return Ok(false);
        }
        let path = self.waitlist_path();
        let f = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(DbError::Io(e)),
        };
        let br = BufReader::new(f);
        for line in br.lines() {
            let line = line?;
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if let Ok(r) = serde_json::from_str::<WaitlistRecord>(t) {
                if r.email.trim().eq_ignore_ascii_case(target) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub fn whitelist_count(&self) -> Result<usize, DbError> {
        Ok(self.whitelist_list()?.len())
    }

    pub fn waitlist_remove(&self, email: &str) -> Result<(), DbError> {
        let target = email.trim();
        if target.is_empty() {
            return Ok(());
        }

        let path = self.waitlist_path();
        let f = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(DbError::Io(e)),
        };

        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        let br = BufReader::new(f);
        let mut bw = BufWriter::new(tmp.as_file_mut());

        for line in br.lines() {
            let line = line?;
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if let Ok(r) = serde_json::from_str::<WaitlistRecord>(t) {
                if r.email.trim().eq_ignore_ascii_case(target) {
                    continue;
                }
                writeln!(bw, "{}", t)?;
            } else {
                // Keep unknown lines.
                writeln!(bw, "{}", t)?;
            }
        }
        bw.flush()?;
        // Ensure the writer is dropped before persisting/moving the temp file.
        drop(bw);

        tmp.persist(&path).map_err(|e| DbError::Io(e.error))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn whitelist_remove(&self, email: &str) -> Result<(), DbError> {
        let target = email.trim();
        if target.is_empty() {
            return Ok(());
        }

        let path = self.whitelist_path();
        let f = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(DbError::Io(e)),
        };

        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        let br = BufReader::new(f);
        let mut bw = BufWriter::new(tmp.as_file_mut());

        for line in br.lines() {
            let line = line?;
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if let Ok(r) = serde_json::from_str::<WaitlistRecord>(t) {
                if r.email.trim().eq_ignore_ascii_case(target) {
                    continue;
                }
                writeln!(bw, "{}", t)?;
            } else {
                writeln!(bw, "{}", t)?;
            }
        }
        bw.flush()?;
        drop(bw);
        tmp.persist(&path).map_err(|e| DbError::Io(e.error))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn whitelist_has(&self, email: &str) -> Result<bool, DbError> {
        let e = email.trim();
        if e.is_empty() {
            return Ok(false);
        }
        let path = self.whitelist_path();
        let f = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(DbError::Io(e)),
        };
        let br = BufReader::new(f);
        for line in br.lines() {
            let line = line?;
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if let Ok(r) = serde_json::from_str::<WaitlistRecord>(t) {
                if r.email.trim().eq_ignore_ascii_case(e) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub fn whitelist_add(&self, email: &str) -> Result<(), DbError> {
        let e = email.trim();
        if !looks_like_email(e) {
            return Err(DbError::InvalidEmail);
        }

        let path = self.whitelist_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }

        // Avoid duplicates.
        if self.whitelist_has(email)? {
            return Ok(());
        }

        let now = now_ms();
        let rec = WaitlistRecord {
            email: e.to_string(),
            created_at: now,
        };
        let f = OpenOptions::new().create(true).append(true).open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        serde_json::to_writer(&f, &rec)?;
        writeln!(&f)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitlistRecord {
    pub email: String,
    #[serde(deserialize_with = "de_ms_or_rfc3339")]
    pub created_at: i64,
}

pub struct UsersStore {
    conn: Connection,
}

impl UsersStore {
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
        let tx = self.conn.unchecked_transaction()?;
        if user_delta != 0 {
            tx.execute(
                "INSERT INTO stats(key, value) VALUES ('user_messages', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = value + excluded.value",
                params![user_delta],
            )?;
        }
        if saelora_delta != 0 {
            tx.execute(
                "INSERT INTO stats(key, value) VALUES ('saelora_messages', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = value + excluded.value",
                params![saelora_delta],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_user_status(&self, user_id: &str, status: &str) -> Result<(), DbError> {
        let id = user_id.trim();
        let st = status.trim();
        if id.is_empty() {
            return Ok(());
        }
        if st != "active" && st != "disabled" && st != "pending" {
            return Err(DbError::InvalidStatus);
        }
        self.conn.execute(
            "UPDATE users SET status = ?1 WHERE id = ?2",
            params![st, id],
        )?;
        Ok(())
    }

    pub fn create_magic_token(&self, email: &str, ttl_seconds: i64) -> Result<String, DbError> {
        let e = email.trim();
        if !looks_like_email(e) {
            return Err(DbError::InvalidEmail);
        }
        // Disallow issuing tokens for disabled accounts.
        if let Some((_id, status)) = self.user_id_status_by_email(e)? {
            if status == "disabled" {
                return Err(DbError::Unauthorized);
            }
        }

        let ttl = if ttl_seconds <= 0 { 3600 } else { ttl_seconds };
        let raw = random_token_hex(32);
        let token_hash = sha256_hex(raw.as_bytes());
        let now = now_ms();
        let exp = now.saturating_add(ttl.saturating_mul(1000));
        let idtok = Uuid::new_v4().to_string();

        self.conn.execute(
            "INSERT INTO magic_tokens(id, email, token_hash, kind, created_at, expires_at, used_at) VALUES (?1, ?2, ?3, 'setup', ?4, ?5, NULL)",
            params![idtok, e, token_hash, now, exp],
        )?;
        Ok(raw)
    }

    pub fn peek_magic_token_email(&self, raw_token: &str) -> Result<String, DbError> {
        let tok = raw_token.trim();
        if tok.is_empty() {
            return Err(DbError::TokenInvalid);
        }
        let now = now_ms();
        let th = sha256_hex(tok.as_bytes());

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
        if password_hash.trim().is_empty() {
            return Err(DbError::Unauthorized);
        }
        let now = now_ms();
        let th = sha256_hex(tok.as_bytes());

        let tx = self.conn.unchecked_transaction()?;

        // Token row.
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
                id: r.get(0)?,
                email: r.get(1)?,
                status: r.get(2)?,
                created_at: r.get(3)?,
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

fn migrate_users(conn: &mut Connection) -> Result<(), DbError> {
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

    // Self-heal older schemas (tokens are ephemeral; we can safely rebuild that table).
    ensure_magic_tokens_schema(conn)?;
    ensure_stats_keys(conn)?;
    Ok(())
}

fn ensure_magic_tokens_schema(conn: &mut Connection) -> Result<(), DbError> {
    let mut stmt = conn.prepare("PRAGMA table_info(magic_tokens);")?;
    let rows = stmt.query_map(params![], |r| r.get::<_, String>(1))?;
    let mut cols = Vec::<String>::new();
    for rr in rows {
        cols.push(rr?);
    }
    if cols.iter().any(|c| c == "email") {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        DROP TABLE IF EXISTS magic_tokens;
        CREATE TABLE magic_tokens (
            id TEXT PRIMARY KEY,
            email TEXT NOT NULL COLLATE NOCASE,
            token_hash TEXT NOT NULL UNIQUE,
            kind TEXT NOT NULL CHECK (kind IN ('setup')),
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            used_at INTEGER
        );
        CREATE INDEX IF NOT EXISTS magic_tokens_email_idx ON magic_tokens(email);
        CREATE INDEX IF NOT EXISTS magic_tokens_created_idx ON magic_tokens(created_at);
        "#,
    )?;
    Ok(())
}

fn ensure_stats_keys(conn: &mut Connection) -> Result<(), DbError> {
    // Older versions used "assistant_messages"; unify on "saelora_messages".
    conn.execute(
        "INSERT INTO stats(key, value)
         SELECT 'saelora_messages', value FROM stats WHERE key='assistant_messages'
         ON CONFLICT(key) DO UPDATE SET value = value + excluded.value",
        params![],
    )?;
    conn.execute(
        "DELETE FROM stats WHERE key='assistant_messages'",
        params![],
    )?;
    Ok(())
}

fn random_token_hex(nbytes: usize) -> String {
    use rand::RngCore as _;
    let mut b = vec![0u8; nbytes];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}

fn sha256_hex(b: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b);
    let out = h.finalize();
    hex::encode(out)
}

pub fn hash_password(password: &str) -> Result<String, DbError> {
    use argon2::password_hash::{PasswordHasher as _, SaltString};
    use argon2::Argon2;

    let pw = password.as_bytes();
    let salt = SaltString::generate(&mut rand::rngs::OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(pw, &salt)
        .map_err(|_| DbError::Unauthorized)?
        .to_string();
    Ok(hash)
}

fn verify_password(password: &str, hash: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier as _};
    use argon2::Argon2;

    let Ok(ph) = PasswordHash::new(hash) else {
        // Constant-time-ish failure path: still run a hash with a dummy salt.
        let _ = hash_password("dummy");
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &ph)
        .is_ok()
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn de_ms_or_rfc3339<'de, D>(d: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Any {
        Ms(i64),
        Text(String),
    }

    let v = Any::deserialize(d)?;
    Ok(match v {
        Any::Ms(ms) => ms,
        Any::Text(s) => chrono::DateTime::parse_from_rfc3339(s.trim())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or(0),
    })
}

pub fn looks_like_email(s: &str) -> bool {
    // We keep this intentionally simple; we just need to stop obvious garbage.
    let s = s.trim();
    if s.is_empty() || s.len() > 254 {
        return false;
    }
    if s.contains(' ') || s.contains('\t') || s.contains('\n') || s.contains('\r') {
        return false;
    }
    let Some(at) = s.find('@') else { return false };
    if at == 0 || at + 1 >= s.len() {
        return false;
    }
    let domain = &s[at + 1..];
    domain.contains('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_mgr() -> (tempfile::TempDir, Manager) {
        let td = tempfile::tempdir().expect("tempdir");
        let mgr = Manager::new(td.path().to_path_buf());
        (td, mgr)
    }

    #[test]
    fn looks_like_email_rejects_obvious_garbage() {
        assert!(looks_like_email("a@b.co"));
        assert!(looks_like_email("Test.User+tag@example.com"));

        assert!(!looks_like_email(""));
        assert!(!looks_like_email("   "));
        assert!(!looks_like_email("no-at-sign.example.com"));
        assert!(!looks_like_email("@example.com"));
        assert!(!looks_like_email("a@"));
        assert!(!looks_like_email("a@example")); // no dot
        assert!(!looks_like_email("a@exa mple.com"));
        // Leading/trailing whitespace should be tolerated (we trim user input).
        assert!(looks_like_email("a@example.com\n"));
        assert!(!looks_like_email("a@example.\ncom"));
    }

    #[test]
    fn waitlist_add_is_case_insensitive_and_dedupes() {
        let (_td, mgr) = new_mgr();

        mgr.waitlist_add("Test@Example.com").unwrap();
        mgr.waitlist_add("test@example.com").unwrap();
        mgr.waitlist_add("TEST@EXAMPLE.COM").unwrap();

        assert_eq!(mgr.waitlist_count().unwrap(), 1);
        assert!(mgr.waitlist_has("test@example.com").unwrap());
        assert!(mgr.waitlist_has("TEST@example.com").unwrap());
    }

    #[test]
    fn whitelist_add_remove_case_insensitive_and_dedupes() {
        let (_td, mgr) = new_mgr();

        mgr.whitelist_add("A@Example.com").unwrap();
        mgr.whitelist_add("a@example.com").unwrap();
        assert_eq!(mgr.whitelist_count().unwrap(), 1);
        assert!(mgr.whitelist_has("a@example.com").unwrap());

        mgr.whitelist_remove("A@EXAMPLE.COM").unwrap();
        assert_eq!(mgr.whitelist_count().unwrap(), 0);
        assert!(!mgr.whitelist_has("a@example.com").unwrap());
    }

    #[test]
    fn create_user_and_verify_login_are_case_insensitive() {
        let (_td, mgr) = new_mgr();
        let us = mgr.users().unwrap();

        let ph = hash_password("password123").unwrap();
        let _uid = us.create_user("Test@Example.com", &ph, "active").unwrap();

        // Login should work regardless of email case.
        assert!(us.verify_login("test@example.com", "password123").is_ok());
        assert!(us.verify_login("TEST@EXAMPLE.COM", "password123").is_ok());

        // Wrong password should not authenticate.
        assert!(matches!(
            us.verify_login("test@example.com", "wrong"),
            Err(DbError::Unauthorized)
        ));
    }

    #[test]
    fn disabled_user_cannot_login_or_receive_magic_tokens() {
        let (_td, mgr) = new_mgr();
        let us = mgr.users().unwrap();

        let ph = hash_password("password123").unwrap();
        let _uid = us
            .create_user("disabled@example.com", &ph, "disabled")
            .unwrap();

        assert!(matches!(
            us.verify_login("disabled@example.com", "password123"),
            Err(DbError::Unauthorized)
        ));

        // DB-level protection: refuse tokens for disabled accounts.
        assert!(matches!(
            us.create_magic_token("disabled@example.com", 3600),
            Err(DbError::Unauthorized)
        ));
    }

    #[test]
    fn magic_token_requires_allow_create_user_for_new_accounts_and_is_one_time() {
        let (_td, mgr) = new_mgr();
        let us = mgr.users().unwrap();

        let tok = us.create_magic_token("new@example.com", 3600).unwrap();
        assert_eq!(us.peek_magic_token_email(&tok).unwrap(), "new@example.com");

        let ph = hash_password("password123").unwrap();

        // If creation isn't allowed, a new-account token should NOT create a user and should remain usable.
        assert!(matches!(
            us.consume_magic_token_set_password(&tok, &ph, false),
            Err(DbError::TokenInvalid)
        ));
        assert_eq!(us.peek_magic_token_email(&tok).unwrap(), "new@example.com");
        assert_eq!(us.count_users().unwrap(), 0);

        // Allow creation: should create user and consume the token.
        let uid = us
            .consume_magic_token_set_password(&tok, &ph, true)
            .unwrap();
        assert!(!uid.trim().is_empty());
        assert_eq!(us.count_users().unwrap(), 1);
        assert!(us.verify_login("new@example.com", "password123").is_ok());
        assert!(matches!(
            us.peek_magic_token_email(&tok),
            Err(DbError::TokenInvalid)
        ));
        assert!(matches!(
            us.consume_magic_token_set_password(&tok, &ph, true),
            Err(DbError::TokenInvalid)
        ));
    }

    #[test]
    fn magic_token_expiry_is_enforced() {
        let (_td, mgr) = new_mgr();
        let us = mgr.users().unwrap();

        let tok = us.create_magic_token("exp@example.com", 3600).unwrap();
        let th = sha256_hex(tok.as_bytes());
        us.conn
            .execute(
                "UPDATE magic_tokens SET expires_at = 0 WHERE token_hash = ?1",
                params![th],
            )
            .unwrap();

        assert!(matches!(
            us.peek_magic_token_email(&tok),
            Err(DbError::TokenInvalid)
        ));
        let ph = hash_password("password123").unwrap();
        assert!(matches!(
            us.consume_magic_token_set_password(&tok, &ph, true),
            Err(DbError::TokenInvalid)
        ));
    }

    #[test]
    fn session_tokens_revoke_and_expire_and_require_active_user() {
        let (_td, mgr) = new_mgr();
        let us = mgr.users().unwrap();

        let ph = hash_password("password123").unwrap();
        let uid = us.create_user("u@example.com", &ph, "active").unwrap();

        let tok = us.create_session_token(&uid, 3600).unwrap();
        assert!(us.auth_user_from_token(&tok).is_ok());

        us.revoke_session(&tok).unwrap();
        assert!(matches!(
            us.auth_user_from_token(&tok),
            Err(DbError::Unauthorized)
        ));

        // Expired tokens should not authenticate.
        let tok2 = us.create_session_token(&uid, 3600).unwrap();
        let th2 = sha256_hex(tok2.as_bytes());
        us.conn
            .execute(
                "UPDATE sessions SET expires_at = 0 WHERE token_hash = ?1",
                params![th2],
            )
            .unwrap();
        assert!(matches!(
            us.auth_user_from_token(&tok2),
            Err(DbError::Unauthorized)
        ));

        // Disabled users should not authenticate even with an unexpired token.
        let tok3 = us.create_session_token(&uid, 3600).unwrap();
        us.set_user_status(&uid, "disabled").unwrap();
        assert!(matches!(
            us.auth_user_from_token(&tok3),
            Err(DbError::Unauthorized)
        ));
    }
}
