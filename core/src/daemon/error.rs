use thiserror::Error;

pub type Result<T> = std::result::Result<T, DaemonError>;

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("Service operation failed: {0}")]
    ServiceError(String),

    #[error("IPC error: {0}")]
    IpcError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Resource governor error: {0}")]
    ResourceGovernor(String),

    #[error("Event bus error: {0}")]
    EventBus(String),

    #[error("WorldModel error: {0}")]
    WorldModel(#[from] anyhow::Error),
}
