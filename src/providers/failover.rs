//! Model failover provider — automatic provider/model switching.
//!
//! [`FailoverProvider`] is an [`AiProvider`] *decorator*. It wraps an ordered
//! chain — the live default provider plus a static list of fallbacks, each
//! provider expanded across its configured model list — and transparently
//! walks the chain when a call fails. The harness loop sees a single
//! `Arc<dyn AiProvider>` and never learns failover happened: Redline R10 (the
//! dumb loop performs no error-recovery strategy selection) holds because all
//! of that lives here, in the provider layer.
//!
//! # Failure handling
//!
//! Each failure is classified by [`llm_retry`](crate::providers::llm_retry) —
//! the shared error classifier — into one [`Decision`]:
//!
//! - **transient** (network blip, 529 overloaded) → retried in place a few
//!   times with backoff, then treated as a provider-level failure;
//! - **provider-level** (rate limit, auth, exhausted transient) → the
//!   provider's circuit breaker trips and the walk advances to the next
//!   provider;
//! - **model-level** (404 model not found) → the walk advances to the next
//!   model of the *same* provider;
//! - **fatal** (400 bad request) → returned immediately — switching provider
//!   cannot fix a malformed request;
//! - **context overflow** (413) → returned immediately, since the harness
//!   context-compactor owns that recovery path.
//!
//! # Circuit breaker
//!
//! Per-provider health is a three-state breaker (`Closed → Open → HalfOpen`)
//! keyed by provider name in a [`FailoverHealth`] map. That map is shared
//! (via `Arc`) across the global chain and every per-agent chain, so one
//! provider's outage is visible everywhere.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::config::types::RouteMode;
use crate::error::{AlephError, ErrorClass, Result};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::llm_retry::{classify, classify_exhausted, RetryVerdict};
use crate::providers::route_handle::RouteHandle;
use crate::providers::route_policy::{order_candidates, CandidateAction, EndpointTier};
use crate::providers::{AiProvider, DefaultProviderHandle};
use crate::sandbox::exec_approval::gate::ApprovalRequester;
use crate::sync_primitives::Arc;

/// Consecutive failures at which a provider's circuit breaker opens.
const CIRCUIT_OPEN_THRESHOLD: u32 = 3;
/// Hard ceiling on the circuit-breaker cooldown.
const MAX_COOLDOWN: Duration = Duration::from_secs(600);
/// Backoff used for a bare transient error whose message carried no delay hint.
const DEFAULT_TRANSIENT_DELAY: Duration = Duration::from_millis(300);

// =============================================================================
// Configuration
// =============================================================================

/// Failover tuning knobs.
///
/// Internal — *not* a TOML type. The operator-facing surface is
/// `[fallback_provider].chain`; see `config::types::phase6_wiring`.
#[derive(Debug, Clone)]
pub struct FailoverConfig {
    /// Same-candidate retries on a transient error before the chain advances.
    pub max_retries: u32,
    /// Initial circuit-breaker cooldown. Doubles on each HalfOpen probe
    /// failure, capped at [`MAX_COOLDOWN`].
    pub unhealthy_cooldown: Duration,
}

impl Default for FailoverConfig {
    fn default() -> Self {
        Self {
            max_retries: 2,
            unhealthy_cooldown: Duration::from_secs(300),
        }
    }
}

/// One provider in the failover chain, with the models to try in order.
#[derive(Clone)]
pub struct FailoverNode {
    /// Provider name — the circuit-breaker key.
    pub name: String,
    /// Models to attempt, in order. Empty → a single attempt that lets the
    /// provider pick its own configured default model.
    pub models: Vec<String>,
    /// The underlying provider implementation.
    pub provider: Arc<dyn AiProvider>,
    /// Endpoint locality tier, used by the route policy. Defaults to
    /// [`EndpointTier::Unknown`] so existing literals stay valid and the node
    /// is treated as the operator's configured default (always allowed).
    pub tier: EndpointTier,
}

