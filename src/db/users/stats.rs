use rusqlite::params;

use super::super::DbError;
use super::UsersStore;

impl UsersStore {
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
}
