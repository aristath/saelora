mod accounts;
mod magic_tokens;
mod schema;
mod sessions;
mod stats;
mod types;

use std::path::Path;

use rusqlite::Connection;

use super::DbError;

pub use types::UserRecord;

pub struct UsersStore {
    conn: Connection,
}

impl UsersStore {
    pub(crate) fn open(path: &Path) -> Result<Self, DbError> {
        let mut conn = schema::open_sqlite(path)?;
        schema::init_schema(&mut conn)?;
        Ok(Self { conn })
    }
}
