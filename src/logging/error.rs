use thiserror::Error;

/// Errors returned by logging operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LoggingError {
    /// Failed to resolve the log directory path.
    #[error("{0}")]
    LogDirectory(#[source] Box<dyn std::error::Error>),
}
