use thiserror::Error;

/// Client SDK error types
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    #[error("RPC error: {0}")]
    RpcError(String),

    #[error("Pairing timeout")]
    PairingTimeout,

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("WebSocket error: {0}")]
    WebSocketError(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Timeout")]
    Timeout,

    #[error("Connection closed")]
    ConnectionClosed,
}

#[cfg(feature = "transport")]
impl From<tokio_tungstenite::tungstenite::Error> for ClientError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        ClientError::WebSocketError(e.to_string())
    }
}

#[cfg(feature = "rpc")]
impl From<serde_json::Error> for ClientError {
    fn from(e: serde_json::Error) -> Self {
        ClientError::SerializationError(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, ClientError>;
