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

use std::collections::BTreeMap;

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
/// enum precedent (Copy + Default + `snake_case` serde).
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
/// Pure **infrastructure** (R7 enabling layer): decides ordering from prompt-blind hard
/// signals only — runtime in-flight counts, observed latency, a rotation
/// counter — never from message content. Maps the reference routers' strategies
/// (`LiteLLM` `least-busy` / `latency-based`, Bifrost weighted selection) onto
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
    /// Prefer the fallback with the most *remaining* rate headroom — the lowest
    /// rpm/tpm utilisation against its configured [`ProviderRateLimit`]. Maps
    /// `LiteLLM` `usage-based-routing-v2` (pick the least-used deployment) onto the
    /// lock-free [`crate::providers::load_stats`] window counters. When no
    /// `rate_limits` are configured every provider reads 0% utilisation, so this
    /// degrades to [`Ordered`](LoadBalanceStrategy::Ordered).
    UsageBased,
    /// Prefer the *cheapest* fallback — the lowest blended `input + output`
    /// per-million-token price from the static [`crate::pricing`] rate card of
    /// each candidate's first model. Maps `LiteLLM` `cost-based-routing`
    /// (`lowest-cost`) and `RouteLLM`'s cheap-vs-strong cost axis onto Aleph's
    /// already-shipped price table — pure **infrastructure** (R7 enabling layer): the
    /// price is a static `(provider, model)` fact, never inferred from the
    /// prompt.
    ///
    /// An *unpriced* candidate is ranked by its endpoint tier, not by a single
    /// "free" default: a
    /// [`Local`](crate::providers::route_policy::EndpointTier::Local) endpoint
    /// carries no rate card because on-machine inference really is free, so it
    /// reads `0` and sorts first; an unpriced
    /// [`Cloud`](crate::providers::route_policy::EndpointTier::Cloud) or
    /// [`Unknown`](crate::providers::route_policy::EndpointTier::Unknown) one
    /// has *unknown* cost — a new or typo'd model — and reads [`u64::MAX`] so it
    /// can never out-rank a price we can confirm. Both facts are the whole of
    /// `failover::provider::unpriced_cost`, pinned by
    /// `unpriced_local_is_free_and_sorts_first` /
    /// `unpriced_cloud_is_unknown_cost_and_sorts_last`.
    ///
    /// So this is a no-op only over a pool that is *both* wholly unpriced and
    /// tier-homogeneous (every member `MAX`, or every member `0`), where the
    /// stable sort preserves the configured order. A pool mixing unpriced local
    /// with unpriced cloud is still reordered — local first — which is the
    /// intended answer, not a degradation.
    CostAware,
}

impl LoadBalanceStrategy {
    /// The one spelling of this strategy: the serde name (`rename_all =
    /// "snake_case"`), which is simultaneously the TOML value, the
    /// `route_config` wire value and the `route_status` rendering.
    ///
    /// It exists because three modules each kept their own `match` over these
    /// six variants — the RPC handler's, the observability snapshot's, and the
    /// rejection message's — plus serde's, which is the one an operator's TOML
    /// actually goes through. Four derivations of one table, free to disagree
    /// about a seventh variant. `strategy_spelling_is_the_serde_name` pins this
    /// one against serde.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ordered => "ordered",
            Self::RoundRobin => "round_robin",
            Self::LeastBusy => "least_busy",
            Self::LatencyAware => "latency_aware",
            Self::UsageBased => "usage_based",
            Self::CostAware => "cost_aware",
        }
    }
}

