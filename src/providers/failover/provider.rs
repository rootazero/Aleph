//! The [`FailoverProvider`] decorator and its [`AiProvider`] failover walk.

use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::capability_gate::{retain_capable_models, RequestRequirements};
use crate::providers::llm_retry::{backoff_delay, is_transient_overload};
use crate::providers::{AiProvider, DefaultProviderHandle, DeltaSink, ProviderDelta};
use crate::sync_primitives::Arc;

use super::decision::{cooldown_window, decide, strike_for, Decision, FailureKind};
use super::health::{CircuitState, FailoverHealth, ModelCooldown, ProviderCooldown};
use super::{
    FailoverConfig, FailoverNode, CIRCUIT_OPEN_THRESHOLD, DEFAULT_MODEL_COOLDOWN, MAX_COOLDOWN,
    MAX_OVERLOAD_RETRY_DELAY, MAX_RETRY_DELAY,
};

/// Which slot of the chain a candidate occupies.
///
/// The primary slot owns an explicitly pinned request model. Fallbacks always
/// use their own model catalog so a model selected for the primary is never
/// sent to a different provider by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotKind {
    Primary,
    Fallback,
}

/// One request's resolved candidate chain in configured order.
struct CandidatePlan {
    candidates: Vec<(FailoverNode, SlotKind)>,
}

impl CandidatePlan {
    /// The endpoint this plan sets out for, as the route witness's anchor.
    fn anchor(&self) -> Option<crate::providers::route_witness::Dialed> {
        let (node, _) = self.candidates.first()?;
        if node.name == super::NESTED_CHAIN_NODE {
            return None;
        }
        Some(crate::providers::route_witness::Dialed::endpoint(
            &node.name,
        ))
    }
}

/// Fallback chain membership for the next request, in order.
///
/// The single description of "who is in the chain". It has exactly one
/// production caller, [`FailoverProvider::fallback_nodes`], which materialises
/// these names into nodes for the failure-only walk. Three cases, matching how
/// the chain was assembled:
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

/// A [`DeltaSink`] pass-through that remembers what reached the caller, on the
/// two axes the layers above ask about *separately*.
///
/// * [`has_emitted`](Self::has_emitted) — did any **content** delta pass?
///   Bookkeeping-only deltas ([`Usage`](ProviderDelta::Usage),
///   [`Done`](ProviderDelta::Done), [`Error`](ProviderDelta::Error)) do not
///   count. This is the **chain-terminal** predicate: once a candidate has
///   produced structural output the walk stops advancing, because a second
///   candidate would assemble a rival answer over a half-built one.
/// * [`has_shown_user_output`](Self::has_shown_user_output) — did anything reach
///   the user's **transcript** ([`ProviderDelta::is_user_visible`])? Strictly
///   narrower: a stream that emitted only tool-call deltas latches the first
///   bit and not this one.
///
/// Keeping them apart is what stops [`super::PARTIAL_OUTPUT_EMITTED`] from
/// over-firing. That marker's whole job is to stop the gateway re-dispatching a
/// run whose user already has half an answer on screen; asserting it off
/// `has_emitted` would plant it on failures where nothing user-visible ever
/// existed — a truncated tool call, say — and forfeit a re-dispatch that is
/// safe there precisely because the screen is still blank. One guard, two
/// facts, two questions.
struct EmissionGuard<'a> {
    inner: &'a dyn DeltaSink,
    emitted: std::sync::atomic::AtomicBool,
    user_visible: std::sync::atomic::AtomicBool,
}