impl FailoverNode {
    /// Construct a node with an explicit tier.
    pub fn with_tier(
        name: String,
        models: Vec<String>,
        provider: Arc<dyn AiProvider>,
        tier: EndpointTier,
    ) -> Self {
        Self {
            name,
            models,
            provider,
            tier,
        }
    }
}

// =============================================================================
// Circuit breaker
// =============================================================================

/// Circuit-breaker state for a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Healthy — requests flow through.
    Closed,
    /// Tripped — requests skip this provider until the cooldown expires.
    Open,
    /// Cooldown expired — the next request is a single probe.
    HalfOpen,
}

/// Per-provider health tracked by the circuit breaker.
#[derive(Debug, Clone)]
struct HealthState {
    circuit: CircuitState,
    last_failure: Option<Instant>,
    failure_count: u32,
    last_error: Option<String>,
    cooldown: Duration,
}

impl Default for HealthState {
    fn default() -> Self {
        Self {
            circuit: CircuitState::Closed,
            last_failure: None,
            failure_count: 0,
            last_error: None,
            cooldown: Duration::from_secs(300),
        }
    }
}

/// Per-provider circuit-breaker state, keyed by provider name.
///
/// Cloning shares the same underlying map (`Arc`): one provider's outage
/// recorded by the global chain is immediately visible to every per-agent
/// chain that was built with the same `FailoverHealth`.
#[derive(Clone)]
pub struct FailoverHealth(Arc<RwLock<HashMap<String, HealthState>>>);

impl Default for FailoverHealth {
    fn default() -> Self {
        Self(Arc::new(RwLock::new(HashMap::new())))
    }
}

// =============================================================================
// Decision
// =============================================================================

/// How a provider-level failure should shape the circuit breaker.
///
/// The breaker treats the two kinds differently: a `Transient` outage needs a
/// few strikes before it sidelines a provider (so a momentary blip does not
/// evict a healthy one), whereas a `Permanent` failure — a revoked or
/// misconfigured credential — is shed on the first strike with a long cooldown,
/// so the hot path stops paying a full round-trip to a known-dead provider on
/// every subsequent request. Mirrors openclaw's transient-vs-preserved probe
/// slots and hermes' permanent/transient split.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum FailureKind {
    /// Recoverable soon (rate limit, overload, network): strike-then-probe.
    Transient,
    /// Won't recover this session (bad/expired key, forbidden): shed at once.
    Permanent,
}

/// What to do after one failed `process()` attempt.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// Retry the same provider + model after `delay`.
    RetrySame(Duration),
    /// Advance to the next model of the same provider.
    NextModel,
    /// Trip this provider's circuit and advance to the next provider.
    NextProvider(FailureKind),
    /// Abort the walk and return the error to the caller.
    Stop,
}