/// Per-provider rate ceiling for usage-aware routing.
///
/// Mirrors a `LiteLLM` deployment's `rpm`/`tpm`: a soft budget the
/// [`UsageBased`](LoadBalanceStrategy::UsageBased) strategy spreads load under,
/// and a hard ceiling above which the provider is deprioritised to the back of
/// its tier (never dropped — the failover chain must never starve). Both fields
/// are optional; an unset dimension is simply unbounded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderRateLimit {
    /// Requests per rolling 60s window. `None` = unbounded request rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm: Option<u32>,
    /// Tokens (input + output) per rolling 60s window. `None` = unbounded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tpm: Option<u32>,
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
    /// `false` (default), cloud candidates are dropped outright in `AlwaysLocal`.
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

    /// Per-provider rate ceilings (rpm/tpm), keyed by provider name from the
    /// configured `[providers]` — same name-reference contract as
    /// [`local_provider`](Self::local_provider). Consumed by
    /// [`LoadBalanceStrategy::UsageBased`] (spread by headroom) and the
    /// over-limit deprioritisation gate (any strategy). Empty (default) = no
    /// rate awareness, byte-identical to pre-usage failover. `BTreeMap` keeps
    /// serialisation deterministic.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rate_limits: BTreeMap<String, ProviderRateLimit>,

    /// Background health-probe interval for circuit-open providers, in
    /// seconds. `None`/`0` (default) disables the prober: probing spends real
    /// requests (and money) against the endpoint, so it is strictly opt-in.
    /// When set, a background task pings each **enabled** provider whose
    /// circuit breaker is open and resets the breaker on a successful ping, so
    /// a low-traffic provider does not stay sidelined for the full breaker
    /// cooldown waiting for a real request to probe it. A provider the operator
    /// disabled is never dialled. Hot-tunable via `route_config.update` (and
    /// from the Panel's route page): the loop re-reads this at most 30s after
    /// the change, so switching it off does not buy one more probe pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_probe_interval_secs: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `as_str` must be serde's name, because an operator's `[route]
    /// load_balance = "..."` is parsed by serde while every other face renders
    /// through `as_str`. A disagreement here would let `route_status` print a
    /// value the config file cannot contain.
    ///
    /// Derived by round-tripping through serde in BOTH directions, so a variant
    /// added without an `as_str` arm is a compile error and one spelled
    /// differently is a red test.
    #[test]
    fn strategy_spelling_is_the_serde_name() {
        for strategy in [
            LoadBalanceStrategy::Ordered,
            LoadBalanceStrategy::RoundRobin,
            LoadBalanceStrategy::LeastBusy,
            LoadBalanceStrategy::LatencyAware,
            LoadBalanceStrategy::UsageBased,
            LoadBalanceStrategy::CostAware,
        ] {
            let spelling = strategy.as_str();
            assert_eq!(
                serde_json::to_value(strategy).unwrap(),
                serde_json::Value::String(spelling.to_string()),
                "`as_str` disagrees with serde for {strategy:?}"
            );
            assert_eq!(
                serde_json::from_value::<LoadBalanceStrategy>(serde_json::json!(spelling)).unwrap(),
                strategy,
                "'{spelling}' does not parse back to {strategy:?}"
            );
        }
    }

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

    #[test]
    fn usage_based_strategy_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&LoadBalanceStrategy::UsageBased).unwrap(),
            "\"usage_based\""
        );
        let c: ModelRouteConfig =
            toml::from_str("mode = \"auto\"\nload_balance = \"usage_based\"\n").unwrap();
        assert_eq!(c.load_balance, LoadBalanceStrategy::UsageBased);
    }

    #[test]
    fn cost_aware_strategy_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&LoadBalanceStrategy::CostAware).unwrap(),
            "\"cost_aware\""
        );
        let c: ModelRouteConfig =
            toml::from_str("mode = \"auto\"\nload_balance = \"cost_aware\"\n").unwrap();
        assert_eq!(c.load_balance, LoadBalanceStrategy::CostAware);
    }

    #[test]
    fn rate_limits_default_empty_and_omitted_on_wire() {
        let c = ModelRouteConfig::default();
        assert!(c.rate_limits.is_empty());
        // Empty map stays off the wire (backward-compatible).
        let json = serde_json::to_string(&c).unwrap();
        assert!(!json.contains("rate_limits"));
    }

    #[test]
    fn rate_limits_round_trip_and_partial_dims() {
        let toml_src = "mode = \"auto\"\n\
            [rate_limits.anthropic]\nrpm = 60\ntpm = 90000\n\
            [rate_limits.ollama]\nrpm = 1000\n";
        let c: ModelRouteConfig = toml::from_str(toml_src).unwrap();
        let a = c.rate_limits.get("anthropic").unwrap();
        assert_eq!(a.rpm, Some(60));
        assert_eq!(a.tpm, Some(90_000));
        // tpm omitted → unbounded on that dimension.
        let o = c.rate_limits.get("ollama").unwrap();
        assert_eq!(o.rpm, Some(1000));
        assert_eq!(o.tpm, None);
    }

    #[test]
    fn health_probe_interval_defaults_off_and_round_trips() {
        // Off by default and off the wire (probing spends real requests).
        let c = ModelRouteConfig::default();
        assert_eq!(c.health_probe_interval_secs, None);
        let json = serde_json::to_string(&c).unwrap();
        assert!(!json.contains("health_probe_interval_secs"));

        let c: ModelRouteConfig =
            toml::from_str("mode = \"auto\"\nhealth_probe_interval_secs = 60\n").unwrap();
        assert_eq!(c.health_probe_interval_secs, Some(60));
    }
}
