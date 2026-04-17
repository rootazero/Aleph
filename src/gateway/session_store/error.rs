use thiserror::Error;

#[derive(Debug, Error)]
pub enum SessionStoreError {
    #[error("Database error: {0}")]
    DatabaseError(String),
    #[error("Session not found: {0}")]
    NotFound(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Unsupported operation on this backend")]
    Unsupported,
}
