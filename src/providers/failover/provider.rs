//! The [`FailoverProvider`] decorator and its [`AiProvider`] failover walk.

use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use crate::config::types::{LoadBalanceStrategy, RouteMode};
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::capability_gate::{retain_capable_models, RequestRequirements};
use crate::providers::llm_retry::{backoff_delay, is_transient_overload};
use crate::providers::load_stats::LoadStats;
use crate::providers::route_handle::{RouteHandle, RouteState};
use crate::providers::route_policy::{
    classify_candidate, order_candidates, order_candidates_balanced, CandidateAction, EndpointTier,
    RateLimits, RouteTargets,
};
use crate::providers::{AiProvider, DefaultProviderHandle, DeltaSink, ProviderDelta};
use crate::sandbox::exec_approval::gate::ApprovalRequester;
use crate::sync_primitives::Arc;

use super::decision::{cooldown_window, decide, strike_for, Decision, FailureKind};
use super::health::{CircuitState, FailoverHealth, ModelCooldown, ProviderCooldown};
use super::{
    FailoverConfig, FailoverNode, CIRCUIT_OPEN_THRESHOLD, MAX_COOLDOWN, MAX_OVERLOAD_RETRY_DELAY,
    MAX_RETRY_DELAY,
};

/// Cost-routing sort key for a model with no static rate card.
///
/// A missing rate card means the price is *unknown*, not necessarily zero. Only
/// on-machine / local-network endpoints are genuinely free, so those keep `0`
/// (sorted first under [`CostAware`](LoadBalanceStrategy::CostAware)). An
/// unpriced [`Cloud`](EndpointTier::Cloud) endpoint — a new or typo'd model
/// absent from the table — is unknown-cost: ranking it as "free" would let it
/// win over a model whose price we can confirm, so it sorts *last*
/// ([`u64::MAX`]). The [`Unknown`](EndpointTier::Unknown) tier (an unresolved
/// `base_url`) is treated conservatively as cloud for the same reason. Unpriced
/// candidates are never dropped — `MAX` only deprioritises them within the
/// fresh group, so the chain can still fall back to them.
const fn unpriced_cost(tier: EndpointTier) -> u64 {
    match tier {
        EndpointTier::Local => 0,
        EndpointTier::Cloud | EndpointTier::Unknown => u64::MAX,
    }
}

/// Every token this call put through the provider's rate window.
///
/// [`TokenUsage`](crate::providers::adapter::TokenUsage) counters are
/// **disjoint** by adapter post-condition — `input_tokens` excludes both cache
/// counters — so the prompt is only whole once all three are added. Summing
/// input+output alone understates a cache-heavy turn by the entire cached
/// prefix (a 48k-token cached prompt reads as ~120 tokens), which silently
/// disarms everything downstream of the rate window: `over_limit` never trips,
/// so the saturated-provider deprioritisation and `usage_based` ordering sit
/// idle while the account is being throttled for real, and `route_status`
/// reports the same understated figure back to whoever is diagnosing it.
///
/// Providers bill cached reads at a discount, but the rate *limit* counts them
/// at face value — this is a throughput ceiling, not a bill.
pub(super) fn billed_tokens(usage: &crate::providers::adapter::TokenUsage) -> u64 {
    u64::from(usage.input_tokens)
        + u64::from(usage.output_tokens)
        + u64::from(usage.cache_read_tokens.unwrap_or(0))
        + u64::from(usage.cache_creation_tokens.unwrap_or(0))
}

/// Which slot of the chain a candidate occupies.
///
/// Only the walk's *model-list* resolution needs this, and it needs it to be
/// exact: an explicitly pinned request model (a `select_model` pick, an agent
/// `model_hint`, a `BrainRef::Strict` model — whatever `ModelOverrideProvider`
/// stamped onto `payload.model`) belongs to the endpoint the caller actually
/// chose, i.e. the primary slot. Stamping it onto a cross-provider fallback
/// dials that fallback with a model id it does not serve.
///
/// This used to be inferred from `tier == EndpointTier::Unknown`, a proxy that
/// was wrong in **both** directions:
///
/// * a *pinned* chain tags its primary with the pin's real tier
///   (`with_primary_tier`, only ever `Local`/`Cloud`), so the pinned model was
///   silently discarded and the provider's first catalog model used instead;
/// * a *live-derived* fallback was tagged `Unknown`, so every fallback was
///   treated as the primary slot and dialed with the primary's model.
///
/// The slot is now carried explicitly, so tier means only "local or cloud".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotKind {
    /// The caller's chosen endpoint — owns any explicitly pinned request model.
    Primary,
    /// A cross-provider safety net — always walks its own model catalog.
    Fallback,
}

/// One request's resolved candidate chain plus the prompt-blind gate facts the
/// walk needs about it.
///
/// The set travels *with* the chain because both are derived from one route
/// snapshot: recomputing "is this provider over its ceiling" inside the walk
/// would read a possibly-newer config generation than the one that ordered the
/// chain, and the two disagreeing is how a candidate ends up sorted last for a
/// reason the walk then declines to act on.
struct CandidatePlan {
    /// The chain to walk, in order, each entry with the route action it must
    /// enforce and the slot that decides whose model list wins.
    candidates: Vec<(FailoverNode, CandidateAction, SlotKind)>,
    /// Providers at or over a configured `[route].rate_limits` ceiling right
    /// now. Empty whenever no ceilings are configured (the default), so the
    /// gate below is a no-op on an unconfigured deployment.
    saturated: std::collections::HashSet<String>,
    /// The single route-state generation this plan was ordered from. Carried
    /// so the walk's gates (the saturation gate's pin exemption) and the
    /// empty-chain error read the SAME snapshot that produced the candidate
    /// set — re-reading the live handle mid-walk could observe a config that
    /// hot-swapped in after the ordering pass and name a mode that had nothing
    /// to do with why the chain is empty.
    route: Arc<RouteState>,
}

/// One step of the chain the next request would walk, as rendered by
/// `route_status` ([`FailoverProvider::preview_order`]).
#[derive(Debug, Clone)]
pub struct RouteStep {
    /// Provider name — the breaker / cooldown / load key.
    pub provider: String,
    /// Endpoint locality the route policy gated on.
    pub tier: EndpointTier,
    /// What the walk will enforce before dialing it.
    pub action: CandidateAction,
    /// Whether this is the primary slot (the only slot that honours an
    /// explicitly pinned request model).
    pub primary: bool,
    /// Whether a configured rate ceiling currently sidelines it — deprioritised
    /// within its tier and skipped while a healthier candidate remains.
    pub sidelined: bool,
}

/// Why a chain came back empty, phrased for whoever has to read it.
///
/// An empty chain is always the route policy's doing — it is the only stage that
/// removes candidates outright — so "all 0 failover candidates failed" was both
/// true and useless: nothing was attempted, and the reason was a mode the
/// operator set.
fn empty_chain_error(mode: RouteMode) -> AlephError {
    AlephError::provider(match mode {
        RouteMode::AlwaysLocal => "route mode 'always_local' left no candidate: every configured \
             provider resolves to a cloud endpoint and cloud escalation is off. Set \
             [route] allow_cloud_escalation = true, configure a local provider, \
             or switch [route] mode."
            .to_string(),
        RouteMode::AlwaysCloud => "route mode 'always_cloud' left no candidate: no configured \
             provider resolves to a cloud endpoint."
            .to_string(),
        RouteMode::Auto => "no provider is configured to serve this request".to_string(),
    })
}

