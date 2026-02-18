use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use super::super::DbError;

pub(super) fn open_sqlite(path: &Path) -> Result<Connection, DbError> {
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

pub(super) fn init_schema(conn: &mut Connection) -> Result<(), DbError> {
    let stmts = [
        r#"CREATE TABLE IF NOT EXISTS users (
					id TEXT PRIMARY KEY,
					email TEXT NOT NULL UNIQUE COLLATE NOCASE,
					password_hash TEXT NOT NULL,
					status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','disabled','pending')),
                    is_admin INTEGER NOT NULL DEFAULT 0,
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
    ensure_admin_column_and_seed(conn)?;
    Ok(())
}

fn ensure_admin_column_and_seed(conn: &Connection) -> Result<(), DbError> {
    let mut has_is_admin = false;
    let mut stmt = conn.prepare("PRAGMA table_info(users)")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
    for row in rows {
        if row?.trim() == "is_admin" {
            has_is_admin = true;
            break;
        }
    }
    if !has_is_admin {
        conn.execute_batch("ALTER TABLE users ADD COLUMN is_admin INTEGER NOT NULL DEFAULT 0;")?;
    }
    conn.execute_batch(
        r#"CREATE INDEX IF NOT EXISTS users_admin_status_idx
           ON users(is_admin, status, created_at);"#,
    )?;

    let admin_count: i64 = conn.query_row(
        "SELECT COUNT(1) FROM users WHERE is_admin = 1 AND status = 'active'",
        [],
        |r| r.get(0),
    )?;
    if admin_count == 0 {
        conn.execute(
            r#"UPDATE users
               SET is_admin = 1
               WHERE id = (
                   SELECT id
                   FROM users
                   WHERE status = 'active'
                   ORDER BY created_at ASC, rowid ASC
                   LIMIT 1
               )"#,
            [],
        )?;
    }
    Ok(())
}
