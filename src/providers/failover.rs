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

use crate::config::types::{LoadBalanceStrategy, RouteMode};
use crate::error::{AlephError, ErrorClass, Result};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::capability_gate::{retain_capable_models, RequestRequirements};
use crate::providers::llm_retry::{
    backoff_delay, classify, classify_exhausted, extract_retry_after_str, is_transient_overload,
    RetryVerdict,
};
use crate::providers::load_stats::LoadStats;
use crate::providers::route_handle::RouteHandle;
use crate::providers::route_policy::{
    classify_candidate, order_candidates, order_candidates_balanced, CandidateAction, EndpointTier,
    RateLimits, RouteTargets,
};
use crate::providers::{AiProvider, DefaultProviderHandle};
use crate::sandbox::exec_approval::gate::ApprovalRequester;
use crate::sync_primitives::Arc;

/// Consecutive failures at which a provider's circuit breaker opens.
const CIRCUIT_OPEN_THRESHOLD: u32 = 3;
/// Hard ceiling on the circuit-breaker cooldown.
const MAX_COOLDOWN: Duration = Duration::from_secs(600);
/// Backoff used for a bare transient error whose message carried no delay hint.
const DEFAULT_TRANSIENT_DELAY: Duration = Duration::from_millis(300);
/// Ceiling on a single in-place retry wait. Matches `llm_retry`'s backoff cap
/// (`MAX_DELAY`) so the failover loop and the standalone `retry_async` helper
/// grow their backoff identically.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
/// Ceiling on a single in-place wait while *riding out a transient server
/// overload* (429 "please wait a moment", 529). Higher than [`MAX_RETRY_DELAY`]
/// so a large server `Retry-After` is honored in full rather than silently
/// clamped to 30s — a paid primary (e.g. Kimi) that says "wait 60s" should be
/// waited out on, not abandoned to a fallback. Also caps the proactive
/// per-provider cooldown wait. Matches hermes' 120s `Retry-After` cap.
const MAX_OVERLOAD_RETRY_DELAY: Duration = Duration::from_secs(120);
/// In-place retry budget for a *transient server overload* — a 429 whose body
/// says "please wait a moment and try again", or a 529 `overloaded`. Deeper
/// than [`FailoverConfig::max_retries`] because in a single-provider setup
/// there is no sibling to fail over to, and the server explicitly asked the
/// client to wait and retry: a flat 2-retry budget gave up after ~4s and
/// hard-failed the whole flow (the 08:00 scheduled job's 429 crash). With the
/// exponential backoff applied in `process()` this rides out a throttle for up
/// to ~60s (2+4+8+16+32) before advancing the chain — and honors a larger
/// server `Retry-After` up to [`MAX_OVERLOAD_RETRY_DELAY`]. A genuine
/// account/quota 429 stays `Fatal` upstream and never reaches this budget.
const OVERLOAD_RETRY_BUDGET: u32 = 5;
/// Default sideline window for a *model-specific* 429 when the server gave no
/// `Retry-After` hint. Short enough that a transient per-model throttle clears
/// quickly; a longer server hint wins (capped by [`MAX_COOLDOWN`]).
const DEFAULT_MODEL_COOLDOWN: Duration = Duration::from_secs(60);

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

/// Read-only view of one provider's circuit-breaker state, surfaced by the
/// `self_config` `route_status` diagnostic (see
/// [`route_observe`](crate::providers::route_observe)).
#[derive(Debug, Clone)]
pub struct ProviderHealthView {
    /// Provider name (the circuit-breaker key).
    pub provider: String,
    /// Circuit state: `"closed"`, `"open"`, or `"half_open"`.
    pub circuit: &'static str,
    /// Consecutive failures recorded since the last success.
    pub failure_count: u32,
    /// The error that drove the most recent failure, if any.
    pub last_error: Option<String>,
    /// Seconds until an `Open` circuit allows a probe; `0` means the next
    /// request probes immediately (effectively half-open). `None` when the
    /// circuit is not open.
    pub cooldown_remaining_secs: Option<u64>,
}

impl FailoverHealth {
    /// Snapshot every tracked provider's breaker state, name-sorted.
    /// Diagnostic only — the hot path keeps using `circuit_allows`.
    pub async fn snapshot(&self) -> Vec<ProviderHealthView> {
        let map = self.0.read().await;
        let mut out: Vec<ProviderHealthView> = map
            .iter()
            .map(|(name, st)| {
                let circuit = match st.circuit {
                    CircuitState::Closed => "closed",
                    CircuitState::Open => "open",
                    CircuitState::HalfOpen => "half_open",
                };
                let cooldown_remaining_secs = (st.circuit == CircuitState::Open).then(|| {
                    st.last_failure
                        .map(|at| st.cooldown.saturating_sub(at.elapsed()).as_secs())
                        .unwrap_or(0)
                });
                ProviderHealthView {
                    provider: name.clone(),
                    circuit,
                    failure_count: st.failure_count,
                    last_error: st.last_error.clone(),
                    cooldown_remaining_secs,
                }
            })
            .collect();
        out.sort_by(|a, b| a.provider.cmp(&b.provider));
        out
    }
}