impl<'a> EmissionGuard<'a> {
    const fn new(inner: &'a dyn DeltaSink) -> Self {
        Self {
            inner,
            emitted: std::sync::atomic::AtomicBool::new(false),
            user_visible: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn has_emitted(&self) -> bool {
        self.emitted.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn has_shown_user_output(&self) -> bool {
        self.user_visible.load(std::sync::atomic::Ordering::Relaxed)
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
        if delta.is_user_visible() {
            self.user_visible
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
    /// Shared circuit-breaker state.
    health: FailoverHealth,
    /// Failure-only retry and circuit-breaker tuning.
    config: FailoverConfig,
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
            model_cooldown: None,
            provider_cooldown: None,
            derive_fallbacks_live: false,
        }
    }

    /// Derive the fallback set live from the primary handle's registry on every
    /// request, instead of the boot-time static snapshot. Set by
    /// `build_failover_chain` only for an *auto-derived* global chain (no
    /// operator `[fallback_provider].chain`), so runtime provider add/remove is
    /// reflected without a restart. See `Self::derive_fallbacks_live`.
    #[must_use]
    pub const fn with_live_fallback_derivation(mut self) -> Self {
        self.derive_fallbacks_live = true;
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

    /// Build the ordered candidate list for one request in explicit order.
    async fn candidates(&self) -> CandidatePlan {
        let primary = self.primary.current();
        let primary_name = primary.name().to_string();
        let primary_models = self
            .model_catalog
            .get(&primary_name)
            .cloned()
            .unwrap_or_default();
        let primary_node = FailoverNode {
            name: primary_name.clone(),
            models: primary_models,
            provider: primary,
        };
        let fallbacks = self.fallback_nodes(&primary_name);
        let mut candidates = Vec::with_capacity(fallbacks.len() + 1);
        candidates.push((primary_node, SlotKind::Primary));
        candidates.extend(fallbacks.into_iter().map(|node| (node, SlotKind::Fallback)));
        CandidatePlan { candidates }
    }

    /// The fallback names this chain would walk right now, in order.
    #[must_use]
    pub(crate) fn fallback_names(&self) -> Vec<String> {
        let primary_name = self.primary.current().name().to_string();
        self.fallback_nodes(&primary_name)
            .into_iter()
            .map(|node| node.name)
            .collect()
    }

    /// The fallback slots this chain would walk right now, materialised.
    ///
    /// Membership comes from the one shared description
    /// ([`effective_fallback_names`]) and is then materialised into nodes for
    /// the walk: a boot node when the operator configured the chain, otherwise
    /// a live registry lookup for an auto-derived chain.
    fn fallback_nodes(&self, primary_name: &str) -> Vec<FailoverNode> {
        let live_names = self.primary.provider_names();
        let configured: Vec<String> = self.fallbacks.iter().map(|n| n.name.clone()).collect();
        effective_fallback_names(
            &live_names,
            primary_name,
            &configured,
            self.derive_fallbacks_live,
        )
        .into_iter()
        .filter_map(|name| {
            // An auto-derived chain dials the *live* provider instance, so one
            // rebuilt at runtime (rotated key, edited base_url) is used without
            // a restart; its model list and tier still come from the boot
            // catalogs. An operator-configured chain uses its boot node verbatim
            // (that node already carries all three).
            if self.derive_fallbacks_live {
                if let Some(provider) = self.primary.provider_by_name(&name) {
                    return Some(FailoverNode {
                        models: self.model_catalog.get(&name).cloned().unwrap_or_default(),
                        name,
                        provider,
                    });
                }
            }
            // rust-doctor-disable-next-line excessive-clone
            self.fallbacks.iter().find(|fb| fb.name == name).cloned()
        })
        .collect()
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
            let mut plan = self.candidates().await;
            let total = plan.candidates.len();
            let mut last_error: Option<AlephError> = None;
            // The route witness's `session_id`, read once: the same
            // `metadata["session_id"]` the metering provider and the OpenAI
            // prompt-cache key already key on. Absent on every path that builds
            // a payload without it (non-gateway callers), which simply go
            // unrecorded.
            let witness_session = metadata.as_ref().and_then(|m| m.get("session_id"));
            // Anchor the run on the endpoint this walk SET OUT FOR, before any
            // gate below can pass it over. Every cheap reason to skip a
            // candidate — open circuit, rate ceiling, 429 pacing park — is
            // settled before the first request is built, so from the second run
            // of an outage onward the first *dial* is already the fallback and
            // anchoring there would report the outage as "nothing deviated".
            //
            // Model deliberately left unresolved: which model this slot would
            // dial is only settled further down by the capability floor and the
            // cooling sideline, so naming the catalog head here could publish a
            // model the walk then filtered out. The dial below refines it.
            if let Some(session) = witness_session {
                if let Some(anchor) = plan.anchor() {
                    crate::providers::route_witness::record_attempt(session, anchor);
                }
            }
            // The first endpoint this walk actually dials. Refines the anchor
            // above with the model that endpoint was really asked for, and is
            // the run's only anchor when there is no plan-time one to refine:
            // the head was a sentinel (no seed) or its anchor was withdrawn by
            // a declined escalation.
            let mut first_attempt: Option<crate::providers::route_witness::Dialed> = None;
            // Records whether any candidate has already pushed content to the
            // caller's sink; see the note on `walk`.
            let emission = sink.map(EmissionGuard::new);

            // The chain is taken OUT of the plan rather than the plan being
            // consumed: the gates below are the plan's own methods
            // (`rate_deferred`), and a partially-moved plan cannot answer them.
            let chain = std::mem::take(&mut plan.candidates);
            for (idx, (cand, slot)) in chain.into_iter().enumerate() {
                // "Is there somewhere else to go?" — derived once per candidate
                // and read by every gate below plus `decide`. Four rules yield
                // to a later candidate and insist on the last one (breaker,
                // rate ceiling, 429 pacing, spent-timeout-window); each having
                // spelled out `idx + 1 < total` for itself is how one of them
                // silently ends up describing a different set.
                let has_later_candidate = idx + 1 < total;
                // The circuit breaker may skip a candidate only while a later
                // one remains; the final candidate is always attempted so a
                // transient outage cannot hard-fail every request behind an
                // open circuit. `circuit_allows` still runs for its
                // `Open → HalfOpen` bookkeeping.
                //
                // This runs BEFORE the escalation gate below: the gate can
                // block on a user prompt, and asking someone to authorise
                // spending on a cloud provider we are then going to skip for
                // being dead is a prompt that buys nothing.
                //
                // All THREE cheap, local reasons to pass over a candidate are
                // settled before that prompt, and this is the whole list:
                // breaker (here), rate ceiling (next), 429 pacing (the skip
                // half below). Order is the whole point: a pacing skip placed
                // *after* the prompt means that under `always_local +
                // allow_cloud_escalation` a cooling cloud candidate costs a
                // real human interruption and is then passed over anyway —
                // one interruption per cooling candidate.
                let circuit_ok = self.circuit_allows(&cand.name).await;
                if !circuit_ok && has_later_candidate {
                    tracing::debug!(provider = %cand.name, "failover: circuit open, skipping");
                    continue;
                }

                // 429 pacing gate — the *skip* half only, which is the half
                // that belongs among the cheap local reasons. Its twin, the
                // last-resort wait, stays below the model resolution: hoisting
                // the wait here would block for up to two minutes BEFORE asking
                // a human to authorise a borrow, which is worse than the prompt
                // it would be trying to save.
                //
                // The skip must sit above the escalation gate for the same
                // reason the breaker and the rate ceiling do: the gate can block
                // on a user prompt, and asking someone to authorise spending on
                // a provider the very next gate passes over for cooling is a
                // prompt that buys nothing — and, with two cooling cloud
                // candidates, one prompt per candidate.
                if has_later_candidate {
                    if let Some(pc) = &self.provider_cooldown {
                        if let Some(remaining) = pc.remaining(&cand.name).await {
                            tracing::debug!(
                                provider = %cand.name,
                                remaining_ms = remaining.as_millis() as u64,
                                "failover: provider cooling from a recent 429, \
                                 deferring to a later candidate",
                            );
                            continue;
                        }
                    }
                }

                // Model-list resolution, in precedence:
                //
                // 1. An *explicitly pinned* request model on the PRIMARY slot.
                //    This is the explicit model directive — a `select_model` pick,
                //    an explicit model/request override, or a `BrainRef::Strict` model —
                //    that `ModelOverrideProvider` stamped onto `payload.model`. It
                //    targets the endpoint the caller chose, so it overrides that slot's static catalog
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
                //
                // The same match also answers "whose statement is this ladder,
                // and how many rungs did that statement have?" — the boot
                // CATALOG's, or one model the caller named. Only the catalog's
                // answer can indict the provider (see the `NextModel`
                // bookkeeping below), and deriving it here — BEFORE the two
                // request-shaped filters below narrow the list — is what keeps
                // the pin exemption and the strike describing the same set.
                //
                // `catalog_rungs == 0` is the single spelling of "not the
                // catalog's statement": the pinned-model arm and the
                // empty-catalog arm both yield it, and the catalog arm cannot
                // (its guard already proved `cand.models` non-empty). A count
                // rather than a count-plus-flag, so the two can never disagree.
                let (models, catalog_rungs): (Vec<Option<String>>, usize) = match (slot, &req_model)
                {
                    (SlotKind::Primary, Some(pinned)) => (
                        retain_capable_models(vec![pinned.clone()], &reqs)
                            .into_iter()
                            .map(Some)
                            .collect(),
                        0,
                    ),
                    _ if cand.models.is_empty() => (vec![None], 0),
                    // rust-doctor-disable-next-line excessive-clone
                    _ => (
                        retain_capable_models(cand.models.clone(), &reqs)
                            .into_iter()
                            .map(Some)
                            .collect(),
                        cand.models.len(),
                    ),
                };
                // Sideline models still cooling from an earlier 429, preferring a
                // healthy sibling (fail-open if all are cooling).
                let models = self.drop_cooling_models(&cand.name, models).await;
                // How many rungs answered "no such model". Counted rather than
                // latched on a flag, because the question after the loop is an
                // *equality* against `catalog_rungs` that no single boolean can
                // state honestly: an early `break 'model` leaves rungs untried,
                // and a ladder that never ran must never read as exhausted.
                let mut missing_models: usize = 0;

                // Proactive rate-limit pacing, the *wait* half: this provider
                // 429'd recently, is still inside its recorded cooldown, and is
                // the last resort.
                //
                // "Last resort" is read from the SAME `has_later_candidate` the
                // skip half read — not inferred from having survived it. The
                // two halves each re-read `pc.remaining()`, and between the
                // reads sit the escalation gate (which can block on a human
                // approval prompt for minutes) and model resolution;
                // `ProviderCooldown` is shared `Arc` state that any concurrent
                // walk parks on its own 429. So "it was not cooling at the skip
                // gate" is not still true here, and without this guard a
                // NON-last candidate could sleep out a window a concurrent turn
                // opened while the human was being asked — exactly the wait the
                // skip half exists to prevent.
                //
                // Waiting it out keeps a single paid primary (e.g. Kimi) in use
                // instead of eating a fresh 429. (LiteLLM and Bifrost both drop
                // a cooling deployment from selection outright; Aleph keeps it
                // as the terminal candidate so a single-provider setup still
                // gets its request served — the binding invariant here is that
                // the last candidate ALWAYS dials.)
                //
                // The wait is spent only when it can actually end the window.
                // `remaining.min(MAX_OVERLOAD_RETRY_DELAY)` would not be: a
                // window longer than the cap — a server `Retry-After` of 300s,
                // capped by `cooldown_window` at `MAX_COOLDOWN` = 600s — bought
                // a 120s sleep that provably could not outlast it and then
                // dialed *inside* the very window it had just waited for. Worse
                // than useless: `aleph-server` gives a turn a 120s wall-clock
                // watchdog by default (`build_stability_triple`), so the sleep
                // alone consumed the entire turn budget and the dial that
                // followed it never got to happen. When the wait cannot finish
                // the job, dial now and let the provider's own answer (a fresh
                // 429 re-parks it, a success clears it) settle the question.
                if !has_later_candidate {
                    if let Some(pc) = &self.provider_cooldown {
                        if let Some(remaining) = pc.remaining(&cand.name).await {
                            if remaining <= MAX_OVERLOAD_RETRY_DELAY {
                                tracing::warn!(
                                    provider = %cand.name,
                                    wait_ms = remaining.as_millis() as u64,
                                    "failover: last candidate is cooling from a recent 429, \
                                     pacing out the whole window before re-request",
                                );
                                tokio::time::sleep(remaining).await;
                            } else {
                                tracing::warn!(
                                    provider = %cand.name,
                                    remaining_secs = remaining.as_secs(),
                                    cap_secs = MAX_OVERLOAD_RETRY_DELAY.as_secs(),
                                    "failover: last candidate's 429 window outlasts the pacing \
                                     cap; dialing inside it rather than spending the turn \
                                     budget on a wait that cannot end it",
                                );
                            }
                        }
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
                        // Same sentinel exclusion the load guard makes: it is
                        // not an endpoint, so it must not become the `original`
                        // half of a fallback notice either.
                        //
                        // This runs even when the plan already anchored the run:
                        // the two writes answer different halves of the same
                        // question. The plan-time anchor names the endpoint the
                        // walk set out for (which a gate may then pass over);
                        // this one names the model that endpoint was really
                        // asked for, and is the only anchor at all for a walk
                        // that had no seed to refine (a sentinel head, or a head
                        // whose anchor a denied escalation withdrew). The witness
                        // merges them (`RouteWitness::absorb`): the anchor's
                        // model is refined only by a dial of that same endpoint.
                        if first_attempt.is_none() && cand.name != super::NESTED_CHAIN_NODE {
                            let dialed = crate::providers::route_witness::Dialed::new(
                                &cand.name,
                                // rust-doctor-disable-next-line excessive-clone
                                model.clone(),
                            );
                            if let Some(session) = witness_session {
                                // rust-doctor-disable-next-line excessive-clone
                                crate::providers::route_witness::record_attempt(
                                    session,
                                    dialed.clone(),
                                );
                            }
                            first_attempt = Some(dialed);
                        }
                        let attempt_result = match &emission {
                            Some(guard) => cand.provider.execute_streaming_dyn(inner, guard).await,
                            None => cand.provider.process(inner).await,
                        };
                        match attempt_result {
                            Ok(resp) => {
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
                                    if let Some(session) = witness_session {
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
                                // The gateway's outer dispatch loop asks a
                                // *narrower* question than the one this arm just
                                // answered, and must not answer it from the
                                // provider's own wording: a proxy that cuts a
                                // long stream says "connection reset" / "timed
                                // out", which `harness_bridge::error` reads as
                                // `FlowError::Transient` and re-dispatches on
                                // the same run_id — appending a whole second
                                // answer under the half-written one. State the
                                // fact instead of leaving it to be re-derived.
                                //
                                // Narrower, because the harm is "a second answer
                                // lands under a visible first one" — which needs
                                // a visible first one. `has_emitted` also latches
                                // on tool-call deltas, and those never reach the
                                // transcript (the sole production sink,
                                // `harness::agent::think::CallbackSink`, drops
                                // every variant but text and thinking). Marking
                                // off `has_emitted` would forfeit the gateway's
                                // re-dispatch for a truncated tool call, where a
                                // fresh attempt is the correct and blameless
                                // recovery. So the routing verdict comes from the
                                // wide bit and the marker from the narrow one.
                                if emission
                                    .as_ref()
                                    .is_some_and(EmissionGuard::has_shown_user_output)
                                {
                                    return Err(super::mark_partial_output_emitted(&e));
                                }
                                return Err(e);
                            }
                            Err(e) => match decide(
                                &e,
                                attempt,
                                self.config.max_retries,
                                has_later_candidate,
                            ) {
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
                                    tokio::time::sleep(jittered).await;
                                    attempt += 1;
                                    continue;
                                }
                                Decision::NextModel => {
                                    tracing::warn!(
                                        provider = %cand.name, model = ?model, error = %e,
                                        "failover: model unavailable, trying next model",
                                    );
                                    // Sideline the missing model the same way a
                                    // 429'd one is sidelined, so the *next*
                                    // turn prefers a sibling that exists
                                    // instead of re-paying the 404 round-trip.
                                    // `DEFAULT_MODEL_COOLDOWN` (60s), not
                                    // `MAX_COOLDOWN`: "model not found" is
                                    // routinely a deploy in flight — an
                                    // `ollama pull` finishing, a serverless
                                    // endpoint warming, a catalog entry the
                                    // operator is in the middle of adding — so
                                    // the window has to be short enough that a
                                    // model deployed a minute later is used a
                                    // minute later. `drop_cooling_models` is
                                    // fail-open, so cooling every rung never
                                    // empties the ladder.
                                    if let (Some(cd), Some(m)) = (&self.model_cooldown, &model) {
                                        cd.cool(&cand.name, m, DEFAULT_MODEL_COOLDOWN).await;
                                    }
                                    missing_models += 1;
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

                // Every rung of a CATALOG ladder answered "no such model". That
                // is a statement about the PROVIDER, not about any one model:
                // the endpoint is up, authenticated and answering, and serves
                // nothing we know how to ask it for (a base_url pointed at the
                // wrong gateway, a proxy whose model set was renamed, a local
                // server with no models pulled). A `NextModel` exit that
                // recorded no strike would leave "in what state does this
                // breaker go red for a provider that 404s everything?" with
                // the answer *never*: circuit closed forever, `cooling` false
                // forever, LeastBusy/LatencyAware still ranking it first, and
                // every turn paying N wasted round-trips.
                // `Transient` (not `Permanent`) keeps the 3-strike threshold, so
                // a catalog that is briefly out of step with a redeploy is not
                // shed on one walk.
                //
                // Read from the ladder's own provenance (`catalog_rungs`,
                // derived where the list was chosen): a single model the CALLER
                // named — a `select_model` pick, an explicit model/request
                // override, a `BrainRef::Strict` id, or simply a typo — indicts
                // that name, never the endpoint, so it must not shed a healthy provider.
                //
                // The equality is against what the CATALOG stated, never
                // against the survivor set the request narrowed it to. Both
                // filters between the two can yield a proper subset — the
                // capability floor fails open only when *every* model is
                // dropped, `drop_cooling_models` only when *every* model is
                // cooling — so a provider whose m1 is parked by a 429 and whose
                // m2 404s would otherwise be indicted for "serving none of its
                // catalog models" while m1 was never dialed and is healthy.
                // Since the survivor set is a subset, `missing_models` can only
                // reach `catalog_rungs` when nothing was withheld AND every
                // rung answered 404 — one comparison covering both.
                if catalog_rungs > 0 && missing_models == catalog_rungs {
                    tracing::warn!(
                        provider = %cand.name,
                        models = catalog_rungs,
                        "failover: provider serves none of its catalog models; \
                         recording a strike against the provider",
                    );
                    tripped.get_or_insert(FailureKind::Transient);
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
                    // No candidate was available, so report the failure without
                    // depending on removed route-policy state.
                    AlephError::provider("no provider is configured to serve this request")
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

    /// Estimators take the live primary's policy; a failover to another target
    /// is re-projected for that target at its own wire.
    fn reasoning_replay(
        &self,
        model: Option<&str>,
    ) -> crate::providers::reasoning_replay::ReasoningReplay {
        self.primary.current().reasoning_replay(model)
    }

    /// The live primary's answer, read once per run by the preflight
    /// pipeline; a failover onto a target that does not clear keeps the local
    /// passes off for the rest of that run (a documented gap).
    fn clears_tool_results_server_side(&self) -> bool {
        self.primary.current().clears_tool_results_server_side()
    }
}
