//! Integration coverage for `gateway.identity.get` (Spec 2 / G4).
//!
//! The handler takes a self-contained `GatewayIdentitySnapshot`, so this
//! exercises the public handler surface directly — no server spawn
//! needed. Restart-detection and protocol-negotiation semantics are
//! asserted on the response shape.

use std::sync::Arc;

use alephcore::gateway::handlers::gateway_identity::{
    handle_gateway_identity_get, GatewayIdentitySnapshot,
};
use alephcore::gateway::protocol::JsonRpcRequest;
use alephcore::gateway::state_version::StateVersionTracker;
use serde_json::json;

fn snapshot_with(method_count: usize) -> GatewayIdentitySnapshot {
    GatewayIdentitySnapshot {
        instance_id: "integration-instance-xyz".to_string(),
        started_at_unix: 1_700_000_000,
        state_versions: Arc::new(StateVersionTracker::new()),
        method_count,
    }
}

#[tokio::test]
async fn identity_get_returns_required_fields() {
    let snapshot = snapshot_with(100);
    let req = JsonRpcRequest::with_id("gateway.identity.get", None, json!(1));
    let resp = handle_gateway_identity_get(req, snapshot).await;

    assert!(resp.is_success(), "expected success: {:?}", resp.error);
    let result = resp.result.unwrap();

    assert_eq!(result["instance_id"], "integration-instance-xyz");
    assert_eq!(result["started_at_unix"], 1_700_000_000);
    assert_eq!(result["registered_method_count"], 100);
    assert!(result["version"].is_string());

    let protocols = result["supported_protocols"]
        .as_array()
        .expect("supported_protocols must be an array");
    assert!(protocols.iter().any(|p| p == "jsonrpc/2.0"));

    assert!(result["state_version"]["presence"].is_number());
    assert!(result["state_version"]["health"].is_number());
    assert!(result["state_version"]["config"].is_number());
}

#[tokio::test]
async fn identity_get_state_version_reflects_bumps() {
    let snapshot = snapshot_with(1);
    snapshot.state_versions.bump_config();
    snapshot.state_versions.bump_config();
    snapshot.state_versions.bump_health();

    let req = JsonRpcRequest::with_id("gateway.identity.get", None, json!(1));
    let resp = handle_gateway_identity_get(req, snapshot).await;
    let result = resp.result.unwrap();

    assert_eq!(result["state_version"]["config"], 2);
    assert_eq!(result["state_version"]["health"], 1);
    assert_eq!(result["state_version"]["presence"], 0);
}