/// Per-(provider, model) rate-limit cooldown.
///
/// A model that returned a *model-specific* 429 is sidelined until its cooldown
/// expires, so the walk prefers a healthy sibling model (or provider) instead
/// of re-hitting the throttled one. Distinct from the provider-level circuit
/// breaker ([`FailoverHealth`]): a rate-limited model does **not** trip the
/// whole provider — its siblings stay live. Cloning shares the same map
/// (`Arc`), so a throttle recorded by the global chain is visible to every
/// per-hint override (scoped exactly like `FailoverHealth`).
#[derive(Clone)]
pub struct ModelCooldown(Arc<RwLock<HashMap<(String, String), Instant>>>);

impl Default for ModelCooldown {
    fn default() -> Self {
        Self(Arc::new(RwLock::new(HashMap::new())))
    }
}

impl ModelCooldown {
    /// Sideline `(provider, model)` for `dur`.
    async fn cool(&self, provider: &str, model: &str, dur: Duration) {
        let until = Instant::now() + dur;
        self.0
            .write()
            .await
            .insert((provider.to_string(), model.to_string()), until);
    }

    /// Whether `(provider, model)` is still within its cooldown window. An
    /// expired entry reads as not-cooling (and is overwritten on the next
    /// [`cool`](Self::cool)).
    async fn is_cooling(&self, provider: &str, model: &str) -> bool {
        let key = (provider.to_string(), model.to_string());
        self.0
            .read()
            .await
            .get(&key)
            .is_some_and(|&until| until > Instant::now())
    }

    /// Snapshot every `(provider, model)` pair still inside its cooldown
    /// window as `(provider, model, remaining_secs)`, sorted. Expired entries
    /// are omitted — same strict `until > now` reading as
    /// [`is_cooling`](Self::is_cooling). Diagnostic only — feeds the
    /// `route_status` snapshot.
    pub async fn snapshot(&self) -> Vec<(String, String, u64)> {
        let now = Instant::now();
        let mut out: Vec<(String, String, u64)> = self
            .0
            .read()
            .await
            .iter()
            .filter(|&((_, _), &until)| until > now)
            .map(|((p, m), &until)| (p.clone(), m.clone(), (until - now).as_secs()))
            .collect();
        out.sort();
        out
    }
}

/// Per-provider rate-limit cooldown gate (proactive pacing).
///
/// After a provider 429s / overloads and the walk gives up its in-place retries,
/// the provider name is parked until `available_at`. Before re-dialing that
/// provider on a *later* turn, the walk waits out the **remaining** window
/// instead of immediately re-triggering the same throttle — so a single paid
/// primary (e.g. Kimi) is paced to its rate limit and kept in use, rather than
/// eating a fresh 429 and bouncing to a fallback every turn. Provider-keyed twin
/// of [`ModelCooldown`]; it *waits* (the primary has no equivalent sibling)
/// where the model cooldown *sidelines*. Mirrors hermes'
/// `nous_rate_limit_remaining()` pre-request wait and opensquilla's credential
/// park. Cloning shares the same map (`Arc`), scoped exactly like
/// [`FailoverHealth`].
#[derive(Clone)]
pub struct ProviderCooldown(Arc<RwLock<HashMap<String, Instant>>>);

impl Default for ProviderCooldown {
    fn default() -> Self {
        Self(Arc::new(RwLock::new(HashMap::new())))
    }
}

impl ProviderCooldown {
    /// Park `provider` for `dur`. Extends an existing window, never shortens it,
    /// so a longer server `Retry-After` is not clobbered by a later default.
    async fn cool(&self, provider: &str, dur: Duration) {
        let until = Instant::now() + dur;
        let mut map = self.0.write().await;
        let slot = map.entry(provider.to_string()).or_insert(until);
        *slot = (*slot).max(until);
    }

    /// Remaining cooldown for `provider`, or `None` if it is not currently
    /// cooling (no entry, or the window already elapsed).
    async fn remaining(&self, provider: &str) -> Option<Duration> {
        let now = Instant::now();
        self.0
            .read()
            .await
            .get(provider)
            .and_then(|&until| until.checked_duration_since(now))
    }

