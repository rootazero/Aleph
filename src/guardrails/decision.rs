//! Guardrail decision returned by Input/Output/ToolCall trait methods.
//!
//! Decisions reuse Stage 1 [`ErrorClass`] (src/error.rs) so all rejection
//! modes share the same retry / fixable / unexpected vocabulary as the rest
//! of the harness.

use crate::error::ErrorClass;

/// Outcome of a guardrail evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardrailDecision {
    /// Content is fine — pass through unchanged.
    Allow,
    /// Replace the content (e.g. PII redaction). Caller MUST swap in
    /// `replacement.text` before continuing.
    Sanitize(Replacement),
    /// Reject and abort. `class` tells the orchestrator how to propagate:
    /// `Fixable` feeds back into the model, `Unexpected` aborts the session.
    Block { reason: String, class: ErrorClass },
    /// Allow but record the warning (no caller-visible mutation).
    Warn { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    pub text: String,
    /// Human-readable label of the rule that fired (used in audit + tracing).
    pub source: String,
}

impl GuardrailDecision {
    pub fn is_block(&self) -> bool {
        matches!(self, GuardrailDecision::Block { .. })
    }
    pub fn is_allow(&self) -> bool {
        matches!(self, GuardrailDecision::Allow)
    }
    pub fn is_sanitize(&self) -> bool {
        matches!(self, GuardrailDecision::Sanitize(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_classifies_correctly() {
        assert!(GuardrailDecision::Allow.is_allow());
        assert!(!GuardrailDecision::Allow.is_block());
        assert!(!GuardrailDecision::Allow.is_sanitize());
    }

    #[test]
    fn block_classifies_correctly() {
        let d = GuardrailDecision::Block {
            reason: "x".into(),
            class: ErrorClass::Fixable,
        };
        assert!(d.is_block());
        assert!(!d.is_allow());
    }

    #[test]
    fn sanitize_classifies_correctly() {
        let d = GuardrailDecision::Sanitize(Replacement {
            text: "redacted".into(),
            source: "pii".into(),
        });
        assert!(d.is_sanitize());
        assert!(!d.is_allow());
        assert!(!d.is_block());
    }
}
