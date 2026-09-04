//! Guardrail decision returned by Input/Output/ToolCall trait methods.
//!
//! Decisions reuse Stage 1 [`ErrorClass`] (src/error.rs) so all rejection
//! modes share the same retry / fixable / unexpected vocabulary as the rest
//! of the harness.

use crate::error::ErrorClass;

/// Outcome of a guardrail evaluation.
///
/// `#[non_exhaustive]` guards against downstream match-statements breaking
/// when a new variant is added (e.g. a future `Quarantine` or `Defer` outcome).
/// External callers should add a wildcard arm or use the `is_*` helpers
/// below; adding a variant without `#[non_exhaustive]` is a breaking change
/// for every external match.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GuardrailDecision {
    /// Content is fine — pass through unchanged.
    Allow,
    /// Replace the content (e.g. PII redaction). Caller MUST swap in
    /// `replacement.text` before continuing.
    Sanitize(Replacement),
    /// Reject. `class` is advisory metadata describing the *intended*
    /// propagation: `Fixable` = a content/policy block the model could
    /// self-correct, `Unexpected` = a fail-closed/terminal failure.
    ///
    /// It is surfaced through the wrapped `HarnessError` so
    /// `HarnessError::class()` and the security-block trace are accurate, but
    /// control flow does NOT branch on it today: the output call-site turns
    /// every `Block` into a terminal `HarnessError`, the input call-site ends
    /// the turn, and the tool-call call-site skips the single dispatch. The
    /// orchestrator classifies harness errors by message text, not by this
    /// `class` field — a structural class-based matcher was sketched at one
    /// point under a "phase6c" name but no commit landed it. Set `class`
    /// correctly anyway so a future classifier switch is a no-op at the
    /// call-sites.
    Block { reason: String, class: ErrorClass },
    /// Allow but record the warning (no caller-visible mutation).
    Warn { reason: String },
}

/// `#[non_exhaustive]` keeps the field set open for growth — future audit
/// metadata, confidence score, or rule id can be added without breaking
/// downstream destructuring. External callers should construct via
/// [`Replacement::new`] and access fields through methods rather than via
/// struct-literal destructuring.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Replacement {
    pub text: String,
    /// Human-readable label of the rule that fired (used in audit + tracing).
    pub source: String,
}

impl Replacement {
    /// Construct a new `Replacement`. Preferred over struct-literal
    /// initialization so future field additions stay source-compatible.
    #[must_use]
    pub fn new(text: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            source: source.into(),
        }
    }
}

impl GuardrailDecision {
    // Production callers all `match` on the enum directly (which #[non_exhaustive]
    // permits). The four accessors below are kept pub(crate) for the unit-test
    // suite (`tests/` under this module) — they were previously `pub`, but no
    // production caller anywhere in src/ consults them, so widening the API
    // surface was dead. (severed-wire audit 2026-09-04, sw-guardrails-2-1.)
    #[must_use]
    pub(crate) const fn is_block(&self) -> bool {
        matches!(self, Self::Block { .. })
    }
    #[must_use]
    pub(crate) const fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }
    #[must_use]
    pub(crate) const fn is_sanitize(&self) -> bool {
        matches!(self, Self::Sanitize(_))
    }
    #[must_use]
    pub(crate) const fn is_warn(&self) -> bool {
        matches!(self, Self::Warn { .. })
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

    #[test]
    fn warn_classifies_correctly() {
        let d = GuardrailDecision::Warn {
            reason: "suspicious".into(),
        };
        assert!(d.is_warn());
        assert!(!d.is_allow());
        assert!(!d.is_block());
        assert!(!d.is_sanitize());
    }
}
