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
    #[error("invalid conversation")]
    InvalidConversation,
    #[error("invalid data: {0}")]
    InvalidData(String),
    #[error("not found")]
    NotFound,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
