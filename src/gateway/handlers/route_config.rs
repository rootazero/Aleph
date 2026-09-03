//! Local/cloud route-mode configuration RPC handlers.
//!
//! Exposes the `[route]` section ([`ModelRouteConfig`]) to the panel:
//! `route_config.get` returns the live mode plus a tier-classified view of the
//! configured providers (so the UI can show *which* providers each mode will
//! target without re-deriving locality in WASM); `route_config.update` writes
//! the new mode, persists it, and **hot-applies it through
//! [`live_apply::apply_live_sections`](crate::config::live_apply::apply_live_sections)**
//! — the same executor `config.patch`, `ConfigPatcher::rollback` and
//! `config.reload` run, so the running failover chain *and* `route_status`'s
//! `config_problems` both see the new config, and the next prompt routes the
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
    /// Load-balancing strategy for the same-tier fallback pool. The accepted
    /// spellings are [`LOAD_BALANCE_VALUES`] — do not restate them here; this
    /// doc listed five of the six for the whole life of `cost_aware`, which is
    /// the same drift the rejection message had.
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
            "load_balance": cfg.route.load_balance.as_str(),
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
        // Hot-apply through the shared executor, NOT by hand: `[route]` has
        // two live faces (the chain's `RouteHandle` and `route_status`'s
        // `config_problems`), and this handler used to poke both itself while
        // the generic path (`config.patch` / `ConfigPatcher::rollback` /
        // `config.reload`) poked only the first — so a route write through the
        // very RPC `route_status`'s own tool text recommends left
        // `config_problems` describing the previous generation. One arm, one
        // derivation, every write face.
        //
        // Called while the write guard is still held: `apply_live_sections` is
        // synchronous and only pokes process-global handles, so it never
        // re-enters this lock, and holding it means the poke sees exactly the
        // config that was persisted.
        let _ = crate::config::live_apply::apply_live_sections(&cfg, &["route"]);
    }

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("route".to_string()),
        value: serde_json::json!({
            "mode": mode_to_str(mode),
            "allow_cloud_escalation": new_route.allow_cloud_escalation,
            "load_balance": new_route.load_balance.as_str(),
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
    use crate::utils::paths::AlephHomeEnvGuard;

    /// The Panel's route DTOs, read at compile time. Comparing against the
    /// *source* is deliberate: `alephcore` does not depend on `aleph-panel`, so
    /// `RouteConfigUpdate` cannot be constructed here, and a test that
    /// round-tripped this crate's own payload would only be testing serde.
    /// Precedent: `handlers/cron/real.rs`'s Panel-DTO scan.
    const PANEL_SETTINGS_API: &str =
        include_str!("../../../interfaces/webchat/src/api/settings.rs");
    const THIS_HANDLER: &str = include_str!("route_config.rs");

    /// The two route faces that build a `RouteConfigUpdate` and POST it. Both
    /// are scanned because a field wired on one face and dropped on the other
    /// is exactly the asymmetry that hid `health_probe_interval_secs`.
    const PANEL_ROUTE_FACES: [(&str, &str); 2] = [
        (
            "wide/views/settings/route.rs",
            include_str!("../../../interfaces/webchat/src/platform/wide/views/settings/route.rs"),
        ),
        (
            "phone/settings/model_route.rs",
            include_str!("../../../interfaces/webchat/src/platform/phone/settings/model_route.rs"),
        ),
    ];

    /// Collect the field names of a struct from Rust source (`pub` optional).
    fn struct_fields(source: &str, struct_name: &str) -> Vec<String> {
        let start = source
            .find(&format!("struct {struct_name} {{"))
            .unwrap_or_else(|| panic!("struct {struct_name} not found"));
        let body = &source[start..];
        let end = body.find("\n}").expect("unterminated struct");
        body[..end]
            .lines()
            .skip(1)
            .filter_map(|line| {
                let rest = line.trim();
                let rest = rest.strip_prefix("pub ").unwrap_or(rest);
                let name = rest.split(':').next()?.trim();
                (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                    .then(|| name.to_string())
            })
            .collect()
    }

    /// Body of the single `RouteConfigUpdate { … }` literal in a Panel view,
    /// as `(field, value expression)` pairs. Panics if the file builds the
    /// struct in more than one place — a second save path would be a second
    /// chance to drop a field, and this guard would only have read one.
    fn update_literal_bindings(source: &str, face: &str) -> Vec<(String, String)> {
        let needle = "RouteConfigUpdate {";
        assert_eq!(
            source.matches(needle).count(),
            1,
            "{face} builds RouteConfigUpdate more than once; this guard reads only the first"
        );
        let start = source.find(needle).expect("literal counted above");
        let body = &source[start + needle.len()..];
        let end = body
            .find("};")
            .expect("unterminated RouteConfigUpdate literal");
        body[..end]
            .lines()
            .filter_map(|line| {
                let (name, value) = line.trim().split_once(':')?;
                (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                    .then(|| (name.to_string(), value.trim().to_string()))
            })
            .collect()
    }

    /// `handle_update` replaces the whole `[route]` section: whatever the Panel
    /// leaves out of its payload is *erased*, not preserved. So every field
    /// this handler parses must exist on the Panel's update DTO — and on its
    /// view DTO, or the Panel could never have loaded the value it sends back.
    ///
    /// `health_probe_interval_secs` shipped on the server and on the tool face
    /// while both Panel structs stayed silent about it, so any route save from
    /// the Panel switched a running health prober off — no log, nothing on
    /// screen, and no way to tell it had happened.
    #[test]
    fn panel_update_payload_carries_every_field_handle_update_replaces() {
        let payload = RouteModePayload {
            mode: "auto".to_string(),
            allow_cloud_escalation: false,
            local_provider: None,
            cloud_provider: None,
            load_balance: None,
            rate_limits: BTreeMap::new(),
            health_probe_interval_secs: None,
        };
        let Value::Object(wire) = serde_json::to_value(&payload).expect("serialize") else {
            panic!("payload must serialize to an object");
        };
        // `serde_json::Map` orders its keys, so compare against a sorted list.
        let wire_keys: Vec<String> = wire.keys().cloned().collect();
        // The wire key set must still be the whole struct: a `skip_serializing_if`
        // added here would otherwise quietly shrink what this guard checks.
        let mut declared = struct_fields(THIS_HANDLER, "RouteModePayload");
        declared.sort();
        assert_eq!(
            wire_keys, declared,
            "RouteModePayload's wire keys no longer match its fields"
        );

        for dto in ["RouteConfigUpdate", "RouteConfigView"] {
            let panel = struct_fields(PANEL_SETTINGS_API, dto);
            let missing: Vec<&String> = wire_keys.iter().filter(|k| !panel.contains(k)).collect();
            assert!(
                missing.is_empty(),
                "Panel {dto} never names {missing:?}; handle_update full-replaces \
                 [route], so a save from the Panel wipes those settings"
            );
        }
    }

    /// Naming the field is not carrying its value. The guard above proves both
    /// Panel DTOs *have* `health_probe_interval_secs`; it stays green if a save
    /// closure hard-codes `health_probe_interval_secs: None` — which reproduces
    /// the exact bug (a full-replace save silently switching a running prober
    /// off) with the DTO still innocent. So: every field `handle_update`
    /// replaces must be bound, in each face's `RouteConfigUpdate` literal, to an
    /// expression that reads a signal (`.get()`) — the value that face loaded,
    /// never a constant.
    ///
    /// The field list is derived from `RouteModePayload` rather than typed out,
    /// so a field added to the handler is covered here the day it lands.
    #[test]
    fn panel_save_closures_forward_the_values_they_loaded() {
        let replaced = struct_fields(THIS_HANDLER, "RouteModePayload");
        assert!(
            replaced.contains(&"health_probe_interval_secs".to_string()),
            "derivation broke: RouteModePayload no longer parses"
        );
        for (face, source) in PANEL_ROUTE_FACES {
            let bindings = update_literal_bindings(source, face);
            for field in &replaced {
                let value = bindings
                    .iter()
                    .find(|(name, _)| name == field)
                    .map(|(_, value)| value.as_str())
                    .unwrap_or_else(|| {
                        panic!(
                            "{face} never binds `{field}`; handle_update full-replaces [route], \
                             so saving from this face erases it"
                        )
                    });
                assert!(
                    value.contains(".get()"),
                    "{face} binds `{field}` to `{value}` — a constant, not the value this face \
                     loaded; saving from here overwrites the operator's setting"
                );
            }
        }
    }

    /// The rejection message must list exactly the strategies that exist. These
    /// drifted once — `cost_aware` was accepted but advertised as invalid — and
    /// the only symptom was users (and option-list-building clients) believing a
    /// shipped strategy did not exist.
    ///
    /// The "nothing is missing" half used to be a **third** hand-written list of
    /// the same six strings, so a seventh variant would have been absent from
    /// the constant, absent from the parser and absent from the list checking
    /// them — three copies agreeing about a world that had moved (判据 §0:
    /// 守卫的绿只覆盖它认得的那种形状). It is now derived from the type: the
    /// enum's own serde spellings, read out of its `JsonSchema`, which is where
    /// `[route].load_balance` is deserialised from in TOML anyway. Add a
    /// variant and this goes red without anyone remembering to edit a list.
    #[test]
    fn advertised_load_balance_values_match_the_parser() {
        for value in LOAD_BALANCE_VALUES {
            assert!(
                lb_from_str(value).is_some(),
                "advertised '{value}' is rejected by the parser"
            );
        }

        // Every variant of the enum, by its serde name (`rename_all =
        // "snake_case"`), straight from the schema. schemars renders a
        // documented unit enum as `oneOf: [{const: …}]` and a bare one as
        // `enum: [...]`; both shapes are read so a schemars upgrade cannot
        // quietly turn this into a vacuous pass.
        let schema = serde_json::to_value(schemars::schema_for!(LoadBalanceStrategy))
            .expect("schema serialises");
        let mut variants: Vec<String> = Vec::new();
        if let Some(values) = schema["enum"].as_array() {
            variants.extend(values.iter().filter_map(|v| v.as_str().map(String::from)));
        }
        if let Some(branches) = schema["oneOf"].as_array() {
            variants.extend(
                branches
                    .iter()
                    .filter_map(|b| b["const"].as_str().map(String::from)),
            );
        }
        assert!(
            variants.len() >= LOAD_BALANCE_VALUES.len(),
            "the schema yielded only {variants:?} — fewer spellings than the {} advertised, so \
             this half is not reading the enum any more (schemars shape changed?)",
            LOAD_BALANCE_VALUES.len()
        );
        for variant in &variants {
            assert!(
                LOAD_BALANCE_VALUES.contains(&variant.as_str()),
                "the strategy '{variant}' exists but the rejection message never mentions it; \
                 the parser will also refuse it"
            );
            assert!(
                lb_from_str(variant).is_some(),
                "the strategy '{variant}' exists but `lb_from_str` rejects it"
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

    /// A successful update must reach BOTH live faces of `[route]`, through
    /// the shared executor.
    ///
    /// This handler used to poke the route handle and `config_problems` by
    /// hand; the generic path (`config.patch` / rollback / `config.reload`)
    /// poked only the first, so `config_problems` went stale on every write
    /// that did not come through here. Both now run `apply_live_sections`.
    ///
    /// `apply_live_sections` returns which targets landed, but this handler
    /// does not surface that vec (the RPC answers `{"success": true}` either
    /// way), so what is asserted is what `applied == ["route"]` *means*: the
    /// process-global `RouteHandle` carries the new mode, and the
    /// observability bundle carries the new config's problems. Asserting the
    /// call happened would pass just as well against a poke that stored
    /// nothing.
    ///
    /// ⚠️ Both handles are install-once process globals shared by the whole
    /// `--lib` binary; the serial key is the same one
    /// `config::live_apply::tests::a_route_patch_through_the_executor_republishes_config_problems`
    /// takes, because that test asserts on the very bundle this one writes.
    #[tokio::test]
    #[serial_test::serial(route_observability_global)]
    async fn update_hot_applies_both_route_faces_through_the_executor() {
        use crate::providers::default_handle::StaticDefault;
        use crate::providers::mock::MockProvider;
        use crate::providers::route_observe::{
            global_route_observability, set_global_route_observability, test_observability,
        };

        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        // Both process-global handles the `route` arm pokes. `global_route_handle`
        // is get-or-init and the bundle slot is install-once, so an earlier
        // installer in this binary wins — hence the assertions below read the
        // handles back rather than assuming these instances.
        let handle =
            crate::providers::route_handle::global_route_handle(&ModelRouteConfig::default());
        set_global_route_observability(test_observability(
            Arc::new(StaticDefault::new(Arc::new(MockProvider::new("ok")))),
            std::collections::HashMap::from([("ollama".to_string(), EndpointTier::Local)]),
        ));
        let obs = global_route_observability().expect("bundle installed");

        let config = Arc::new(RwLock::new(Config::default()));
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::from(1)),
            method: "route_config.update".to_string(),
            params: Some(serde_json::json!({
                "mode": "always_local",
                // A pin naming a provider that is not configured: the runtime
                // diagnosis this write must republish.
                "local_provider": "olama",
            })),
        };
        let resp = handle_update(req, Arc::clone(&config), Arc::new(GatewayEventBus::new())).await;
        assert!(resp.error.is_none(), "update failed: {resp:?}");

        assert_eq!(
            handle.snapshot().mode,
            RouteMode::AlwaysLocal,
            "the live failover chain must see the new mode without a restart"
        );
        let problems = obs.snapshot().await["config_problems"].clone();
        let problems = problems.as_array().expect("array").clone();
        assert_eq!(
            problems.len(),
            1,
            "route_status must diagnose the config that was just written; got: {problems:?}"
        );
        assert_eq!(problems[0]["field"], "local_provider");
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
