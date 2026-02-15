use rusqlite::params;
use uuid::Uuid;

use super::super::util::{now_ms, random_token_hex, sha256_hex, verify_password};
use super::super::DbError;
use super::{UserRecord, UsersStore};

impl UsersStore {
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
