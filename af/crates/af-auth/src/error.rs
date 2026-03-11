use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid API key")]
    InvalidKey,
    #[error("API key expired")]
    Expired,
    #[error("user account disabled")]
    Disabled,
    #[error("database error: {0}")]
    DbError(String),
}

impl From<sqlx::Error> for AuthError {
    fn from(e: sqlx::Error) -> Self {
        AuthError::DbError(e.to_string())
    }
}
