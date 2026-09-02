//! Read-only runtime observability for the failover routing chain.
//!
//! `build_failover_chain` assembles rich runtime state — the circuit breaker
//! ([`FailoverHealth`]), the per-model and per-provider rate-limit cooldowns,
//! and the live load registry ([`LoadStats`]) — but that state previously had
//! no exit: it shaped every candidate ordering yet was invisible to the model
//! and the operator ("why did my request fall back / stall / which provider
//! is throttled" was unanswerable). [`RouteObservability`] bundles cheap
//! clones of those shared handles (cloning shares the same `Arc` maps, so a
//! snapshot is always live) plus the boot-time chain composition, and renders
//! one JSON snapshot for the `self_config` `route_status` tool action.
//!
//! R7/R8 stance: this surfaces HARD runtime facts (circuit states, cooldown
//! windows, in-flight counts, EWMA latency, rolling RPM/TPM usage) so the
//! *model* can reason about provider health when it picks models or diagnoses
//! a stall. It decides nothing itself — pure read-only infrastructure layer. Mirrors the
//! reference routers' status surfaces (`LiteLLM`'s health-state cache, the
//! semantic-router's `x-vsr-*` decision headers, `RouteLLM`'s per-route model
//! counts) without any of their classifier machinery.

use arc_swap::ArcSwap;
use serde_json::json;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::config::types::{LoadBalanceStrategy, ModelRouteConfig, RouteMode};
use crate::providers::default_handle::DefaultProviderHandle;
use crate::providers::failover::{FailoverHealth, ModelCooldown, ProviderCooldown};
use crate::providers::load_stats::LoadStats;
use crate::providers::route_handle::RouteHandle;
use crate::providers::route_policy::{
    route_problems, EndpointTier, RateLimits, RouteProblem, RouteTargets,
};
use crate::sync_primitives::Arc;

/// Boot-time composition of one fallback candidate: name, model walk order,
/// and endpoint tier. The live per-request ordering additionally applies the
/// route mode, pins, and load-balancing on top of this configured baseline.
#[derive(Debug, Clone)]
pub struct ChainCandidate {
    pub name: String,
    pub models: Vec<String>,
    pub tier: EndpointTier,
}

/// Shared handles onto the failover chain's live runtime state.
///
/// Built once by `build_failover_chain` alongside the chain itself; every
/// handle is a clone that shares the same underlying `Arc` map the chain
/// mutates, so [`snapshot`](Self::snapshot) always reads the live picture.
#[derive(Clone)]
pub struct RouteObservability {
    /// Live primary slot (hot-reload aware): `current().name()` is the
    /// provider the next request dials first.
    pub primary: Arc<dyn DefaultProviderHandle>,
    /// Boot-time fallback chain composition, in configured order. Chain
    /// *membership* for the rendered snapshot is recomputed through
    /// [`effective_fallback_names`](crate::providers::failover::effective_fallback_names)
    /// — the same function the walk uses — so the reported chain is the chain
    /// that will actually be dialed. This vec supplies the per-name model list
    /// and tier, and is the whole answer for an operator-configured chain.
    pub fallbacks: Vec<ChainCandidate>,
    /// Whether the chain was *auto-derived* (no operator
    /// `[fallback_provider].chain`), in which case membership is re-derived
    /// from the live registry on every request.
    pub auto_derived: bool,
    /// Shared circuit-breaker map (same instance the chains mutate).
    pub health: FailoverHealth,
    /// Shared per-(provider, model) 429 sideline map.
    pub model_cooldown: ModelCooldown,
    /// Shared per-provider rate-limit pacing map.
    pub provider_cooldown: ProviderCooldown,
    /// Shared in-flight / latency / rolling-usage registry.
    pub load: Arc<LoadStats>,
    /// Live route handle (mode / strategy / pins / limits). `None` in tests —
    /// the snapshot then reports the safe defaults (auto / ordered / no pins).
    pub route: Option<Arc<RouteHandle>>,
    /// The global chain itself, asked (read-only) for the order the next request
    /// will walk. `None` in tests — the snapshot then omits `next_order`.
    ///
    /// Holding the chain rather than re-deriving the order here is the whole
    /// point: the ordering is the product of the route mode, the pins, the
    /// strategy, the breaker, the pacing windows and the rate ceilings, and a
    /// second implementation of that composition would be wrong the first time
    /// any one of them changed.
    pub chain: Option<Arc<crate::providers::failover::FailoverProvider>>,
    /// `[route]` settings that are set but cannot take effect
    /// ([`route_problems`]), computed at boot from the same provider/tier
    /// picture the chain was built from — and RE-computed on every `[route]`
    /// hot write ([`hot_apply_problems`](Self::hot_apply_problems)). An
    /// `ArcSwap` (same RCU idiom as [`RouteHandle`]) because the boot value
    /// alone went stale the moment the panel hot-applied a config: a typo'd
    /// pin written at runtime would never show up here, and `route_status`
    /// kept answering "why did my routing configuration do nothing" with the
    /// boot-time list forever. Empty on a clean config.
    pub problems: Arc<ArcSwap<Vec<RouteProblem>>>,
    /// Boot-time provider name → endpoint tier, the same catalog the chain was
    /// built from. Immutable (a provider's tier follows its `base_url`, which a
    /// hot route write cannot change); carried so `hot_apply_problems` can
    /// re-run [`route_problems`] against the same provider picture the boot
    /// list was computed from.
    pub tiers: Arc<std::collections::HashMap<String, EndpointTier>>,
}

