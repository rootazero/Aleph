use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

// ============================================================================
// General Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    pub default_provider: Option<String>,
    pub language: Option<String>,
}

pub struct GeneralConfigApi;

impl GeneralConfigApi {
    pub async fn get(state: &DashboardState) -> Result<GeneralConfig, String> {
        let result = state.rpc_call("general_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: GeneralConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("general_config.update", params).await?;
        Ok(())
    }
}

// ============================================================================
// Behavior Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorConfig {
    pub output_mode: String,
    pub typing_speed: u32,
}

pub struct BehaviorConfigApi;

impl BehaviorConfigApi {
    pub async fn get(state: &DashboardState) -> Result<BehaviorConfig, String> {
        let result = state.rpc_call("behavior_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: BehaviorConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("behavior_config.update", params).await?;
        Ok(())
    }
}

// ============================================================================
// Generation Config API
// ============================================================================

/// The `generation_config.*` body, from the protocol crate rather than a hand
/// copy here.
///
/// The copy this replaces declared `output_dir: String` while the server has
/// always sent `Option<String>`, so on an install that never set one the
/// response was `null` and serde failed the entire object — the settings
/// section showed a bare `invalid type: null, expected a string` and none of
/// its eight controls. Sharing the type makes that shape of drift a compile
/// error.
pub use aleph_protocol::providers::GenerationSettings as GenerationConfig;

pub struct GenerationConfigApi;

impl GenerationConfigApi {
    pub async fn get(state: &DashboardState) -> Result<GenerationConfig, String> {
        let result = state.rpc_call("generation_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: GenerationConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("generation_config.update", params).await?;
        Ok(())
    }
}

// ============================================================================
// Route Config API — local/cloud three-state route mode
// ============================================================================

/// One configured provider as the route engine sees it: classified by the
/// server's `base_url` locality check (so the panel never re-derives tier in
/// WASM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteProviderInfo {
    pub name: String,
    /// "local" | "cloud" | "unknown".
    pub tier: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
}

/// Per-provider soft rate ceiling, mirroring the backend `ProviderRateLimit`
/// (`[route.rate_limits.<provider>]`). `skip_serializing_if` matches the wire
/// bytes exactly: an omitted dimension means "unbounded on that axis", so the
/// `usage_based` strategy treats it as infinite headroom.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tpm: Option<u32>,
}

/// `route_config.get` response: current mode plus the tier-classified providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfigView {
    /// "auto" | "`always_local`" | "`always_cloud`".
    pub mode: String,
    #[serde(default)]
    pub allow_cloud_escalation: bool,
    /// Preferred local provider name (one of `providers` with tier "local"), or
    /// `None` for "use configured order".
    #[serde(default)]
    pub local_provider: Option<String>,
    /// Preferred cloud provider name (one of `providers` with tier "cloud").
    #[serde(default)]
    pub cloud_provider: Option<String>,
    #[serde(default)]
    pub providers: Vec<RouteProviderInfo>,
    /// Active load-balancing strategy: "ordered" | "`round_robin`" | "`least_busy`"
    /// | "`latency_aware`" | "`usage_based`". `None` from an older daemon → treated
    /// as "ordered" by the view.
    #[serde(default)]
    pub load_balance: Option<String>,
    /// Per-provider rpm/tpm ceilings keyed by provider name. Empty when unset.
    #[serde(default)]
    pub rate_limits: BTreeMap<String, RateLimit>,
    /// Background health-probe interval for circuit-open providers, in seconds.
    /// `None`/`0` = the prober idles. Loaded so the save closure can send it
    /// back — see [`RouteConfigUpdate::health_probe_interval_secs`].
    #[serde(default)]
    pub health_probe_interval_secs: Option<u64>,
}

/// `route_config.update` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfigUpdate {
    pub mode: String,
    pub allow_cloud_escalation: bool,
    /// Empty string clears the pin (server normalises blank → `None`).
    #[serde(default)]
    pub local_provider: Option<String>,
    #[serde(default)]
    pub cloud_provider: Option<String>,
    /// Chosen load-balancing strategy (same key set as the view). Sent on every
    /// save so the backend full-replace does not reset it to `Ordered`.
    #[serde(default)]
    pub load_balance: Option<String>,
    /// Per-provider rpm/tpm ceilings. Sent on every save so the backend
    /// full-replace does not wipe configured limits.
    #[serde(default)]
    pub rate_limits: BTreeMap<String, RateLimit>,
    /// Background health-probe interval in seconds (`None`/`0` = off). Sent on
    /// every save so the backend full-replace does not switch a running prober
    /// off. Both faces edit it directly (a number field on the wide route page,
    /// an inline cell on the phone one), so the value the user sees is the
    /// value that goes back.
    #[serde(default)]
    pub health_probe_interval_secs: Option<u64>,
}

