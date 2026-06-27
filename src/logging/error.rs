use thiserror::Error;

/// Errors returned by logging operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LoggingError {
    /// Failed to initialize logging for a component.
    #[error("{0}")]
    Init(#[source] Box<dyn std::error::Error>),

    /// Failed to resolve the log directory path.
    #[error("{0}")]
    LogDirectory(#[source] Box<dyn std::error::Error>),

    /// Failed to clean up old log files.
    #[error("{0}")]
    Cleanup(#[source] Box<dyn std::error::Error>),
}
