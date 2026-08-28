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
    /// The session exists but is in a terminal state (stopped) and cannot be
    /// silently resumed — the caller must explicitly reopen it first.
    #[error("Session is stopped: {0}")]
    Stopped(String),
}
