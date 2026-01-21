use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error, Serialize)]
pub enum AetherError {
    #[error("Network error: {0}")]
    Network(String),

    #[error("API timeout after {0}ms")]
    Timeout(u64),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("MCP server error: {0}")]
    McpServer(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Window error: {0}")]
    Window(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Core error: {0}")]
    Core(String),

    #[error("IO error: {0}")]
    Io(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Unknown error: {0}")]
    Unknown(String),
}

impl From<std::io::Error> for AetherError {
    fn from(e: std::io::Error) -> Self {
        AetherError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for AetherError {
    fn from(e: serde_json::Error) -> Self {
        AetherError::Serialization(e.to_string())
    }
}

impl From<tauri::Error> for AetherError {
    fn from(e: tauri::Error) -> Self {
        AetherError::Window(e.to_string())
    }
}

// Convert to String for Tauri command returns
impl From<AetherError> for String {
    fn from(e: AetherError) -> Self {
        serde_json::to_string(&e).unwrap_or_else(|_| e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AetherError>;
