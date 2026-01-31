//! CLI error types.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum CliError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("RPC error: {0}")]
    RpcError(String),

    #[error("Timeout after {0}ms")]
    Timeout(u64),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}
