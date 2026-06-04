//! `[route]` config section — local-vs-cloud failover routing mode.
//!
//! A hard operator preference that shapes how the failover chain
//! ([`crate::providers::FailoverProvider`]) orders and gates its candidates by
//! endpoint tier (on-machine vs public API). The mode is an *explicit user
//! choice* and the tier is *base-url-derived data* — so the route policy reads
//! only hard signals, never the prompt (R7 LLM sovereignty preserved).
//!
//! [`Auto`](RouteMode::Auto) (the default) is a no-op: candidates keep their
//! configured order, byte-identical to pre-route failover.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Three-state local/cloud routing preference.
///
/// - [`Auto`](RouteMode::Auto) (default): no tier shaping. The operator's
///   `[providers]` / `[fallback_provider].chain` order is the routing decision.
/// - [`AlwaysLocal`](RouteMode::AlwaysLocal): prefer on-machine endpoints;
///   cloud candidates are dropped (or gated behind approval when
///   `allow_cloud_escalation` is set).
/// - [`AlwaysCloud`](RouteMode::AlwaysCloud): prefer public-API endpoints;
///   local candidates are appended last as an ungated safe degrade.
///
/// Mirrors the [`CacheRetention`](crate::config::types::provider::CacheRetention)
/// enum precedent (Copy + Default + snake_case serde).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RouteMode {
    /// No tier preference — configured order is the route (byte-identical
    /// to pre-route failover).
    #[default]
    Auto,
    /// Prefer on-machine endpoints; cloud is dropped or approval-gated.
    AlwaysLocal,
    /// Prefer public-API endpoints; local is an ungated last-resort degrade.
    AlwaysCloud,
}

/// Load-balancing strategy applied *within* the same-tier candidate group of
/// the failover chain (the fallback pool around the live primary).
///
/// Pure **infrastructure** (R7 赋能层): decides ordering from prompt-blind hard
/// signals only — runtime in-flight counts, observed latency, a rotation
/// counter — never from message content. Maps the reference routers' strategies
/// (LiteLLM `least-busy` / `latency-based`, Bifrost weighted selection) onto
/// Aleph's lock-free atomics.
///
/// [`Ordered`](LoadBalanceStrategy::Ordered) (the default) is a no-op: the
/// configured chain order is preserved, byte-identical to pre-balance failover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoadBalanceStrategy {
    /// Keep the configured fallback order (byte-identical to pre-balance).
    #[default]
    Ordered,
    /// Rotate the first-choice fallback each request by a global counter, so a
    /// steady stream of requests is spread evenly across equivalent endpoints.
    RoundRobin,
    /// Prefer the fallback with the fewest in-flight requests right now.
    LeastBusy,
    /// Prefer the fallback with the lowest observed EWMA latency (unsampled
    /// endpoints are tried first so they gain a measurement).
    LatencyAware,
}

