//! Connect handler — session handshake + Gateway-token authorization seam.
//!
//! Transport model: the Panel connects over plain WS (same-origin HTTP),
//! identical to a browser opening the core's LAN IP — there is no channel
//! pipeline and no shell-only shortcut. Authorization is a single shared
//! Gateway token (`aleph-<uuid>`, provisioned at boot by `SharedTokenManager`):
//!
//! - **loopback** (the local desktop App / same machine) is always authorized
//!   as operator — zero-config, no token required.
//! - **remote** connections must present the shared Gateway token; a valid
//!   token grants the *same* operator authority as local (single tier — there
//!   is no Chat/Config split). A missing/invalid token leaves the connection
//!   unauthorized: the WS dispatch login wall refuses everything but `connect`,
//!   and the Panel renders a token box.
//!
//! `handle_connect` returns only the session baseline. The actual
//! authorization decision needs the per-connection client IP (loopback?) and
//! the process-global `SharedTokenManager`, both of which live in
//! `server::handler`; it calls [`connect_authorized`] at the handshake and
//! stamps the resolved role onto the connection state. Keeping the decision in
//! one tested pure function lets the handshake stay a thin wiring layer.

use crate::sync_primitives::Arc;
use serde_json::json;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Context for the connect handshake.
pub struct ConnectContext {
    /// Monotonic state-version tracker, shared with `GatewayServer`.
    /// Surfaced in the `connect` success response so clients capture a
    /// baseline snapshot at handshake time.
    pub state_versions: Arc<crate::gateway::state_version::StateVersionTracker>,
    /// Transport keep-alive policy returned to clients at handshake time.
    /// Sourced from `GatewayConfig::{ping_interval_secs, idle_timeout_secs}`
    /// so client and server agree on the live cadence.
    pub transport_policy: crate::gateway::handlers::auth::TransportPolicy,
}

/// Authorization decision for a WS handshake under the Gateway-token model.
///
/// Loopback is always authorized (zero-config operator). A remote connection
/// is authorized only when it presents a non-empty token that `validate`
/// accepts. `validate` is injected so this stays a pure, host-testable
/// predicate — production passes a closure over
/// `SharedTokenManager::global().validate`.
#[must_use]
pub fn connect_authorized(
    is_loopback: bool,
    token: Option<&str>,
    validate: impl Fn(&str) -> bool,
) -> bool {
    if is_loopback {
        return true;
    }
    matches!(token, Some(t) if !t.is_empty() && validate(t))
}

/// Handle "connect" — returns the session baseline. `server::handler` overlays
/// the authorization verdict (`role` / `authorized` / `needs_token`) computed
/// via [`connect_authorized`]; the `role` here is just a default for any path
/// that bypasses that overlay.
pub async fn handle_connect(request: JsonRpcRequest, ctx: Arc<ConnectContext>) -> JsonRpcResponse {
    JsonRpcResponse::success(
        request.id,
        json!({
            "role": "operator",
            "state_version": ctx.state_versions.snapshot(),
            "keepalive": ctx.transport_policy.clone(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::handlers::auth::TransportPolicy;

    fn ctx() -> Arc<ConnectContext> {
        Arc::new(ConnectContext {
            state_versions: Arc::new(crate::gateway::state_version::StateVersionTracker::new()),
            transport_policy: TransportPolicy::defaults(),
        })
    }

    #[test]
    fn loopback_is_always_authorized() {
        // Local machine never needs a token (zero-config operator).
        assert!(connect_authorized(true, None, |_| false));
        assert!(connect_authorized(true, Some(""), |_| false));
        assert!(connect_authorized(true, Some("anything"), |_| false));
    }

    #[test]
    fn remote_requires_a_valid_token() {
        let valid = |t: &str| t == "aleph-good";
        assert!(connect_authorized(false, Some("aleph-good"), valid));
        assert!(!connect_authorized(false, Some("aleph-bad"), valid));
        assert!(!connect_authorized(false, Some(""), |_| true));
        assert!(!connect_authorized(false, None, |_| true));
    }

    #[tokio::test]
    async fn bare_connect_returns_session_baseline() {
        let req = JsonRpcRequest::with_id("connect", Some(json!({})), json!(1));
        let resp = handle_connect(req, ctx()).await;
        assert!(resp.is_success(), "{resp:?}");
        let result = resp.result.unwrap();
        // Baseline role; the real verdict is overlaid by server::handler.
        assert_eq!(
            result.get("role").and_then(|v| v.as_str()),
            Some("operator")
        );
        assert!(result.get("state_version").is_some());
        assert!(result.get("keepalive").is_some());
    }
}
