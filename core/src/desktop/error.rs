use thiserror::Error;

#[derive(Debug, Error)]
pub enum DesktopError {
    #[error("Aleph macOS App is not running. Open Aleph.app to use desktop capabilities.")]
    AppNotRunning,

    #[error("Desktop bridge connection failed: {0}")]
    ConnectionFailed(#[from] std::io::Error),

    #[error("Desktop bridge protocol error: {0}")]
    Protocol(String),

    #[error("Desktop operation failed: {0}")]
    Operation(String),
}
