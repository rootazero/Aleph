//! Cortex 2.0 unified error types
//!
//! Provides structured error types for all Cortex operations:
//! - JSON parsing and repair
//! - Schema validation
//! - Security filtering
//! - Token budget management
//! - User confirmation flows

use thiserror::Error;

/// Unified error type for Cortex 2.0 operations
#[derive(Debug, Error)]
pub enum CortexError {
    /// JSON parsing failed, possibly recoverable with repair
    #[error("JSON parse failed: {message}")]
    ParseError {
        message: String,
        raw_input: String,
        recovery_attempted: bool,
    },

    /// Tool arguments don't match expected schema
    #[error("Schema validation failed for tool '{tool}': {reason}")]
    SchemaValidationError {
        tool: String,
        reason: String,
        expected_schema: Option<String>,
    },

    /// Input blocked by security rules
    #[error("Input blocked by security rule '{rule}': {reason}")]
    SecurityBlocked {
        rule: String,
        reason: String,
        severity: SecuritySeverity,
    },

    /// PII was detected and masked (informational, not fatal)
    #[error("PII detected and masked: {count} occurrences")]
    PiiMasked { count: usize },

    /// No tool matched the user's intent
    #[error("No tool matched (confidence: {confidence:.2})")]
    NoMatch { confidence: f32 },

    /// User confirmation timed out
    #[error("Confirmation timeout after {timeout_ms}ms")]
    ConfirmationTimeout { timeout_ms: u64 },

    /// User explicitly rejected tool execution
    #[error("User rejected tool execution")]
    UserRejected,

    /// Context window overflow
    #[error("Context overflow: {overflow} tokens over limit")]
    ContextOverflow { overflow: usize },

    /// Token counting operation failed
    #[error("Token counting failed: {reason}")]
    TokenCountError { reason: String },

    /// Configuration file load failure
    #[error("Config load failed: {path}")]
    ConfigLoadError {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// Configuration validation failure
    #[error("Config validation failed: {reason}")]
    ConfigValidationError { reason: String },
}

/// Security severity levels for blocked inputs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecuritySeverity {
    /// Minor concern, possibly false positive
    Low,
    /// Moderate concern, should be reviewed
    Medium,
    /// Serious security risk
    High,
    /// Immediate threat, must be blocked
    Critical,
}

/// Suggested recovery action for errors
#[derive(Debug, Clone)]
pub enum RecoveryHint {
    /// Retry with JSON repair applied
    RetryWithRepair,
    /// Fall back to conversational response
    FallbackToChat,
    /// Truncate context and retry
    TruncateAndRetry,
    /// Ask user to clarify or confirm
    PromptUserAgain,
    /// No recovery possible, abort
    Abort,
}

impl CortexError {
    /// Returns a suggested recovery action based on error type
    pub fn recovery_hint(&self) -> RecoveryHint {
        match self {
            Self::ParseError {
                recovery_attempted: false,
                ..
            } => RecoveryHint::RetryWithRepair,
            Self::NoMatch { confidence } if *confidence > 0.2 => RecoveryHint::FallbackToChat,
            Self::ContextOverflow { .. } => RecoveryHint::TruncateAndRetry,
            Self::ConfirmationTimeout { .. } => RecoveryHint::PromptUserAgain,
            _ => RecoveryHint::Abort,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_error_recovery_hint() {
        let err = CortexError::ParseError {
            message: "unexpected EOF".to_string(),
            raw_input: "{\"foo\":".to_string(),
            recovery_attempted: false,
        };
        assert!(matches!(err.recovery_hint(), RecoveryHint::RetryWithRepair));

        let err_retried = CortexError::ParseError {
            message: "still broken".to_string(),
            raw_input: "{{{".to_string(),
            recovery_attempted: true,
        };
        assert!(matches!(err_retried.recovery_hint(), RecoveryHint::Abort));
    }

    #[test]
    fn test_no_match_recovery_hint() {
        let low_confidence = CortexError::NoMatch { confidence: 0.1 };
        assert!(matches!(low_confidence.recovery_hint(), RecoveryHint::Abort));

        let medium_confidence = CortexError::NoMatch { confidence: 0.3 };
        assert!(matches!(
            medium_confidence.recovery_hint(),
            RecoveryHint::FallbackToChat
        ));
    }

    #[test]
    fn test_context_overflow_recovery() {
        let err = CortexError::ContextOverflow { overflow: 1000 };
        assert!(matches!(
            err.recovery_hint(),
            RecoveryHint::TruncateAndRetry
        ));
    }

    #[test]
    fn test_security_severity_equality() {
        assert_eq!(SecuritySeverity::High, SecuritySeverity::High);
        assert_ne!(SecuritySeverity::Low, SecuritySeverity::Critical);
    }

    #[test]
    fn test_error_display() {
        let err = CortexError::SecurityBlocked {
            rule: "command_injection".to_string(),
            reason: "detected shell metacharacters".to_string(),
            severity: SecuritySeverity::Critical,
        };
        assert!(err.to_string().contains("command_injection"));
        assert!(err.to_string().contains("shell metacharacters"));
    }
}
