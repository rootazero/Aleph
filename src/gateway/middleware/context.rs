//! Middleware request context — carried through the Tower middleware chain.
//!
//! `GatewayRequestContext` is created at the edge of each inbound JSON-RPC request
//! and propagated through all middleware layers to the terminal `HandlerService`.
//! It provides request-scoped identity and tracing information without using
//! `AsyncLocalStorage` — all context is explicit and passed via Tower's `Service` pattern.

use std::fmt;

use uuid::Uuid;

use crate::gateway::security::identity_map::UserId;

/// Request-scoped context propagated through the middleware chain.
///
/// Each inbound request gets a fresh context with a generated `request_id`
/// for trace correlation. Auth middleware populates `user_id`; downstream
/// middleware and the handler can read it.
#[derive(Debug, Clone)]
pub struct GatewayRequestContext {
    /// Unique identifier for trace correlation across layers.
    pub request_id: Uuid,
    /// Authenticated user identity, set by `AuthLayer` if token is valid.
    pub user_id: Option<UserId>,
    /// Client identifier (IP address, device fingerprint, or pairing identity).
    pub client_id: String,
    /// Trace flags for fine-grained observability control.
    pub trace_flags: TraceFlags,
}

impl GatewayRequestContext {
    /// Create a new context with a fresh UUID and no authenticated identity.
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            request_id: Uuid::new_v4(),
            user_id: None,
            client_id: client_id.into(),
            trace_flags: TraceFlags::default(),
        }
    }

    /// Set the authenticated user identity.
    pub fn with_user_id(mut self, user_id: UserId) -> Self {
        self.user_id = Some(user_id);
        self
    }

    /// Set trace flags.
    pub fn with_trace_flags(mut self, flags: TraceFlags) -> Self {
        self.trace_flags = flags;
        self
    }
}

impl Default for GatewayRequestContext {
    fn default() -> Self {
        Self::new("anonymous")
    }
}

/// Trace control flags for a single request.
///
/// These flags allow middleware to conditionally emit trace data based on
/// user preferences or runtime configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TraceFlags {
    /// Enable detailed span timing for this request.
    pub detailed_timing: bool,
    /// Include request parameters in trace logs.
    pub include_params: bool,
    /// Propagate trace context to sub-requests.
    pub propagate: bool,
}

impl TraceFlags {
    /// Standard trace flags suitable for most requests.
    pub fn standard() -> Self {
        Self {
            detailed_timing: true,
            include_params: false,
            propagate: false,
        }
    }
}

impl fmt::Display for GatewayRequestContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "req_id={}", self.request_id)?;
        if let Some(ref uid) = self.user_id {
            write!(f, " user={uid:?}")?;
        }
        write!(f, " client={}", self.client_id)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_new_generates_unique_id() {
        let ctx1 = GatewayRequestContext::new("test_client");
        let ctx2 = GatewayRequestContext::new("test_client");
        assert_ne!(ctx1.request_id, ctx2.request_id);
        assert!(ctx1.user_id.is_none());
        assert_eq!(ctx1.client_id, "test_client");
    }

    #[test]
    fn test_context_with_user_id() {
        let ctx = GatewayRequestContext::new("test_client").with_user_id(UserId::Owner);
        assert_eq!(ctx.user_id, Some(UserId::Owner));
    }

    #[test]
    fn test_context_clone_is_independent() {
        let ctx1 = GatewayRequestContext::new("test_client");
        let ctx2 = ctx1.clone();
        // Mutating clone doesn't affect original (user_id is immutable per request)
        assert_eq!(ctx1.request_id, ctx2.request_id);
        assert_eq!(ctx1.client_id, ctx2.client_id);
    }

    #[test]
    fn test_trace_flags_default() {
        let flags = TraceFlags::default();
        assert!(!flags.detailed_timing);
        assert!(!flags.include_params);
        assert!(!flags.propagate);
    }

    #[test]
    fn test_trace_flags_standard() {
        let flags = TraceFlags::standard();
        assert!(flags.detailed_timing);
        assert!(!flags.include_params);
        assert!(!flags.propagate);
    }
}
