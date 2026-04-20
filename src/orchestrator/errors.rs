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
}