    /// Snapshot every provider still inside its pacing window as
    /// `(provider, remaining_secs)`, name-sorted. Expired entries are omitted
    /// — same strict `until > now` reading as [`remaining`](Self::remaining).
    /// Diagnostic only — feeds the `route_status` snapshot.
    pub async fn snapshot(&self) -> Vec<(String, u64)> {
        let now = Instant::now();
        let mut out: Vec<(String, u64)> = self
            .0
            .read()
            .await
            .iter()
            .filter(|&(_, &until)| until > now)
            .map(|(p, &until)| (p.clone(), (until - now).as_secs()))
            .collect();
        out.sort();
        out
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
    /// This model hit a *model-specific* 429. Record a per-model cooldown
    /// (`Some(d)` carries the server `Retry-After`), then prefer a sibling
    /// model before advancing providers. Does **not** trip the provider
    /// circuit — sibling models stay live.
    RateLimited(Option<Duration>),
    /// Trip this provider's circuit and advance to the next provider.
    NextProvider(FailureKind),
    /// Abort the walk and return the error to the caller.
    Stop,
}

/// Server-guided `Retry-After` from a *typed* error's `suggestion` field.
///
/// The Anthropic adapter stashes "Rate limited. Retry after N seconds." in
/// `RateLimitError.suggestion` / `ProviderError.suggestion`, but the error's
/// `Display` only renders `message` — so the string classifier never sees the
/// hint. Reading the typed field recovers it.
fn retry_after_from_suggestion(err: &AlephError) -> Option<Duration> {
    let suggestion = match err {
        AlephError::RateLimitError { suggestion, .. }
        | AlephError::ProviderError { suggestion, .. } => suggestion.as_deref(),
        _ => None,
    }?;
    extract_retry_after_str(suggestion)
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

    // Server-guided `Retry-After` from the *typed* error's `suggestion` field.
    // The `Display` impl drops `suggestion`, so the string classifier never sees
    // it; reading it here lets a real provider hint beat the body-parsed/default
    // delay (matches openclaw/Pi, which honour Retry-After).
    let server_delay = retry_after_from_suggestion(err);
    // A transient error the string classifier recognises is worth a brief
    // in-place retry before the chain advances. A server `Retry-After` (if any)
    // overrides the classifier's delay — but only when the classifier already
    // judged the error transient, so an account/quota 429 whose suggestion
    // carries a Retry-After is not lifted into a retry it should not get.
    let transient_delay = match classify(&msg) {
        RetryVerdict::Retry { delay } => Some(server_delay.unwrap_or(delay)),
        _ => None,
    };
    // A transient *server overload* (429 "please wait a moment", 529) earns a
    // deeper in-place retry budget than the cross-provider `max_retries`: the
    // server told us to wait, and a single-provider setup has no sibling to
    // advance to. Plain network blips keep the shallow budget — there a sibling
    // provider is the better next bet than hammering a flaky socket. The wait
    // grows exponentially (capped at `MAX_RETRY_DELAY`) per attempt in
    // `process()`.
    // `transient_delay.is_some()` gates out account/quota 429s: `classify`
    // returns `Fatal` (not `Retry`) for an account-scoped limit even when its
    // body says "please wait a moment", so they keep the shallow budget and are
    // never hammered in place.
    let retry_budget = if transient_delay.is_some() && is_transient_overload(&lower) {
        max_retries.max(OVERLOAD_RETRY_BUDGET)
    } else {
        max_retries
    };
    let can_retry = attempt < retry_budget;

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
            } else if reason.starts_with("rate limited") {
                // A model-specific 429 (account/quota limits classify `Fatal`
                // upstream and never reach here): sideline this model and try a
                // sibling before advancing providers. The overload path above
                // exhausts its deeper in-place budget first, then lands here.
                Decision::RateLimited(server_delay)
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
                    Decision::RetrySame(server_delay.unwrap_or(DEFAULT_TRANSIENT_DELAY))
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

    /// Tag the primary slot with a concrete endpoint tier so a hard-guardrail
    /// route mode can gate it. Used by `build_failover_chain` for the per-pin
    /// override chains (an explicit `select_model` / agent `provider_hint`
    /// target): the pinned provider's real tier makes `AlwaysLocal` route a
    /// cloud pin through the borrow-cloud approval instead of silently allowing
    /// it. The global default chain omits this — its primary stays `Unknown`
    /// (the operator's configured default is always allowed).
    pub fn with_primary_tier(mut self, tier: EndpointTier) -> Self {
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
    pub fn with_model_cooldown(mut self, cooldown: ModelCooldown) -> Self {
        self.model_cooldown = Some(cooldown);
        self
    }

    /// Attach the shared per-provider rate-limit cooldown gate. Wired only in
    /// production (`build_failover_chain`) with one registry cloned across all
    /// chains; tests omit it and keep no pacing (byte-identical to before).
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
                kept.push(m.clone());
            }
        }
        if kept.is_empty() {
            models
        } else {
            kept
        }
    }

    /// The load-balancing strategy to apply *now*: the live handle if attached,
    /// else the safe no-op [`LoadBalanceStrategy::Ordered`] (tests / `new()`).
    fn route_load_balance(&self) -> LoadBalanceStrategy {
        self.route_handle
            .as_ref()
            .map(|h| h.load_balance())
            .unwrap_or_default()
    }

    /// The route preference to apply *now*: the live handle if attached, else
    /// the boot snapshot.
    fn route_preference(&self) -> (RouteMode, bool) {
        match &self.route_handle {
            Some(h) => h.load(),
            None => (self.route_mode, self.allow_cloud_escalation),
        }
    }

    /// The operator's provider pins to apply *now*: the live handle if attached,
    /// else empty (no promotion). Pins only ever enter via the boot-wired handle,
    /// so `new()`/tests see an empty set — byte-identical to unpinned ordering.
    fn route_targets(&self) -> Arc<RouteTargets> {
        match &self.route_handle {
            Some(h) => h.targets(),
            None => Arc::new(RouteTargets::default()),
        }
    }

    /// The operator's per-provider rate ceilings to apply *now*: the live handle
    /// if attached, else empty (no rate awareness). Limits only ever enter via
    /// the boot-wired handle, so `new()`/tests see an empty set — byte-identical
    /// to pre-usage ordering.
    fn route_limits(&self) -> Arc<RateLimits> {
        match &self.route_handle {
            Some(h) => h.limits(),
            None => Arc::new(RateLimits::default()),
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
                    "Route mode is AlwaysLocal; borrow cloud provider '{name}' \
                     for this request?"
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
            name: primary_name.clone(),
            models: primary_models,
            provider: primary,
            tier: self.primary_tier,
        };
        let mut fallbacks: Vec<FailoverNode> = Vec::with_capacity(self.fallbacks.len());
        for fb in &self.fallbacks {
            if fb.name == primary_name {
                continue; // dedup: the primary slot already covers it
            }
            fallbacks.push(fb.clone());
        }
        let (mode, allow_escalation) = self.route_preference();
        let targets = self.route_targets();

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
        let strategy = self.route_load_balance();
        let limits = self.route_limits();
        let needs_balance = strategy != LoadBalanceStrategy::Ordered || !limits.is_empty();
        let ordered = match &self.load {
            Some(load) if needs_balance => {
                // One rotation tick per request drives RoundRobin; the sort
                // strategies ignore it.
                let rr_base = load.next_round_robin();
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
                    // scalars. The ordering logic stays limit-blind (R7: pure
                    // infrastructure) — the policy never sees the config.
                    |name| {
                        let mut m = load.metric(name);
                        let (util, over) = limits.assess(name, m.rpm_used, m.tpm_used);
                        m.utilization_permille = util;
                        m.over_limit = over;
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

    /// Whether `name`'s circuit is currently open. Diagnostic accessor used by
    /// tests; the provider-health status surface is [`FailoverHealth::snapshot`]
    /// (rendered by `route_observe` for the `route_status` tool action).
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
        // C floor: derive the request's structural capability requirements once
        // (image blocks → vision, tools array → tool-calling, text size →
        // context window). Prompt-blind; shapes the candidate model set below.
        let reqs = RequestRequirements::from_request(
            &messages,
            tools.as_ref().is_some_and(|t| !t.is_empty()),
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
                                            u.input_tokens as u64 + u.output_tokens as u64,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;
    use crate::providers::StaticDefault;
    use crate::sync_primitives::Mutex;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
    fn decide_model_rate_limit_returns_rate_limited() {
        // A model-specific 429 (no account/quota markers) no longer trips the
        // whole provider at the decide() level: it returns RateLimited so
        // process() sidelines just this model and prefers a sibling before the
        // provider's circuit is considered. The server gave no Retry-After, so
        // the cooldown hint is None.
        let e = AlephError::provider("HTTP 429 too many requests");
        assert_eq!(decide(&e, 0, 2), Decision::RateLimited(None));
    }

    #[test]
    fn decide_model_rate_limit_honors_typed_retry_after() {
        // Item #1: the server Retry-After lives in the typed `suggestion` field
        // (Display drops it). It must flow through as the RateLimited cooldown
        // hint so the model is sidelined for exactly as long as the server asked.
        let e = AlephError::RateLimitError {
            message: "HTTP 429 rate limit exceeded for this model".into(),
            suggestion: Some("Rate limited. Retry after 42 seconds.".into()),
        };
        assert_eq!(
            decide(&e, 0, 2),
            Decision::RateLimited(Some(Duration::from_secs(42)))
        );
    }

    #[test]
    fn decide_overload_429_honors_typed_retry_after() {
        // Item #1 on the in-place-retry path: a server-overload 429 whose
        // Retry-After sits in `suggestion` retries in place with the server's
        // delay rather than the classifier's 2s default.
        let e = AlephError::RateLimitError {
            message: "Anthropic API rate limited (429): please wait a moment and try again".into(),
            suggestion: Some("Retry after 7 seconds.".into()),
        };
        assert_eq!(
            decide(&e, 0, 2),
            Decision::RetrySame(Duration::from_secs(7))
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
    fn decide_transient_overload_429_gets_deep_retry_budget() {
        // The generic Anthropic server-throttle 429 ("please wait a moment")
        // must be retried in place well past the shallow `max_retries=2` so a
        // single-provider setup rides out the spike instead of hard-failing the
        // flow (the 08:00 scheduled-job 429 crash). Once OVERLOAD_RETRY_BUDGET is
        // exhausted it escalates to the per-model cooldown path (RateLimited),
        // which process() turns into a sibling-model migration and, only if the
        // provider's models are exhausted, a provider advance.
        let e = AlephError::RateLimitError {
            message: "Anthropic API rate limited (429): We're receiving too many \
                      requests at the moment. Please wait a moment and try again."
                .into(),
            suggestion: None,
        };
        assert!(matches!(decide(&e, 2, 2), Decision::RetrySame(_)));
        assert!(matches!(
            decide(&e, OVERLOAD_RETRY_BUDGET - 1, 2),
            Decision::RetrySame(_)
        ));
        assert_eq!(
            decide(&e, OVERLOAD_RETRY_BUDGET, 2),
            Decision::RateLimited(None)
        );
    }

    #[test]
    fn decide_plain_network_transient_keeps_shallow_budget() {
        // A bare connection blip is NOT a server-asked-to-wait overload, so it
        // keeps the shallow `max_retries` budget — a sibling provider is the
        // better next bet than hammering a flaky socket. Guards the overload
        // budget from leaking into ordinary transient errors.
        let e = AlephError::provider("connection reset by peer");
        assert!(matches!(decide(&e, 1, 2), Decision::RetrySame(_)));
        assert_eq!(
            decide(&e, 2, 2),
            Decision::NextProvider(FailureKind::Transient)
        );
    }

    #[test]
    fn decide_account_quota_429_excluded_from_deep_budget() {
        // An account/quota 429 classifies `Fatal` upstream, so even though its
        // body says "please wait a moment" it must NOT be lifted into the deep
        // overload budget: it advances the chain at `max_retries`, whereas a
        // genuine transient overload (above) is still `RetrySame` at attempt 2.
        let e = AlephError::provider("429 account quota exceeded; please wait a moment");
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
    async fn model_specific_rate_limit_migrates_to_sibling_model() {
        // Item #3: a model-specific 429 on the first model sidelines just that
        // model and tries the provider's next model in the SAME request, rather
        // than advancing providers (or hard-failing a single-provider setup).
        let primary = ScriptProvider::new(
            "anthropic",
            vec![Err("HTTP 429 too many requests".into()), Ok(())],
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
    async fn cooling_model_is_skipped_on_next_request() {
        // Item #3 (cross-request): once a model is cooled by a 429, a later
        // request skips it and dials the healthy sibling first.
        let primary = ScriptProvider::new(
            "anthropic",
            vec![
                Err("HTTP 429 too many requests".into()), // req1: opus 429
                Ok(()),                                   // req1: sonnet ok
                Ok(()),                                   // req2: straight to sonnet
            ],
        );
        let catalog = [(
            "anthropic".to_string(),
            vec!["opus".to_string(), "sonnet".to_string()],
        )]
        .into_iter()
        .collect();
        let fp = FailoverProvider::new(
            Arc::new(StaticDefault::new(primary.clone())),
            vec![],
            catalog,
            FailoverHealth::default(),
            FailoverConfig::default(),
        )
        .with_model_cooldown(ModelCooldown::default());

        let msgs = [UnifiedMessage::user("hi")];
        // Request 1: opus 429 → cool opus → sonnet serves.
        let _ = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        // Request 2: opus is still cooling → dial sonnet directly.
        let _ = fp.process(RequestPayload::new(&msgs)).await.unwrap();

        assert_eq!(
            primary.models(),
            vec![
                Some("opus".to_string()),
                Some("sonnet".to_string()),
                Some("sonnet".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn model_cooldown_tracks_per_model() {
        let cd = ModelCooldown::default();
        assert!(!cd.is_cooling("p", "a").await);
        cd.cool("p", "a", Duration::from_secs(60)).await;
        assert!(cd.is_cooling("p", "a").await);
        // A different model on the same provider is unaffected.
        assert!(!cd.is_cooling("p", "b").await);
        // A zero-duration cool expires immediately.
        cd.cool("p", "b", Duration::from_secs(0)).await;
        assert!(!cd.is_cooling("p", "b").await);
    }

    #[tokio::test]
    async fn provider_cooldown_cools_extends_and_isolates() {
        let pc = ProviderCooldown::default();
        // Not cooling until parked.
        assert!(pc.remaining("kimi").await.is_none());
        pc.cool("kimi", Duration::from_secs(60)).await;
        let r = pc.remaining("kimi").await.expect("should be cooling");
        assert!(r > Duration::from_secs(58) && r <= Duration::from_secs(60));
        // A shorter cool never shortens an existing window (server Retry-After
        // wins over a later default).
        pc.cool("kimi", Duration::from_millis(1)).await;
        assert!(pc.remaining("kimi").await.expect("still cooling") > Duration::from_secs(58));
        // A different provider is independent.
        assert!(pc.remaining("other").await.is_none());
    }

    #[tokio::test]
    async fn provider_cooldown_remaining_is_none_after_window() {
        let pc = ProviderCooldown::default();
        pc.cool("p", Duration::from_millis(5)).await;
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(pc.remaining("p").await.is_none());
    }

    #[tokio::test]
    async fn health_snapshot_reports_open_circuit_with_remaining() {
        let health = FailoverHealth::default();
        {
            let mut map = health.0.write().await;
            let st = map.entry("p".to_string()).or_default();
            st.circuit = CircuitState::Open;
            st.failure_count = 3;
            st.last_error = Some("boom".to_string());
            st.last_failure = Some(Instant::now());
            st.cooldown = Duration::from_secs(300);
        }
        let snap = health.snapshot().await;
        assert_eq!(snap.len(), 1);
        let v = &snap[0];
        assert_eq!(v.provider, "p");
        assert_eq!(v.circuit, "open");
        assert_eq!(v.failure_count, 3);
        assert_eq!(v.last_error.as_deref(), Some("boom"));
        let rem = v.cooldown_remaining_secs.expect("open carries remaining");
        assert!(rem <= 300);
    }

    #[tokio::test]
    async fn health_snapshot_closed_circuit_has_no_cooldown() {
        let health = FailoverHealth::default();
        health.0.write().await.entry("p".to_string()).or_default();
        let snap = health.snapshot().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].circuit, "closed");
        assert!(snap[0].cooldown_remaining_secs.is_none());
        assert!(snap[0].last_error.is_none());
    }

    #[tokio::test]
    async fn model_cooldown_snapshot_lists_active_and_skips_expired() {
        let cd = ModelCooldown::default();
        cd.cool("p", "hot", Duration::from_secs(60)).await;
        cd.cool("p", "stale", Duration::from_millis(5)).await;
        tokio::time::sleep(Duration::from_millis(25)).await;
        let snap = cd.snapshot().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].0, "p");
        assert_eq!(snap[0].1, "hot");
        assert!(snap[0].2 <= 60);
    }

    #[tokio::test]
    async fn provider_cooldown_snapshot_lists_active_and_skips_expired() {
        let pc = ProviderCooldown::default();
        pc.cool("hot", Duration::from_secs(60)).await;
        pc.cool("stale", Duration::from_millis(5)).await;
        tokio::time::sleep(Duration::from_millis(25)).await;
        let snap = pc.snapshot().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].0, "hot");
        assert!(snap[0].1 <= 60);
    }

    #[tokio::test]
    async fn vision_request_skips_blind_model_in_candidate_list() {
        // The primary "openai" offers a blind model (o1-mini) before a seeing
        // one (gpt-4o). A request carrying an image must skip o1-mini entirely
        // (C floor) and dial only gpt-4o.
        let primary = ScriptProvider::ok("openai");
        let fp = build(
            primary.clone(),
            vec![("openai", vec!["o1-mini", "gpt-4o"])],
            vec![],
        );

        let msgs = [UnifiedMessage::user_with_content(vec![
            crate::providers::message::ContentBlock::Image {
                data: "base64".into(),
                mime_type: "image/png".into(),
            },
        ])];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "openai");
        assert_eq!(primary.models(), vec![Some("gpt-4o".to_string())]);
    }

    #[tokio::test]
    async fn pinned_request_model_overrides_primary_catalog() {
        // The dynamic-routing directive (select_model / agent model_hint /
        // BrainRef::Strict) reaches the failover as a stamped `payload.model`.
        // On the primary/default slot it must win over the provider's static
        // catalog — otherwise the model the LLM explicitly chose is silently
        // discarded (the bug this guards).
        let primary = ScriptProvider::ok("anthropic");
        let fp = build(
            primary.clone(),
            vec![("anthropic", vec!["opus", "sonnet"])],
            vec![],
        );

        let msgs = [UnifiedMessage::user("hi")];
        let payload = RequestPayload::new(&msgs).with_model(Some("gpt-5".to_string()));
        let resp = fp.process(payload).await.unwrap();
        assert_eq!(resp.text_content(), "anthropic");
        // Dialed with the pinned model only — never the opus/sonnet catalog.
        assert_eq!(primary.models(), vec![Some("gpt-5".to_string())]);
    }

    #[tokio::test]
    async fn pinned_request_model_does_not_leak_into_fallback_catalog() {
        // The pinned model targets the *default* endpoint, not its cross-provider
        // safety net: when the primary fails over, the fallback walks its own
        // catalog, not the pinned model (which may belong to a different vendor).
        let primary = ScriptProvider::err("anthropic", "HTTP 429 too many requests");
        let fallback = ScriptProvider::ok("openai");
        let fp = build(
            primary.clone(),
            vec![("anthropic", vec!["opus"])],
            vec![FailoverNode::with_tier(
                "openai".to_string(),
                vec!["gpt-4o".to_string()],
                fallback.clone(),
                EndpointTier::Cloud,
            )],
        );

        let msgs = [UnifiedMessage::user("hi")];
        let payload = RequestPayload::new(&msgs).with_model(Some("gpt-5".to_string()));
        let resp = fp.process(payload).await.unwrap();
        assert_eq!(resp.text_content(), "openai");
        // Primary (Unknown slot) honoured the pin; fallback kept its own catalog.
        assert_eq!(primary.models(), vec![Some("gpt-5".to_string())]);
        assert_eq!(fallback.models(), vec![Some("gpt-4o".to_string())]);
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
        let fp = build(dead.clone(), vec![], vec![node("healthy", healthy.clone())]);

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

    // --- hard-guardrail: route mode gates an explicit cross-tier pin ---------

    /// Build a chain whose *primary* slot carries an explicit tier — the shape
    /// of a per-`provider_hint` / `select_model(provider=…)` override.
    fn build_pinned(
        primary: Arc<dyn AiProvider>,
        primary_tier: EndpointTier,
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
        .with_primary_tier(primary_tier)
    }

    #[tokio::test]
    async fn always_local_pinned_cloud_borrows_with_approval() {
        // An explicit cloud pin (the primary) under AlwaysLocal is no longer
        // silently allowed: it must pass the borrow-cloud approval. Approved →
        // the pin is honoured (dialed first), the local fallback untouched.
        let cloud = ScriptProvider::ok("openai");
        let local = ScriptProvider::ok("ollama");
        let approver = MockApprover::new(true);
        let fp = build_pinned(
            cloud.clone(),
            EndpointTier::Cloud,
            vec![tiered_node("ollama", local.clone(), EndpointTier::Local)],
            RouteMode::AlwaysLocal,
            true,
            Some(approver.clone()),
        );
        let msgs = [UnifiedMessage::user("hi")];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "openai");
        assert_eq!(approver.call_count(), 1);
        assert_eq!(local.call_count(), 0);
    }

    #[tokio::test]
    async fn always_local_pinned_cloud_denied_falls_to_local() {
        // Denied borrow-cloud → the cloud pin is skipped and the chain degrades
        // to the local fallback. The pin never reaches the wire.
        let cloud = ScriptProvider::ok("openai");
        let local = ScriptProvider::ok("ollama");
        let approver = MockApprover::new(false);
        let fp = build_pinned(
            cloud.clone(),
            EndpointTier::Cloud,
            vec![tiered_node("ollama", local.clone(), EndpointTier::Local)],
            RouteMode::AlwaysLocal,
            true,
            Some(approver.clone()),
        );
        let msgs = [UnifiedMessage::user("hi")];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "ollama");
        assert_eq!(approver.call_count(), 1);
        assert_eq!(cloud.call_count(), 0);
    }

    #[tokio::test]
    async fn always_local_pinned_cloud_no_escalation_skips_to_local_ungated() {
        // Escalation off: the cloud pin is dropped (Skip) with no approval
        // prompt at all — a hard wall, falling straight through to local.
        let cloud = ScriptProvider::ok("openai");
        let local = ScriptProvider::ok("ollama");
        let approver = MockApprover::new(false); // must not be consulted
        let fp = build_pinned(
            cloud.clone(),
            EndpointTier::Cloud,
            vec![tiered_node("ollama", local.clone(), EndpointTier::Local)],
            RouteMode::AlwaysLocal,
            false,
            Some(approver.clone()),
        );
        let msgs = [UnifiedMessage::user("hi")];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "ollama");
        assert_eq!(approver.call_count(), 0);
        assert_eq!(cloud.call_count(), 0);
    }

    #[tokio::test]
    async fn auto_mode_pinned_cloud_primary_used_directly() {
        // The guardrail is scoped to AlwaysLocal: in Auto an explicit cloud pin
        // is dialed directly, no approval consulted (regression guard).
        let cloud = ScriptProvider::ok("openai");
        let local = ScriptProvider::ok("ollama");
        let approver = MockApprover::new(false);
        let fp = build_pinned(
            cloud.clone(),
            EndpointTier::Cloud,
            vec![tiered_node("ollama", local.clone(), EndpointTier::Local)],
            RouteMode::Auto,
            false,
            Some(approver.clone()),
        );
        let msgs = [UnifiedMessage::user("hi")];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "openai");
        assert_eq!(approver.call_count(), 0);
        assert_eq!(local.call_count(), 0);
    }

    #[tokio::test]
    async fn always_local_unknown_primary_served_without_gate() {
        // The operator's configured default (the global chain, primary tier
        // Unknown) stays always-allowed under AlwaysLocal — only *pins* carry a
        // real tier. No approval is consulted for the default.
        let primary = ScriptProvider::ok("default");
        let approver = MockApprover::new(false);
        let fp = build_routed(
            primary.clone(),
            vec![],
            RouteMode::AlwaysLocal,
            true,
            Some(approver.clone()),
        );
        let msgs = [UnifiedMessage::user("hi")];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "default");
        assert_eq!(approver.call_count(), 0);
    }

    // --- load-balance integration -----------------------------------------

    #[tokio::test]
    async fn least_busy_orders_fallback_pool_by_in_flight() {
        use crate::config::types::ModelRouteConfig;
        // Primary fails; two equal cloud fallbacks. fb_a is pre-loaded with one
        // in-flight request, so the LeastBusy strategy must try fb_b (idle)
        // first — proving the live load registry shapes the candidate order.
        let primary = ScriptProvider::err("primary", "HTTP 429 rate limit");
        let fb_a = ScriptProvider::ok("fb_a");
        let fb_b = ScriptProvider::ok("fb_b");
        let stats = Arc::new(LoadStats::new());
        let handle = Arc::new(RouteHandle::from_config(&ModelRouteConfig {
            load_balance: LoadBalanceStrategy::LeastBusy,
            ..Default::default()
        }));
        let fp = FailoverProvider::new(
            Arc::new(StaticDefault::new(primary)),
            vec![
                tiered_node("fb_a", fb_a.clone(), EndpointTier::Cloud),
                tiered_node("fb_b", fb_b.clone(), EndpointTier::Cloud),
            ],
            HashMap::new(),
            FailoverHealth::default(),
            FailoverConfig::default(),
        )
        .with_route_live(handle)
        .with_load_stats(stats.clone());

        // Hold one in-flight request against fb_a for the duration of the call.
        let _busy = stats.begin("fb_a");

        let msgs = [UnifiedMessage::user("hi")];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "fb_b");
        // fb_b (idle) was preferred → fb_a never dialed.
        assert_eq!(fb_a.call_count(), 0);
    }

    #[tokio::test]
    async fn over_limit_provider_deprioritised_in_fallback_pool() {
        use crate::config::types::{ModelRouteConfig, ProviderRateLimit};
        // Primary fails; fb_a carries an rpm ceiling of 1 and is already at it,
        // so the over-limit gate must deprioritise it behind fb_b even under the
        // default Ordered strategy (configured order is fb_a, fb_b). Proves the
        // rate-limit gate shapes ordering without any non-Ordered strategy.
        let primary = ScriptProvider::err("primary", "HTTP 429 rate limit");
        let fb_a = ScriptProvider::ok("fb_a");
        let fb_b = ScriptProvider::ok("fb_b");
        let stats = Arc::new(LoadStats::new());
        let handle = Arc::new(RouteHandle::from_config(&ModelRouteConfig {
            rate_limits: [(
                "fb_a".to_string(),
                ProviderRateLimit {
                    rpm: Some(1),
                    tpm: None,
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        }));
        let fp = FailoverProvider::new(
            Arc::new(StaticDefault::new(primary)),
            vec![
                tiered_node("fb_a", fb_a.clone(), EndpointTier::Cloud),
                tiered_node("fb_b", fb_b.clone(), EndpointTier::Cloud),
            ],
            HashMap::new(),
            FailoverHealth::default(),
            FailoverConfig::default(),
        )
        .with_route_live(handle)
        .with_load_stats(stats.clone());

        // Saturate fb_a's rpm window (ceiling is 1). The guard's in-flight count
        // drops on `drop`, but the window request count persists this minute.
        drop(stats.begin("fb_a"));

        let msgs = [UnifiedMessage::user("hi")];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        // fb_b (fresh) preferred over the saturated fb_a → fb_a never dialed.
        assert_eq!(resp.text_content(), "fb_b");
        assert_eq!(fb_a.call_count(), 0);
    }

    #[tokio::test]
    async fn no_load_stats_is_byte_identical_ordering() {
        // Without a load registry the configured order holds even if a strategy
        // is set on the handle — balancing is inert until wired (regression).
        use crate::config::types::ModelRouteConfig;
        let primary = ScriptProvider::err("primary", "HTTP 429 rate limit");
        let fb_a = ScriptProvider::ok("fb_a");
        let fb_b = ScriptProvider::ok("fb_b");
        let handle = Arc::new(RouteHandle::from_config(&ModelRouteConfig {
            load_balance: LoadBalanceStrategy::RoundRobin,
            ..Default::default()
        }));
        let fp = FailoverProvider::new(
            Arc::new(StaticDefault::new(primary)),
            vec![
                tiered_node("fb_a", fb_a.clone(), EndpointTier::Cloud),
                tiered_node("fb_b", fb_b.clone(), EndpointTier::Cloud),
            ],
            HashMap::new(),
            FailoverHealth::default(),
            FailoverConfig::default(),
        )
        .with_route_live(handle); // no with_load_stats

        let msgs = [UnifiedMessage::user("hi")];
        let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
        // Configured order: fb_a first.
        assert_eq!(resp.text_content(), "fb_a");
    }
}
