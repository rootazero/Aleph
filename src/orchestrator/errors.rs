//! Flow dispatch error type. See design §6.

use crate::orchestrator::flow_spec::{AgentId, FlowId, ProviderId};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FlowError {
    #[error("unknown flow id: {0}")]
    UnknownFlow(FlowId),

    #[error("unknown agent id: {0}")]
    UnknownAgent(AgentId),

    #[error("flow dispatch cancelled")]
    Cancelled,

    #[error("flow recursion limit ({max}) exceeded")]
    RecursionLimit { max: u8 },

    #[error("session {0} already dispatching")]
    SessionConflict(String),

    #[error("sandbox provision failed: {0}")]
    SandboxProvisionFailed(String),

    #[error("provider unavailable: {0}")]
    ProviderUnavailable(ProviderId),

    #[error("invalid flow config: {0}")]
    InvalidConfig(String),

    #[error("internal dispatch error: {0}")]
    Internal(String),

    /// Transient harness/provider error eligible for provider fallback retry.
    /// Gateway's outer retry loop treats this distinctly from `Internal`:
    /// transient errors allow the loop to resolve another provider and retry.
    #[error("transient harness error ({provider}): {message}")]
    Transient { provider: String, message: String },
}

impl FlowError {
    /// `true` when a caller should pick another provider and dispatch again.
    /// Only `Transient` qualifies.
    ///
    /// The gateway's outer retry loop — which this doc used to name as the
    /// consumer — does not call it: `run_loop/inner.rs` matches
    /// `DispatchFailure::Transient` structurally. Kept as the named predicate
    /// on the public error type so an out-of-crate caller does not have to
    /// re-derive which variants are worth another attempt; if a second
    /// definition of "retryable" ever appears, collapse it onto this one
    /// rather than the other way round.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Transient { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_is_retryable() {
        let err = FlowError::Transient {
            provider: "anthropic".into(),
            message: "network timeout".into(),
        };
        assert!(err.is_retryable());
    }

    #[test]
    fn internal_is_not_retryable() {
        let err = FlowError::Internal("boom".into());
        assert!(!err.is_retryable());
    }

    #[test]
    fn cancelled_is_not_retryable() {
        let err = FlowError::Cancelled;
        assert!(!err.is_retryable());
    }
}
