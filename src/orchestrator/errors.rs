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
    /// `true` when the Gateway's outer retry loop should pick another
    /// provider and dispatch again. Currently only `Transient` qualifies.
    pub fn is_retryable(&self) -> bool {
        matches!(self, FlowError::Transient { .. })
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
