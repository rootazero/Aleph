use thiserror::Error;

/// Errors returned by logging operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LoggingError {
    /// Failed to resolve the log directory path.
    #[error("{0}")]
    LogDirectory(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Runtime log filter could not be updated (e.g. shared logging not yet
    /// initialized). The reported atomic level may still have been updated;
    /// callers can choose to surface or ignore this.
    #[error("runtime log filter unavailable: {0}")]
    FilterUnavailable(String),
}