const fn tier_str(tier: EndpointTier) -> &'static str {
    match tier {
        EndpointTier::Local => "local",
        EndpointTier::Cloud => "cloud",
        EndpointTier::Unknown => "unknown",
    }
}

const fn mode_str(mode: RouteMode) -> &'static str {
    match mode {
        RouteMode::Auto => "auto",
        RouteMode::AlwaysLocal => "always_local",
        RouteMode::AlwaysCloud => "always_cloud",
    }
}

const fn lb_str(strategy: LoadBalanceStrategy) -> &'static str {
    match strategy {
        LoadBalanceStrategy::Ordered => "ordered",
        LoadBalanceStrategy::RoundRobin => "round_robin",
        LoadBalanceStrategy::LeastBusy => "least_busy",
        LoadBalanceStrategy::LatencyAware => "latency_aware",
        LoadBalanceStrategy::UsageBased => "usage_based",
        LoadBalanceStrategy::CostAware => "cost_aware",
    }
}

/// Blended `input + output` price in milli-USD per million tokens for a
/// `(provider, model)` pair, or `None` when the model is not in the static
/// [`crate::pricing`] table. The same scalar the [`CostAware`] strategy sorts
/// on — surfaced here so `route_status` shows *why* cost routing ranked a
/// provider where it did.
///
/// [`CostAware`]: crate::config::types::LoadBalanceStrategy::CostAware
fn price_milli_per_mtok(provider: &str, model: &str) -> Option<u64> {
    let card = crate::pricing::rate_card(provider, model)?;
    let usd = card.input_per_mtok.unwrap_or(0.0) + card.output_per_mtok.unwrap_or(0.0);
    Some((usd * 1000.0).round() as u64)
}

impl RouteObservability {
    /// Recompute `config_problems` for a hot-applied `[route]` config and
    /// publish them as one atomic swap (the same RCU idiom the route handle
    /// uses for the config itself). Called from the `route` arm of
    /// [`config::live_apply::apply_live_sections`](crate::config::live_apply::apply_live_sections),
    /// alongside the route handle's store — so EVERY hot `[route]` write
    /// republishes it (`route_config.update`, the `update_config` tool /
    /// `config.patch` RPC, `ConfigPatcher::rollback`, `config.reload`) and the
    /// very next `snapshot` reports the problems of the config that is
    /// actually live, not the ones the boot config had. Naming one write face
    /// here is what let the other three go stale; do not re-inline this call
    /// into a handler. The provider/tier picture is the boot catalog
    /// ([`tiers`](Self::tiers)): a route write cannot change which providers
    /// exist or where they point.
    pub fn hot_apply_problems(&self, cfg: &ModelRouteConfig) {
        self.problems
            .store(Arc::new(route_problems(cfg, &self.tiers)));
    }

