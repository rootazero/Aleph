use thiserror::Error;

#[derive(Error, Debug)]
pub enum NativeBaileysError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Message send failed: {0}")]
    SendFailed(String),

    #[error("Vault error: {0}")]
    VaultError(String),

    #[error("Event mapping error: {0}")]
    EventMappingError(String),

    #[error("Media error: {0}")]
    MediaError(String),

    #[error("Client not connected")]
    NotConnected,

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Protocol error: {0}")]
    ProtocolError(String),
}
