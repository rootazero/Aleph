//! Error types for Aleph CLI

use thiserror::Error;

/// CLI error type
#[derive(Error, Debug)]
pub enum CliError {
    #[error("Connection error: {0}")]
    Connection(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("RPC error ({code}): {message}")]
    Rpc { code: i32, message: String },

    #[error("Timeout waiting for response: {0}")]
    Timeout(String),

    #[error("Server disconnected: {0}")]
    Disconnected(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("{0}")]
    Other(String),
}

/// Result type alias for CLI operations
pub type CliResult<T> = Result<T, CliError>;
