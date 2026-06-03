//! Local/cloud route-mode configuration RPC handlers.
//!
//! Exposes the `[route]` section ([`ModelRouteConfig`]) to the panel:
//! `route_config.get` returns the live mode plus a tier-classified view of the
//! configured providers (so the UI can show *which* providers each mode will
//! target without re-deriving locality in WASM); `route_config.update` writes
//! the new mode, persists it, and **hot-applies it to the running failover
//! chain** via the process-global [`RouteHandle`] — the next prompt routes the
//! new way with no daemon restart.
//!
//! R7/R10 unchanged: this moves two HARD operator signals (mode + escalation),
//! never the prompt. The route decision still lives in
//! [`route_policy`](crate::providers::route_policy).

use crate::config::types::{ModelRouteConfig, RouteMode};
use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::orchestrator::deps_builder::provider_tier;
use crate::providers::route_handle::try_global_route_handle;
use crate::providers::route_policy::EndpointTier;
use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

/// Wire shape the panel sends/receives for the route mode itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RouteModePayload {
    /// "auto" | "always_local" | "always_cloud".
    mode: String,
    #[serde(default)]
    allow_cloud_escalation: bool,
}

fn mode_to_str(mode: RouteMode) -> &'static str {
    match mode {
        RouteMode::Auto => "auto",
        RouteMode::AlwaysLocal => "always_local",
        RouteMode::AlwaysCloud => "always_cloud",
    }
}

fn mode_from_str(raw: &str) -> Option<RouteMode> {
    match raw {
        "auto" => Some(RouteMode::Auto),
        "always_local" => Some(RouteMode::AlwaysLocal),
        "always_cloud" => Some(RouteMode::AlwaysCloud),
        _ => None,
    }
}

fn tier_to_str(tier: EndpointTier) -> &'static str {
    match tier {
        EndpointTier::Local => "local",
        EndpointTier::Cloud => "cloud",
        EndpointTier::Unknown => "unknown",
    }
}

/// Get current route mode plus the tier-classified provider list.
///
/// Response:
/// ```json
/// { "mode": "auto", "allow_cloud_escalation": false,
///   "providers": [ { "name": "ollama", "tier": "local", "models": [...] }, ... ] }
/// ```
pub async fn handle_get(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let cfg = config.read().await;

    let providers: Vec<Value> = cfg
        .providers
        .iter()
        .map(|(name, pc)| {
            serde_json::json!({
                "name": name,
                "tier": tier_to_str(provider_tier(pc)),
                "models": pc.all_models(),
                "enabled": pc.enabled,
            })
        })
        .collect();

    JsonRpcResponse::success(
        request.id,
        serde_json::json!({
            "mode": mode_to_str(cfg.route.mode),
            "allow_cloud_escalation": cfg.route.allow_cloud_escalation,
            "providers": providers,
        }),
    )
}

/// Update route mode: persist + hot-apply to the live failover chain.
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params = match request.params {
        Some(p) => p,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    let payload: RouteModePayload = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid params: {}", e),
            );
        }
    };

    let mode = match mode_from_str(&payload.mode) {
        Some(m) => m,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "mode must be one of auto|always_local|always_cloud, got '{}'",
                    payload.mode
                ),
            );
        }
    };

    let new_route = ModelRouteConfig {
        mode,
        allow_cloud_escalation: payload.allow_cloud_escalation,
    };

    {
        let mut cfg = config.write().await;
        cfg.route = new_route.clone();
        if let Err(e) = cfg.save_incremental(&["route"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Hot-apply: the live failover chain reads this on the next request, so the
    // switch takes effect without a restart. `None` only before boot wiring —
    // then the on-disk write above still lands at the next start.
    if let Some(handle) = try_global_route_handle() {
        handle.store(&new_route);
    }

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("route".to_string()),
        value: serde_json::json!({
            "mode": mode_to_str(mode),
            "allow_cloud_escalation": new_route.allow_cloud_escalation,
        }),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_returns_mode_and_classified_providers() {
        let config = Arc::new(RwLock::new(Config::default()));
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::from(1)),
            method: "route_config.get".to_string(),
            params: None,
        };
        let resp = handle_get(req, config).await;
        let result = resp.result.expect("result");
        assert_eq!(result["mode"], "auto");
        assert_eq!(result["allow_cloud_escalation"], false);
        assert!(result["providers"].is_array());
    }

    #[tokio::test]
    async fn update_rejects_unknown_mode() {
        let config = Arc::new(RwLock::new(Config::default()));
        let bus = Arc::new(GatewayEventBus::new());
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::from(1)),
            method: "route_config.update".to_string(),
            params: Some(serde_json::json!({ "mode": "turbo" })),
        };
        let resp = handle_update(req, config, bus).await;
        assert!(resp.error.is_some());
    }

    #[test]
    fn mode_string_round_trips() {
        for m in [RouteMode::Auto, RouteMode::AlwaysLocal, RouteMode::AlwaysCloud] {
            assert_eq!(mode_from_str(mode_to_str(m)), Some(m));
        }
        assert_eq!(mode_from_str("nope"), None);
    }
}