/// `[route]` section. Defaults to `Auto` + no escalation — fully
/// backward-compatible (absent section == today's failover behaviour).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelRouteConfig {
    /// Local/cloud routing preference. Default [`RouteMode::Auto`].
    #[serde(default)]
    pub mode: RouteMode,

    /// Load-balancing strategy for the same-tier fallback pool. Default
    /// [`LoadBalanceStrategy::Ordered`] — configured order, byte-identical to
    /// pre-balance failover. A live operator signal like [`RouteMode`].
    #[serde(default)]
    pub load_balance: LoadBalanceStrategy,

    /// In [`RouteMode::AlwaysLocal`], whether a cloud candidate may still be
    /// tried as an *approval-gated* terminal fallback ("borrow cloud"). When
    /// `false` (default), cloud candidates are dropped outright in AlwaysLocal.
    /// Ignored in `Auto` / `AlwaysCloud`.
    #[serde(default)]
    pub allow_cloud_escalation: bool,

    /// Preferred *local* provider, chosen by name from the already-configured
    /// `[providers]` (the panel populates a dropdown — no provider is redefined
    /// here). When set, a local candidate with this name is promoted to the
    /// front of its tier so the active route dials it first. `None` (default)
    /// keeps the configured order — byte-identical to pre-selection routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_provider: Option<String>,

    /// Preferred *cloud* provider, chosen by name from the configured
    /// `[providers]` (same reuse contract as
    /// [`local_provider`](Self::local_provider)). When set, a cloud candidate
    /// with this name is promoted to the front of its tier. `None` (default)
    /// keeps the configured order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_provider: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_auto_no_escalation() {
        let c = ModelRouteConfig::default();
        assert_eq!(c.mode, RouteMode::Auto);
        assert!(!c.allow_cloud_escalation);
    }

    #[test]
    fn mode_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&RouteMode::AlwaysLocal).unwrap(),
            "\"always_local\""
        );
        assert_eq!(
            serde_json::to_string(&RouteMode::AlwaysCloud).unwrap(),
            "\"always_cloud\""
        );
        assert_eq!(serde_json::to_string(&RouteMode::Auto).unwrap(), "\"auto\"");
    }

    #[test]
    fn toml_round_trip() {
        let toml_src = "mode = \"always_local\"\nallow_cloud_escalation = true\n";
        let c: ModelRouteConfig = toml::from_str(toml_src).unwrap();
        assert_eq!(c.mode, RouteMode::AlwaysLocal);
        assert!(c.allow_cloud_escalation);
    }

    #[test]
    fn empty_toml_uses_defaults() {
        let c: ModelRouteConfig = toml::from_str("").unwrap();
        assert_eq!(c, ModelRouteConfig::default());
    }

    #[test]
    fn provider_pins_default_to_none() {
        let c = ModelRouteConfig::default();
        assert_eq!(c.local_provider, None);
        assert_eq!(c.cloud_provider, None);
    }

    #[test]
    fn provider_pins_round_trip_and_omit_when_none() {
        // Absent pins stay absent on the wire (backward-compatible).
        let json = serde_json::to_string(&ModelRouteConfig::default()).unwrap();
        assert!(!json.contains("local_provider"));
        assert!(!json.contains("cloud_provider"));

        // Present pins survive a TOML round-trip.
        let toml_src = "mode = \"always_local\"\nlocal_provider = \"ollama\"\ncloud_provider = \"anthropic\"\n";
        let c: ModelRouteConfig = toml::from_str(toml_src).unwrap();
        assert_eq!(c.local_provider.as_deref(), Some("ollama"));
        assert_eq!(c.cloud_provider.as_deref(), Some("anthropic"));
    }

    #[test]
    fn load_balance_defaults_to_ordered() {
        assert_eq!(
            ModelRouteConfig::default().load_balance,
            LoadBalanceStrategy::Ordered
        );
        // Absent from an old config → defaults in, byte-identical behaviour.
        let c: ModelRouteConfig = toml::from_str("mode = \"auto\"\n").unwrap();
        assert_eq!(c.load_balance, LoadBalanceStrategy::Ordered);
    }

    #[test]
    fn load_balance_serializes_snake_case_and_round_trips() {
        assert_eq!(
            serde_json::to_string(&LoadBalanceStrategy::LeastBusy).unwrap(),
            "\"least_busy\""
        );
        assert_eq!(
            serde_json::to_string(&LoadBalanceStrategy::LatencyAware).unwrap(),
            "\"latency_aware\""
        );
        assert_eq!(
            serde_json::to_string(&LoadBalanceStrategy::RoundRobin).unwrap(),
            "\"round_robin\""
        );
        let c: ModelRouteConfig =
            toml::from_str("mode = \"auto\"\nload_balance = \"least_busy\"\n").unwrap();
        assert_eq!(c.load_balance, LoadBalanceStrategy::LeastBusy);
    }
}
