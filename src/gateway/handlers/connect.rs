//! Connect handler — session handshake (no authentication).
//!
//! LAN-trust model: every connection is implicitly the owner/operator.
//! The handshake only delivers server state baseline + keepalive policy.

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

/// Handle "connect" — accepts and ignores any legacy params (token/
/// device_name/...) so old clients don't break mid-rollout.
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

    #[tokio::test]
    async fn bare_connect_succeeds_as_operator() {
        let req = JsonRpcRequest::with_id("connect", Some(json!({})), json!(1));
        let resp = handle_connect(req, ctx()).await;
        assert!(resp.is_success(), "{resp:?}");
        let result = resp.result.unwrap();
        // Panel reads `role`; LAN-trust always reports operator.
        assert_eq!(
            result.get("role").and_then(|v| v.as_str()),
            Some("operator")
        );
        assert!(result.get("state_version").is_some());
        assert!(result.get("keepalive").is_some());
    }

    #[tokio::test]
    async fn legacy_token_params_are_ignored() {
        // Old clients still send a token; the handshake must accept and ignore it.
        let req = JsonRpcRequest::with_id(
            "connect",
            Some(json!({"token": "legacy:sig", "device_name": "Old Client"})),
            json!(2),
        );
        let resp = handle_connect(req, ctx()).await;
        assert!(resp.is_success(), "{resp:?}");
        assert_eq!(
            resp.result.unwrap().get("role").and_then(|v| v.as_str()),
            Some("operator")
        );
    }
}
