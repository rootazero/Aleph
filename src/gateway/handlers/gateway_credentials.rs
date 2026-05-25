//! `gateway.credentials` — read-only diagnostic snapshot of the auth surface.
//!
//! Wraps [`build_credential_plan`] for the JSON-RPC layer. Lives on the
//! Query lane (registered alongside `gateway.metrics.lanes` in
//! `Lane::override_for`). No secrets leave this handler — only flags
//! and counts.

use std::sync::Arc;

use super::super::config::GatewayServerConfig;
use super::super::credential_planner::build_credential_plan;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};

/// Handle `gateway.credentials`. Returns the structured [`CredentialPlan`]
/// derived from the live `GatewayServerConfig` plus the running process'
/// environment.
pub async fn handle_gateway_credentials(
    request: JsonRpcRequest,
    cfg: Arc<GatewayServerConfig>,
) -> JsonRpcResponse {
    let plan = build_credential_plan(&cfg, |name| std::env::var(name).ok());
    match serde_json::to_value(&plan) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(err) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("credential plan serialize failed: {}", err),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::config::{AuthConfig, AuthMode, GatewayServerConfig};
    use serde_json::json;

    #[tokio::test]
    async fn returns_token_mode_by_default() {
        let cfg = Arc::new(GatewayServerConfig::default());
        let req = JsonRpcRequest::with_id("gateway.credentials", None, json!(1));
        let resp = handle_gateway_credentials(req, cfg).await;
        assert!(resp.is_success());
        let value = resp.result.expect("result present");
        assert_eq!(value["auth_mode"], "token");
        assert_eq!(value["auth_required"], true);
        assert_eq!(value["bind_address"], "127.0.0.1:18790");
    }

    #[tokio::test]
    async fn reflects_auth_none() {
        let mut cfg = GatewayServerConfig::default();
        cfg.auth = AuthConfig {
            mode: AuthMode::None,
            ..AuthConfig::default()
        };
        let cfg = Arc::new(cfg);
        let req = JsonRpcRequest::with_id("gateway.credentials", None, json!(1));
        let resp = handle_gateway_credentials(req, cfg).await;
        let value = resp.result.expect("result present");
        assert_eq!(value["auth_mode"], "none");
        assert_eq!(value["auth_required"], false);
    }
}
