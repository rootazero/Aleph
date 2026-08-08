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

use std::collections::BTreeMap;

use crate::config::types::{LoadBalanceStrategy, ModelRouteConfig, ProviderRateLimit, RouteMode};
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
    /// "auto" | "`always_local`" | "`always_cloud`".
    mode: String,
    #[serde(default)]
    allow_cloud_escalation: bool,
    /// Preferred local provider name (from the configured `[providers]`), or
    /// `null`/absent for none. The panel populates this from a dropdown.
    #[serde(default)]
    local_provider: Option<String>,
    /// Preferred cloud provider name, same contract as `local_provider`.
    #[serde(default)]
    cloud_provider: Option<String>,
    /// Load-balancing strategy for the same-tier fallback pool:
    /// "ordered" | "`round_robin`" | "`least_busy`" | "`latency_aware`" | "`usage_based`".
    /// Absent → unchanged default ("ordered"), backward-compatible with the
    /// pre-balance payload.
    #[serde(default)]
    load_balance: Option<String>,
    /// Per-provider rate ceilings (rpm/tpm), keyed by provider name. Absent →
    /// no rate awareness (cleared), backward-compatible with the pre-usage
    /// payload. Drives `usage_based` ordering and the over-limit gate.
    #[serde(default)]
    rate_limits: BTreeMap<String, ProviderRateLimit>,
    /// Background health-probe interval for circuit-open providers (seconds;
    /// `0`/absent = off). Hot-tunes the gateway health prober on its next tick.
    #[serde(default)]
    health_probe_interval_secs: Option<u64>,
}

/// Normalise a UI-supplied provider name: blank / whitespace-only → `None` so a
/// cleared dropdown clears the pin rather than pinning the empty string.
fn normalize_pin(raw: Option<String>) -> Option<String> {
    raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

const fn mode_to_str(mode: RouteMode) -> &'static str {
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

const fn lb_to_str(s: LoadBalanceStrategy) -> &'static str {
    match s {
        LoadBalanceStrategy::Ordered => "ordered",
        LoadBalanceStrategy::RoundRobin => "round_robin",
        LoadBalanceStrategy::LeastBusy => "least_busy",
        LoadBalanceStrategy::LatencyAware => "latency_aware",
        LoadBalanceStrategy::UsageBased => "usage_based",
        LoadBalanceStrategy::CostAware => "cost_aware",
    }
}

/// Every value [`lb_from_str`] accepts, for the rejection message.
///
/// Kept beside the parser and asserted against it, because the two drifted:
/// `cost_aware` shipped, the parser took it, and the error text kept telling
/// users it did not exist — including any Panel/CLI client that builds its
/// option list from that string.
const LOAD_BALANCE_VALUES: &[&str] = &[
    "ordered",
    "round_robin",
    "least_busy",
    "latency_aware",
    "usage_based",
    "cost_aware",
];

fn lb_from_str(raw: &str) -> Option<LoadBalanceStrategy> {
    match raw {
        "ordered" => Some(LoadBalanceStrategy::Ordered),
        "round_robin" => Some(LoadBalanceStrategy::RoundRobin),
        "least_busy" => Some(LoadBalanceStrategy::LeastBusy),
        "latency_aware" => Some(LoadBalanceStrategy::LatencyAware),
        "usage_based" => Some(LoadBalanceStrategy::UsageBased),
        "cost_aware" => Some(LoadBalanceStrategy::CostAware),
        _ => None,
    }
}

