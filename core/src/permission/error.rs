// Aether/core/src/permission/error.rs
//! Permission error types.

use super::rule::PermissionRule;
use thiserror::Error;

/// Permission-related errors
#[derive(Debug, Clone, Error)]
pub enum PermissionError {
    /// User rejected the permission request (no message)
    #[error("User rejected permission for this operation")]
    Rejected,

    /// User rejected with feedback message
    /// The message can be used as guidance for the agent
    #[error("User rejected with feedback: {message}")]
    Corrected {
        /// User's feedback/correction message
        message: String,
    },

    /// Permission denied by rule (not user interaction)
    #[error("Permission denied by rule: {permission} on pattern '{pattern}'")]
    Denied {
        /// Permission type that was denied
        permission: String,
        /// Pattern that was denied
        pattern: String,
        /// The rule that caused the denial
        rule: PermissionRule,
    },

    /// Request timed out waiting for user response
    #[error("Permission request timed out after {timeout_ms}ms")]
    Timeout {
        /// Request ID
        request_id: String,
        /// Timeout duration in milliseconds
        timeout_ms: u64,
    },

    /// Internal error
    #[error("Permission system error: {0}")]
    Internal(String),
}

impl PermissionError {
    /// Create a denied error
    pub fn denied(permission: impl Into<String>, pattern: impl Into<String>, rule: PermissionRule) -> Self {
        Self::Denied {
            permission: permission.into(),
            pattern: pattern.into(),
            rule,
        }
    }

    /// Create a corrected error
    pub fn corrected(message: impl Into<String>) -> Self {
        Self::Corrected {
            message: message.into(),
        }
    }

    /// Create a timeout error
    pub fn timeout(request_id: impl Into<String>, timeout_ms: u64) -> Self {
        Self::Timeout {
            request_id: request_id.into(),
            timeout_ms,
        }
    }

    /// Check if this error allows continued execution with modified behavior
    pub fn allows_continuation(&self) -> bool {
        matches!(self, Self::Corrected { .. })
    }

    /// Check if this is a hard rejection
    pub fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected | Self::Denied { .. })
    }

    /// Get the feedback message if this is a corrected error
    pub fn feedback(&self) -> Option<&str> {
        match self {
            Self::Corrected { message } => Some(message),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::PermissionAction;

    #[test]
    fn test_permission_error_display() {
        let err = PermissionError::Rejected;
        assert!(err.to_string().contains("rejected"));

        let err = PermissionError::corrected("Use safer command");
        assert!(err.to_string().contains("Use safer command"));

        let rule = PermissionRule {
            permission: "bash".into(),
            pattern: "rm -rf *".into(),
            action: PermissionAction::Deny,
        };
        let err = PermissionError::denied("bash", "rm -rf /", rule);
        assert!(err.to_string().contains("bash"));
        assert!(err.to_string().contains("denied"));
    }

    #[test]
    fn test_error_classification() {
        assert!(PermissionError::Rejected.is_rejected());
        assert!(!PermissionError::Rejected.allows_continuation());

        let corrected = PermissionError::corrected("try this instead");
        assert!(!corrected.is_rejected());
        assert!(corrected.allows_continuation());
        assert_eq!(corrected.feedback(), Some("try this instead"));
    }
}
