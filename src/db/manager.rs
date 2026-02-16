use std::path::{Path, PathBuf};

use super::lists;
use super::{looks_like_email, DbError, UserDataStore, UsersStore, WaitlistRecord};

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

    pub fn user_data_db_path(&self, user_id: &str) -> Result<PathBuf, DbError> {
        let uid = normalize_user_id(user_id)?;
        Ok(self
            .root()
            .join("users")
            .join("userdata")
            .join(format!("{}.db", uid)))
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
        UsersStore::open(&path)
    }

    pub fn user_data(&self, user_id: &str) -> Result<UserDataStore, DbError> {
        let path = self.user_data_db_path(user_id)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
        }
        UserDataStore::open(&path)
    }

    pub fn waitlist_add(&self, email: &str) -> Result<(), DbError> {
        let e = email.trim();
        if !looks_like_email(e) {
            return Err(DbError::InvalidEmail);
        }
        lists::add(&self.waitlist_path(), e)
    }

    pub fn waitlist_list(&self) -> Result<Vec<WaitlistRecord>, DbError> {
        lists::list(&self.waitlist_path())
    }

    pub fn waitlist_count(&self) -> Result<usize, DbError> {
        Ok(self.waitlist_list()?.len())
    }

    pub fn waitlist_has(&self, email: &str) -> Result<bool, DbError> {
        lists::has(&self.waitlist_path(), email)
    }

    pub fn waitlist_remove(&self, email: &str) -> Result<(), DbError> {
        lists::remove(&self.waitlist_path(), email)
    }

    pub fn whitelist_add(&self, email: &str) -> Result<(), DbError> {
        let e = email.trim();
        if !looks_like_email(e) {
            return Err(DbError::InvalidEmail);
        }
        lists::add(&self.whitelist_path(), e)
    }

    pub fn whitelist_list(&self) -> Result<Vec<WaitlistRecord>, DbError> {
        lists::list(&self.whitelist_path())
    }

    pub fn whitelist_count(&self) -> Result<usize, DbError> {
        Ok(self.whitelist_list()?.len())
    }

    pub fn whitelist_has(&self, email: &str) -> Result<bool, DbError> {
        lists::has(&self.whitelist_path(), email)
    }

    pub fn whitelist_remove(&self, email: &str) -> Result<(), DbError> {
        lists::remove(&self.whitelist_path(), email)
    }
}

fn normalize_user_id(user_id: &str) -> Result<&str, DbError> {
    let uid = user_id.trim();
    if uid.is_empty() {
        return Err(DbError::Unauthorized);
    }
    if !uid
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(DbError::Unauthorized);
    }
    Ok(uid)
}
