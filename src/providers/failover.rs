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

use crate::error::{AlephError, ErrorClass, Result};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::llm_retry::{classify, classify_exhausted, RetryVerdict};
use crate::providers::{AiProvider, DefaultProviderHandle};
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

/// What to do after one failed `process()` attempt.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// Retry the same provider + model after `delay`.
    RetrySame(Duration),
    /// Advance to the next model of the same provider.
    NextModel,
    /// Trip this provider's circuit and advance to the next provider.
    NextProvider,
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

    match classify_exhausted(&msg) {
        // 413 — the harness context-compactor owns this recovery path.
        RetryVerdict::CompactAndRetry { .. } => Decision::Stop,
        RetryVerdict::Fallback { reason } => {
            if reason.starts_with("model not found") {
                Decision::NextModel
            } else if let (Some(delay), true) = (transient_delay, can_retry) {
                Decision::RetrySame(delay)
            } else {
                Decision::NextProvider
            }
        }
        // `classify_exhausted` never yields `Retry`; handled defensively.
        RetryVerdict::Retry { delay } if can_retry => Decision::RetrySame(delay),
        RetryVerdict::Retry { .. } => Decision::NextProvider,
        RetryVerdict::Fatal => {
            let explicit_bad_request = lower.contains("400")
                && (lower.contains("bad request") || lower.contains("invalid"));
            if !explicit_bad_request && err.class() == ErrorClass::Transient {
                if can_retry {
                    Decision::RetrySame(DEFAULT_TRANSIENT_DELAY)
                } else {
                    Decision::NextProvider
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
        }
    }

    /// Build the ordered candidate list for one request: the live primary
    /// first, then each fallback whose name differs from the primary's.
    fn candidates(&self) -> Vec<FailoverNode> {
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
        }];
        for fb in &self.fallbacks {
            if fb.name == primary_name {
                continue; // dedup: the primary slot already covers it
            }
            out.push(fb.clone());
        }
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
    async fn mark_unhealthy(&self, name: &str, error: String) {
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
                if st.failure_count >= CIRCUIT_OPEN_THRESHOLD {
                    st.circuit = CircuitState::Open;
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

            for (idx, cand) in candidates.into_iter().enumerate() {
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

                let mut tripped = false;
                'model: for model in models {
                    let mut attempt: u32 = 0;
                    loop {
                        let inner = RequestPayload {
                            messages: &messages,
                            system_prompt: system_prompt.as_deref(),
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
                                    tracing::warn!(
                                        provider = %cand.name, model = ?model, attempt,
                                        error = %e, "failover: transient, retrying in place",
                                    );
                                    tokio::time::sleep(delay).await;
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
                                Decision::NextProvider => {
                                    tracing::warn!(
                                        provider = %cand.name, error = %e,
                                        "failover: provider unavailable, advancing chain",
                                    );
                                    last_error = Some(e);
                                    tripped = true;
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

                if tripped {
                    let reason = last_error
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default();
                    self.mark_unhealthy(&cand.name, reason).await;
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

    fn supports_thinking(&self) -> bool {
        self.primary.current().supports_thinking()
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
        }
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
        assert_eq!(decide(&e, 0, 2), Decision::NextProvider);
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
        assert_eq!(decide(&e, 2, 2), Decision::NextProvider);
    }

    #[test]
    fn decide_typed_timeout_with_no_http_code_still_fails_over() {
        // `Timeout` Display is "Request timed out" — no HTTP keyword — but the
        // typed class is Transient, so the walk must still advance.
        let e = AlephError::Timeout { suggestion: None };
        assert!(matches!(decide(&e, 0, 2), Decision::RetrySame(_)));
        assert_eq!(decide(&e, 2, 2), Decision::NextProvider);
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
}
