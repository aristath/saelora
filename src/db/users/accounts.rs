use rusqlite::params;
use uuid::Uuid;

use super::super::util::now_ms;
use super::super::{looks_like_email, DbError};
use super::{UserRecord, UsersStore};

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

    pub fn is_admin_user(&self, user_id: &str) -> Result<bool, DbError> {
        let uid = user_id.trim();
        if uid.is_empty() {
            return Ok(false);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id, status FROM users ORDER BY rowid ASC LIMIT 1")?;
        let row = stmt.query_row(params![], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        });
        match row {
            Ok((admin_id, status)) => Ok(admin_id == uid && status == "active"),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(e) => Err(DbError::Sql(e)),
        }
    }
}
