//! Private admission seam for asynchronous special-capability calls.

use crate::capability::facade::ResolveError;
use crate::capability::policy::PolicyDenial;
use crate::capability::special::CapabilityDispatchError;

/// Trusted identity carried only by a captured Core domain adapter's owned task.
#[derive(Clone)]
pub(crate) struct DomainToolCall {
    pub(crate) context: crate::capability::policy_runtime::CapabilityCallContext,
    pub(crate) owner_actor: Option<String>,
    pub(crate) cancel: tokio_util::sync::CancellationToken,
}

tokio::task_local! {
    pub(crate) static DOMAIN_TOOL_CALL: DomainToolCall;
}

/// Origin evidence produced by a canonical authenticated gate. Client fields
/// cannot construct this enum because its variants are crate-private.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CapabilityInvocationOrigin {
    CanonicalIngress,
    GatewayDomain,
}

#[derive(Debug, thiserror::Error)]
pub enum CapabilityInvocationError {
    #[error("special capability source is unavailable")]
    Unavailable,
    #[error("special capability resolution failed: {0:?}")]
    Resolve(ResolveError),
    #[error("special capability policy denied: {0}")]
    Policy(#[source] PolicyDenial),
    #[error("special capability handler failed: {0}")]
    Handler(#[source] CapabilityDispatchError),
    #[error("special capability invocation was cancelled")]
    Cancelled,
}

impl From<CapabilityDispatchError> for CapabilityInvocationError {
    fn from(error: CapabilityDispatchError) -> Self {
        match error {
            CapabilityDispatchError::Cancelled => Self::Cancelled,
            CapabilityDispatchError::Unavailable => Self::Unavailable,
            other => Self::Handler(other),
        }
    }
}