const fn tier_to_str(tier: EndpointTier) -> &'static str {
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
            "load_balance": lb_to_str(cfg.route.load_balance),
            "local_provider": cfg.route.local_provider,
            "cloud_provider": cfg.route.cloud_provider,
            "rate_limits": cfg.route.rate_limits,
            "health_probe_interval_secs": cfg.route.health_probe_interval_secs,
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
                format!("Invalid params: {e}"),
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

    // Strategy: absent → Ordered (no-op, backward-compatible); present-but-bad
    // → reject rather than silently coerce (mirrors the mode contract).
    let load_balance = match payload.load_balance.as_deref() {
        None => LoadBalanceStrategy::Ordered,
        Some(raw) => match lb_from_str(raw) {
            Some(s) => s,
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!(
                        "load_balance must be one of {}, got '{raw}'",
                        LOAD_BALANCE_VALUES.join("|")
                    ),
                );
            }
        },
    };

    let new_route = ModelRouteConfig {
        mode,
        load_balance,
        allow_cloud_escalation: payload.allow_cloud_escalation,
        local_provider: normalize_pin(payload.local_provider),
        cloud_provider: normalize_pin(payload.cloud_provider),
        rate_limits: payload.rate_limits,
        // 0 and absent both mean "off"; normalise to absent so the wire form
        // stays minimal (`skip_serializing_if`).
        health_probe_interval_secs: payload.health_probe_interval_secs.filter(|&s| s > 0),
    };

    {
        let mut cfg = config.write().await;
        cfg.route = new_route.clone();
        if let Err(e) = cfg.save_incremental(&["route"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Hot-apply: the live failover chain reads this on the next request, so the
    // switch takes effect without a restart. `None` only before boot wiring —
    // then the on-disk write above still lands at the next start.
    if let Some(handle) = try_global_route_handle() {
        handle.store(&new_route);
    }
    // The `config_problems` half of the same hot-apply: the observability
    // bundle's boot-time list says nothing about a config written at runtime
    // (e.g. a typo'd pin), so recompute it against the bundle's provider/tier
    // picture and republish — `route_status` then diagnoses the config that is
    // actually live. `None` only before boot wiring, same contract as above.
    if let Some(obs) = crate::providers::route_observe::global_route_observability() {
        obs.hot_apply_problems(&new_route);
    }

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("route".to_string()),
        value: serde_json::json!({
            "mode": mode_to_str(mode),
            "allow_cloud_escalation": new_route.allow_cloud_escalation,
            "load_balance": lb_to_str(new_route.load_balance),
            "local_provider": new_route.local_provider,
            "cloud_provider": new_route.cloud_provider,
            "rate_limits": new_route.rate_limits,
            "health_probe_interval_secs": new_route.health_probe_interval_secs,
        }),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_gateway_event(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rejection message must list exactly what the parser takes. These two
    /// drifted once — `cost_aware` was accepted but advertised as invalid — and
    /// the only symptom was users (and option-list-building clients) believing a
    /// shipped strategy did not exist.
    #[test]
    fn advertised_load_balance_values_match_the_parser() {
        for value in LOAD_BALANCE_VALUES {
            assert!(
                lb_from_str(value).is_some(),
                "advertised '{value}' is rejected by the parser"
            );
        }
        // And nothing the parser takes is missing from the advertisement.
        for value in [
            "ordered",
            "round_robin",
            "least_busy",
            "latency_aware",
            "usage_based",
            "cost_aware",
        ] {
            assert!(
                LOAD_BALANCE_VALUES.contains(&value),
                "parser accepts '{value}' but the error message never mentions it"
            );
        }
    }

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
        for m in [
            RouteMode::Auto,
            RouteMode::AlwaysLocal,
            RouteMode::AlwaysCloud,
        ] {
            assert_eq!(mode_from_str(mode_to_str(m)), Some(m));
        }
        assert_eq!(mode_from_str("nope"), None);
    }

    #[test]
    fn normalize_pin_blanks_to_none() {
        assert_eq!(normalize_pin(None), None);
        assert_eq!(normalize_pin(Some(String::new())), None);
        assert_eq!(normalize_pin(Some("   ".to_string())), None);
        assert_eq!(
            normalize_pin(Some("  ollama ".to_string())).as_deref(),
            Some("ollama")
        );
    }

    #[test]
    fn payload_parses_provider_pins_and_tolerates_absence() {
        // Pins present.
        let p: RouteModePayload = serde_json::from_value(serde_json::json!({
            "mode": "always_cloud",
            "allow_cloud_escalation": false,
            "local_provider": "ollama",
            "cloud_provider": "anthropic",
        }))
        .unwrap();
        assert_eq!(p.local_provider.as_deref(), Some("ollama"));
        assert_eq!(p.cloud_provider.as_deref(), Some("anthropic"));

        // Pins absent — backward-compatible with the old two-field payload.
        let p2: RouteModePayload =
            serde_json::from_value(serde_json::json!({ "mode": "auto" })).unwrap();
        assert_eq!(p2.local_provider, None);
        assert_eq!(p2.cloud_provider, None);
        // Old payloads omit load_balance entirely.
        assert_eq!(p2.load_balance, None);
    }

    #[test]
    fn lb_string_round_trips() {
        for s in [
            LoadBalanceStrategy::Ordered,
            LoadBalanceStrategy::RoundRobin,
            LoadBalanceStrategy::LeastBusy,
            LoadBalanceStrategy::LatencyAware,
            LoadBalanceStrategy::UsageBased,
        ] {
            assert_eq!(lb_from_str(lb_to_str(s)), Some(s));
        }
        assert_eq!(lb_from_str("nope"), None);
    }

    #[test]
    fn payload_parses_rate_limits_and_tolerates_absence() {
        let p: RouteModePayload = serde_json::from_value(serde_json::json!({
            "mode": "auto",
            "load_balance": "usage_based",
            "rate_limits": { "anthropic": { "rpm": 60, "tpm": 90000 } },
        }))
        .unwrap();
        assert_eq!(p.load_balance.as_deref(), Some("usage_based"));
        let a = p.rate_limits.get("anthropic").unwrap();
        assert_eq!(a.rpm, Some(60));
        assert_eq!(a.tpm, Some(90_000));

        // Absent rate_limits → empty map (backward-compatible).
        let p2: RouteModePayload =
            serde_json::from_value(serde_json::json!({ "mode": "auto" })).unwrap();
        assert!(p2.rate_limits.is_empty());
    }

    #[test]
    fn payload_parses_health_probe_interval_and_tolerates_absence() {
        let p: RouteModePayload = serde_json::from_value(serde_json::json!({
            "mode": "auto",
            "health_probe_interval_secs": 60,
        }))
        .unwrap();
        assert_eq!(p.health_probe_interval_secs, Some(60));

        // Absent → None (off), backward-compatible with the pre-prober payload.
        let p2: RouteModePayload =
            serde_json::from_value(serde_json::json!({ "mode": "auto" })).unwrap();
        assert_eq!(p2.health_probe_interval_secs, None);
    }

    #[tokio::test]
    async fn update_rejects_unknown_load_balance() {
        let config = Arc::new(RwLock::new(Config::default()));
        let bus = Arc::new(GatewayEventBus::new());
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::from(1)),
            method: "route_config.update".to_string(),
            params: Some(serde_json::json!({ "mode": "auto", "load_balance": "magic" })),
        };
        let resp = handle_update(req, config, bus).await;
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn get_includes_load_balance() {
        let config = Arc::new(RwLock::new(Config::default()));
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::from(1)),
            method: "route_config.get".to_string(),
            params: None,
        };
        let resp = handle_get(req, config).await;
        let result = resp.result.expect("result");
        assert_eq!(result["load_balance"], "ordered");
    }
}