/// Classify one failed attempt into a [`Decision`].
///
/// Two-stage: the string classifier ([`classify`]) recognises an in-place
/// retry opportunity (529 / network keywords); [`classify_exhausted`] then
/// gives the final verdict. A `Fatal` string verdict is overridden to a
/// provider-level failover when the *typed* error is transient — covering
/// errors whose `Display` carried no HTTP code (e.g. `Timeout` →
/// "Request timed out").
fn decide(err: &AlephError, attempt: u32, max_retries: u32) -> Decision {
    let msg = err.to_string();
    let lower = msg.to_lowercase();

    // A transient error the string classifier recognises is worth a brief
    // in-place retry before the chain advances.
    let transient_delay = match classify(&msg) {
        RetryVerdict::Retry { delay } => Some(delay),
        _ => None,
    };
    let can_retry = attempt < max_retries;

    // When the walk advances providers, tag *why*: a permanent credential
    // failure sheds the provider immediately; everything else is transient.
    let next_provider = if crate::providers::llm_retry::is_permanent_failure(&msg) {
        Decision::NextProvider(FailureKind::Permanent)
    } else {
        Decision::NextProvider(FailureKind::Transient)
    };

    match classify_exhausted(&msg) {
        // 413 — the harness owns this recovery path via
        // `AgentHarness::try_reactive_compact_and_retry` (see
        // `harness::agent::think`). The failover layer stops so the
        // verdict reaches the harness intact instead of being swallowed
        // by sibling-provider attempts that would hit the same overflow.
        RetryVerdict::CompactAndRetry { .. } => Decision::Stop,
        RetryVerdict::Fallback { reason } => {
            if reason.starts_with("model not found") {
                Decision::NextModel
            } else if let (Some(delay), true) = (transient_delay, can_retry) {
                Decision::RetrySame(delay)
            } else {
                next_provider
            }
        }
        // `classify_exhausted` never yields `Retry`; handled defensively.
        RetryVerdict::Retry { delay } if can_retry => Decision::RetrySame(delay),
        RetryVerdict::Retry { .. } => next_provider,
        RetryVerdict::Fatal => {
            let explicit_bad_request = lower.contains("400")
                && (lower.contains("bad request") || lower.contains("invalid"));
            if !explicit_bad_request && err.class() == ErrorClass::Transient {
                if can_retry {
                    Decision::RetrySame(DEFAULT_TRANSIENT_DELAY)
                } else {
                    // A typed-transient error with no HTTP code: transient.
                    Decision::NextProvider(FailureKind::Transient)
                }
            } else {
                Decision::Stop
            }
        }
    }
}