    /// Render the live routing picture as one JSON value: route knobs, the
    /// chain composition, per-provider runtime health/load, and any active
    /// model cooldowns. Consumed by the `self_config` `route_status` action.
    pub async fn snapshot(&self) -> serde_json::Value {
        // Live route knobs; safe defaults when no handle is wired (tests).
        let (mode, allow_escalation, strategy, targets, limits) = match &self.route {
            Some(h) => {
                // One coherent generation for the whole status render.
                let s = h.snapshot();
                (
                    s.mode,
                    s.allow_escalation,
                    s.load_balance,
                    Arc::clone(&s.targets),
                    Arc::clone(&s.limits),
                )
            }
            None => (
                RouteMode::Auto,
                false,
                LoadBalanceStrategy::Ordered,
                Arc::new(RouteTargets::default()),
                Arc::new(RateLimits::default()),
            ),
        };

        let primary_name = self.primary.current().name().to_string();
        // Chain membership for the *next* request, through the same function the
        // walk uses. An auto-derived chain re-derives from the live registry
        // every request, so the boot snapshot alone would report providers that
        // have since been removed and hide ones added at runtime.
        let configured: Vec<String> = self.fallbacks.iter().map(|c| c.name.clone()).collect();
        let member_names = crate::providers::failover::effective_fallback_names(
            &self.primary.provider_names(),
            &primary_name,
            &configured,
            self.auto_derived,
        );
        let chain: Vec<ChainCandidate> = member_names
            .into_iter()
            .map(|name| {
                let boot = self.fallbacks.iter().find(|c| c.name == name);
                ChainCandidate {
                    // rust-doctor-disable-next-line excessive-clone
                    models: boot.map(|c| c.models.clone()).unwrap_or_default(),
                    // Mirrors `FailoverProvider::node_tier`: a provider that
                    // joined after boot has no catalog entry and is treated as
                    // cloud — the conservative side of gating and cost ranking.
                    tier: boot.map_or(EndpointTier::Cloud, |c| c.tier),
                    name,
                }
            })
            .collect();
        let health = self.health.snapshot().await;
        let pacing = self.provider_cooldown.snapshot().await;
        let cooling = self.model_cooldown.snapshot().await;
        let loads = self.load.all_metrics();

        // Union of every provider name any signal knows about, so a provider
        // that only ever appears in (say) the breaker map still shows up.
        let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        // rust-doctor-disable-next-line excessive-clone
        names.insert(primary_name.clone());
        // rust-doctor-disable-next-line excessive-clone
        names.extend(chain.iter().map(|c| c.name.clone()));
        // rust-doctor-disable-next-line excessive-clone
        names.extend(health.iter().map(|h| h.provider.clone()));
        // rust-doctor-disable-next-line excessive-clone
        names.extend(pacing.iter().map(|(n, _)| n.clone()));
        // rust-doctor-disable-next-line excessive-clone
        names.extend(loads.iter().map(|(n, _)| n.clone()));

        let health_by: std::collections::HashMap<
            &str,
            &crate::providers::failover::ProviderHealthView,
        > = health.iter().map(|h| (h.provider.as_str(), h)).collect();
        let pacing_by: std::collections::HashMap<&str, u64> =
            pacing.iter().map(|(n, s)| (n.as_str(), *s)).collect();
        let loads_by: std::collections::HashMap<&str, crate::providers::route_policy::LoadMetric> =
            loads.iter().map(|(n, m)| (n.as_str(), *m)).collect();
        // Provider → first model, the model the cost-aware sort prices (it is the
        // head of each candidate's model walk). Built from the boot chain.
        let model_by: std::collections::HashMap<&str, &str> = chain
            .iter()
            .filter_map(|c| c.models.first().map(|m| (c.name.as_str(), m.as_str())))
            .collect();
        // Provider → endpoint tier, from the boot chain. Surfaced per provider so
        // an operator can see *why* cost routing ranked an unpriced provider
        // where it did (free local sorts first; unknown-cost cloud sorts last).
        // The live-primary slot is absent here — its tier is intentionally
        // unresolved (`Unknown`), mirroring its `null` price.
        let tier_by: std::collections::HashMap<&str, EndpointTier> =
            chain.iter().map(|c| (c.name.as_str(), c.tier)).collect();

        let providers: serde_json::Map<String, serde_json::Value> = names
            .into_iter()
            .map(|name| {
                let h = health_by.get(name.as_str()).copied();
                let m = loads_by.get(name.as_str()).copied().unwrap_or_default();
                let (util_permille, over_limit) =
                    limits.assess(name.as_str(), m.rpm_used, m.tpm_used);
                let (rpm_limit, tpm_limit) = limits.ceiling(name.as_str()).unwrap_or((None, None));
                // Cost-routing sort key (milli-USD/Mtok); `null` when the
                // provider's first model is unknown or unpriced.
                let price = model_by
                    .get(name.as_str())
                    .and_then(|model| price_milli_per_mtok(name.as_str(), model));
                let entry = json!({
                    "circuit": h.map_or("closed", |h| h.circuit),
                    "failure_count": h.map_or(0, |h| h.failure_count),
                    "last_error": h.and_then(|h| h.last_error.clone()),
                    "breaker_cooldown_remaining_secs": h.and_then(|h| h.cooldown_remaining_secs),
                    "rate_pacing_remaining_secs": pacing_by.get(name.as_str()).copied(),
                    "in_flight": m.in_flight,
                    "latency_ms": m.latency_us / 1000,
                    "rpm_used": m.rpm_used,
                    "tpm_used": m.tpm_used,
                    "rpm_limit": rpm_limit,
                    "tpm_limit": tpm_limit,
                    // Per-mille, not percent: integer-dividing by 10 rendered
                    // every provider under 10% of its ceiling as `0` — the same
                    // value "no limit configured" produces, so the one field an
                    // operator would use to ask "why was this deprioritised"
                    // could not tell idle from unconfigured.
                    "utilization_permille": util_permille,
                    "rate_limited": limits.ceiling(name.as_str()).is_some(),
                    "over_limit": over_limit,
                    "price_milli_per_mtok": price,
                    "endpoint_tier": tier_by.get(name.as_str()).copied().map(tier_str),
                });
                (name, entry)
            })
            .collect();

        // The order the next request will actually walk, straight from the
        // chain. This is the field that answers "why that provider" — the rest
        // of the snapshot is the evidence, this is the verdict.
        let next_order: Option<Vec<serde_json::Value>> = match &self.chain {
            Some(chain) => Some(
                chain
                    .preview_order()
                    .await
                    .into_iter()
                    .map(|step| {
                        json!({
                            "provider": step.provider,
                            "tier": tier_str(step.tier),
                            "slot": if step.primary { "primary" } else { "fallback" },
                            "gate": match step.action {
                                crate::providers::route_policy::CandidateAction::Allow => "allow",
                                crate::providers::route_policy::CandidateAction::CrossTier {
                                    requires_approval: true,
                                } => "cross_tier_needs_approval",
                                crate::providers::route_policy::CandidateAction::CrossTier {
                                    requires_approval: false,
                                } => "cross_tier_degrade",
                                crate::providers::route_policy::CandidateAction::Skip => "skip",
                            },
                            "rate_sidelined": step.sidelined,
                        })
                    })
                    .collect(),
            ),
            None => None,
        };

        json!({
            "mode": mode_str(mode),
            "allow_cloud_escalation": allow_escalation,
            "load_balance": lb_str(strategy),
            "pins": {
                "local": targets.local_provider.clone(),
                "cloud": targets.cloud_provider.clone(),
            },
            "primary": primary_name,
            // Where the chain came from, so an operator reading a surprising
            // member knows whether to edit `[fallback_provider].chain` or to
            // look at which providers are registered.
            "chain_source": if self.auto_derived { "auto_derived" } else { "configured" },
            "fallback_chain": chain
                .iter()
                .map(|c| {
                    json!({
                        "provider": c.name,
                        "tier": tier_str(c.tier),
                        "models": c.models,
                    })
                })
                .collect::<Vec<_>>(),
            // Dial order for the next request, gates included. Absent (not
            // empty) when no chain is attached, so a consumer can tell "nothing
            // to walk" from "nobody asked the chain".
            "next_order": next_order,
            "providers": providers,
            "cooling_models": cooling
                .iter()
                .map(|(p, m, s)| {
                    json!({ "provider": p, "model": m, "remaining_secs": s })
                })
                .collect::<Vec<_>>(),
            // `[route]` settings that are set but inert. Empty on a clean
            // config; a non-empty list is the answer to "why did my routing
            // configuration do nothing". Read fresh (one RCU load) so a
            // hot-applied config's problems show up instead of the boot list.
            "config_problems": self.problems
                .load_full()
                .iter()
                .map(|p| json!({ "field": p.field, "detail": p.detail }))
                .collect::<Vec<_>>(),
        })
    }
}

