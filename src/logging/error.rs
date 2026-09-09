use thiserror::Error;

/// Errors returned by logging operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LoggingError {
    /// Failed to resolve the log directory path. The wrapped error carries
    /// the underlying cause; a descriptive prefix here makes the variant
    /// discoverable in flat `format!("{e}")` log lines (the RPC layer in
    /// `gateway/handlers/logs.rs` uses this shape and would otherwise emit
    /// the cause's bare message).
    #[error("failed to resolve log directory: {0}")]
    LogDirectory(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Runtime log filter could not be updated (e.g. shared logging not yet
    /// initialized). The reported atomic level may still have been updated;
    /// callers can choose to surface or ignore this.
    #[error("runtime log filter unavailable: {0}")]
    FilterUnavailable(String),
}