/// Fallback chain **membership** for the next request, in order.
///
/// The single description of "who is in the chain", shared by the walk
/// ([`FailoverProvider::candidates`]) and the `route_status` renderer
/// ([`crate::providers::route_observe`]) so the diagnostic can never disagree
/// with the chain it is describing. Three cases, matching how the chain was
/// assembled:
///
/// * the handle exposes no live registry (tests / non-registry boot) → the
///   boot-time configured order verbatim;
/// * `derive_live` (an *auto-derived* chain, i.e. no operator
///   `[fallback_provider].chain`) → every currently-registered provider, so one
///   added or removed at runtime joins/leaves without a restart;
/// * otherwise → the operator's configured order, minus entries that no longer
///   exist in the live registry.
///
/// The primary is excluded in every case (its own slot already covers it).
pub(crate) fn effective_fallback_names(
    live_names: &[String],
    primary_name: &str,
    configured_names: &[String],
    derive_live: bool,
) -> Vec<String> {
    let configured_minus_primary = || {
        configured_names
            .iter()
            .filter(|n| n.as_str() != primary_name)
            .cloned()
            .collect::<Vec<String>>()
    };
    if live_names.is_empty() {
        return configured_minus_primary();
    }
    if derive_live {
        return live_names
            .iter()
            .filter(|n| n.as_str() != primary_name)
            .cloned()
            .collect();
    }
    let live: std::collections::HashSet<&str> = live_names.iter().map(String::as_str).collect();
    configured_minus_primary()
        .into_iter()
        .filter(|n| live.contains(n.as_str()))
        .collect()
}

/// A [`DeltaSink`] pass-through that remembers whether any *content* reached
/// the caller.
///
/// The failover walk needs one bit the sink itself does not expose: has the user
/// already been shown part of an answer? Bookkeeping-only deltas
/// ([`Usage`](ProviderDelta::Usage), [`Done`](ProviderDelta::Done),
/// [`Error`](ProviderDelta::Error)) do not count — they carry nothing a second
/// candidate's answer could contradict.
struct EmissionGuard<'a> {
    inner: &'a dyn DeltaSink,
    emitted: std::sync::atomic::AtomicBool,
}

