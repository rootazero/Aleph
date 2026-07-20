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
use crate::providers::{AiProvider, DefaultProviderHandle};
use crate::sandbox::exec_approval::gate::ApprovalRequester;
use crate::sync_primitives::Arc;

use super::decision::{decide, Decision, FailureKind};
use super::health::{CircuitState, FailoverHealth, ModelCooldown, ProviderCooldown};
use super::{
    FailoverConfig, FailoverNode, CIRCUIT_OPEN_THRESHOLD, DEFAULT_MODEL_COOLDOWN, MAX_COOLDOWN,
    MAX_OVERLOAD_RETRY_DELAY, MAX_RETRY_DELAY,
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
    /// promotion and tier gating. Each pair carries the [`CandidateAction`] the
    /// walk must enforce.
    fn candidates(&self) -> Vec<(FailoverNode, CandidateAction)> {
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
        // Raw fallback pool: live-derived from the primary handle's registry
        // when configured (auto-derived chains). For explicit operator chains,
        // still consult the live registry so a provider removed at runtime is
        // dropped from the chain without a restart; the explicit order is
        // preserved and unknown names are skipped. Live nodes are minimal —
        // empty model list (→ the caller's model) and `Unknown` tier (→ always
        // route-allowed). Falls back to the boot-time static snapshot only when
        // the handle exposes no live providers (tests / non-registry boot paths).
        let live_names = self.primary.provider_names();
        let fallbacks: Vec<FailoverNode> = if live_names.is_empty() {
            let mut v = Vec::with_capacity(self.fallbacks.len());
            for fb in &self.fallbacks {
                if fb.name == primary_name {
                    continue; // dedup: the primary slot already covers it
                }
                // rust-doctor-disable-next-line excessive-clone
                v.push(fb.clone());
            }
            v
        } else if self.derive_fallbacks_live {
            live_names
                .into_iter()
                .filter(|name| name != &primary_name) // dedup: primary slot covers it
                .filter_map(|name| {
                    self.primary
                        .provider_by_name(&name)
                        .map(|provider| FailoverNode {
                            name,
                            models: Vec::new(),
                            provider,
                            tier: EndpointTier::Unknown,
                        })
                })
                .collect()
        } else {
            // Explicit `[fallback_provider].chain`: keep operator order but
            // drop entries that no longer exist in the live registry.
            let live_set: std::collections::HashSet<String> = live_names.into_iter().collect();
            self.fallbacks
                .iter()
                .filter(|fb| fb.name != primary_name && live_set.contains(&fb.name))
                .cloned()
                .collect()
        };
        // One coherent route snapshot for the whole ordering pass — mode,
        // targets, strategy and limits all read from a single config generation.
        let route = self.route_snapshot();
        let (mode, allow_escalation) = (route.mode, route.allow_escalation);
        let targets = Arc::clone(&route.targets);

        // Classify the primary in place. A `Skip` (a hard-guardrail mode with
        // escalation off, on a cross-tier pin) drops it so the chain falls
        // straight through to the fallbacks; `Allow`/`CrossTier` keep it first.
        let mut out: Vec<(FailoverNode, CandidateAction)> = Vec::with_capacity(fallbacks.len() + 1);
        match classify_candidate(mode, primary_node.tier, allow_escalation) {
            CandidateAction::Skip => {}
            action => out.push((primary_node, action)),
        }
        // Order the fallback pool. The balanced path runs when there is a load
        // registry AND either a non-`Ordered` strategy (sort by live signals) or
        // configured rate limits (the over-limit gate must deprioritise
        // saturated providers even under `Ordered`); otherwise the
        // configured-order path stays byte-identical to before.
        let strategy = route.load_balance;
        let limits = Arc::clone(&route.limits);
        let needs_balance = strategy != LoadBalanceStrategy::Ordered || !limits.is_empty();
        let ordered = match &self.load {
            Some(load) if needs_balance => {
                // One rotation tick per request drives RoundRobin; the sort
                // strategies ignore it.
                let rr_base = load.next_round_robin();
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
                        let (util, over) = limits.assess(name, m.rpm_used, m.tpm_used);
                        m.utilization_permille = util;
                        m.over_limit = over;
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
        out.extend(ordered);
        out
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

    /// Whether `name`'s circuit is currently open. Diagnostic accessor used by
    /// tests; the provider-health status surface is [`FailoverHealth::snapshot`]
    /// (rendered by `route_observe` for the `route_status` tool action).
    pub async fn circuit_open(&self, name: &str) -> bool {
        self.health
            .0
            .read()
            .await
            .get(name)
            .is_some_and(|h| h.circuit == CircuitState::Open)
    }
}

impl AiProvider for FailoverProvider {
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
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
        let reqs = RequestRequirements::from_request(
            messages,
            tools.is_some_and(|t| !t.is_empty()),
        );

        Box::pin(async move {
            let candidates = self.candidates();
            let total = candidates.len();
            let mut last_error: Option<AlephError> = None;

            for (idx, (cand, action)) in candidates.into_iter().enumerate() {
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

                // The circuit breaker may skip a candidate only while a later
                // one remains; the final candidate is always attempted so a
                // transient outage cannot hard-fail every request behind an
                // open circuit. `circuit_allows` still runs for its
                // `Open → HalfOpen` bookkeeping.
                let circuit_ok = self.circuit_allows(&cand.name).await;
                if !circuit_ok && idx + 1 < total {
                    tracing::debug!(provider = %cand.name, "failover: circuit open, skipping");
                    continue;
                }

                // Model-list resolution, in precedence:
                //
                // 1. An *explicitly pinned* request model on the primary/default
                //    slot (tier `Unknown`). This is the dynamic-routing model
                //    directive — a `select_model` pick, an agent `model_hint`,
                //    or a `BrainRef::Strict` model — that `ModelOverrideProvider`
                //    stamped onto `payload.model`. It targets the operator's
                //    configured default endpoint, so it overrides that slot's
                //    static catalog walk (otherwise the catalog silently
                //    discarded the model the LLM/agent explicitly chose). Still
                //    passed through the C floor (fail-open) for consistency.
                //    Fallback slots keep their own catalog — the pinned model
                //    belongs to the default endpoint, not its cross-provider
                //    safety net.
                // 2. Empty catalog → a single attempt with the caller's model
                //    (or the provider's own default when that is `None` too).
                // 3. Otherwise the C floor drops models that structurally cannot
                //    serve this request (no vision / no tools / over context
                //    window), failing open so the chain is never emptied.
                let models: Vec<Option<String>> = match (cand.tier, &req_model) {
                    (EndpointTier::Unknown, Some(pinned)) => {
                        retain_capable_models(vec![pinned.clone()], &reqs)
                            .into_iter()
                            .map(Some)
                            .collect()
                    }
                    _ if cand.models.is_empty() => vec![req_model.clone()],
                    // rust-doctor-disable-next-line excessive-clone
                    _ => retain_capable_models(cand.models.clone(), &reqs)
                        .into_iter()
                        .map(Some)
                        .collect(),
                };
                // Sideline models still cooling from an earlier 429, preferring a
                // healthy sibling (fail-open if all are cooling).
                let models = self.drop_cooling_models(&cand.name, models).await;

                // Proactive rate-limit pacing: if this provider 429'd recently
                // and is still inside its recorded cooldown, wait out the
                // *remaining* window before re-dialing it instead of eating a
                // fresh 429. Only the candidate we are about to try is paced
                // (skipped candidates `continue` above). Keeps a single paid
                // primary (e.g. Kimi) in use rather than bouncing to a fallback
                // every turn; capped so a turn never blocks unboundedly (the
                // harness per-turn watchdog is the outer bound). Mirrors hermes'
                // `nous_rate_limit_remaining()` pre-request wait.
                if let Some(pc) = &self.provider_cooldown {
                    if let Some(remaining) = pc.remaining(&cand.name).await {
                        let wait = remaining.min(MAX_OVERLOAD_RETRY_DELAY);
                        tracing::warn!(
                            provider = %cand.name,
                            wait_ms = wait.as_millis() as u64,
                            "failover: provider cooling from a recent 429, pacing before re-request",
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
                        let _load_guard = self.load.as_ref().map(|l| l.begin(&cand.name));
                        let started = Instant::now();
                        match cand.provider.process(inner).await {
                            Ok(resp) => {
                                // Feed the successful round-trip into the EWMA so
                                // LatencyAware ordering reflects reality, and the
                                // token usage into the rolling rate window so
                                // UsageBased / the over-limit gate see real TPM.
                                if let Some(g) = &_load_guard {
                                    g.record_latency(started.elapsed());
                                    if let Some(u) = &resp.usage {
                                        g.record_tokens(
                                            u64::from(u.input_tokens) + u64::from(u.output_tokens),
                                        );
                                    }
                                }
                                self.mark_healthy(&cand.name).await;
                                return Ok(resp);
                            }
                            Err(e) => match decide(&e, attempt, self.config.max_retries) {
                                Decision::RetrySame(delay) => {
                                    // Grow the wait exponentially per in-place
                                    // attempt (capped at MAX_RETRY_DELAY), then
                                    // jitter. The exponential growth mirrors
                                    // `llm_retry::retry_async` so a stubborn
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
                                    let dur =
                                        hint.unwrap_or(DEFAULT_MODEL_COOLDOWN).min(MAX_COOLDOWN);
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
                AlephError::provider(format!("all {total} failover candidates failed"))
            }))
        })
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
