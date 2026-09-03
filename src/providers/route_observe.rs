//! Read-only runtime observability for the failover routing chain.
//!
//! `build_failover_chain` assembles rich runtime state — the circuit breaker
//! ([`FailoverHealth`]), the per-model and per-provider rate-limit cooldowns,
//! and the live load registry ([`LoadStats`]) — but that state previously had
//! no exit: it shaped every candidate ordering yet was invisible to the model
//! and the operator ("why did my request fall back / stall / which provider
//! is throttled" was unanswerable). [`RouteObservability`] bundles cheap
//! clones of those shared handles (cloning shares the same `Arc` maps, so a
//! snapshot is always live) plus the chain itself, which answers both the
//! order of the next walk and its membership, and renders one JSON snapshot
//! for the `self_config` `route_status` tool action.
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
use crate::config::types::{ModelRouteConfig, RouteMode};
use crate::providers::default_handle::DefaultProviderHandle;
use crate::providers::failover::{FailoverHealth, ModelCooldown, ProviderCooldown};
use crate::providers::load_stats::LoadStats;
use crate::providers::route_handle::RouteState;
use crate::providers::route_policy::{route_problems, EndpointTier, RouteGate, RouteProblem};
use crate::sync_primitives::Arc;

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
    /// Whether the chain was *auto-derived* (no operator
    /// `[fallback_provider].chain`), in which case membership is re-derived
    /// from the live registry on every request. Rendered as `chain_source`;
    /// the composition itself comes from the chain
    /// ([`FailoverProvider::chain_composition`](crate::providers::failover::FailoverProvider::chain_composition)).
    pub auto_derived: bool,
    /// Shared circuit-breaker map (same instance the chains mutate).
    pub health: FailoverHealth,
    /// Shared per-(provider, model) 429 sideline map.
    pub model_cooldown: ModelCooldown,
    /// Shared per-provider rate-limit pacing map.
    pub provider_cooldown: ProviderCooldown,
    /// Shared in-flight / latency / rolling-usage registry.
    pub load: Arc<LoadStats>,
    /// The global chain itself, asked (read-only) for the order the next request
    /// will walk **and for the route generation that order was computed from**
    /// ([`RoutePreview`](crate::providers::failover::RoutePreview)) — the chain
    /// is the bundle's ONLY route generation. A second copy of the live
    /// [`RouteHandle`](crate::providers::route_handle::RouteHandle) beside it
    /// would be shadowed in every process that has a chain (which is every
    /// process: `build_failover_chain` always attaches one) and stand ready
    /// only to disagree with it. `None` in tests — the snapshot then omits
    /// `next_order` and reports [`RouteState::unconfigured`] (auto / ordered /
    /// no pins).
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
    /// `ArcSwap` (same RCU idiom as
    /// [`RouteHandle`](crate::providers::route_handle::RouteHandle)) because the boot value
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
        // The order the next request will actually walk, straight from the
        // chain. This is the field that answers "why that provider" — the rest
        // of the snapshot is the evidence, this is the verdict.
        //
        // Asked FIRST because it also settles which route generation this whole
        // render describes. The chain loads its own generation to order with;
        // loading a second one here for the header is how a `route_config
        // .update` landing between the two publishes a header naming one
        // strategy beside an order produced under another.
        let preview = match &self.chain {
            Some(chain) => Some(chain.preview_order().await),
            None => None,
        };
        // One coherent generation for the whole status render, from a single
        // source: the generation the order was computed from. The chain resolves
        // that itself (`route_snapshot` — live handle if wired, else the boot
        // route over `RouteState::unconfigured`), so there is no second load
        // here to drift from it. Chain-less bundles exist only in tests and
        // render the same shared spelling of "no `[route]` section".
        let route: Arc<RouteState> = match &preview {
            Some(p) => Arc::clone(&p.route),
            None => Arc::new(RouteState::unconfigured()),
        };
        let (mode, allow_escalation, strategy) =
            (route.mode, route.allow_escalation, route.load_balance);
        let targets = Arc::clone(&route.targets);
        let limits = Arc::clone(&route.limits);

        let primary_name = self.primary.current().name().to_string();
        // Chain composition for the *next* request, from the chain itself
        // ([`FailoverProvider::chain_composition`]) — membership, model ladder
        // and endpoint tier all materialised by the SAME code the walk runs.
        //
        // Materialising it a second time here is what must not happen:
        // membership through the shared `effective_fallback_names` (correct),
        // then each member looked up in a boot-time vec and defaulted to
        // `Cloud` with an empty ladder when absent. The boot fallback vec
        // excludes the boot primary, so one `providers.setDefault` makes the
        // ex-primary a chain member that vec never held — `fallback_chain`
        // would call it cloud/`[]`/unpriced while `next_order`, asking the
        // chain, called it local with its real ladder. Empty without a chain —
        // a bundle nobody attached one to has no composition to report, and
        // inventing one is how the two faces drift apart.
        let chain = self
            .chain
            .as_ref()
            .map(|c| c.chain_composition())
            .unwrap_or_default();
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
                // One read of the ceiling for all three fields it feeds: the two
                // rendered numbers and the "does one bound anything" flag. It
                // goes through `effective_ceiling`, the same rule `assess`
                // (above) and `route_problems` (below, in this same payload)
                // apply — a raw read here would print `rpm_limit: 0` beside an
                // `over_limit: false` the engine derived by ignoring that zero.
                let (rpm_limit, tpm_limit) = limits.effective_ceiling(name.as_str());
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
                    // A static config fact — "some dimension of this provider
                    // is actually bounded" — sitting beside two LIVE ones
                    // (`over_limit`, `rate_pacing_remaining_secs`). It was
                    // called `rate_limited`, which in this neighbourhood reads
                    // as "is being throttled right now" and is answered `true`
                    // for every provider with a ceiling, idle or not. It is
                    // derived from the two effective numbers rather than from
                    // "is there an entry", so an inert entry (`rpm = 0`) reads
                    // `false` here and shows up under `config_problems`.
                    "has_rate_ceiling": rpm_limit.is_some() || tpm_limit.is_some(),
                    "over_limit": over_limit,
                    "price_milli_per_mtok": price,
                    "endpoint_tier": tier_by.get(name.as_str()).copied().map(tier_str),
                });
                (name, entry)
            })
            .collect();

        // Render of the preview taken above. Three gate values, not four: a
        // dropped candidate is not IN the chain, so there is no "skip" step to
        // report (the type says so — see `RouteGate`).
        let next_order: Option<Vec<serde_json::Value>> = preview.map(|p| {
            p.steps
                .into_iter()
                .map(|step| {
                    json!({
                        "provider": step.provider,
                        "tier": tier_str(step.tier),
                        "slot": if step.primary { "primary" } else { "fallback" },
                        "gate": match step.action {
                            RouteGate::Allow => "allow",
                            RouteGate::CrossTier { requires_approval: true } =>
                                "cross_tier_needs_approval",
                            RouteGate::CrossTier { requires_approval: false } =>
                                "cross_tier_degrade",
                        },
                        // Both flags mean the same thing to a reader: the walk
                        // passes this step over **while a later candidate
                        // remains**, and dials it anyway when it is the last
                        // one. They differ only in which registry says so.
                        "rate_sidelined": step.rate_sidelined,
                        "health_sidelined": step.health_sidelined,
                    })
                })
                .collect()
        });

        json!({
            "mode": mode_str(mode),
            "allow_cloud_escalation": allow_escalation,
            "load_balance": strategy.as_str(),
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
/// `gateway::handlers::route_config`'s handler face) and a whole-struct
/// literal copied into each is one chance per field for the copies to
/// disagree about what "no chain behind it" means (the count is deliberately
/// not written down here — it moved once already). `primary` and `tiers` are the only
/// parts a caller cares about; everything else is the empty/default state a
/// pre-boot process has.
#[cfg(test)]
pub(crate) fn test_observability(
    primary: Arc<dyn DefaultProviderHandle>,
    tiers: std::collections::HashMap<String, EndpointTier>,
) -> RouteObservability {
    RouteObservability {
        primary,
        auto_derived: false,
        health: FailoverHealth::default(),
        model_cooldown: ModelCooldown::default(),
        provider_cooldown: ProviderCooldown::default(),
        load: Arc::new(LoadStats::new()),
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

    /// A bundle with no chain attached — the pre-boot / test-only shape. It has
    /// no chain to ask, so it reports no composition at all; the tests that need
    /// a `fallback_chain` build [`observability_with_chain`], which is what
    /// production always is.
    fn observability() -> RouteObservability {
        test_observability(
            Arc::new(StaticDefault::new(Arc::new(NamedProvider("kimi")))),
            std::collections::HashMap::new(),
        )
    }

    #[tokio::test]
    async fn hot_applied_problems_replace_the_boot_list() {
        // `config_problems` used to be frozen at boot: hot-apply a typo'd pin
        // through the panel and `route_status` kept showing the boot-time
        // (empty) list — the one field that could explain "my routing
        // configuration did nothing" never saw the config that broke it.
        let mut obs = observability();
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
        let obs = observability();
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
        let obs = observability_with_chain(None);
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

    /// A bundle with a real chain behind it — the production shape, where
    /// `next_order` is present and the header renders the generation that
    /// chain ordered with. `route` is the chain's own boot route snapshot.
    fn observability_with_chain(route: Option<(RouteMode, bool)>) -> RouteObservability {
        let mut obs = observability();
        let chain = kimi_over_x302();
        let chain = match route {
            Some((mode, escalate)) => chain.with_route(mode, escalate, None),
            None => chain,
        };
        obs.chain = Some(Arc::new(chain));
        obs
    }

    /// The one chain shape these tests walk: primary `kimi`, one cloud fallback
    /// `x302` on `gpt-5`.
    fn kimi_over_x302() -> crate::providers::failover::FailoverProvider {
        use crate::providers::failover::{FailoverConfig, FailoverNode, FailoverProvider};
        FailoverProvider::new(
            Arc::new(StaticDefault::new(Arc::new(NamedProvider("kimi")))),
            vec![FailoverNode::with_tier(
                "x302".to_string(),
                vec!["gpt-5".to_string()],
                Arc::new(NamedProvider("x302")),
                EndpointTier::Cloud,
            )],
            std::collections::HashMap::new(),
            FailoverHealth::default(),
            FailoverConfig::default(),
        )
    }

    /// The same chain reading a LIVE `[route]` generation — the production
    /// wiring (`build_failover_chain` always calls `with_route_live`), and the
    /// only shape that puts real `[route].rate_limits` in front of the render.
    /// `config_problems` is primed from the same config, so one snapshot shows
    /// what the render says about an entry *and* what the diagnostic says.
    fn observability_with_route(cfg: &ModelRouteConfig) -> RouteObservability {
        let mut obs = observability();
        obs.tiers = Arc::new(std::collections::HashMap::from([
            ("kimi".to_string(), EndpointTier::Local),
            ("x302".to_string(), EndpointTier::Cloud),
        ]));
        obs.chain = Some(Arc::new(kimi_over_x302().with_route_live(Arc::new(
            crate::providers::route_handle::RouteHandle::from_config(cfg),
        ))));
        obs.hot_apply_problems(cfg);
        obs
    }

    #[tokio::test]
    async fn an_inert_ceiling_reads_false_and_renders_no_bound() {
        // `has_rate_ceiling` was `limits.ceiling(name).is_some()` — "is there an
        // entry" — read raw, beside `rpm_limit`/`tpm_limit` printed just as
        // raw. Both `assess` (which produced the `over_limit` and the
        // utilisation two lines away) and `route_problems` (which fills
        // `config_problems` in the SAME payload) ignore a zero, so `rpm = 0`
        // rendered `has_rate_ceiling: true, rpm_limit: 0, over_limit: false`
        // next to a problem entry calling that very entry inert. Three
        // derivations of one fact; the two on the provider row were the lying
        // ones. All three now fold through `effective_ceiling`.
        let entry = |rpm, tpm| crate::config::types::ProviderRateLimit { rpm, tpm };
        let obs = observability_with_route(&ModelRouteConfig {
            rate_limits: [
                ("x302".to_string(), entry(Some(0), None)),
                ("kimi".to_string(), entry(Some(60), Some(0))),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        });
        let snap = obs.snapshot().await;

        let inert = &snap["providers"]["x302"];
        assert_eq!(inert["has_rate_ceiling"], false, "{snap}");
        assert_eq!(inert["rpm_limit"], serde_json::Value::Null, "{snap}");
        assert_eq!(inert["tpm_limit"], serde_json::Value::Null, "{snap}");

        // A half-inert entry: the label is honest about the dimension that
        // really binds, and silent about the one that does not.
        let half = &snap["providers"]["kimi"];
        assert_eq!(half["has_rate_ceiling"], true, "{snap}");
        assert_eq!(half["rpm_limit"], 60, "{snap}");
        assert_eq!(half["tpm_limit"], serde_json::Value::Null, "{snap}");

        // …and the same payload says why, for BOTH zeroed dimensions.
        let fields: Vec<&str> = snap["config_problems"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["field"].as_str().unwrap())
            .collect();
        assert_eq!(
            fields,
            vec!["rate_limits.kimi", "rate_limits.x302"],
            "the render and the diagnostic must agree about which entries bind: {snap}"
        );
    }

    /// A default handle whose `current()` can be swapped after construction,
    /// backed by a live registry — the production shape. `providers.setDefault`
    /// moves the primary at runtime, and the ex-primary becomes an ordinary
    /// chain member that the BOOT fallback list never contained (a boot chain
    /// excludes the boot primary by construction).
    struct SwappableDefault {
        current: std::sync::RwLock<Arc<dyn AiProvider>>,
        registry: Vec<(&'static str, Arc<dyn AiProvider>)>,
    }

    impl SwappableDefault {
        fn new(registry: Vec<(&'static str, Arc<dyn AiProvider>)>, default: &str) -> Self {
            let current = registry
                .iter()
                .find(|(n, _)| *n == default)
                .map(|(_, p)| Arc::clone(p))
                .expect("default must be registered");
            Self {
                current: std::sync::RwLock::new(current),
                registry,
            }
        }

        fn set_default(&self, name: &str) {
            let next = self
                .registry
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, p)| Arc::clone(p))
                .expect("new default must be registered");
            *self.current.write().unwrap_or_else(|e| e.into_inner()) = next;
        }
    }

    impl DefaultProviderHandle for SwappableDefault {
        fn current(&self) -> Arc<dyn AiProvider> {
            Arc::clone(&self.current.read().unwrap_or_else(|e| e.into_inner()))
        }
        fn provider_names(&self) -> Vec<String> {
            self.registry
                .iter()
                .map(|(n, _)| (*n).to_string())
                .collect()
        }
        fn provider_by_name(&self, name: &str) -> Option<Arc<dyn AiProvider>> {
            self.registry
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, p)| Arc::clone(p))
        }
    }

    /// `fallback_chain` and `next_order` must state ONE tier and ONE model
    /// ladder per member, because that is one fact.
    ///
    /// They used not to. `fallback_chain` recomputed *membership* through the
    /// shared `effective_fallback_names` (correct) and then materialised each
    /// member out of the bundle's boot-time vec, defaulting a name it could not
    /// find to `tier: Cloud, models: []`. The boot fallback vec excludes the
    /// boot primary, so one `providers.setDefault` was enough: the ex-primary
    /// came back as a member, missed that lookup, and was rendered `cloud` with
    /// an empty ladder and a `null` price — while `next_order`, asking the
    /// chain, reported the same provider `local` with its real ladder. Two
    /// derivations of one fact, contradicting each other inside one payload.
    ///
    /// ⚠️ What this does NOT prove: that `local` is *true*. Both faces now read
    /// `FailoverProvider::node_tier`, whose boot catalog has no entry for a
    /// provider registered after boot and answers `Cloud` for it. That residual
    /// is stated on `chain_composition` and deliberately left open — the point
    /// here is that the two faces cannot disagree, not that the catalog is
    /// complete.
    #[tokio::test]
    async fn both_faces_state_one_tier_and_ladder_for_a_swapped_primary() {
        use crate::providers::failover::{FailoverConfig, FailoverNode, FailoverProvider};
        use std::collections::HashMap;

        let ollama: Arc<dyn AiProvider> = Arc::new(NamedProvider("ollama"));
        let kimi: Arc<dyn AiProvider> = Arc::new(NamedProvider("kimi"));
        let handle = Arc::new(SwappableDefault::new(
            vec![("ollama", Arc::clone(&ollama)), ("kimi", Arc::clone(&kimi))],
            "ollama",
        ));
        let tiers = HashMap::from([
            ("ollama".to_string(), EndpointTier::Local),
            ("kimi".to_string(), EndpointTier::Cloud),
        ]);
        let chain = FailoverProvider::new(
            Arc::clone(&handle) as Arc<dyn DefaultProviderHandle>,
            // The boot chain: everything except the boot primary (ollama).
            vec![FailoverNode::with_tier(
                "kimi".to_string(),
                vec!["k2".to_string()],
                Arc::clone(&kimi),
                EndpointTier::Cloud,
            )],
            HashMap::from([
                ("ollama".to_string(), vec!["qwen3".to_string()]),
                ("kimi".to_string(), vec!["k2".to_string()]),
            ]),
            FailoverHealth::default(),
            FailoverConfig::default(),
        )
        // rust-doctor-disable-next-line excessive-clone
        .with_tier_catalog(tiers.clone())
        .with_live_fallback_derivation();

        let mut obs =
            test_observability(Arc::clone(&handle) as Arc<dyn DefaultProviderHandle>, tiers);
        obs.auto_derived = true;
        obs.chain = Some(Arc::new(chain));

        // The runtime swap. Nothing is rebuilt — this is what
        // `providers.setDefault` does to a running process.
        handle.set_default("kimi");

        let snap = obs.snapshot().await;
        let find = |face: &str, snap: &serde_json::Value| {
            snap[face]
                .as_array()
                .unwrap_or_else(|| panic!("{face} must be an array: {snap}"))
                .iter()
                .find(|e| e["provider"] == "ollama")
                .unwrap_or_else(|| panic!("the ex-primary must be a member of {face}: {snap}"))
                .clone()
        };
        let member = find("fallback_chain", &snap);
        let step = find("next_order", &snap);

        assert_eq!(
            member["tier"], step["tier"],
            "fallback_chain and next_order must not state two tiers for one provider"
        );
        assert_eq!(member["tier"], "local", "the chain's own catalog says local");
        assert_eq!(
            member["models"],
            json!(["qwen3"]),
            "the ex-primary's ladder is the catalog's, not an empty default"
        );
        assert_eq!(
            snap["providers"]["ollama"]["endpoint_tier"], "local",
            "the per-provider block reads the same composition"
        );
    }

    #[tokio::test]
    async fn snapshot_schema_is_locked() {
        // `route_status` is read by the model (the `self_config` tool text
        // points it at `data.runtime`) and by operators reading raw JSON —
        // neither is compiled against this shape, so a field rename would ship
        // silently. There is no Panel consumer: `next_order` and the rest of
        // this snapshot exist for those two readers only.
        //
        // BOTH bundle shapes are pinned. The chain-less one is what most tests
        // in this crate build; the chain-attached one is what production
        // always is, and it is the only one that renders `next_order` steps and
        // `fallback_chain` members — locking only the first would document a
        // shape production never emits.
        for (label, obs) in [
            ("chain-less", observability()),
            ("with chain", observability_with_chain(None)),
        ] {
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
            assert_eq!(
                top, expected_top,
                "route_status top-level schema drifted ({label})"
            );

            // The primary, which every bundle shape reports (the chain-less one
            // has no members to report besides it).
            let provider: std::collections::BTreeSet<&str> = snap["providers"]["kimi"]
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
                "has_rate_ceiling",
                "over_limit",
                "price_milli_per_mtok",
                "endpoint_tier",
            ]
            .into_iter()
            .collect();
            assert_eq!(
                provider, expected_provider,
                "route_status per-provider schema drifted ({label})"
            );

            // `fallback_chain` is the chain's own composition. A bundle with no
            // chain has none to report and renders `[]` — it must not invent
            // members out of a boot-time vec, which is exactly how this face
            // and `next_order` came to state two tiers for one provider.
            let members = snap["fallback_chain"]
                .as_array()
                .unwrap_or_else(|| panic!("fallback_chain must be an array ({label})"));
            match &obs.chain {
                None => assert!(
                    members.is_empty(),
                    "a chain-less bundle has no composition to report ({label})"
                ),
                Some(_) => {
                    let chain_step: std::collections::BTreeSet<&str> = members[0]
                        .as_object()
                        .unwrap()
                        .keys()
                        .map(String::as_str)
                        .collect();
                    let expected_step: std::collections::BTreeSet<&str> =
                        ["provider", "tier", "models"].into_iter().collect();
                    assert_eq!(
                        chain_step, expected_step,
                        "route_status fallback_chain schema drifted ({label})"
                    );
                }
            }

            // `next_order` is absent (null), not empty, without a chain — a
            // consumer must be able to tell "nothing to walk" from "nobody
            // asked the chain". With one, every step carries the full key set,
            // including BOTH skip flags: a step rendered without them reads as
            // a healthy dial the walk is about to make.
            match &obs.chain {
                None => assert!(snap["next_order"].is_null(), "{label}"),
                Some(_) => {
                    let step: std::collections::BTreeSet<&str> = snap["next_order"][0]
                        .as_object()
                        .unwrap()
                        .keys()
                        .map(String::as_str)
                        .collect();
                    let expected: std::collections::BTreeSet<&str> = [
                        "provider",
                        "tier",
                        "slot",
                        "gate",
                        "rate_sidelined",
                        "health_sidelined",
                    ]
                    .into_iter()
                    .collect();
                    assert_eq!(step, expected, "route_status next_order schema drifted");
                    assert_eq!(snap["next_order"][0]["slot"], "primary");
                    assert_eq!(snap["next_order"][0]["gate"], "allow");
                }
            }
        }
    }

    #[tokio::test]
    async fn the_header_renders_the_generation_the_order_was_computed_from() {
        // `snapshot` used to load the route state once for the header and the
        // chain loaded a SECOND one to order with, so a `route_config.update`
        // landing between the two published a header naming one policy beside
        // an order produced under another — the round-5 fix that gave the walk
        // one generation (`CandidatePlan.route`) had never been carried to the
        // diagnostic face.
        //
        // The bundle no longer keeps a route handle of its own to disagree
        // with; the chain resolves the generation and the header must follow
        // it. Deterministic here because the chain's boot route is NOT the
        // unconfigured default this render falls back to without a chain, so a
        // header built from anything other than `next_order`'s own generation
        // reads `auto` and this goes red.
        let obs = observability_with_chain(Some((RouteMode::AlwaysCloud, true)));

        let snap = obs.snapshot().await;
        assert_eq!(
            snap["mode"], "always_cloud",
            "the header must describe the generation the order came from"
        );
        assert_eq!(snap["allow_cloud_escalation"], true);
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
