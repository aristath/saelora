use rusqlite::params;
#[cfg(test)]
use uuid::Uuid;

#[cfg(test)]
use super::super::looks_like_email;
use super::super::util::now_ms;
use super::super::DbError;
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

    #[cfg(test)]
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
        let make_admin = if st == "active" {
            let active_admins: i64 = self.conn.query_row(
                "SELECT COUNT(1) FROM users WHERE is_admin = 1 AND status = 'active'",
                params![],
                |r| r.get(0),
            )?;
            if active_admins == 0 {
                1_i64
            } else {
                0_i64
            }
        } else {
            0_i64
        };

        let id = Uuid::new_v4().to_string();
        let now = now_ms();

        let res = self.conn.execute(
            "INSERT INTO users(id,email,password_hash,status,is_admin,created_at) VALUES (?,?,?,?,?,?)",
            params![id.as_str(), e, password_hash, st, make_admin, now],
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
        let changed = self.conn.execute(
            "UPDATE users SET status = ?1, created_at = created_at WHERE id = ?2",
            params![st, uid],
        )?;
        if changed == 0 {
            return Err(DbError::NotFound);
        }
        // Revoke sessions for disabled users (best-effort).
        if st == "disabled" {
            let _ = self.conn.execute(
                "UPDATE sessions SET revoked_at = ?1 WHERE user_id = ?2 AND revoked_at IS NULL",
                params![now, uid],
            );
        }
        self.ensure_active_admin_exists()?;
        Ok(())
    }

    pub fn is_admin_user(&self, user_id: &str) -> Result<bool, DbError> {
        let uid = user_id.trim();
        if uid.is_empty() {
            return Ok(false);
        }
        self.ensure_active_admin_exists()?;
        let mut stmt = self
            .conn
            .prepare("SELECT status, is_admin FROM users WHERE id = ?1 LIMIT 1")?;
        let row = stmt.query_row(params![uid], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        });
        match row {
            Ok((status, is_admin)) => Ok(status == "active" && is_admin == 1),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(e) => Err(DbError::Sql(e)),
        }
    }

    fn ensure_active_admin_exists(&self) -> Result<(), DbError> {
        let active_admins: i64 = self.conn.query_row(
            "SELECT COUNT(1) FROM users WHERE is_admin = 1 AND status = 'active'",
            params![],
            |r| r.get(0),
        )?;
        if active_admins > 0 {
            return Ok(());
        }
        self.conn.execute(
            r#"UPDATE users
               SET is_admin = 1
               WHERE id = (
                   SELECT id
                   FROM users
                   WHERE status = 'active'
                   ORDER BY created_at ASC, rowid ASC
                   LIMIT 1
               )"#,
            params![],
        )?;
        Ok(())
    }
}