/// Process-global observability bundle. The daemon is an OS-level singleton
/// (flock), so one cell per process is the whole world — same contract as
/// [`route_handle`](crate::providers::route_handle)'s global. Production
/// installs it from the boot path (`orchestrator_init`); the `--lib` binary
/// installs chainless [`test_observability`] bundles here too, from the tests
/// that exercise a `[route]` hot write against the face `route_status`
/// actually reads.
///
/// The slot is install-once with NO uninstall, so from the first such test
/// onward every later test in that process sees a populated global. Which
/// tests those are is not a list anyone maintains: every test that reaches
/// this slot — or the route handle the same arm pokes beside it — carries
/// `#[serial_test::serial(route_observability_global)]`, and
/// `tests::every_test_that_reaches_the_route_globals_is_tagged` derives that
/// membership from the CALL across the whole crate. So a test that needs the
/// absent (`None`) branch cannot get it by running first; build it a local
/// [`RouteObservability`] instead.
///
/// `ConsumerDecides`: six production call sites, each choosing differently.
/// `self_config`'s `route_status` drops the whole `data.runtime` object and the
/// sentence that tells the model to read it — the same tool then says nothing
/// about live health, so the model does exactly what that sentence warns
/// against and guesses why a provider was chosen. `health_prober` `continue`s
/// every tick forever, a prober that never probes and never says so.
/// `live_apply`'s `route` arm skips the `config_problems` republish silently
/// (the route handle still stores, because a missing bundle is not a failed
/// route apply), and three more readers each pick their own answer.
///
/// ⚠️ The paragraph above this one points at
/// [`route_handle`](crate::providers::route_handle)'s global for "the same
/// contract". After batch D that is now the ONE handle in `src/` still on a raw
/// `OnceLock` — deliberately: it is first-caller-wins (its initialiser closes
/// over the caller's `cfg`), which `CapabilitySlot::install` cannot express
/// without forging an `Installed` stamp for a boot that never happened.
/// Task 13 adjudicates it. Do not "finish the job" by migrating it here.
static GLOBAL: CapabilitySlot<RouteObservability> = CapabilitySlot::new(
    "providers/route-observability",
    MissingSemantics::ConsumerDecides,
);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn global_route_observability_slot() -> &'static dyn SlotStatus {
    &GLOBAL
}

