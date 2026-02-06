//! Error types for Aleph CLI

use thiserror::Error;

/// CLI error type
#[derive(Error, Debug)]
pub enum CliError {
    #[error("Connection error: {0}")]
    Connection(String),

    #[error("WebSocket error: {0}")]
    WebSocket(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("RPC error ({code}): {message}")]
    Rpc { code: i32, message: String },

    #[error("Authentication required")]
    AuthRequired,

    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Timeout waiting for response")]
    Timeout,

    #[error("Server disconnected")]
    Disconnected,

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("{0}")]
    Other(String),
}

/// Result type alias for CLI operations
pub type CliResult<T> = Result<T, CliError>;

impl From<anyhow::Error> for CliError {
    fn from(err: anyhow::Error) -> Self {
        CliError::Other(err.to_string())
    }
}
