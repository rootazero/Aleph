//! `gateway.identity.get` — per-process gateway identity for client
//! sanity checks (version match, restart detection, protocol negotiation).
//!
//! Distinct from the `identity.*` handlers in `identity.rs`, which manage
//! the AI soul/persona. This handler reports the *gateway process*
//! identity, not the assistant's.

use std::sync::Arc;

use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::gateway::state_version::StateVersionTracker;

/// Protocols this gateway build speaks. Static for the process lifetime.
const SUPPORTED_PROTOCOLS: [&str; 3] = ["jsonrpc/2.0", "openai-compat/v1", "a2a/v1"];

/// Immutable snapshot captured at phase-2 boot and moved into the
/// `gateway.identity.get` handler closure.
///
/// Deliberately small: it carries only the three process-identity values
/// plus a method-count snapshot. It does NOT capture `Arc<GatewaySharedState>`
/// — doing so would clone the `Arc<HandlerRegistry>` and make a later
/// `handlers_mut()` (which relies on `Arc::get_mut`) panic, and would also
/// create a registry→closure→registry reference cycle.
#[derive(Clone)]
pub struct GatewayIdentitySnapshot {
    /// Per-process UUID v4, regenerated on every restart.
    pub instance_id: String,
    /// Unix epoch seconds at server construction.
    pub started_at_unix: i64,
    /// Live state-version tracker, shared with `GatewayServer`.
    pub state_versions: Arc<StateVersionTracker>,
    /// Count of registered RPC methods captured at wire time. Approximate
    /// (a diagnostic), not a live count.
    pub method_count: usize,
}

/// Handle `gateway.identity.get`. Returns version + instance identity +
/// supported protocols + method count + current state-version snapshot.
pub async fn handle_gateway_identity_get(
    request: JsonRpcRequest,
    snapshot: GatewayIdentitySnapshot,
) -> JsonRpcResponse {
    JsonRpcResponse::success(
        request.id,
        json!({
            "version": env!("ALEPH_VERSION"),
            "instance_id": snapshot.instance_id,
            "started_at_unix": snapshot.started_at_unix,
            "supported_protocols": SUPPORTED_PROTOCOLS,
            "registered_method_count": snapshot.method_count,
            "state_version": snapshot.state_versions.snapshot(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_snapshot() -> GatewayIdentitySnapshot {
        GatewayIdentitySnapshot {
            instance_id: "test-instance-abc".to_string(),
            started_at_unix: 1_700_000_000,
            state_versions: Arc::new(StateVersionTracker::new()),
            method_count: 42,
        }
    }

    #[tokio::test]
    async fn returns_expected_shape() {
        let snapshot = make_snapshot();
        let req = JsonRpcRequest::with_id("gateway.identity.get", None, json!(1));
        let resp = handle_gateway_identity_get(req, snapshot).await;

        assert!(resp.is_success());
        let result = resp.result.unwrap();
        assert_eq!(result["instance_id"], "test-instance-abc");
        assert_eq!(result["started_at_unix"], 1_700_000_000);
        assert_eq!(result["registered_method_count"], 42);
        assert!(result["supported_protocols"].is_array());
        assert_eq!(result["supported_protocols"][0], "jsonrpc/2.0");
        assert!(result["state_version"].is_object());
        assert_eq!(result["state_version"]["presence"], 0);
    }

    #[tokio::test]
    async fn state_version_reflects_live_bumps() {
        let snapshot = make_snapshot();
        snapshot.state_versions.bump_health();
        snapshot.state_versions.bump_health();

        let req = JsonRpcRequest::with_id("gateway.identity.get", None, json!(1));
        let resp = handle_gateway_identity_get(req, snapshot).await;

        let result = resp.result.unwrap();
        assert_eq!(result["state_version"]["health"], 2);
    }
}
