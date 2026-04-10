//! Signal channel errors
//!
//! Structured error types for Signal operations including API errors,
//! SSE errors, and configuration validation errors.

use std::time::Duration;

use thiserror::Error;

/// Signal-specific errors
#[derive(Debug, Error)]
pub enum SignalError {
    // =====================================================================
    // API Errors
    // =====================================================================
    /// HTTP request failed
    #[error("HTTP request failed: {0}")]
    HttpRequest(#[from] reqwest::Error),

    /// Server returned an error response
    #[error("Signal API error ({status}): {message}")]
    ApiError {
        status: reqwest::StatusCode,
        message: String,
    },

    /// Rate limited by signal-cli
    #[error("Rate limited by signal-cli, retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    // =====================================================================
    // SSE Errors
    // =====================================================================
    /// SSE connection failed
    #[error("SSE connection failed: {0}")]
    SseConnection(String),

    /// SSE stream error
    #[error("SSE stream error: {0}")]
    SseStream(String),

    /// SSE connection lost after being established
    #[error("SSE connection lost: {0}")]
    SseConnectionLost(String),

    /// Maximum retry attempts exceeded for SSE
    #[error("SSE max retries ({max_retries}) exceeded, last error: {last_error}")]
    SseMaxRetries {
        max_retries: u32,
        last_error: String,
    },

    // =====================================================================
    // Message Parsing Errors
    // =====================================================================
    /// Failed to parse JSON response
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    /// Invalid message envelope from signal-cli
    #[error("Invalid message envelope: {0}")]
    InvalidEnvelope(String),

    /// Empty message content
    #[error("Empty message content")]
    EmptyMessage,

    // =====================================================================
    // Send Errors
    // =====================================================================
    /// Message too large
    #[error("Message too large: {size} bytes (max: {max_size})")]
    MessageTooLarge { size: usize, max_size: usize },

    /// Failed to send message
    #[error("Send failed: {0}")]
    SendFailed(String),

    // =====================================================================
    // Configuration Errors
    // =====================================================================
    /// Invalid phone number format
    #[error("Invalid phone number format: {0}")]
    InvalidPhoneNumber(String),

    /// Invalid API URL
    #[error("Invalid API URL: {0}")]
    InvalidApiUrl(String),

    // =====================================================================
    // Channel Errors
    // =====================================================================
    /// Inbound channel closed
    #[error("Inbound channel closed")]
    ChannelClosed,

    /// Not connected to signal-cli
    #[error("Not connected to signal-cli")]
    NotConnected,
}

// =============================================================================
// Error Conversion
// =============================================================================

impl SignalError {
    /// Convert a reqwest status error into a SignalError
    pub fn from_status(status: reqwest::StatusCode, body: &str) -> Self {
        if status.as_u16() == 429 {
            // Try to extract retry-after from body
            let retry_after = body
                .lines()
                .find(|l| l.starts_with("retry-after"))
                .and_then(|l| l.split(':').nth(1))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(60);

            Self::RateLimited {
                retry_after_secs: retry_after,
            }
        } else {
            Self::ApiError {
                status,
                message: body.to_string(),
            }
        }
    }

    /// Check if error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. }
                | Self::SseConnection(_)
                | Self::SseConnectionLost(_)
                | Self::HttpRequest(_)
        )
    }

    /// Get suggested retry delay for retryable errors
    pub fn retry_delay(&self, default: Duration) -> Duration {
        match self {
            Self::RateLimited { retry_after_secs } => Duration::from_secs(*retry_after_secs),
            _ => default,
        }
    }
}

// =============================================================================
// From ChannelError for ergonomic conversion
// =============================================================================

impl From<SignalError> for crate::gateway::channel::ChannelError {
    fn from(err: SignalError) -> Self {
        use crate::gateway::channel::ChannelError;
        match err {
            SignalError::HttpRequest(e) => ChannelError::ReceiveFailed(e.to_string()),
            SignalError::ApiError { status, message } => {
                ChannelError::ReceiveFailed(format!("({status}): {message}"))
            }
            SignalError::RateLimited { retry_after_secs } => {
                ChannelError::RateLimited { retry_after_secs }
            }
            SignalError::MessageTooLarge { size, max_size } => {
                ChannelError::MessageTooLarge { size, max_size }
            }
            SignalError::SendFailed(msg) => ChannelError::SendFailed(msg),
            SignalError::NotConnected => ChannelError::NotConnected("Signal".to_string()),
            SignalError::InvalidPhoneNumber(msg) => ChannelError::ConfigError(msg),
            SignalError::InvalidApiUrl(msg) => ChannelError::ConfigError(msg),
            SignalError::ChannelClosed => {
                ChannelError::Internal("Signal channel closed".to_string())
            }
            // SSE, JSON, envelope errors map to ReceiveFailed
            SignalError::SseConnection(e) => ChannelError::ReceiveFailed(e.to_string()),
            SignalError::SseStream(e) => ChannelError::ReceiveFailed(e.to_string()),
            SignalError::SseConnectionLost(e) => ChannelError::ReceiveFailed(e),
            SignalError::SseMaxRetries { last_error, .. } => {
                ChannelError::ReceiveFailed(format!("SSE max retries exceeded: {last_error}"))
            }
            SignalError::JsonParse(e) => ChannelError::ReceiveFailed(e.to_string()),
            SignalError::InvalidEnvelope(e) => ChannelError::ReceiveFailed(e),
            SignalError::EmptyMessage => ChannelError::ReceiveFailed("Empty message".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_error_display() {
        let err = SignalError::InvalidPhoneNumber("+123".to_string());
        assert!(err.to_string().contains("Invalid phone number"));

        let err = SignalError::RateLimited {
            retry_after_secs: 30,
        };
        assert!(err.to_string().contains("retry after 30s"));
    }

    #[test]
    fn test_signal_error_is_retryable() {
        assert!(SignalError::RateLimited {
            retry_after_secs: 60
        }
        .is_retryable());
        assert!(!SignalError::InvalidPhoneNumber("x".to_string()).is_retryable());
    }

    #[test]
    fn test_from_status_rate_limited() {
        let err = SignalError::from_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "rate limit exceeded",
        );
        match err {
            SignalError::RateLimited { retry_after_secs } => {
                assert_eq!(retry_after_secs, 60);
            }
            _ => panic!("expected RateLimited"),
        }
    }

    #[test]
    fn test_from_status_other() {
        let err = SignalError::from_status(reqwest::StatusCode::BAD_REQUEST, "invalid request");
        match err {
            SignalError::ApiError { status, message } => {
                assert_eq!(status.as_u16(), 400);
                assert_eq!(message, "invalid request");
            }
            _ => panic!("expected ApiError"),
        }
    }
}
