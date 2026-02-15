use rusqlite::params;
use uuid::Uuid;

use super::super::util::{now_ms, random_token_hex, sha256_hex};
use super::super::{looks_like_email, DbError};
use super::UsersStore;

impl UsersStore {
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

    #[cfg(test)]
    pub fn count_magic_tokens(&self) -> Result<usize, DbError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(1) FROM magic_tokens", params![], |r| r.get(0))?;
        Ok(n.max(0) as usize)
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
}
