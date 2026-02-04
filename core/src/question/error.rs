// Aleph/core/src/question/error.rs
//! Question error types.

use thiserror::Error;

/// Question-related errors
#[derive(Debug, Clone, Error)]
pub enum QuestionError {
    /// User dismissed/rejected the question
    #[error("User dismissed the question")]
    Rejected,

    /// Question request timed out
    #[error("Question timed out after {timeout_ms}ms")]
    Timeout {
        /// Request ID
        request_id: String,
        /// Timeout duration in milliseconds
        timeout_ms: u64,
    },

    /// Invalid answer format
    #[error("Invalid answer: {reason}")]
    InvalidAnswer {
        /// Reason the answer is invalid
        reason: String,
    },

    /// Internal error
    #[error("Question system error: {0}")]
    Internal(String),
}

impl QuestionError {
    /// Create a timeout error
    pub fn timeout(request_id: impl Into<String>, timeout_ms: u64) -> Self {
        Self::Timeout {
            request_id: request_id.into(),
            timeout_ms,
        }
    }

    /// Create an invalid answer error
    pub fn invalid_answer(reason: impl Into<String>) -> Self {
        Self::InvalidAnswer {
            reason: reason.into(),
        }
    }

    /// Check if this is a user rejection
    pub fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected)
    }

    /// Check if this is a timeout
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_question_error_display() {
        let err = QuestionError::Rejected;
        assert!(err.to_string().contains("dismissed"));

        let err = QuestionError::timeout("q-1", 5000);
        assert!(err.to_string().contains("5000ms"));

        let err = QuestionError::invalid_answer("missing required field");
        assert!(err.to_string().contains("missing required field"));
    }

    #[test]
    fn test_error_classification() {
        assert!(QuestionError::Rejected.is_rejected());
        assert!(!QuestionError::Rejected.is_timeout());

        let timeout = QuestionError::timeout("q-1", 1000);
        assert!(timeout.is_timeout());
        assert!(!timeout.is_rejected());
    }
}