impl<'a> EmissionGuard<'a> {
    const fn new(inner: &'a dyn DeltaSink) -> Self {
        Self {
            inner,
            emitted: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn has_emitted(&self) -> bool {
        self.emitted.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl DeltaSink for EmissionGuard<'_> {
    async fn on_delta(&self, delta: &ProviderDelta) {
        if !matches!(
            delta,
            ProviderDelta::Usage(_) | ProviderDelta::Done(_) | ProviderDelta::Error(_)
        ) {
            self.emitted
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.inner.on_delta(delta).await;
    }
}

/// An `AiProvider` that fails over across an ordered provider/model chain.
pub struct FailoverProvider {
    /// Live primary slot. `current()` is read on every call so a UI
    /// `set_default` swap takes effect on the next turn (hot-reload).
    primary: Arc<dyn DefaultProviderHandle>,
    /// Static fallback chain, tried after the primary in order.
    fallbacks: Vec<FailoverNode>,
    /// Provider name → model list. Boot snapshot; lets the live primary
    /// resolve its model list by name.
    model_catalog: HashMap<String, Vec<String>>,
    /// Provider name → endpoint tier. Boot snapshot from the same
    /// `provider_tier(base_url)` derivation the static chain uses, so a
    /// *live-derived* fallback carries its real local/cloud tier instead of the
    /// `Unknown` placeholder that made `AlwaysLocal` a no-op and inverted
    /// `CostAware` for on-machine endpoints. A provider registered after boot is
    /// absent here and resolves to [`EndpointTier::Cloud`] — the conservative
    /// side of both decisions (gated under `AlwaysLocal`, never assumed free).
    tier_catalog: HashMap<String, EndpointTier>,
    /// Shared circuit-breaker state.
    health: FailoverHealth,
    config: FailoverConfig,
    /// Local/cloud route preference. `Auto` (default) is a no-op — candidates
    /// keep their configured order (byte-identical to pre-route failover).
    route_mode: RouteMode,
    /// In `AlwaysLocal`, whether a cloud candidate may be tried as an
    /// approval-gated terminal fallback ("borrow cloud").
    allow_cloud_escalation: bool,
    /// Gate consulted before dialing an approval-gated cross-tier candidate.
    /// `None` (the default) fails escalation closed.
    approval: Option<Arc<dyn ApprovalRequester>>,
    /// Live route preference. When `Some`, it overrides the boot-snapshot
    /// `route_mode` / `allow_cloud_escalation` fields on *every* request, so a
    /// mode switch hot-applies with no rebuild. `None` (tests, `new()`) keeps
    /// the snapshot fields — byte-identical to pre-handle behaviour.
    route_handle: Option<Arc<RouteHandle>>,
    /// Endpoint tier of the primary slot. `Unknown` (the default) is the
    /// operator's configured default — always allowed, so route mode only ever
    /// shapes the *fallbacks* around it. A *pinned* chain (an explicit
    /// `select_model` / agent `provider_hint` override) sets this to the pinned
    /// provider's real tier so a hard-guardrail route mode (`AlwaysLocal`) can
    /// gate or skip an explicit cross-tier pin via the borrow-cloud approval —
    /// the dynamic pick stops silently overriding the operator's policy.
    primary_tier: EndpointTier,
    /// Shared runtime load registry driving the load-balancing strategy. `None`
    /// (tests, `new()`) disables balancing entirely — the chain keeps configured
    /// order, byte-identical to pre-balance failover. Shared (`Arc`) across the
    /// global chain and every per-hint override, like [`FailoverHealth`], so one
    /// endpoint's in-flight/latency picture is visible to every chain.
    load: Option<Arc<LoadStats>>,
    /// Shared per-model rate-limit cooldown. `None` (tests, `new()`) disables
    /// cooldown entirely — the chain keeps every model, byte-identical to
    /// pre-cooldown failover. Shared across the global chain and every per-hint
    /// override (like [`FailoverHealth`]) so one model's throttle is visible
    /// everywhere.
    model_cooldown: Option<ModelCooldown>,
    /// Shared per-provider rate-limit cooldown gate. `None` (tests, `new()`)
    /// disables proactive pacing entirely — byte-identical to pre-gate failover.
    /// Wired only in production (`build_failover_chain`), one registry cloned
    /// across all chains like [`FailoverHealth`].
    provider_cooldown: Option<ProviderCooldown>,
    /// Derive the fallback set *live* from the primary handle's registry on
    /// every request, instead of the boot-time static `fallbacks` snapshot.
    ///
    /// `false` (the default, tests, and any chain with an explicit
    /// `[fallback_provider].chain`) keeps the static snapshot — byte-identical
    /// to pre-live failover. `true` is set by `build_failover_chain` only when
    /// the chain was *auto-derived* (no operator chain), so a provider
    /// added/removed at runtime is reflected in the next turn's fallback set
    /// without a restart. Falls back to the static snapshot if the handle
    /// exposes no live providers (e.g. a non-registry boot path).
    derive_fallbacks_live: bool,
}

impl FailoverProvider {
    /// Build a failover chain.
    ///
    /// * `primary` — the live primary slot; `current()` is read per call.
    /// * `fallbacks` — the static fallback chain.
    /// * `model_catalog` — provider name → model list; lets the live primary
    ///   resolve its model list by name.
    /// * `health` — shared circuit-breaker state (clone it to share across
    ///   per-agent chains).
    pub fn new(
        primary: Arc<dyn DefaultProviderHandle>,
        fallbacks: Vec<FailoverNode>,
        model_catalog: HashMap<String, Vec<String>>,
        health: FailoverHealth,
        config: FailoverConfig,
    ) -> Self {
        Self {
            primary,
            fallbacks,
            model_catalog,
            tier_catalog: HashMap::new(),
            health,
            config,
            route_mode: RouteMode::Auto,
            allow_cloud_escalation: false,
            approval: None,
            route_handle: None,
            primary_tier: EndpointTier::Unknown,
            load: None,
            model_cooldown: None,
            provider_cooldown: None,
            derive_fallbacks_live: false,
        }
    }

    /// Derive the fallback set live from the primary handle's registry on every
    /// request, instead of the boot-time static snapshot. Set by
    /// `build_failover_chain` only for an *auto-derived* global chain (no
    /// operator `[fallback_provider].chain`), so runtime provider add/remove is
    /// reflected without a restart. See [`Self::derive_fallbacks_live`].
    #[must_use]
    pub const fn with_live_fallback_derivation(mut self) -> Self {
        self.derive_fallbacks_live = true;
        self
    }

    /// Attach the boot provider → endpoint-tier map.
    ///
    /// Required for [`with_live_fallback_derivation`](Self::with_live_fallback_derivation)
    /// to be *correct*: without it every live-derived node falls back to
    /// [`EndpointTier::Cloud`], and the route policy can only gate on the
    /// conservative side. `build_failover_chain` supplies the same map the
    /// static chain derives its node tiers from, so both paths gate identically.
    #[must_use]
    pub fn with_tier_catalog(mut self, tiers: HashMap<String, EndpointTier>) -> Self {
        self.tier_catalog = tiers;
        self
    }

    /// Attach a local/cloud route preference and the escalation approval gate.
    ///
    /// `new()` alone stays `Auto` + no-gate (today's behaviour). In `Auto` the
    /// `approval` gate is never consulted. In `AlwaysLocal` with
    /// `allow_cloud_escalation`, the gate authorises borrowing a cloud
    /// endpoint as a terminal fallback; absent a gate, escalation fails closed.
    #[must_use]
    pub fn with_route(
        mut self,
        mode: RouteMode,
        allow_cloud_escalation: bool,
        approval: Option<Arc<dyn ApprovalRequester>>,
    ) -> Self {
        self.route_mode = mode;
        self.allow_cloud_escalation = allow_cloud_escalation;
        self.approval = approval;
        self
    }

    /// Attach a live [`RouteHandle`] so the route preference is read fresh on
    /// every request instead of frozen at boot. The handle overrides the
    /// snapshot set by [`with_route`](Self::with_route); the approval gate is
    /// still supplied via `with_route`. Wired only in production
    /// (`build_failover_chain`); tests omit it and keep the boot snapshot.
    pub fn with_route_live(mut self, handle: Arc<RouteHandle>) -> Self {
        self.route_handle = Some(handle);
        self
    }

    /// Tag the primary slot with a concrete endpoint tier so a hard-guardrail
    /// route mode can gate it. Used by `build_failover_chain` for the per-pin
    /// override chains (an explicit `select_model` / agent `provider_hint`
    /// target): the pinned provider's real tier makes `AlwaysLocal` route a
    /// cloud pin through the borrow-cloud approval instead of silently allowing
    /// it. The global default chain omits this — its primary stays `Unknown`
    /// (the operator's configured default is always allowed).
    #[must_use]
    pub const fn with_primary_tier(mut self, tier: EndpointTier) -> Self {
        self.primary_tier = tier;
        self
    }

    /// Attach the shared runtime load registry that drives the load-balancing
    /// strategy. Wired only in production (`build_failover_chain`) with one
    /// registry cloned across all chains; tests omit it and keep the configured
    /// order (byte-identical to pre-balance failover).
    pub fn with_load_stats(mut self, load: Arc<LoadStats>) -> Self {
        self.load = Some(load);
        self
    }

    /// Attach the shared per-model rate-limit cooldown registry. Wired only in
    /// production (`build_failover_chain`) with one registry cloned across all
    /// chains; tests omit it and keep no cooldown (byte-identical to before).
    #[must_use]
    pub fn with_model_cooldown(mut self, cooldown: ModelCooldown) -> Self {
        self.model_cooldown = Some(cooldown);
        self
    }

    /// Attach the shared per-provider rate-limit cooldown gate. Wired only in
    /// production (`build_failover_chain`) with one registry cloned across all
    /// chains; tests omit it and keep no pacing (byte-identical to before).
    #[must_use]
    pub fn with_provider_cooldown(mut self, cooldown: ProviderCooldown) -> Self {
        self.provider_cooldown = Some(cooldown);
        self
    }

    /// Drop models currently in rate-limit cooldown for `provider`, so the walk
    /// prefers a healthy sibling. Fail-open: if *every* model is cooling the
    /// original list is kept (better to re-probe a throttled model than to empty
    /// the candidate). `None` registry (tests) is a no-op.
    async fn drop_cooling_models(
        &self,
        provider: &str,
        models: Vec<Option<String>>,
    ) -> Vec<Option<String>> {
        let Some(cd) = &self.model_cooldown else {
            return models;
        };
        let mut kept = Vec::with_capacity(models.len());
        for m in &models {
            let cooling = match m {
                // An unnamed default model can't be sidelined by name.
                Some(name) => cd.is_cooling(provider, name).await,
                None => false,
            };
            if !cooling {
                // rust-doctor-disable-next-line excessive-clone
                kept.push(m.clone());
            }
        }
        if kept.is_empty() {
            models
        } else {
            kept
        }
    }

    /// One coherent snapshot of the live route state for this candidate-ordering
    /// pass: the live handle if attached, else the boot config frozen into a
    /// [`RouteState`]. Reading mode/targets/strategy/limits from a *single*
    /// snapshot means a config hot-swap landing mid-pass is seen whole or not at
    /// all — never a torn mix (new mode with stale targets). Pins/limits only
    /// ever enter via the boot-wired handle, so `new()`/tests see an empty set —
    /// byte-identical to unpinned/pre-usage ordering.
    fn route_snapshot(&self) -> Arc<RouteState> {
        match &self.route_handle {
            Some(h) => h.snapshot(),
            None => Arc::new(RouteState {
                mode: self.route_mode,
                allow_escalation: self.allow_cloud_escalation,
                load_balance: LoadBalanceStrategy::default(),
                targets: Arc::new(RouteTargets::default()),
                limits: Arc::new(RateLimits::default()),
                health_probe_interval_secs: 0,
            }),
        }
    }

    /// Blended `input + output` price for `provider`'s first model, in milli-USD
    /// per million tokens (USD/Mtok × 1000), looked up from the static
    /// [`crate::pricing`] table. The sort key fed to the
    /// [`CostAware`](LoadBalanceStrategy::CostAware) strategy — the bridge that
    /// finally wires Aleph's shipped price card into route ordering (it had only
    /// ever fed post-hoc cost *estimation* before).
    ///
    /// When the provider's first model carries no rate card the price is
    /// *unknown*, and the candidate's [`EndpointTier`] decides how to rank it
    /// (see [`unpriced_cost`]): a [`Local`](EndpointTier::Local) endpoint is
    /// genuinely free (`0`, sorts first), but an unpriced [`Cloud`] or
    /// [`Unknown`](EndpointTier::Unknown) endpoint is unknown-cost — cost routing
    /// must not assume it is free, so it sorts *last* rather than ahead of a
    /// model whose price we can actually confirm. Stays prompt-blind (R7) —
    /// price and tier are static `(provider, model)` / `base_url` facts, never
    /// the message.
    ///
    /// [`Cloud`]: EndpointTier::Cloud
    fn price_hint(&self, provider: &str, tier: EndpointTier) -> u64 {
        let Some(model) = self
            .model_catalog
            .get(provider)
            .and_then(|models| models.first())
        else {
            return unpriced_cost(tier);
        };
        match crate::pricing::rate_card(provider, model) {
            Some(card) => {
                let usd = card.input_per_mtok.unwrap_or(0.0) + card.output_per_mtok.unwrap_or(0.0);
                (usd * 1000.0).round() as u64
            }
            None => unpriced_cost(tier),
        }
    }

    /// Whether a cloud-borrow escalation for `name` is authorised right now.
    ///
    /// Fails closed: no gate wired → denied (a warn is logged). Mirrors the
    /// sandbox escalation contract — the money-spending action is gated at the
    /// moment it would happen, not at config-write time.
    async fn escalation_allowed(&self, name: &str) -> bool {
        // rust-doctor-disable-next-line excessive-clone
        match self.approval.clone() {
            Some(gate) => {
                let reason = format!(
                    "Route mode is AlwaysLocal; borrow cloud provider '{name}' \
                     for this request?"
                );
                let action = crate::sandbox::exec_approval::ApprovalAction::bare(
                    "__route_escalate_cloud",
                    reason,
                );
                gate.request_approval(&action).await.outcome.is_approved()
            }
            None => {
                tracing::warn!(
                    provider = %name,
                    "route: cloud escalation requested but no approval gate wired; denying"
                );
                false
            }
        }
    }

    /// Build the ordered candidate list for one request: the primary slot
    /// first, then each fallback whose name differs from the primary's, shaped
    /// by the route policy (tier ordering + cross-tier gating).
    ///
    /// The primary slot keeps its position 0 but is **classified in place** by
    /// the route policy:
    ///
    /// * the global default chain tags it [`EndpointTier::Unknown`] — its
    ///   `base_url` is not resolvable from the live `DefaultProviderHandle` and
    ///   it is the operator's configured default, so it always classifies to
    ///   [`Allow`](CandidateAction::Allow) (byte-identical to before);
    /// * a *pinned* override chain tags it with the pin's real tier (via
    ///   [`with_primary_tier`](Self::with_primary_tier)), so a hard-guardrail
    ///   `AlwaysLocal` can turn an explicit cloud pin into a
    ///   [`CrossTier`](CandidateAction::CrossTier) (borrow-cloud approval) or a
    ///   [`Skip`](CandidateAction::Skip) (escalation off) — the dynamic pick no
    ///   longer bypasses the operator's policy.
    ///
    /// The primary is never reordered below its own fallbacks (it is the
    /// operator default or the explicitly-chosen provider); only the *fallback*
    /// list is run through [`order_candidates`] for local-first ordering, pin
    /// promotion and tier gating. Each entry carries the [`CandidateAction`] the
    /// walk must enforce plus the [`SlotKind`] that decides whose model list wins.
    ///
    /// `advance_rotation` consumes a round-robin tick (what a real request
    /// does); the read-only [`preview_order`](Self::preview_order) passes
    /// `false` so looking at the order does not rotate it.
    async fn candidates(&self, advance_rotation: bool) -> CandidatePlan {
        let primary = self.primary.current();
        let primary_name = primary.name().to_string();
        let primary_models = self
            .model_catalog
            .get(&primary_name)
            .cloned()
            .unwrap_or_default();
        let primary_node = FailoverNode {
            // rust-doctor-disable-next-line excessive-clone
            name: primary_name.clone(),
            models: primary_models,
            provider: primary,
            tier: self.primary_tier,
        };
        // Chain membership comes from the one shared description
        // ([`effective_fallback_names`]) so `route_status` cannot describe a
        // different chain than the one that is walked. Each name is then
        // materialised into a node: a boot node when the operator configured
        // the chain (it already carries the built provider, its model list and
        // its tier), otherwise a live registry lookup whose model list and
        // endpoint tier come from the same boot catalogs the static path uses —
        // a live-derived candidate is a real chain member, not a placeholder.
        let live_names = self.primary.provider_names();
        let configured: Vec<String> = self.fallbacks.iter().map(|n| n.name.clone()).collect();
        let member_names = effective_fallback_names(
            &live_names,
            &primary_name,
            &configured,
            self.derive_fallbacks_live,
        );
        let fallbacks: Vec<FailoverNode> = member_names
            .into_iter()
            .filter_map(|name| {
                // An auto-derived chain dials the *live* provider instance, so
                // one rebuilt at runtime (rotated key, edited base_url) is used
                // without a restart; its model list and tier still come from the
                // boot catalogs. An operator-configured chain uses its boot node
                // verbatim (that node already carries all three).
                if self.derive_fallbacks_live {
                    if let Some(provider) = self.primary.provider_by_name(&name) {
                        return Some(FailoverNode {
                            models: self.model_catalog.get(&name).cloned().unwrap_or_default(),
                            tier: self.node_tier(&name),
                            name,
                            provider,
                        });
                    }
                }
                // rust-doctor-disable-next-line excessive-clone
                self.fallbacks.iter().find(|fb| fb.name == name).cloned()
            })
            .collect();
        // One coherent route snapshot for the whole ordering pass — mode,
        // targets, strategy and limits all read from a single config generation.
        let route = self.route_snapshot();
        let (mode, allow_escalation) = (route.mode, route.allow_escalation);
        let targets = Arc::clone(&route.targets);

        // Classify the primary in place. A `Skip` (a hard-guardrail mode with
        // escalation off, on a cross-tier pin) drops it so the chain falls
        // straight through to the fallbacks; `Allow`/`CrossTier` keep it first.
        let mut out: Vec<(FailoverNode, CandidateAction, SlotKind)> =
            Vec::with_capacity(fallbacks.len() + 1);
        match classify_candidate(mode, primary_node.tier, allow_escalation) {
            CandidateAction::Skip => {}
            action => out.push((primary_node, action, SlotKind::Primary)),
        }
        // Order the fallback pool. The balanced path runs when there is a load
        // registry AND either a non-`Ordered` strategy (sort by live signals) or
        // configured rate limits (the over-limit gate must deprioritise
        // saturated providers even under `Ordered`); otherwise the
        // configured-order path stays byte-identical to before.
        let strategy = route.load_balance;
        let limits = Arc::clone(&route.limits);
        // Providers the shared breaker/pacing registries currently consider
        // unhealthy. Gathered once per pass (the registries are async, the
        // ordering closure is not) and folded into `LoadMetric.cooling`, so an
        // outage shapes the *order of the next request* instead of only being
        // discovered mid-walk. This is the feedback edge LiteLLM closes with
        // `_filter_cooldown_deployments` ahead of every strategy; Aleph
        // deprioritises rather than removes, so a chain of cooling providers
        // still resolves instead of raising "no deployments available".
        let sidelined = self
            .sidelined_providers(fallbacks.iter().map(|n| n.name.as_str()))
            .await;
        // Rate-window saturation, folded ONCE per pass for every candidate the
        // primary included. Two consumers read this single answer: the ordering
        // below (deprioritise to the back of the tier) and the walk's gate
        // (skip while a healthier candidate is still ahead) — so a provider
        // cannot be sorted last for a ceiling the walk then ignores. Empty
        // without `[route].rate_limits`, which is the default.
        let saturated: std::collections::HashSet<String> = match &self.load {
            Some(load) if !limits.is_empty() => std::iter::once(primary_name.as_str())
                .chain(fallbacks.iter().map(|n| n.name.as_str()))
                .filter(|name| {
                    let m = load.metric(name);
                    limits.assess(name, m.rpm_used, m.tpm_used).1
                })
                .map(ToString::to_string)
                .collect(),
            _ => std::collections::HashSet::new(),
        };
        let needs_balance =
            strategy != LoadBalanceStrategy::Ordered || !limits.is_empty() || !sidelined.is_empty();
        let ordered = match &self.load {
            Some(load) if needs_balance => {
                // One rotation tick per request drives RoundRobin; the sort
                // strategies ignore it. A preview must not consume one.
                let rr_base = if advance_rotation {
                    load.next_round_robin()
                } else {
                    load.peek_round_robin()
                };
                // Provider → endpoint tier, captured before `fallbacks` is moved
                // into the ordering call. Cost routing needs each candidate's
                // tier to rank an *unpriced* model (free local vs unknown-cost
                // cloud — see `unpriced_cost`); the metric closure only receives
                // the provider name, so the tier rides in via this map.
                let tier_by_name: HashMap<String, EndpointTier> =
                    // rust-doctor-disable-next-line excessive-clone
                    fallbacks.iter().map(|n| (n.name.clone(), n.tier)).collect();
                order_candidates_balanced(
                    fallbacks,
                    mode,
                    allow_escalation,
                    &targets,
                    |n| n.tier,
                    |n| n.name.as_str(),
                    strategy,
                    rr_base,
                    // Fold each provider's live window counts against its
                    // configured ceiling into the derived utilisation/over-limit
                    // scalars, and (only under cost routing) its static price.
                    // The ordering logic stays limit- and price-blind (R7: pure
                    // infrastructure) — it just sorts the scalars handed to it.
                    |name| {
                        let mut m = load.metric(name);
                        m.utilization_permille = limits.assess(name, m.rpm_used, m.tpm_used).0;
                        m.over_limit = saturated.contains(name);
                        m.cooling = sidelined.contains(name);
                        // Price lookup only when it is the active sort key —
                        // every other strategy ignores `price_per_mtok`. The
                        // tier disambiguates an unpriced model (free local vs
                        // unknown-cost cloud); an unmapped name falls back to
                        // `Unknown` (treated as cloud — never assumed free).
                        if strategy == LoadBalanceStrategy::CostAware {
                            let tier = tier_by_name
                                .get(name)
                                .copied()
                                .unwrap_or(EndpointTier::Unknown);
                            m.price_per_mtok = self.price_hint(name, tier);
                        }
                        m
                    },
                )
            }
            _ => order_candidates(
                fallbacks,
                mode,
                allow_escalation,
                &targets,
                |n| n.tier,
                |n| n.name.as_str(),
            ),
        };
        out.extend(
            ordered
                .into_iter()
                .map(|(node, action)| (node, action, SlotKind::Fallback)),
        );
        CandidatePlan {
            candidates: out,
            saturated,
            route,
        }
    }

    /// The chain the *next* request would walk: `(provider, tier, action, slot)`
    /// per candidate, in dial order.
    ///
    /// Read-only twin of [`candidates`](Self::candidates) — same function, same
    /// route snapshot, same gates — so `route_status` cannot report an order the
    /// walk would not produce. Answering "why did it pick that provider / why is
    /// my pin not leading / did the strategy take effect" needs the *result* of
    /// the ordering, not the raw signals that feed it; before this the operator
    /// had to re-run the sort in their head.
    ///
    /// Observes without disturbing: it consumes no round-robin tick and performs
    /// no `Open → HalfOpen` breaker transition (that belongs to a real dial).
    pub async fn preview_order(&self) -> Vec<RouteStep> {
        let plan = self.candidates(false).await;
        plan.candidates
            .into_iter()
            .map(|(node, action, slot)| RouteStep {
                sidelined: plan.saturated.contains(&node.name),
                provider: node.name,
                tier: node.tier,
                action,
                primary: slot == SlotKind::Primary,
            })
            .collect()
    }

    /// The subset of `names` the shared registries currently consider unhealthy:
    /// circuit breaker open (cooldown not yet elapsed), or parked inside a
    /// rate-limit pacing window.
    ///
    /// Strictly read-only — it must **not** perform the `Open → HalfOpen`
    /// transition, which belongs to [`circuit_allows`](Self::circuit_allows) at
    /// dial time. Ordering asks "is this a good bet right now"; only an actual
    /// dial may spend the probe.
    async fn sidelined_providers<'n>(
        &self,
        names: impl Iterator<Item = &'n str>,
    ) -> std::collections::HashSet<String> {
        // The nested-chain sentinel has no health of its own — the chain it
        // delegates to breaks per real provider.
        let names: Vec<&str> = names.filter(|n| *n != super::NESTED_CHAIN_NODE).collect();
        let mut out = std::collections::HashSet::new();
        {
            let map = self.health.0.read().await;
            for name in &names {
                let open = map.get(*name).is_some_and(|st| {
                    // Open *and still cooling down*: no recorded failure, or the
                    // cooldown has not elapsed yet.
                    st.circuit == CircuitState::Open
                        && st.last_failure.is_none_or(|at| at.elapsed() < st.cooldown)
                });
                if open {
                    out.insert((*name).to_string());
                }
            }
        }
        if let Some(pc) = &self.provider_cooldown {
            for name in &names {
                if pc.remaining(name).await.is_some() {
                    out.insert((*name).to_string());
                }
            }
        }
        out
    }

    /// This candidate's endpoint tier from the boot catalog, defaulting to
    /// [`EndpointTier::Cloud`] for a provider registered after boot — the
    /// conservative side of both decisions the tier drives (gated under
    /// `AlwaysLocal`, never assumed free by `CostAware`).
    fn node_tier(&self, name: &str) -> EndpointTier {
        self.tier_catalog
            .get(name)
            .copied()
            .unwrap_or(EndpointTier::Cloud)
    }

    /// Whether `name` may be tried now. Transitions `Open → HalfOpen` once the
    /// cooldown has elapsed. `HalfOpen` then admits probe traffic — concurrent
    /// requests are *not* serialized to a single probe; the probe outcomes drive
    /// the circuit via [`Self::mark_healthy`] / [`Self::mark_unhealthy`].
    async fn circuit_allows(&self, name: &str) -> bool {
        let mut map = self.health.0.write().await;
        let st = map.entry(name.to_string()).or_default();
        match st.circuit {
            CircuitState::Closed | CircuitState::HalfOpen => true,
            CircuitState::Open => match st.last_failure {
                Some(at) if at.elapsed() >= st.cooldown => {
                    st.circuit = CircuitState::HalfOpen;
                    true
                }
                _ => false,
            },
        }
    }

    /// Record a successful call — close the circuit and reset the cooldown.
    async fn mark_healthy(&self, name: &str) {
        let mut map = self.health.0.write().await;
        let st = map.entry(name.to_string()).or_default();
        st.circuit = CircuitState::Closed;
        st.failure_count = 0;
        st.last_error = None;
        st.cooldown = self.config.unhealthy_cooldown;
    }

    /// Record a provider-level failure and advance the circuit breaker.
    ///
    /// `kind` shapes how fast the circuit trips: a [`FailureKind::Permanent`]
    /// failure (revoked/misconfigured credential) opens the circuit on the
    /// first strike with a long cooldown so the hot path stops re-dialing a
    /// known-dead provider; a [`FailureKind::Transient`] failure keeps the
    /// 3-strike threshold so a brief blip does not evict a healthy provider.
    async fn mark_unhealthy(&self, name: &str, error: String, kind: FailureKind) {
        let mut map = self.health.0.write().await;
        let st = map.entry(name.to_string()).or_default();
        st.last_failure = Some(Instant::now());
        st.failure_count += 1;
        st.last_error = Some(error);
        match st.circuit {
            // A probe failed → re-open with a doubled cooldown.
            CircuitState::HalfOpen => {
                st.cooldown = (st.cooldown * 2).min(MAX_COOLDOWN);
                st.circuit = CircuitState::Open;
            }
            CircuitState::Closed => {
                let should_open = match kind {
                    FailureKind::Permanent => true,
                    FailureKind::Transient => st.failure_count >= CIRCUIT_OPEN_THRESHOLD,
                };
                if should_open {
                    st.circuit = CircuitState::Open;
                    // A dead credential recovers on the scale of minutes-to-hours
                    // (operator rotates the key), not seconds — probe sparingly
                    // by starting at the cooldown ceiling instead of the base.
                    if matches!(kind, FailureKind::Permanent) {
                        st.cooldown = MAX_COOLDOWN;
                    }
                }
            }
            CircuitState::Open => {}
        }
    }

    /// Bookkeeping for an attempt that failed *after* content already reached
    /// the user — the walk's two already-emitted exits:
    /// * an `Err` raised once [`EmissionGuard::has_emitted`] is true, and
    /// * an `Ok` carrying
    ///   [`ProviderResponse::provider_error`](crate::providers::adapter::ProviderResponse::provider_error)
    ///   (the provider reported an in-band fault mid-stream).
    ///
    /// Both are terminal for *routing* — a second candidate would append its
    /// answer to a half-written one — but neither is a success, so both spend
    /// the same [`strike_for`] verdict here rather than each inventing one.
    /// The strike is deliberately the *whole* effect: the request outcome
    /// (partial `Ok`, or the error) is unchanged.
    async fn record_emitted_failure(&self, provider: &str, model: Option<&str>, err: &AlephError) {
        let strike = strike_for(err);
        if let Some(dur) = strike.cooldown {
            if let (Some(cd), Some(m)) = (&self.model_cooldown, model) {
                cd.cool(provider, m, dur).await;
            }
            if let Some(pc) = &self.provider_cooldown {
                pc.cool(provider, dur).await;
            }
        }
        self.mark_unhealthy(provider, err.to_string(), strike.kind)
            .await;
    }

    /// Whether `name`'s circuit is currently open.
    ///
    /// Test-only: the operator/model-facing status surface is
    /// [`FailoverHealth::snapshot`] (rendered by `route_observe` for the
    /// `route_status` tool action), and the walk itself uses
    /// [`circuit_allows`](Self::circuit_allows). Gated so it cannot quietly
    /// become a second, half-featured status API.
    #[cfg(test)]
    pub async fn circuit_open(&self, name: &str) -> bool {
        self.health
            .0
            .read()
            .await
            .get(name)
            .is_some_and(|h| h.circuit == CircuitState::Open)
    }

    /// The failover walk, shared by [`AiProvider::process`] (`sink: None`) and
    /// [`AiProvider::execute_streaming_dyn`] (`sink: Some(..)`).
    ///
    /// One body, one set of retry / breaker / cooldown / route-policy rules, so
    /// the streaming path can never drift from the non-streaming one. The only
    /// difference is how a single attempt is issued — and one extra safety rule
    /// that only streaming needs: once a candidate has pushed content to the
    /// sink, the user has *seen* that text, so a later failure can no longer be
    /// papered over by advancing the chain (the next candidate's answer would be
    /// appended to a half-written one). Emission therefore makes the current
    /// error terminal. Nothing is emitted before the model starts answering, so
    /// the ordinary failure modes — connect errors, 401/403, 429, model-not-found
    /// — all still fail over exactly as they do today.
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    fn walk<'a>(
        &'a self,
        payload: RequestPayload<'a>,
        sink: Option<&'a dyn DeltaSink>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        // The big fields (conversation, system prompt, tool defs) are `&'a`
        // borrows of the caller's data — copy the references so a failover
        // request never deep-clones the whole conversation. Only the small
        // owned fields (tool_choice / model / metadata) are taken by value so
        // each per-attempt rebuild can restamp `model`; they are cloned per
        // attempt below (all tiny — an enum tag, a model name, a header map).
        let messages = payload.messages;
        let system_prompt = payload.system_prompt;
        // Preserve the prompt-cache split (`cache: true` prefix) across the
        // failover rebuild — dropping it here silently negated caching for any
        // caller behind a Failover wrapper (the Guardian judge, the main loop).
        let system_blocks = payload.system_blocks;
        let tools = payload.tools;
        let think_level = payload.think_level;
        let temperature = payload.temperature;
        let max_tokens = payload.max_tokens;
        let tool_choice = payload.tool_choice;
        let req_model = payload.model;
        let metadata = payload.metadata;
        // C floor: derive the request's structural capability requirements once
        // (image blocks → vision, tools array → tool-calling, text size →
        // context window). Prompt-blind; shapes the candidate model set below.
        let reqs =
            RequestRequirements::from_request(messages, tools.is_some_and(|t| !t.is_empty()));

        Box::pin(async move {
            let plan = self.candidates(true).await;
            let total = plan.candidates.len();
            let mut last_error: Option<AlephError> = None;
            // The first endpoint this walk actually dials, for the route
            // witness below. Attempted rather than succeeded: a primary that is
            // down for the whole run has every *success* already on the
            // fallback, and anchoring there would make the commonest migration
            // of all read as "nothing deviated".
            let mut first_attempt: Option<crate::providers::route_witness::Dialed> = None;
            // Records whether any candidate has already pushed content to the
            // caller's sink; see the note on `walk`.
            let emission = sink.map(EmissionGuard::new);

            for (idx, (cand, action, slot)) in plan.candidates.into_iter().enumerate() {
                // The circuit breaker may skip a candidate only while a later
                // one remains; the final candidate is always attempted so a
                // transient outage cannot hard-fail every request behind an
                // open circuit. `circuit_allows` still runs for its
                // `Open → HalfOpen` bookkeeping.
                //
                // This runs BEFORE the escalation gate below: the gate can
                // block on a user prompt, and asking someone to authorise
                // spending on a cloud provider we are then going to skip for
                // being dead is a prompt that buys nothing. Every cheap,
                // local reason to pass over a candidate is settled first.
                let circuit_ok = self.circuit_allows(&cand.name).await;
                if !circuit_ok && idx + 1 < total {
                    tracing::debug!(provider = %cand.name, "failover: circuit open, skipping");
                    continue;
                }

                // Rate-ceiling gate, same shape as the breaker's: a provider at
                // or over its configured `[route].rate_limits` window yields to
                // a later candidate, and is still attempted when it is the last
                // one (the chain must never starve).
                //
                // A pinned provider is EXEMPT, judged by the same
                // `targets.is_pinned` the ordering used: an operator pin is an
                // explicit hard signal, and `order_candidates_balanced` already
                // promises a pin leads its tier even when rate-saturated ("pin
                // beats the over-limit gate"). Skipping it here anyway made the
                // two halves of the same rule contradict each other — the pin
                // sorted first and was then passed over. The exemption covers
                // capacity yield only: the circuit-breaker and pacing gates
                // still skip a pinned candidate, because a pin does not exempt
                // failure.
                //
                // Without this the ceiling only ever *re-ordered* the fallback
                // pool, and the primary slot — which is not part of that pool —
                // ignored it completely: on a single-provider or primary-heavy
                // deployment `[route].rate_limits` changed nothing at all
                // except a number in `route_status`.
                if plan.saturated.contains(&cand.name)
                    && !plan.route.targets.is_pinned(&cand.name)
                    && idx + 1 < total
                {
                    tracing::debug!(
                        provider = %cand.name,
                        "failover: provider at its configured rate ceiling, deferring \
                         to a later candidate",
                    );
                    continue;
                }

                // Route gate: an approval-gated cross-tier candidate (borrow
                // cloud under AlwaysLocal) is skipped unless the user approves
                // — fail-closed, exactly like an open circuit. Cloud→local
                // degrade is `CrossTier{requires_approval:false}` and is never
                // gated (degrading to local spends nothing).
                if let CandidateAction::CrossTier {
                    requires_approval: true,
                } = action
                {
                    if !self.escalation_allowed(&cand.name).await {
                        tracing::warn!(
                            provider = %cand.name,
                            "route: cloud escalation denied, skipping candidate"
                        );
                        last_error.get_or_insert_with(|| {
                            AlephError::provider(format!(
                                "route: cloud escalation to '{}' not approved",
                                cand.name
                            ))
                        });
                        continue;
                    }
                }

                // Model-list resolution, in precedence:
                //
                // 1. An *explicitly pinned* request model on the PRIMARY slot.
                //    This is the dynamic-routing model directive — a
                //    `select_model` pick, an agent `model_hint`, or a
                //    `BrainRef::Strict` model — that `ModelOverrideProvider`
                //    stamped onto `payload.model`. It targets the endpoint the
                //    caller chose, so it overrides that slot's static catalog
                //    walk (otherwise the catalog silently discarded the model
                //    the LLM/agent explicitly chose). Still passed through the C
                //    floor (fail-open) for consistency. The slot is carried
                //    explicitly ([`SlotKind`]) rather than inferred from
                //    `tier == Unknown`, which mis-fired on a *pinned* chain
                //    (real tier ⇒ the pin was discarded) and on every
                //    *live-derived* fallback (placeholder tier ⇒ the fallback
                //    was dialed with the primary's model id).
                // 2. Empty catalog → a single attempt: the caller's model on the
                //    primary slot, the provider's own default on a fallback (a
                //    cross-provider safety net cannot serve the primary's model
                //    id, so forwarding it there is a guaranteed 404).
                // 3. Otherwise the C floor drops models that structurally cannot
                //    serve this request (no vision / no tools / over context
                //    window), failing open so the chain is never emptied.
                let models: Vec<Option<String>> = match (slot, &req_model) {
                    (SlotKind::Primary, Some(pinned)) => {
                        retain_capable_models(vec![pinned.clone()], &reqs)
                            .into_iter()
                            .map(Some)
                            .collect()
                    }
                    _ if cand.models.is_empty() => vec![None],
                    // rust-doctor-disable-next-line excessive-clone
                    _ => retain_capable_models(cand.models.clone(), &reqs)
                        .into_iter()
                        .map(Some)
                        .collect(),
                };
                // Sideline models still cooling from an earlier 429, preferring a
                // healthy sibling (fail-open if all are cooling).
                let models = self.drop_cooling_models(&cand.name, models).await;

                // Proactive rate-limit pacing: this provider 429'd recently and
                // is still inside its recorded cooldown.
                //
                // Waiting it out keeps a single paid primary (e.g. Kimi) in use
                // instead of eating a fresh 429 — but only when there is nothing
                // else to try. With a healthy candidate still ahead in the
                // chain, blocking the turn for up to two minutes to insist on
                // the parked provider is strictly worse than answering now: the
                // window expires on its own and the provider returns to the head
                // of the chain next turn. So the rule mirrors the circuit
                // breaker's exactly — skip while a later candidate remains, and
                // only wait when this is the last resort. (LiteLLM and Bifrost
                // both drop a cooling deployment from selection outright; Aleph
                // keeps it as the terminal candidate so a single-provider setup
                // still gets its request served.)
                if let Some(pc) = &self.provider_cooldown {
                    if let Some(remaining) = pc.remaining(&cand.name).await {
                        if idx + 1 < total {
                            tracing::debug!(
                                provider = %cand.name,
                                remaining_ms = remaining.as_millis() as u64,
                                "failover: provider cooling from a recent 429, \
                                 deferring to a later candidate",
                            );
                            continue;
                        }
                        let wait = remaining.min(MAX_OVERLOAD_RETRY_DELAY);
                        tracing::warn!(
                            provider = %cand.name,
                            wait_ms = wait.as_millis() as u64,
                            "failover: last candidate is cooling from a recent 429, \
                             pacing before re-request",
                        );
                        tokio::time::sleep(wait).await;
                    }
                }

                let mut tripped: Option<FailureKind> = None;
                'model: for model in models {
                    let mut attempt: u32 = 0;
                    loop {
                        let inner = RequestPayload {
                            messages,
                            system_prompt,
                            system_blocks,
                            tools,
                            think_level,
                            temperature,
                            max_tokens,
                            // rust-doctor-disable-next-line excessive-clone
                            tool_choice: tool_choice.clone(),
                            // rust-doctor-disable-next-line excessive-clone
                            model: model.clone(),
                            // rust-doctor-disable-next-line excessive-clone
                            metadata: metadata.clone(),
                        };
                        // Count this attempt as in-flight for the duration of
                        // the await (RAII: decremented on Ok, Err, retry, and
                        // panic alike). `None` load → no-op, zero overhead.
                        // The nested-chain sentinel is excluded: it is not an
                        // endpoint, and the inner chain records the real
                        // provider a moment later — counting both would publish
                        // a phantom provider row whose latency is the sum of two
                        // nested dials (see `NESTED_CHAIN_NODE`).
                        //
                        // Named (not `_`-prefixed) so the RetrySame arm can drop
                        // it BEFORE the backoff sleep: the attempt is over once
                        // the error is classified, and holding the count through
                        // the sleep made a provider that is merely *waiting to
                        // retry* look in-flight for the whole backoff — LeastBusy
                        // and the rate windows read that as real load.
                        let load_guard = self
                            .load
                            .as_ref()
                            .filter(|_| cand.name != super::NESTED_CHAIN_NODE)
                            .map(|l| l.begin(&cand.name));
                        let started = Instant::now();
                        // Same sentinel exclusion the load guard makes: it is
                        // not an endpoint, so it must not become the `original`
                        // half of a fallback notice either.
                        if first_attempt.is_none() && cand.name != super::NESTED_CHAIN_NODE {
                            first_attempt = Some(crate::providers::route_witness::Dialed::new(
                                &cand.name,
                                // rust-doctor-disable-next-line excessive-clone
                                model.clone(),
                            ));
                        }
                        let attempt_result = match &emission {
                            Some(guard) => cand.provider.execute_streaming_dyn(inner, guard).await,
                            None => cand.provider.process(inner).await,
                        };
                        match attempt_result {
                            Ok(resp) => {
                                // Feed the successful round-trip into the EWMA so
                                // LatencyAware ordering reflects reality, and the
                                // token usage into the rolling rate window so
                                // UsageBased / the over-limit gate see real TPM.
                                if let Some(g) = &load_guard {
                                    g.record_latency(started.elapsed());
                                    if let Some(u) = &resp.usage {
                                        g.record_tokens(billed_tokens(u));
                                    }
                                }
                                // A provider that reported a fault mid-stream
                                // is not healthy, even though the partial answer
                                // it already streamed is returned unchanged. The
                                // collector drops the `Error` delta and
                                // `HttpProvider` only promotes it to `Err` when
                                // *nothing* came through, so without this arm a
                                // provider failing this way on every request
                                // stayed `circuit: closed, failure_count: 0`
                                // forever — and each fault also *cleared* its
                                // 429 pacing window below. Fail-closed: the
                                // fault is an answer only about the provider's
                                // health, never a success value.
                                if let Some(msg) = &resp.provider_error {
                                    tracing::warn!(
                                        provider = %cand.name, model = ?model, error = %msg,
                                        "failover: provider reported a fault after emitting \
                                         content; recording the strike and returning the \
                                         partial answer",
                                    );
                                    // rust-doctor-disable-next-line excessive-clone
                                    let err = AlephError::provider(msg.clone());
                                    self.record_emitted_failure(&cand.name, model.as_deref(), &err)
                                        .await;
                                    return Ok(resp);
                                }
                                self.mark_healthy(&cand.name).await;
                                // Tell the gateway which endpoint actually
                                // answered. The `ModelResolved` banner — the
                                // only user-visible fallback signal there is —
                                // used to be driven by a pre-request prediction
                                // made by a health table nothing dialed from, so
                                // a real migration lit nothing. The walk is the
                                // only honest source, so the walk reports.
                                //
                                // The sentinel is excluded for the same reason
                                // the load guard excludes it: it is not an
                                // endpoint, and the chain nested behind it
                                // records the real provider itself.
                                if cand.name != super::NESTED_CHAIN_NODE {
                                    // Same `metadata["session_id"]` the metering
                                    // provider and the OpenAI prompt-cache key
                                    // already read; absent on paths that build a
                                    // payload without it, which simply go
                                    // unrecorded.
                                    if let Some(session) =
                                        metadata.as_ref().and_then(|m| m.get("session_id"))
                                    {
                                        let served = crate::providers::route_witness::Dialed::new(
                                            &cand.name,
                                            // rust-doctor-disable-next-line excessive-clone
                                            model.clone(),
                                        );
                                        crate::providers::route_witness::record_success(
                                            session,
                                            // rust-doctor-disable-next-line excessive-clone
                                            first_attempt.clone().unwrap_or_else(|| served.clone()),
                                            served,
                                        );
                                    }
                                }
                                // A completed call also retires any pacing
                                // window parked on this provider: the window
                                // exists to avoid re-triggering a throttle, and
                                // we just went through it. Without this a
                                // *model*-scoped 429 (which parks the provider
                                // whole) kept the provider parked even though
                                // the sibling-model migration it triggered
                                // answered successfully — so the next turn
                                // deferred a demonstrably working provider.
                                if let Some(pc) = &self.provider_cooldown {
                                    if pc.clear(&cand.name).await {
                                        tracing::debug!(
                                            provider = %cand.name,
                                            "failover: request succeeded, clearing rate pacing",
                                        );
                                    }
                                }
                                return Ok(resp);
                            }
                            // Content already reached the caller's sink: the user
                            // has seen a partial answer, and no later candidate
                            // can un-show it. Retrying or advancing would append
                            // a second answer to a half-written one, so the error
                            // is terminal here even when it would otherwise be
                            // retryable. Only reachable on the streaming path
                            // (`emission` is `None` for `process`), and only
                            // after the model has started answering — connect
                            // errors, auth failures, 429s and model-not-found all
                            // still fail over normally.
                            Err(e) if emission.as_ref().is_some_and(EmissionGuard::has_emitted) => {
                                tracing::warn!(
                                    provider = %cand.name, model = ?model, error = %e,
                                    "failover: stream failed after partial output; \
                                     surfacing instead of restarting on another candidate",
                                );
                                // Terminal for *routing* only. The attempt still
                                // failed, so it earns the same strike a
                                // pre-emission failure would have — the identical
                                // derivation the in-stream-fault arm above uses.
                                self.record_emitted_failure(&cand.name, model.as_deref(), &e)
                                    .await;
                                // The gateway's outer dispatch loop asks the
                                // same question this arm just answered, and
                                // used to answer it from the provider's own
                                // wording: a proxy that cuts a long stream says
                                // "connection reset" / "timed out", which
                                // `harness_bridge::error` read as
                                // `FlowError::Transient` and re-dispatched on
                                // the same run_id — appending a whole second
                                // answer under the half-written one. State the
                                // fact instead of leaving it to be re-derived.
                                return Err(super::mark_partial_output_emitted(&e));
                            }
                            Err(e) => match decide(&e, attempt, self.config.max_retries) {
                                Decision::RetrySame(delay) => {
                                    // Grow the wait exponentially per in-place
                                    // attempt (capped at MAX_RETRY_DELAY), then
                                    // jitter. The exponential growth comes from
                                    // `llm_retry::backoff_delay` so a stubborn
                                    // throttle is ridden out instead of hammered
                                    // at a flat interval; D3: the jitter keeps
                                    // concurrent agents hitting the same
                                    // overloaded provider from retrying in
                                    // lockstep and reigniting the spike.
                                    // A transient server overload may carry a
                                    // server `Retry-After` larger than the 30s
                                    // blip cap; honor it up to the overload
                                    // ceiling so a paid primary that asks to
                                    // wait 60s is waited out, not clamped to 30s
                                    // and abandoned. Plain network blips keep the
                                    // tighter cap.
                                    let cap =
                                        if is_transient_overload(&e.to_string().to_lowercase()) {
                                            MAX_OVERLOAD_RETRY_DELAY
                                        } else {
                                            MAX_RETRY_DELAY
                                        };
                                    let backed_off = backoff_delay(delay, attempt, cap);
                                    let jittered =
                                        crate::providers::retry::apply_jitter(backed_off, 0.25);
                                    tracing::warn!(
                                        provider = %cand.name, model = ?model, attempt,
                                        delay_ms = jittered.as_millis() as u64,
                                        error = %e, "failover: transient, retrying in place",
                                    );
                                    // The attempt ended with the error; only the
                                    // wait remains. Release the in-flight count
                                    // before sleeping so the backoff does not
                                    // read as load (a fresh guard is taken when
                                    // the retry actually goes out).
                                    drop(load_guard);
                                    tokio::time::sleep(jittered).await;
                                    attempt += 1;
                                    continue;
                                }
                                Decision::NextModel => {
                                    tracing::warn!(
                                        provider = %cand.name, model = ?model, error = %e,
                                        "failover: model unavailable, trying next model",
                                    );
                                    last_error = Some(e);
                                    continue 'model;
                                }
                                Decision::RateLimited(hint) => {
                                    // Cool this specific model (server Retry-After
                                    // if given, else default; capped) and prefer a
                                    // sibling model before giving up on the
                                    // provider. `tripped` is set transient so the
                                    // provider's circuit still trips IF every model
                                    // ends up exhausted (single-model providers
                                    // behave exactly as before); it is discarded the
                                    // moment a sibling model succeeds.
                                    let dur = cooldown_window(hint);
                                    if let (Some(cd), Some(m)) = (&self.model_cooldown, &model) {
                                        cd.cool(&cand.name, m, dur).await;
                                    }
                                    // Also park the provider so the *next* turn
                                    // paces itself before re-dialing it (the
                                    // proactive gate above), instead of eating a
                                    // fresh 429 and bouncing to a fallback. For a
                                    // single-model primary (Kimi) this is exactly
                                    // "wait out the provider's stated cooldown";
                                    // for a multi-model provider the brief pace is
                                    // harmless and self-corrects as the window
                                    // elapses.
                                    if let Some(pc) = &self.provider_cooldown {
                                        pc.cool(&cand.name, dur).await;
                                    }
                                    tracing::warn!(
                                        provider = %cand.name, model = ?model, error = %e,
                                        "failover: model rate-limited, cooling down; trying next model",
                                    );
                                    last_error = Some(e);
                                    tripped = Some(FailureKind::Transient);
                                    continue 'model;
                                }
                                Decision::NextProvider(kind) => {
                                    tracing::warn!(
                                        provider = %cand.name, ?kind, error = %e,
                                        "failover: provider unavailable, advancing chain",
                                    );
                                    last_error = Some(e);
                                    tripped = Some(kind);
                                    break 'model;
                                }
                                Decision::Stop => {
                                    tracing::warn!(
                                        provider = %cand.name, error = %e,
                                        "failover: unrecoverable error, aborting",
                                    );
                                    return Err(e);
                                }
                            },
                        }
                    }
                }

                if let Some(kind) = tripped {
                    let reason = last_error
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default();
                    self.mark_unhealthy(&cand.name, reason, kind).await;
                }
            }

            Err(last_error.unwrap_or_else(|| {
                if total == 0 {
                    // Nothing was attempted, so there is no provider error to
                    // report — only the policy that emptied the chain. Read the
                    // mode from the SAME route generation that ordered the
                    // (empty) candidate set: re-reading the live handle here
                    // could name a mode that hot-swapped in after the ordering
                    // pass and had no part in emptying the chain.
                    empty_chain_error(plan.route.mode)
                } else {
                    AlephError::provider(format!("all {total} failover candidates failed"))
                }
            }))
        })
    }
}

impl AiProvider for FailoverProvider {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        self.walk(payload, None)
    }

    /// Stream through the chain: the same walk, issuing each attempt as a
    /// streaming call so live deltas reach the caller from whichever candidate
    /// ends up serving the request.
    ///
    /// Missing this override is what made live streaming unreachable in
    /// production — every real stack runs through this decorator, and without
    /// the override the trait default collapsed the call to `process`.
    fn execute_streaming_dyn<'a>(
        &'a self,
        payload: RequestPayload<'a>,
        sink: &'a dyn DeltaSink,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        self.walk(payload, Some(sink))
    }

    /// The chain streams if the slot it dials first does. A fallback that
    /// cannot stream still *delivers* (the trait default replays its response
    /// through the sink), so the answer is never lost — only batched.
    fn supports_streaming(&self) -> bool {
        self.primary.current().supports_streaming()
    }

    fn name(&self) -> &str {
        "failover"
    }

    fn color(&self) -> &str {
        "#6366f1"
    }

    // The wrapper should look like its live primary for behavior-resolution.
    fn supports_native_tools(&self) -> bool {
        self.primary.current().supports_native_tools()
    }

    // Behavior-resolution must reflect the live primary, like
    // `supports_native_tools` above. `current()` yields a temporary `Arc`,
    // so the value is copied out (`Cow::Owned`) rather than borrowed.
    fn protocol(&self) -> Cow<'_, str> {
        Cow::Owned(self.primary.current().protocol().into_owned())
    }

    fn model_behavior_override(&self) -> Option<Cow<'_, str>> {
        self.primary
            .current()
            .model_behavior_override()
            .map(|c| Cow::Owned(c.into_owned()))
    }

    fn behavior_hint(&self) -> Option<Cow<'_, str>> {
        self.primary
            .current()
            .behavior_hint()
            .map(|c| Cow::Owned(c.into_owned()))
    }

    fn serving_model_hint(&self) -> Option<Cow<'_, str>> {
        self.primary
            .current()
            .serving_model_hint()
            .map(|c| Cow::Owned(c.into_owned()))
    }

    /// `name()` returns the literal `"failover"` (this wrapper is not a
    /// provider); the live primary is who actually serves the call and whose
    /// key pricing must be keyed on.
    fn serving_provider_hint(&self) -> Option<Cow<'_, str>> {
        self.primary
            .current()
            .serving_provider_hint()
            .map(|c| Cow::Owned(c.into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::{unpriced_cost, EndpointTier};

    #[test]
    fn unpriced_local_is_free_and_sorts_first() {
        // A local / self-hosted model with no rate card is genuinely free.
        assert_eq!(unpriced_cost(EndpointTier::Local), 0);
    }

    #[test]
    fn unpriced_cloud_is_unknown_cost_and_sorts_last() {
        // An unpriced cloud model has *unknown* cost, not zero — it must never
        // out-rank a model whose price we can confirm, so it sorts last.
        assert_eq!(unpriced_cost(EndpointTier::Cloud), u64::MAX);
    }

    #[test]
    fn unpriced_unknown_tier_is_treated_as_cloud() {
        // An unresolved base_url is treated conservatively as cloud — never
        // assumed free under cost routing.
        assert_eq!(unpriced_cost(EndpointTier::Unknown), u64::MAX);
    }
}
