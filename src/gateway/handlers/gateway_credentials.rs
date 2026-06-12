//! `gateway.credentials` — read-only diagnostic snapshot of the gateway
//! connection surface (bind address, origin policy, mutation hardening).
//!
//! Wraps [`build_credential_plan`] for the JSON-RPC layer. Lives on the
//! Query lane (registered alongside `gateway.metrics.lanes` in
//! `Lane::override_for`). No secrets leave this handler — only flags
//! and counts.

use crate::sync_primitives::Arc;

use super::super::config::GatewayServerConfig;
use super::super::credential_planner::build_credential_plan;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};

/// Handle `gateway.credentials`. Returns the structured [`CredentialPlan`]
/// derived from the live `GatewayServerConfig`.
pub async fn handle_gateway_credentials(
    request: JsonRpcRequest,
    cfg: Arc<GatewayServerConfig>,
) -> JsonRpcResponse {
    let plan = build_credential_plan(&cfg);
    match serde_json::to_value(&plan) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(err) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("credential plan serialize failed: {err}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::config::GatewayServerConfig;
    use serde_json::json;

    #[tokio::test]
    async fn returns_default_surface_snapshot() {
        let cfg = Arc::new(GatewayServerConfig::default());
        let req = JsonRpcRequest::with_id("gateway.credentials", None, json!(1));
        let resp = handle_gateway_credentials(req, cfg).await;
        assert!(resp.is_success());
        let value = resp.result.expect("result present");
        assert_eq!(value["allow_any_origin"], false);
        assert_eq!(value["allowed_origins_count"], 0);
        assert_eq!(value["bind_address"], "127.0.0.1:18790");
    }

    #[tokio::test]
    async fn reflects_origin_escape_hatch() {
        let cfg = GatewayServerConfig {
            allow_any_origin: true,
            ..GatewayServerConfig::default()
        };
        let cfg = Arc::new(cfg);
        let req = JsonRpcRequest::with_id("gateway.credentials", None, json!(1));
        let resp = handle_gateway_credentials(req, cfg).await;
        let value = resp.result.expect("result present");
        assert_eq!(value["allow_any_origin"], true);
    }
}