// =============================================================================
// FailoverProvider
// =============================================================================

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
        }
    }

    /// Attach a local/cloud route preference and the escalation approval gate.
    ///
    /// `new()` alone stays `Auto` + no-gate (today's behaviour). In `Auto` the
    /// `approval` gate is never consulted. In `AlwaysLocal` with
    /// `allow_cloud_escalation`, the gate authorises borrowing a cloud
    /// endpoint as a terminal fallback; absent a gate, escalation fails closed.
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

    /// The route preference to apply *now*: the live handle if attached, else
    /// the boot snapshot.
    fn route_preference(&self) -> (RouteMode, bool) {
        match &self.route_handle {
            Some(h) => h.load(),
            None => (self.route_mode, self.allow_cloud_escalation),
        }
    }

    /// Whether a cloud-borrow escalation for `name` is authorised right now.
    ///
    /// Fails closed: no gate wired → denied (a warn is logged). Mirrors the
    /// sandbox escalation contract — the money-spending action is gated at the
    /// moment it would happen, not at config-write time.
    async fn escalation_allowed(&self, name: &str) -> bool {
        match self.approval.clone() {
            Some(gate) => {
                let reason = format!(
                    "Route mode is AlwaysLocal but no local provider succeeded; \
                     borrow cloud provider '{name}' to complete this request?"
                );
                gate.request_approval("__route_escalate_cloud", &reason)
                    .await
                    .is_approved()
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

    /// Build the ordered candidate list for one request: the live primary
    /// first, then each fallback whose name differs from the primary's, then
    /// shaped by the route policy (tier ordering + cross-tier gating).
    ///
    /// The primary slot is tagged [`EndpointTier::Unknown`] — its `base_url` is
    /// not resolvable from the live `DefaultProviderHandle`, so the route
    /// policy always allows it as the operator's configured default. Each pair
    /// carries the [`CandidateAction`] the walk must enforce.
    fn candidates(&self) -> Vec<(FailoverNode, CandidateAction)> {
        let primary = self.primary.current();
        let primary_name = primary.name().to_string();
        let primary_models = self
            .model_catalog
            .get(&primary_name)
            .cloned()
            .unwrap_or_default();
        let mut out = vec![FailoverNode {
            name: primary_name.clone(),
            models: primary_models,
            provider: primary,
            tier: EndpointTier::Unknown,
        }];
        for fb in &self.fallbacks {
            if fb.name == primary_name {
                continue; // dedup: the primary slot already covers it
            }
            out.push(fb.clone());
        }
        let (mode, allow_escalation) = self.route_preference();
        order_candidates(out, mode, allow_escalation, |n| n.tier)
    }

    /// Whether `name` may be tried now. Transitions `Open → HalfOpen` when the
    /// cooldown has elapsed (allowing exactly one probe).
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

    /// Whether `name`'s circuit is currently open. Diagnostic accessor —
    /// used by tests and (later) a provider-health status tool.
    pub async fn circuit_open(&self, name: &str) -> bool {
        self.health
            .0
            .read()
            .await
            .get(name)
            .map(|h| h.circuit == CircuitState::Open)
            .unwrap_or(false)
    }
}

impl AiProvider for FailoverProvider {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        // Own every borrowed field so the payload can be rebuilt per attempt.
        let messages = payload.messages.to_vec();
        let system_prompt = payload.system_prompt.map(str::to_string);
        let tools = payload.tools.map(<[_]>::to_vec);
        let think_level = payload.think_level;
        let temperature = payload.temperature;
        let max_tokens = payload.max_tokens;
        let tool_choice = payload.tool_choice.clone();
        let req_model = payload.model.clone();
        let metadata = payload.metadata.clone();

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

                // Empty model list → a single attempt with the caller's model
                // (or the provider's own default when that is `None` too).
                let models: Vec<Option<String>> = if cand.models.is_empty() {
                    vec![req_model.clone()]
                } else {
                    cand.models.iter().cloned().map(Some).collect()
                };

                let mut tripped: Option<FailureKind> = None;
                'model: for model in models {
                    let mut attempt: u32 = 0;
                    loop {
                        let inner = RequestPayload {
                            messages: &messages,
                            system_prompt: system_prompt.as_deref(),
                            system_blocks: None,
                            tools: tools.as_deref(),
                            think_level,
                            temperature,
                            max_tokens,
                            tool_choice: tool_choice.clone(),
                            model: model.clone(),
                            metadata: metadata.clone(),
                        };
                        match cand.provider.process(inner).await {
                            Ok(resp) => {
                                self.mark_healthy(&cand.name).await;
                                return Ok(resp);
                            }
                            Err(e) => match decide(&e, attempt, self.config.max_retries) {
                                Decision::RetrySame(delay) => {
                                    // D3: jitter the backoff so concurrent agents
                                    // hitting the same overloaded provider don't
                                    // retry in lockstep and reignite the spike.
                                    // Equal-jitter shape via `apply_jitter`.
                                    let jittered =
                                        crate::providers::retry::apply_jitter(delay, 0.25);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;
    use crate::providers::StaticDefault;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Test provider: each `process()` call consumes the next scripted
    /// outcome. `Ok(())` → a text response tagged with the provider name;
    /// `Err(msg)` → `AlephError::provider(msg)`. When the script is exhausted
    /// the last outcome repeats — so `["429..."]` means "always 429".
    struct ScriptProvider {
        name: String,
        script: Mutex<VecDeque<std::result::Result<(), String>>>,
        last: Mutex<std::result::Result<(), String>>,
        seen_models: Mutex<Vec<Option<String>>>,
        calls: AtomicUsize,
    }

    impl ScriptProvider {
        fn new(name: &str, script: Vec<std::result::Result<(), String>>) -> Arc<Self> {
            let last = script.last().cloned().unwrap_or(Ok(()));
            Arc::new(Self {
                name: name.to_string(),
                script: Mutex::new(script.into()),
                last: Mutex::new(last),
                seen_models: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
            })
        }

        fn ok(name: &str) -> Arc<Self> {
            Self::new(name, vec![Ok(())])
        }

        fn err(name: &str, msg: &str) -> Arc<Self> {
            Self::new(name, vec![Err(msg.to_string())])
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn models(&self) -> Vec<Option<String>> {
            self.seen_models.lock().unwrap().clone()
        }
    }

    impl AiProvider for ScriptProvider {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen_models.lock().unwrap().push(payload.model.clone());
            let outcome = {
                let mut q = self.script.lock().unwrap();
                match q.pop_front() {
                    Some(o) => {
                        *self.last.lock().unwrap() = o.clone();
                        o
                    }
                    None => self.last.lock().unwrap().clone(),
                }
            };
            let name = self.name.clone();
            Box::pin(async move {
                match outcome {
                    Ok(()) => Ok(ProviderResponse::text_only(name)),
                    Err(msg) => Err(AlephError::provider(msg)),
                }
            })
        }

        fn name(&self) -> &str {
            &self.name
        }

        fn color(&self) -> &str {
            "#000"
        }
    }

    /// Assemble a `FailoverProvider` from a primary + fallback nodes.
    fn build(
        primary: Arc<dyn AiProvider>,
        catalog: Vec<(&str, Vec<&str>)>,
        fallbacks: Vec<FailoverNode>,
    ) -> FailoverProvider {
        let model_catalog = catalog
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.into_iter().map(String::from).collect()))
            .collect();
        FailoverProvider::new(
            Arc::new(StaticDefault::new(primary)),
            fallbacks,
            model_catalog,
            FailoverHealth::default(),
            FailoverConfig::default(),
        )
    }

    fn node(name: &str, provider: Arc<dyn AiProvider>) -> FailoverNode {
        FailoverNode {
            name: name.to_string(),
            models: Vec::new(),
            provider,
            tier: EndpointTier::Unknown,
        }
    }

    /// Node with an explicit endpoint tier, for route-mode tests.
    fn tiered_node(name: &str, provider: Arc<dyn AiProvider>, tier: EndpointTier) -> FailoverNode {
        FailoverNode::with_tier(name.to_string(), Vec::new(), provider, tier)
    }

    // --- decide() unit tests ----------------------------------------------

    #[test]
    fn decide_bad_request_stops() {
        let e = AlephError::provider("HTTP 400 bad request: invalid param");
        assert_eq!(decide(&e, 0, 2), Decision::Stop);
    }

    #[test]
    fn decide_rate_limit_advances_provider() {
        let e = AlephError::provider("HTTP 429 too many requests");
        assert_eq!(
            decide(&e, 0, 2),
            Decision::NextProvider(FailureKind::Transient)
        );
    }

    #[test]
    fn decide_auth_advances_provider_as_permanent() {
        let e = AlephError::provider("HTTP 401 Unauthorized");
        assert_eq!(
            decide(&e, 0, 2),
            Decision::NextProvider(FailureKind::Permanent)
        );
        let e = AlephError::provider("HTTP 403 Forbidden: invalid api key");
        assert_eq!(
            decide(&e, 0, 2),
            Decision::NextProvider(FailureKind::Permanent)
        );
    }

    #[test]
    fn decide_model_not_found_advances_model() {
        let e = AlephError::provider("HTTP 404 model gpt-9 not found");
        assert_eq!(decide(&e, 0, 2), Decision::NextModel);
    }

    #[test]
    fn decide_413_stops_for_compactor() {
        let e = AlephError::provider("HTTP 413 prompt is too long: 200000 tokens > 100000 maximum");
        assert_eq!(decide(&e, 0, 2), Decision::Stop);
    }

    #[test]
    fn decide_transient_retries_then_advances() {
        let e = AlephError::provider("connection reset by peer");
        assert!(matches!(decide(&e, 0, 2), Decision::RetrySame(_)));
        assert!(matches!(decide(&e, 1, 2), Decision::RetrySame(_)));
        assert_eq!(
            decide(&e, 2, 2),
            Decision::NextProvider(FailureKind::Transient)
        );
    }

    #[test]
    fn decide_typed_timeout_with_no_http_code_still_fails_over() {
        // `Timeout` Display is "Request timed out" — no HTTP keyword — but the
        // typed class is Transient, so the walk must still advance.
        let e = AlephError::Timeout { suggestion: None };
        assert!(matches!(decide(&e, 0, 2), Decision::RetrySame(_)));
        assert_eq!(
            decide(&e, 2, 2),
            Decision::NextProvider(FailureKind::Transient)
        );
    }

    // --- process() integration tests --------------------------------------

    #[tokio::test]
    async fn primary_success_skips_fallback() {
        let primary = ScriptProvider::ok("primary");
        let fallback = ScriptProvider::ok("fallback");
        let fp = build(primary, vec![], vec![node("fallback", fallback.clone())]);

        let msgs = [UnifiedMessage::user("hi")];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "primary");
        assert_eq!(fallback.call_count(), 0);
    }

    #[tokio::test]
    async fn rate_limit_fails_over_to_next_provider() {
        let primary = ScriptProvider::err("primary", "HTTP 429 too many requests");
        let fallback = ScriptProvider::ok("fallback");
        let fp = build(primary, vec![], vec![node("fallback", fallback)]);

        let msgs = [UnifiedMessage::user("hi")];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "fallback");
    }

    #[tokio::test]
    async fn model_not_found_walks_to_next_model_same_provider() {
        let primary = ScriptProvider::new(
            "anthropic",
            vec![Err("HTTP 404 model opus not found".into()), Ok(())],
        );
        let fp = build(
            primary.clone(),
            vec![("anthropic", vec!["opus", "sonnet"])],
            vec![],
        );

        let msgs = [UnifiedMessage::user("hi")];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "anthropic");
        assert_eq!(
            primary.models(),
            vec![Some("opus".to_string()), Some("sonnet".to_string())]
        );
    }

    #[tokio::test]
    async fn bad_request_aborts_without_failover() {
        let primary = ScriptProvider::err("primary", "HTTP 400 bad request: invalid");
        let fallback = ScriptProvider::ok("fallback");
        let fp = build(primary, vec![], vec![node("fallback", fallback.clone())]);

        let msgs = [UnifiedMessage::user("hi")];
        assert!(fp.process(RequestPayload::new(&msgs)).await.is_err());
        assert_eq!(fallback.call_count(), 0);
    }

    #[tokio::test]
    async fn context_overflow_propagates_without_failover() {
        let primary = ScriptProvider::err(
            "primary",
            "HTTP 413 prompt is too long: 200000 tokens > 100000 maximum",
        );
        let fallback = ScriptProvider::ok("fallback");
        let fp = build(primary, vec![], vec![node("fallback", fallback.clone())]);

        let msgs = [UnifiedMessage::user("hi")];
        assert!(fp.process(RequestPayload::new(&msgs)).await.is_err());
        assert_eq!(fallback.call_count(), 0);
    }

    #[tokio::test]
    async fn transient_error_retried_in_place_then_succeeds() {
        let primary = ScriptProvider::new("primary", vec![Err("connection reset".into()), Ok(())]);
        let fallback = ScriptProvider::ok("fallback");
        let fp = build(
            primary.clone(),
            vec![],
            vec![node("fallback", fallback.clone())],
        );

        let msgs = [UnifiedMessage::user("hi")];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "primary");
        assert_eq!(primary.call_count(), 2);
        assert_eq!(fallback.call_count(), 0);
    }

    #[tokio::test]
    async fn all_candidates_exhausted_returns_error() {
        let primary = ScriptProvider::err("primary", "HTTP 429 rate limit");
        let fallback = ScriptProvider::err("fallback", "HTTP 429 rate limit");
        let fp = build(primary, vec![], vec![node("fallback", fallback)]);

        let msgs = [UnifiedMessage::user("hi")];
        assert!(fp.process(RequestPayload::new(&msgs)).await.is_err());
    }

    #[tokio::test]
    async fn circuit_opens_after_threshold_failures() {
        let primary = ScriptProvider::err("primary", "HTTP 429 rate limit");
        let fallback = ScriptProvider::ok("fallback");
        let fp = build(primary, vec![], vec![node("fallback", fallback)]);

        let msgs = [UnifiedMessage::user("hi")];
        assert!(!fp.circuit_open("primary").await);
        for _ in 0..CIRCUIT_OPEN_THRESHOLD {
            let _ = fp.process(RequestPayload::new(&msgs)).await;
        }
        assert!(fp.circuit_open("primary").await);
    }

    #[tokio::test]
    async fn permanent_auth_failure_opens_circuit_on_first_strike() {
        // A fallback with a dead credential must be shed immediately so the hot
        // path stops re-dialing it — unlike a transient outage, which needs
        // CIRCUIT_OPEN_THRESHOLD strikes (see `circuit_opens_after_threshold`).
        let primary = ScriptProvider::err("primary", "HTTP 401 Unauthorized");
        let fallback = ScriptProvider::ok("fallback");
        let fp = build(primary, vec![], vec![node("fallback", fallback)]);

        let msgs = [UnifiedMessage::user("hi")];
        assert!(!fp.circuit_open("primary").await);
        // One request is enough: the primary serves the error, fails over to
        // the healthy fallback, and the primary's circuit is already open.
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "fallback");
        assert!(fp.circuit_open("primary").await);
    }

    #[tokio::test]
    async fn permanently_dead_fallback_skipped_on_next_request() {
        // Once a permanent-failure provider's circuit is open, later requests
        // skip it entirely (a later candidate exists), saving the round-trip.
        let dead = ScriptProvider::err("dead", "HTTP 403 Forbidden: bad key");
        let healthy = ScriptProvider::ok("healthy");
        let fp = build(
            dead.clone(),
            vec![],
            vec![node("healthy", healthy.clone())],
        );

        let msgs = [UnifiedMessage::user("hi")];
        // First request: dead is dialed once, trips its circuit, healthy serves.
        let _ = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(dead.call_count(), 1);
        // Second request: dead's circuit is open and a later candidate exists,
        // so it is skipped — call_count stays at 1.
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "healthy");
        assert_eq!(dead.call_count(), 1);
    }

    #[tokio::test]
    async fn lone_candidate_attempted_even_with_open_circuit() {
        // A single provider that keeps failing must still be retried after
        // its circuit opens — there is nowhere else to fail over to.
        let primary = ScriptProvider::err("solo", "HTTP 429 rate limit");
        let fp = build(primary.clone(), vec![], vec![]);

        let msgs = [UnifiedMessage::user("hi")];
        let rounds = CIRCUIT_OPEN_THRESHOLD + 2;
        for _ in 0..rounds {
            let _ = fp.process(RequestPayload::new(&msgs)).await;
        }
        assert!(fp.circuit_open("solo").await);
        // Despite the open circuit, the lone provider was tried every call.
        assert_eq!(primary.call_count(), rounds as usize);
    }

    #[tokio::test]
    async fn fallback_matching_primary_name_is_deduped() {
        let primary = ScriptProvider::ok("anthropic");
        let dup = ScriptProvider::err("anthropic", "HTTP 429 rate limit");
        let fp = build(primary, vec![], vec![node("anthropic", dup.clone())]);

        let msgs = [UnifiedMessage::user("hi")];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "anthropic");
        assert_eq!(dup.call_count(), 0);
    }

    // --- route-mode integration tests -------------------------------------

    /// An `ApprovalRequester` with a fixed verdict, recording its call count.
    struct MockApprover {
        approve: bool,
        calls: AtomicUsize,
    }

    impl MockApprover {
        fn new(approve: bool) -> Arc<Self> {
            Arc::new(Self {
                approve,
                calls: AtomicUsize::new(0),
            })
        }
        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl ApprovalRequester for MockApprover {
        async fn request_approval(&self, _tool: &str, _reason: &str) -> ApprovalOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.approve {
                ApprovalOutcome::Approved
            } else {
                ApprovalOutcome::Denied
            }
        }
    }

    use crate::sandbox::exec_approval::gate::ApprovalOutcome;

    /// Build a route-shaped FailoverProvider from explicitly-tiered fallbacks.
    fn build_routed(
        primary: Arc<dyn AiProvider>,
        fallbacks: Vec<FailoverNode>,
        mode: RouteMode,
        allow_escalation: bool,
        approval: Option<Arc<dyn ApprovalRequester>>,
    ) -> FailoverProvider {
        FailoverProvider::new(
            Arc::new(StaticDefault::new(primary)),
            fallbacks,
            HashMap::new(),
            FailoverHealth::default(),
            FailoverConfig::default(),
        )
        .with_route(mode, allow_escalation, approval)
    }

    #[tokio::test]
    async fn auto_mode_is_byte_identical_to_plain_failover() {
        // Primary fails, a Cloud-tagged fallback succeeds; Auto must not drop
        // it even though the route engine is now in the path.
        let primary = ScriptProvider::err("primary", "HTTP 429 rate limit");
        let fb = ScriptProvider::ok("cloud_fb");
        let fp = build_routed(
            primary,
            vec![tiered_node("cloud_fb", fb, EndpointTier::Cloud)],
            RouteMode::Auto,
            false,
            None,
        );
        let msgs = [UnifiedMessage::user("hi")];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "cloud_fb");
    }

    #[tokio::test]
    async fn always_local_drops_cloud_fallback_without_escalation() {
        // Primary (Unknown, always allowed) fails; the only fallback is Cloud
        // and escalation is off → it is dropped → the request fails entirely.
        let primary = ScriptProvider::err("primary", "HTTP 429 rate limit");
        let cloud = ScriptProvider::ok("cloud_fb");
        let fp = build_routed(
            primary,
            vec![tiered_node("cloud_fb", cloud.clone(), EndpointTier::Cloud)],
            RouteMode::AlwaysLocal,
            false,
            None,
        );
        let msgs = [UnifiedMessage::user("hi")];
        assert!(fp.process(RequestPayload::new(&msgs)).await.is_err());
        // The dropped cloud fallback was never dialed.
        assert_eq!(cloud.call_count(), 0);
    }

    #[tokio::test]
    async fn always_local_borrows_cloud_when_approved() {
        let primary = ScriptProvider::err("primary", "HTTP 429 rate limit");
        let cloud = ScriptProvider::ok("cloud_fb");
        let approver = MockApprover::new(true);
        let fp = build_routed(
            primary,
            vec![tiered_node("cloud_fb", cloud.clone(), EndpointTier::Cloud)],
            RouteMode::AlwaysLocal,
            true,
            Some(approver.clone()),
        );
        let msgs = [UnifiedMessage::user("hi")];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "cloud_fb");
        assert_eq!(approver.call_count(), 1);
        assert_eq!(cloud.call_count(), 1);
    }

    #[tokio::test]
    async fn always_local_denied_escalation_skips_cloud() {
        let primary = ScriptProvider::err("primary", "HTTP 429 rate limit");
        let cloud = ScriptProvider::ok("cloud_fb");
        let approver = MockApprover::new(false);
        let fp = build_routed(
            primary,
            vec![tiered_node("cloud_fb", cloud.clone(), EndpointTier::Cloud)],
            RouteMode::AlwaysLocal,
            true,
            Some(approver.clone()),
        );
        let msgs = [UnifiedMessage::user("hi")];
        assert!(fp.process(RequestPayload::new(&msgs)).await.is_err());
        assert_eq!(approver.call_count(), 1);
        // Denied → the cloud candidate is never dialed.
        assert_eq!(cloud.call_count(), 0);
    }

    #[tokio::test]
    async fn always_cloud_degrades_to_local_ungated() {
        // Primary (Unknown) fails; the Local fallback is a cross-tier degrade
        // with no approval required — it must be tried with no gate consulted.
        let primary = ScriptProvider::err("primary", "HTTP 429 rate limit");
        let local = ScriptProvider::ok("local_fb");
        let approver = MockApprover::new(false); // would deny if consulted
        let fp = build_routed(
            primary,
            vec![tiered_node("local_fb", local.clone(), EndpointTier::Local)],
            RouteMode::AlwaysCloud,
            false,
            Some(approver.clone()),
        );
        let msgs = [UnifiedMessage::user("hi")];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "local_fb");
        // Cloud→local degrade is ungated: the approver was never consulted.
        assert_eq!(approver.call_count(), 0);
        assert_eq!(local.call_count(), 1);
    }
}
