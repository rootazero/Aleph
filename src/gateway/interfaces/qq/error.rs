use crate::gateway::channel::ChannelError;

#[derive(Debug, thiserror::Error)]
pub enum QQError {
    #[error("HTTP error: {status} - {body}")]
    HttpError { status: u16, body: String },

    #[error("Auth failed: {0}")]
    AuthFailed(String),

    #[error("Rate limited: retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    #[error("Send failed: {0}")]
    SendFailed(String),

    #[error("Gateway error: {0}")]
    GatewayError(String),
}

impl From<QQError> for ChannelError {
    fn from(e: QQError) -> Self {
        match e {
            QQError::AuthFailed(msg) => ChannelError::AuthFailed(msg),
            QQError::RateLimited { retry_after_secs } => {
                ChannelError::RateLimited { retry_after_secs }
            }
            QQError::HttpError { status, body } => {
                ChannelError::SendFailed(format!("HTTP {}: {}", status, body))
            }
            QQError::SendFailed(msg) => ChannelError::SendFailed(msg),
            QQError::GatewayError(msg) => ChannelError::NotConnected(msg),
        }
    }
}
