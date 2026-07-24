//! Initialization error types

use thiserror::Error;

/// Error during initialization
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
#[error("[{phase}] {message}")]
pub struct InitError {
    /// Which phase failed
    pub phase: String,
    /// Error message
    pub message: String,
    /// Whether retry might succeed
    pub is_retryable: bool,
}

impl InitError {
    pub fn new(phase: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            phase: phase.into(),
            message: message.into(),
            is_retryable: true,
        }
    }

    pub fn non_retryable(phase: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            phase: phase.into(),
            message: message.into(),
            is_retryable: false,
        }
    }
}