/// Parse the health-probe interval field both route screens render. Blank,
/// non-numeric and `0` all mean "the prober idles" — the same normalisation
/// `route_config.update` applies server-side (it is the authority; this only
/// keeps the field's own value stable across a save).
#[must_use]
pub fn parse_probe_interval(raw: &str) -> Option<u64> {
    raw.trim().parse::<u64>().ok().filter(|&s| s > 0)
}

pub struct RouteConfigApi;

impl RouteConfigApi {
    pub async fn get(state: &DashboardState) -> Result<RouteConfigView, String> {
        let result = state.rpc_call("route_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, update: RouteConfigUpdate) -> Result<(), String> {
        let params = serde_json::to_value(&update).map_err(|e| e.to_string())?;
        state.rpc_call("route_config.update", params).await?;
        Ok(())
    }
}

#[cfg(test)]
mod route_serde_tests {
    use super::*;

    #[test]
    fn rate_limit_omits_none_dims_on_wire() {
        let rl = RateLimit {
            rpm: Some(60),
            tpm: None,
        };
        assert_eq!(
            serde_json::to_value(&rl).unwrap(),
            serde_json::json!({ "rpm": 60 })
        );
    }

    #[test]
    fn update_round_trips_strategy_and_limits() {
        let mut rate_limits = BTreeMap::new();
        rate_limits.insert(
            "anthropic".to_string(),
            RateLimit {
                rpm: Some(60),
                tpm: Some(90_000),
            },
        );
        let u = RouteConfigUpdate {
            mode: "auto".into(),
            allow_cloud_escalation: false,
            local_provider: None,
            cloud_provider: None,
            load_balance: Some("usage_based".into()),
            rate_limits,
            health_probe_interval_secs: None,
        };
        let j = serde_json::to_value(&u).unwrap();
        assert_eq!(j["load_balance"], "usage_based");
        assert_eq!(j["rate_limits"]["anthropic"]["rpm"], 60);
        assert_eq!(j["rate_limits"]["anthropic"]["tpm"], 90_000);
    }

    /// A value the operator set from a TOML edit or the `route_config` tool has
    /// to come back out of the Panel unchanged: `route_config.update` replaces
    /// the whole `[route]` section, so a key the Panel never sends is a key the
    /// backend erases. This mirrors what both save closures do with the loaded
    /// view (`platform/wide/views/settings/route.rs`,
    /// `platform/phone/settings/model_route.rs`) — they seed a signal from the
    /// view and hand it straight back.
    #[test]
    fn probe_interval_survives_the_load_save_round_trip() {
        let view: RouteConfigView = serde_json::from_value(serde_json::json!({
            "mode": "auto",
            "health_probe_interval_secs": 300,
        }))
        .unwrap();
        assert_eq!(view.health_probe_interval_secs, Some(300));

        let u = RouteConfigUpdate {
            mode: view.mode,
            allow_cloud_escalation: view.allow_cloud_escalation,
            local_provider: None,
            cloud_provider: None,
            load_balance: view.load_balance,
            rate_limits: view.rate_limits,
            health_probe_interval_secs: view.health_probe_interval_secs,
        };
        assert_eq!(
            serde_json::to_value(&u).unwrap()["health_probe_interval_secs"],
            300,
            "a Panel save must not switch a running health prober off"
        );
    }

    #[test]
    fn view_tolerates_absent_new_fields() {
        // An older daemon response without the new keys must still parse — this
        // is the backward-compatibility guarantee.
        let v: RouteConfigView =
            serde_json::from_value(serde_json::json!({ "mode": "auto" })).unwrap();
        assert_eq!(v.mode, "auto");
        assert!(v.load_balance.is_none());
        assert!(v.rate_limits.is_empty());
    }
}