/// Register the boot-assembled bundle. First call wins (one chain assembly
/// per boot); later calls are ignored.
pub fn set_global_route_observability(obs: RouteObservability) {
    let _ = GLOBAL.install(obs);
}

/// Record that boot reached this slot and had nothing to install.
///
/// Boot installs this from inside `initialize_orchestrator`, which is gated on
/// a default provider plus a session service; without it `route_status` renders
/// config-only output that looks like a healthy chain with no history.
/// `because` is quoted verbatim to an operator.
pub fn decline_global_route_observability(because: &'static str) {
    GLOBAL.decline(because);
}

/// The registered bundle, if the production chain has been assembled. `None`
/// before boot — and in the `--lib` binary only until the first test installs
/// one (the `GLOBAL` slot doc above states that rule). Callers degrade to
/// config-only output.
pub fn global_route_observability() -> Option<&'static RouteObservability> {
    GLOBAL.get()
}

/// A bundle with no chain behind it, for tests in this crate.
///
/// Lives here, next to the struct, because three test modules need one
/// (`route_observe`'s own, `config::live_apply`'s executor arm, and
/// `gateway::handlers::route_config`'s handler face) and a twelve-field
/// literal copied into each is twelve chances for the copies to disagree
/// about what "no chain behind it" means. `primary` and `tiers` are the only
/// parts a caller cares about; everything else is the empty/default state a
/// pre-boot process has.
#[cfg(test)]
pub(crate) fn test_observability(
    primary: Arc<dyn DefaultProviderHandle>,
    tiers: std::collections::HashMap<String, EndpointTier>,
) -> RouteObservability {
    RouteObservability {
        primary,
        fallbacks: Vec::new(),
        auto_derived: false,
        health: FailoverHealth::default(),
        model_cooldown: ModelCooldown::default(),
        provider_cooldown: ProviderCooldown::default(),
        load: Arc::new(LoadStats::new()),
        route: None,
        chain: None,
        problems: Arc::new(ArcSwap::from_pointee(Vec::new())),
        tiers: Arc::new(tiers),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::default_handle::StaticDefault;
    use crate::providers::AiProvider;
    use std::future::Future;
    use std::pin::Pin;

    struct NamedProvider(&'static str);

    impl AiProvider for NamedProvider {
        fn process<'a>(
            &'a self,
            _req: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            Box::pin(async { Ok(ProviderResponse::text_only("ok".to_string())) })
        }
        fn name(&self) -> &str {
            self.0
        }
        fn color(&self) -> &str {
            "#000000"
        }
    }

    fn observability(fallbacks: Vec<ChainCandidate>) -> RouteObservability {
        let mut obs = test_observability(
            Arc::new(StaticDefault::new(Arc::new(NamedProvider("kimi")))),
            std::collections::HashMap::new(),
        );
        obs.fallbacks = fallbacks;
        obs
    }

    #[tokio::test]
    async fn hot_applied_problems_replace_the_boot_list() {
        // `config_problems` used to be frozen at boot: hot-apply a typo'd pin
        // through the panel and `route_status` kept showing the boot-time
        // (empty) list — the one field that could explain "my routing
        // configuration did nothing" never saw the config that broke it.
        let mut obs = observability(vec![]);
        obs.tiers = Arc::new(std::collections::HashMap::from([(
            "ollama".to_string(),
            EndpointTier::Local,
        )]));
        assert!(obs.snapshot().await["config_problems"]
            .as_array()
            .unwrap()
            .is_empty());

        obs.hot_apply_problems(&ModelRouteConfig {
            local_provider: Some("olama".to_string()),
            ..Default::default()
        });
        let problems = obs.snapshot().await["config_problems"].clone();
        let problems = problems.as_array().unwrap();
        assert_eq!(problems.len(), 1, "got: {problems:?}");
        assert_eq!(problems[0]["field"], "local_provider");
        assert!(problems[0]["detail"].as_str().unwrap().contains("olama"));

        // A later clean write clears the list again.
        obs.hot_apply_problems(&ModelRouteConfig::default());
        assert!(obs.snapshot().await["config_problems"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn snapshot_reports_safe_defaults_without_route_handle() {
        let obs = observability(vec![]);
        let snap = obs.snapshot().await;
        assert_eq!(snap["mode"], "auto");
        assert_eq!(snap["allow_cloud_escalation"], false);
        assert_eq!(snap["load_balance"], "ordered");
        assert_eq!(snap["primary"], "kimi");
        // The primary always appears in the providers map, breaker closed.
        assert_eq!(snap["providers"]["kimi"]["circuit"], "closed");
        assert_eq!(snap["providers"]["kimi"]["in_flight"], 0);
        assert!(snap["cooling_models"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn snapshot_includes_chain_composition_and_live_load() {
        let obs = observability(vec![ChainCandidate {
            name: "x302".to_string(),
            models: vec!["gpt-5".to_string()],
            tier: EndpointTier::Cloud,
        }]);
        let _g = obs.load.begin("x302");
        let snap = obs.snapshot().await;
        let chain = snap["fallback_chain"].as_array().unwrap();
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0]["provider"], "x302");
        assert_eq!(chain[0]["tier"], "cloud");
        assert_eq!(chain[0]["models"][0], "gpt-5");
        // Live load registry feeds the provider entry.
        assert_eq!(snap["providers"]["x302"]["in_flight"], 1);
        assert_eq!(snap["providers"]["x302"]["rpm_used"], 1);
        // Endpoint tier is surfaced per provider so the cost-routing rank is
        // explainable; the live-primary slot stays `null` (tier unresolved).
        assert_eq!(snap["providers"]["x302"]["endpoint_tier"], "cloud");
        assert!(snap["providers"]["kimi"]["endpoint_tier"].is_null());
    }

    #[tokio::test]
    async fn snapshot_schema_is_locked() {
        // `route_status` is consumed by the Panel route page and by operators
        // reading raw JSON — neither is compiled against this shape, so a
        // field rename would ship silently. Pin the full key set: any rename
        // or accidental drop turns this test red.
        let obs = observability(vec![ChainCandidate {
            name: "x302".to_string(),
            models: vec!["gpt-5".to_string()],
            tier: EndpointTier::Cloud,
        }]);
        let snap = obs.snapshot().await;

        let top: std::collections::BTreeSet<&str> = snap
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let expected_top: std::collections::BTreeSet<&str> = [
            "mode",
            "allow_cloud_escalation",
            "load_balance",
            "pins",
            "primary",
            "chain_source",
            "fallback_chain",
            "next_order",
            "providers",
            "cooling_models",
            "config_problems",
        ]
        .into_iter()
        .collect();
        assert_eq!(top, expected_top, "route_status top-level schema drifted");

        let provider: std::collections::BTreeSet<&str> = snap["providers"]["x302"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let expected_provider: std::collections::BTreeSet<&str> = [
            "circuit",
            "failure_count",
            "last_error",
            "breaker_cooldown_remaining_secs",
            "rate_pacing_remaining_secs",
            "in_flight",
            "latency_ms",
            "rpm_used",
            "tpm_used",
            "rpm_limit",
            "tpm_limit",
            "utilization_permille",
            "rate_limited",
            "over_limit",
            "price_milli_per_mtok",
            "endpoint_tier",
        ]
        .into_iter()
        .collect();
        assert_eq!(
            provider, expected_provider,
            "route_status per-provider schema drifted"
        );

        let chain_step: std::collections::BTreeSet<&str> = snap["fallback_chain"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let expected_step: std::collections::BTreeSet<&str> =
            ["provider", "tier", "models"].into_iter().collect();
        assert_eq!(
            chain_step, expected_step,
            "route_status fallback_chain schema drifted"
        );
    }

    /// The variant is the operator-facing severity of this handle going
    /// missing (`FailsOpen` => Error and a non-zero `aleph doctor`;
    /// `IndistinguishableDefault` / `ConsumerDecides` => Warning;
    /// `FailsClosed` => Info), and it is DERIVED from the consumers named on
    /// the static above. Pinned in the module that owns the handle, because
    /// that is the only place a reclassification and a re-read of those
    /// consumers can be made to happen together — the aggregate figure in
    /// FEATURE_LOCATOR cannot tell a reclassification from a new slot.
    /// `census::every_slot_pins_its_own_missing_semantics` requires this by
    /// slot id.
    #[test]
    fn the_observability_slot_pins_its_missing_semantics() {
        assert_eq!(
            global_route_observability_slot().id(),
            "providers/route-observability"
        );
        assert!(
            matches!(global_route_observability_slot().missing(), MissingSemantics::ConsumerDecides),
            "`providers/route-observability` is classified ConsumerDecides from its consumers; changing that \
             means re-reading them, not re-typing this line"
        );
    }

    /// Every test in this crate that reaches the process-global route
    /// observability bundle — or the process-global `RouteHandle` the same
    /// `[route]` hot-apply arm pokes beside it — carries
    /// `#[serial_test::serial(route_observability_global)]`.
    ///
    /// # Why membership is derived from the call, not written down
    ///
    /// Both globals are install-once with no uninstall: the bundle is a
    /// `CapabilitySlot`, the handle a first-caller-wins `OnceLock`. So the
    /// first test that installs either one changes what every LATER test in
    /// the `--lib` binary sees, and the two tests that each assert
    /// `config_problems.len() == 1` on the same bundle
    /// (`config::live_apply`'s executor arm and
    /// `gateway::handlers::route_config`'s handler face) interleave into a
    /// flake if they run concurrently. The key excludes them from each other;
    /// this census is what stops the third one from being written without it.
    ///
    /// The precedent is
    /// `gateway::handlers::pty::tests::every_test_that_reaches_the_global_pty_manager_is_tagged`,
    /// which replaced a hand-written list after a measured 3-failures-in-8-runs
    /// flake. Its lesson — a guard that enumerates its own members, by name or
    /// by file, is blind to the member not yet written (§0 「守卫的绿只覆盖它
    /// 认得的那种形状」) — is carried here rather than rediscovered here
    /// (判据 #16, 孪生子系统).
    ///
    /// # Needles, corpus, attribution
    ///
    /// Two substrings, which subsume every spelling: `set_` and `decline_`
    /// prefix `global_route_observability(`, `try_` prefixes
    /// `global_route_handle(`. A reader is as order-dependent as a writer, so
    /// both faces are in scope. The corpus is `cfg_test_portion` of every
    /// `.rs` file under `src/` — the production callers (`health_prober`,
    /// `codex_token_refresher`, `self_config`, the `route` arm itself) are not
    /// tests and must not be charged — and `code_text` blanks comments and
    /// string-literal payloads, so this test's own constants cannot match
    /// themselves.
    ///
    /// Attribution — which test owns a hit — is
    /// [`scan_test_bodies`](crate::utils::source_scan::scan_test_bodies), the
    /// same walk the pty census uses, because that half of the question is one
    /// fact and a second copy of it would be free to disagree about where a
    /// body ends. A hit in NO test body — a shared helper — fails too, naming
    /// itself: the walk will not guess which tests call a helper, and charging
    /// it to whichever test precedes it would be a verdict about the wrong
    /// function.
    #[test]
    fn every_test_that_reaches_the_route_globals_is_tagged() {
        use crate::utils::source_scan::{
            cfg_test_portion, code_text, rust_sources_under, scan_test_bodies,
        };

        const TAG: &str = "#[serial_test::serial(route_observability_global)]";
        const NEEDLES: [&str; 2] = ["global_route_observability(", "global_route_handle("];
        // Asserted PRESENT, never asserted to be the whole set: a new file may
        // reach these globals, it just has to be tagged. This list is here so
        // that a scan which silently stops finding anything fails loudly
        // instead of passing vacuously.
        const KNOWN_REACHERS: [&str; 2] = [
            "src/config/live_apply.rs",
            "src/gateway/handlers/route_config.rs",
        ];

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut reaching: Vec<String> = Vec::new();
        let mut reaching_tests = 0usize;
        let mut violations: Vec<String> = Vec::new();

        for (path, src) in rust_sources_under(&root) {
            let code = code_text(&cfg_test_portion(&src));
            let reaches = |l: &str| NEEDLES.iter().any(|n| l.contains(n));
            if !code.lines().any(&reaches) {
                continue;
            }
            reaching.push(path.clone());

            let scan = scan_test_bodies(&code, &reaches);
            for test in &scan.tests {
                if !test.reaches {
                    continue;
                }
                reaching_tests += 1;
                if !test.attrs.iter().any(|a| a == TAG) {
                    violations.push(format!(
                        "{path}: `{}` reaches a process-global route handle without {TAG}",
                        test.name
                    ));
                }
            }
            for (line, text) in &scan.uncharged {
                violations.push(format!(
                    "{path}:{line}: `{text}` reaches a process-global route handle outside any \
                     #[test] body (a shared helper?). This guard will not guess which tests \
                     call it — move the call into the tests, or tag every test in that file \
                     with {TAG}"
                ));
            }
        }

        assert!(
            violations.is_empty(),
            "untagged users of the process-global route observability bundle / route handle. \
             Both are install-once with no uninstall, so an untagged test can install its own \
             bundle between another test's `[route]` write and that test's assertion — the \
             `config_problems.len() == 1` pair in `config::live_apply` and \
             `gateway::handlers::route_config` is exactly that shape.\n  {}",
            violations.join("\n  ")
        );
        for known in KNOWN_REACHERS {
            assert!(
                reaching.iter().any(|p| p.ends_with(known)),
                "the scan no longer sees {known} reaching the route globals. Either the call \
                 moved (fine — update this list) or the scanner stopped working (not fine: a \
                 census that finds nothing passes vacuously). Files it did find: {reaching:?}"
            );
        }
        assert!(
            reaching_tests >= 2,
            "the scanner charged only {reaching_tests} test bodies with a route-global call, \
             fewer than the 2 that existed when this census was written — so it is passing \
             vacuously. Files it did find: {reaching:?}"
        );
    }
}
