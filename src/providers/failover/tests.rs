use super::decision::{decide, Decision, FailureKind};
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use super::*;
use crate::providers::message::UnifiedMessage;
use crate::providers::{DefaultProviderHandle, StaticDefault};
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
        // rust-doctor-disable-next-line
        self.seen_models.lock().unwrap().clone()
    }
}

impl AiProvider for ScriptProvider {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // rust-doctor-disable-next-line
        self.seen_models.lock().unwrap().push(payload.model.clone());
        let outcome = {
            // rust-doctor-disable-next-line unwrap-in-production
            let mut q = self.script.lock().unwrap();
            match q.pop_front() {
                Some(o) => {
                    // rust-doctor-disable-next-line
                    *self.last.lock().unwrap() = o.clone();
                    o
                }
                // rust-doctor-disable-next-line
                None => self.last.lock().unwrap().clone(),
            }
        };
        // rust-doctor-disable-next-line excessive-clone
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

/// A provider that has gone silent: every call yields the typed
/// `AlephError::Timeout` that both stream watchdogs raise once an idle window
/// has elapsed with no upstream bytes. `ScriptProvider` cannot stand in — it
/// only ever produces `AlephError::provider`, and the whole point of the
/// spent-window rule is that it reads the *typed variant*, not the wording.
struct SilentProvider {
    name: String,
    calls: AtomicUsize,
}

impl SilentProvider {
    fn new(name: &str) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_string(),
            calls: AtomicUsize::new(0),
        })
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AiProvider for SilentProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(AlephError::Timeout {
                suggestion: Some(
                    "Provider sent no response for 60s after the request was dispatched \
                     (time-to-first-byte timeout)."
                        .into(),
                ),
            })
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn color(&self) -> &str {
        "#000"
    }
}

/// Primary whose behavior-resolution fields are configurable, so the
/// failover wrapper's pass-through can be asserted.
struct BehaviorProvider {
    protocol: &'static str,
    behavior: Option<&'static str>,
}

impl AiProvider for BehaviorProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(async { Ok(ProviderResponse::text_only("primary".to_string())) })
    }
    fn name(&self) -> &str {
        "primary"
    }
    fn color(&self) -> &str {
        "#000"
    }
    fn protocol(&self) -> std::borrow::Cow<'_, str> {
        std::borrow::Cow::Borrowed(self.protocol)
    }
    fn model_behavior_override(&self) -> Option<std::borrow::Cow<'_, str>> {
        self.behavior.map(std::borrow::Cow::Borrowed)
    }
}

#[test]
fn failover_reports_live_primary_behavior() {
    let primary = Arc::new(BehaviorProvider {
        protocol: "anthropic",
        behavior: Some("kimi"),
    });
    let failover = build(primary, vec![], vec![]);
    assert_eq!(failover.protocol().as_ref(), "anthropic");
    assert_eq!(failover.model_behavior_override().as_deref(), Some("kimi"));
}

/// Assemble a `FailoverProvider` from a primary + fallback nodes.
fn build(
    primary: Arc<dyn AiProvider>,
    catalog: Vec<(&str, Vec<&str>)>,
    fallbacks: Vec<FailoverNode>,
) -> FailoverProvider {
    build_with_health(primary, catalog, fallbacks, FailoverHealth::default())
}

/// Same, with a caller-supplied breaker table, so a test can pre-open a circuit
/// (`FailoverHealth::open_for_test`) instead of first driving a whole failing
/// provider walk to trip it.
fn build_with_health(
    primary: Arc<dyn AiProvider>,
    catalog: Vec<(&str, Vec<&str>)>,
    fallbacks: Vec<FailoverNode>,
    health: FailoverHealth,
) -> FailoverProvider {
    let model_catalog = catalog
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.into_iter().map(String::from).collect()))
        .collect();
    FailoverProvider::new(
        Arc::new(StaticDefault::new(primary)),
        fallbacks,
        model_catalog,
        health,
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

/// A `DefaultProviderHandle` backed by a *mutable* provider set, so live
/// fallback derivation can be exercised — including a provider added after the
/// `FailoverProvider` was built (proving the derivation reads the registry each
/// turn, not a boot snapshot). Mirrors `MultiProviderRegistry`'s overrides.
struct LivePool {
    default: Arc<dyn AiProvider>,
    providers: Mutex<Vec<(String, Arc<dyn AiProvider>)>>,
}

impl LivePool {
    fn new(default: Arc<dyn AiProvider>, providers: Vec<(&str, Arc<dyn AiProvider>)>) -> Arc<Self> {
        Arc::new(Self {
            default,
            providers: Mutex::new(
                providers
                    .into_iter()
                    .map(|(n, p)| (n.to_string(), p))
                    .collect(),
            ),
        })
    }

    fn add(&self, name: &str, provider: Arc<dyn AiProvider>) {
        self.providers
            .lock()
            // rust-doctor-disable-next-line unwrap-in-production
            .unwrap()
            .push((name.to_string(), provider));
    }
}

impl DefaultProviderHandle for LivePool {
    fn current(&self) -> Arc<dyn AiProvider> {
        // rust-doctor-disable-next-line excessive-clone
        self.default.clone()
    }
    fn provider_names(&self) -> Vec<String> {
        self.providers
            .lock()
            // rust-doctor-disable-next-line unwrap-in-production
            .unwrap()
            .iter()
            // rust-doctor-disable-next-line excessive-clone
            .map(|(n, _)| n.clone())
            .collect()
    }
    fn provider_by_name(&self, name: &str) -> Option<Arc<dyn AiProvider>> {
        self.providers
            .lock()
            // rust-doctor-disable-next-line unwrap-in-production
            .unwrap()
            .iter()
            .find(|(n, _)| n == name)
            // rust-doctor-disable-next-line excessive-clone
            .map(|(_, p)| p.clone())
    }
}

#[tokio::test]
async fn live_derivation_serves_registry_fallback() {
    // No static fallbacks, but the live pool has a healthy sibling: the
    // primary's 429 fails over to the registry-derived fallback.
    let primary = ScriptProvider::err("primary", "HTTP 429 too many requests");
    let fb = ScriptProvider::ok("fb1");
    let pool = LivePool::new(
        // rust-doctor-disable-next-line excessive-clone
        primary.clone() as Arc<dyn AiProvider>,
        vec![
            ("primary", primary as Arc<dyn AiProvider>),
            ("fb1", fb as Arc<dyn AiProvider>),
        ],
    );
    let fp = FailoverProvider::new(
        pool,
        vec![],
        HashMap::new(),
        FailoverHealth::default(),
        FailoverConfig::default(),
    )
    .with_live_fallback_derivation();

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(resp.text_content(), "fb1");
}

#[tokio::test]
async fn live_derivation_picks_up_provider_added_at_runtime() {
    // The core promise of live derivation: a provider registered AFTER the
    // FailoverProvider was built (mirrors `providers.create` → registry) is
    // used on the very next turn, with no chain rebuild / restart.
    let primary = ScriptProvider::err("primary", "HTTP 429 too many requests");
    let pool = LivePool::new(
        // rust-doctor-disable-next-line excessive-clone
        primary.clone() as Arc<dyn AiProvider>,
        vec![("primary", primary as Arc<dyn AiProvider>)],
    );
    let fp = FailoverProvider::new(
        // rust-doctor-disable-next-line excessive-clone
        pool.clone(),
        vec![],
        HashMap::new(),
        FailoverHealth::default(),
        FailoverConfig::default(),
    )
    .with_live_fallback_derivation();

    let msgs = [UnifiedMessage::user("hi")];
    // Pool has only the (throttled) primary → nowhere to fail over yet.
    assert!(fp.process(RequestPayload::new(&msgs)).await.is_err());

    // Register a healthy provider at runtime → next turn uses it immediately.
    pool.add("added-at-runtime", ScriptProvider::ok("added-at-runtime"));
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(resp.text_content(), "added-at-runtime");
}

#[tokio::test]
async fn live_derivation_falls_back_to_static_when_handle_has_no_registry() {
    // `with_live_fallback_derivation()` is set, but the handle (StaticDefault)
    // exposes no live providers → the boot-time static snapshot is used,
    // byte-identical to non-live failover (the safe degrade for tests / a
    // non-registry boot path).
    let primary = ScriptProvider::err("primary", "HTTP 429 too many requests");
    let fb = ScriptProvider::ok("static-fb");
    let fp = build(primary, vec![], vec![node("static-fb", fb)]).with_live_fallback_derivation();

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(resp.text_content(), "static-fb");
}

#[tokio::test]
async fn explicit_chain_skips_providers_removed_from_live_registry() {
    // An explicit operator `[fallback_provider].chain` can outlive a provider
    // that is deleted at runtime. The chain must be filtered against the live
    // registry so the deleted entry is not attempted, otherwise the request
    // wastes retries on a dead provider before reaching a healthy sibling.
    let primary = ScriptProvider::err("primary", "HTTP 429 too many requests");
    let deleted_fb = ScriptProvider::ok("deleted-fb");
    let live_fb = ScriptProvider::ok("live-fb");
    let pool = LivePool::new(
        // rust-doctor-disable-next-line excessive-clone
        primary.clone() as Arc<dyn AiProvider>,
        vec![("live-fb", live_fb.clone() as Arc<dyn AiProvider>)],
    );
    let fp = FailoverProvider::new(
        pool,
        vec![node("deleted-fb", deleted_fb), node("live-fb", live_fb)],
        HashMap::new(),
        FailoverHealth::default(),
        FailoverConfig::default(),
    );

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(resp.text_content(), "live-fb");
}

// --- decide() unit tests ----------------------------------------------

/// `decide` for the common shape: a later candidate still remains in the
/// chain, so advancing is a real option. Named rather than a bare `true` at
/// two dozen call sites, where the flag would read as noise.
fn decide_in_chain(err: &AlephError, attempt: u32, max_retries: u32) -> Decision {
    decide(err, attempt, max_retries, true)
}

/// `decide` on the *terminal* candidate — a single-provider deployment, or the
/// last link of a chain. Advancing here means failing the request outright, so
/// every in-place budget must survive unchanged.
fn decide_terminal(err: &AlephError, attempt: u32, max_retries: u32) -> Decision {
    decide(err, attempt, max_retries, false)
}

#[test]
fn decide_bad_request_stops() {
    let e = AlephError::provider("HTTP 400 bad request: invalid param");
    assert_eq!(decide_in_chain(&e, 0, 2), Decision::Stop);
}

#[test]
fn decide_model_rate_limit_returns_rate_limited() {
    // A model-specific 429 (no account/quota markers) no longer trips the
    // whole provider at the decide() level: it returns RateLimited so
    // process() sidelines just this model and prefers a sibling before the
    // provider's circuit is considered. The server gave no Retry-After, so
    // the cooldown hint is None.
    let e = AlephError::provider("HTTP 429 too many requests");
    assert_eq!(decide_in_chain(&e, 0, 2), Decision::RateLimited(None));
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
        decide_in_chain(&e, 0, 2),
        Decision::RateLimited(Some(Duration::from_secs(42)))
    );
}

#[test]
fn decide_model_rate_limit_falls_back_to_body_retry_after() {
    // When the typed error carries no `suggestion`, the Retry-After stated in
    // the message body must still win over the blind default cooldown —
    // `classify_rate_limit` already parsed that same text into the reason
    // string, so the hint exists; only the decision discarded it.
    let e = AlephError::RateLimitError {
        message: "HTTP 429 too many requests. Retry after 30 seconds.".into(),
        suggestion: None,
    };
    assert_eq!(
        decide_in_chain(&e, 0, 2),
        Decision::RateLimited(Some(Duration::from_secs(30)))
    );
}

/// An account-scoped 429 is **not** propagated to the user, whatever
/// `classify_rate_limit`'s `Fatal` looks like from inside `llm_retry`.
///
/// Three doc comments said it was ("propagate to user", "propagate
/// immediately", "never reach the breaker"), and all three were wrong in the
/// same way: `Fatal` only says *a sibling model will not help*. `decide`'s
/// `Fatal` arm then reads the TYPED error, and both `RateLimitError` and
/// `ProviderError` are `ErrorClass::Transient` — so the attempt rides out the
/// in-place budget and then sheds the provider onto a sibling, whose account is
/// a different account. This anchors that wording.
#[test]
fn an_account_scoped_429_is_retried_then_sheds_the_provider() {
    let e = AlephError::RateLimitError {
        message: "HTTP 429 rate limit exceeded for your organization".into(),
        suggestion: None,
    };
    // Inside the budget: retried in place, at the plain transient delay (an
    // account limit gets no server hint and earns no deeper overload budget).
    assert_eq!(
        decide_in_chain(&e, 0, 2),
        Decision::RetrySame(Duration::from_millis(300))
    );
    // Budget spent: the chain advances and the breaker counts a strike. Not
    // `Stop`, and not `Permanent` — the account recovers.
    assert_eq!(
        decide_in_chain(&e, 2, 2),
        Decision::NextProvider(FailureKind::Transient)
    );
    // Same on the terminal candidate: only the typed-`Timeout` rule reads
    // position, so a 429 keeps its in-place budget in a single-provider setup.
    assert_eq!(
        decide_terminal(&e, 0, 2),
        Decision::RetrySame(Duration::from_millis(300))
    );
}

#[test]
fn decide_token_count_borrowing_400_digits_is_not_a_bad_request() {
    // `contains("400")` also fires inside a token count, and the Fatal arm used
    // to abort the whole walk (`Decision::Stop`) on such a transient error.
    // `has_status_code` confines the match to a real status token.
    let e = AlephError::provider("upstream hiccup: used 400123 tokens; invalid response");
    assert!(
        matches!(decide_in_chain(&e, 0, 2), Decision::RetrySame(_)),
        "a token count must not read as HTTP 400, got {:?}",
        decide_in_chain(&e, 0, 2)
    );
    // …while a genuine 400 still stops the walk immediately.
    let e = AlephError::provider("HTTP 400 Bad Request: invalid parameter");
    assert_eq!(decide_in_chain(&e, 0, 2), Decision::Stop);
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
        decide_in_chain(&e, 0, 2),
        Decision::RetrySame(Duration::from_secs(7))
    );
}

#[test]
fn decide_auth_advances_provider_as_permanent() {
    let e = AlephError::provider("HTTP 401 Unauthorized");
    assert_eq!(
        decide_in_chain(&e, 0, 2),
        Decision::NextProvider(FailureKind::Permanent)
    );
    let e = AlephError::provider("HTTP 403 Forbidden: invalid api key");
    assert_eq!(
        decide_in_chain(&e, 0, 2),
        Decision::NextProvider(FailureKind::Permanent)
    );
}

#[test]
fn decide_model_not_found_advances_model() {
    let e = AlephError::provider("HTTP 404 model gpt-9 not found");
    assert_eq!(decide_in_chain(&e, 0, 2), Decision::NextModel);
}

#[test]
fn decide_413_stops_for_compactor() {
    let e = AlephError::provider("HTTP 413 prompt is too long: 200000 tokens > 100000 maximum");
    assert_eq!(decide_in_chain(&e, 0, 2), Decision::Stop);
}

#[test]
fn decide_transient_retries_then_advances() {
    let e = AlephError::provider("connection reset by peer");
    assert!(matches!(decide_in_chain(&e, 0, 2), Decision::RetrySame(_)));
    assert!(matches!(decide_in_chain(&e, 1, 2), Decision::RetrySame(_)));
    assert_eq!(
        decide_in_chain(&e, 2, 2),
        Decision::NextProvider(FailureKind::Transient)
    );
}

#[test]
fn decide_transient_overload_429_gets_limited_retry_budget() {
    // The generic Anthropic server-throttle 429 ("please wait a moment")
    // gets one brief in-place retry so a transient spike has a chance to
    // clear, but we no longer ride it out for tens of seconds: a provider
    // that is consistently overloaded should fail fast in interactive chat.
    // Once the single overload retry is exhausted the failure escalates as a
    // PROVIDER-level transient (advance the chain) — it must NOT become a
    // per-model cooldown (`RateLimited`), which sidelined a perfectly healthy
    // model for a server-side transient spike (the `classify_exhausted`
    // overload arm used a narrower word list than `classify` and misread this
    // exact body as a model-specific rate limit).
    let e = AlephError::RateLimitError {
        message: "Anthropic API rate limited (429): We're receiving too many \
                  requests at the moment. Please wait a moment and try again."
            .into(),
        suggestion: None,
    };
    assert!(matches!(decide_in_chain(&e, 0, 2), Decision::RetrySame(_)));
    assert_eq!(
        decide_in_chain(&e, 1, 2),
        Decision::NextProvider(FailureKind::Transient)
    );
}

#[test]
fn decide_kimi_overloaded_429_fails_over_after_one_retry() {
    // Kimi (via Anthropic protocol) returns HTTP 429 with the body containing
    // "engine is currently overloaded". This must get exactly one in-place
    // retry before we advance to the next provider — the bug that made the UI
    // appear to "think forever" was that a high `max_retries` config allowed
    // many back-to-back retries on the same overloaded provider.
    let e = AlephError::provider(
        "Rate limit error: Anthropic API rate limited (429): \
         {\"error\":{\"type\":\"rate_limit_error\",\"message\":\"The engine is currently \
         overloaded, please try again later\"},\"type\":\"error\"}",
    );
    assert!(matches!(decide_in_chain(&e, 0, 2), Decision::RetrySame(_)));
    assert_eq!(
        decide_in_chain(&e, 1, 2),
        Decision::NextProvider(FailureKind::Transient)
    );
}

#[test]
fn decide_overload_429_budget_is_independent_of_max_retries() {
    // Even if the operator configures a large `max_retries`, an overload must
    // not ride it out: the budget stays at one extra attempt, then the chain
    // advances (provider-level transient — never a model cooldown).
    let e = AlephError::RateLimitError {
        message: "Anthropic API rate limited (429): We're receiving too many \
                  requests at the moment. Please wait a moment and try again."
            .into(),
        suggestion: None,
    };
    assert!(matches!(decide_in_chain(&e, 0, 10), Decision::RetrySame(_)));
    assert_eq!(
        decide_in_chain(&e, 1, 10),
        Decision::NextProvider(FailureKind::Transient)
    );
}

#[test]
fn decide_plain_network_transient_keeps_shallow_budget() {
    // A bare connection blip is NOT a server-asked-to-wait overload, so it
    // keeps the shallow `max_retries` budget — a sibling provider is the
    // better next bet than hammering a flaky socket. Guards the overload
    // budget from leaking into ordinary transient errors.
    let e = AlephError::provider("connection reset by peer");
    assert!(matches!(decide_in_chain(&e, 1, 2), Decision::RetrySame(_)));
    assert_eq!(
        decide_in_chain(&e, 2, 2),
        Decision::NextProvider(FailureKind::Transient)
    );
}

#[test]
fn decide_account_quota_429_excluded_from_deep_budget() {
    // An account/quota 429 classifies `Fatal` upstream, so even though its
    // body says "please wait a moment" it must NOT be lifted into the deep
    // overload budget: it advances the chain at `max_retries`, whereas a
    // genuine transient overload (above) is already in the cooldown path at
    // attempt 1.
    let e = AlephError::provider("429 account quota exceeded; please wait a moment");
    assert_eq!(
        decide_in_chain(&e, 2, 2),
        Decision::NextProvider(FailureKind::Transient)
    );
}

#[test]
fn decide_typed_timeout_advances_at_once_but_only_off_a_terminal_candidate() {
    // `Timeout` Display is "Request timed out" — no HTTP keyword — but the
    // typed class is Transient, so the walk must advance rather than stop.
    //
    // Both branches are pinned here on purpose. The rule is narrow: a typed
    // `Timeout` is the one transient error that proves a whole silence window
    // (TTFB / SSE gap / connect) was ALREADY spent on this attempt, so an
    // in-place retry buys a second full window against an endpoint that has
    // demonstrated silence. When a later candidate exists that is a bad bet and
    // the chain advances immediately — no retry at all. When this is the last
    // candidate, advancing means failing the request with zero attempts left,
    // so the ordinary `max_retries` in-place budget must survive untouched.
    // Asserting only the first half would let the rule silently widen into the
    // single-provider shape, which is the commonest personal-runtime setup.
    let e = AlephError::Timeout { suggestion: None };

    assert_eq!(
        decide_in_chain(&e, 0, 2),
        Decision::NextProvider(FailureKind::Transient),
        "a spent idle window must move to the next candidate on the first failure",
    );

    assert!(
        matches!(decide_terminal(&e, 0, 2), Decision::RetrySame(_)),
        "the terminal candidate keeps its in-place retry — there is nowhere to advance to",
    );
    assert!(matches!(decide_terminal(&e, 1, 2), Decision::RetrySame(_)));
    assert_eq!(
        decide_terminal(&e, 2, 2),
        Decision::NextProvider(FailureKind::Transient),
        "and once that budget is exhausted it still escalates as a transient strike",
    );
}

#[test]
fn decide_untyped_timed_out_wording_keeps_its_in_place_retry() {
    // The spent-window rule keys on the typed variant, never on the words
    // "timed out" — which also reach `classify` from untyped provider bodies
    // through `llm_retry`'s network word list. There nothing tells us a
    // watchdog window elapsed, so the ordinary retry budget still applies even
    // with a fallback waiting.
    let e = AlephError::provider("upstream proxy: the request timed out");
    assert!(
        matches!(decide_in_chain(&e, 0, 2), Decision::RetrySame(_)),
        "a provider *body* saying 'timed out' is not a watchdog verdict",
    );
}

// --- process() integration tests --------------------------------------

#[tokio::test]
async fn primary_success_skips_fallback() {
    let primary = ScriptProvider::ok("primary");
    let fallback = ScriptProvider::ok("fallback");
    let fp = build(primary, vec![], vec![node("fallback", fallback.clone())]);

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
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
    // rust-doctor-disable-next-line unwrap-in-production
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
        // rust-doctor-disable-next-line excessive-clone
        primary.clone(),
        vec![("anthropic", vec!["opus", "sonnet"])],
        vec![],
    );

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
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
        // rust-doctor-disable-next-line excessive-clone
        primary.clone(),
        vec![("anthropic", vec!["opus", "sonnet"])],
        vec![],
    );

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
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
        // rust-doctor-disable-next-line excessive-clone
        Arc::new(StaticDefault::new(primary.clone())),
        vec![],
        catalog,
        FailoverHealth::default(),
        FailoverConfig::default(),
    )
    .with_model_cooldown(ModelCooldown::default());

    let msgs = [UnifiedMessage::user("hi")];
    // Request 1: opus 429 → cool opus → sonnet serves.
    // rust-doctor-disable-next-line unwrap-in-production
    let _ = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    // Request 2: opus is still cooling → dial sonnet directly.
    // rust-doctor-disable-next-line unwrap-in-production
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

/// A `Decision::RateLimited` parks the whole PROVIDER and arms a circuit
/// strike, not just the one model.
///
/// The variant's doc said the opposite — "does not trip the provider circuit"
/// — while the walk's arm has always called `ProviderCooldown::cool` on the
/// candidate and set `tripped = Transient`. Sibling models stay live only
/// *within* the walk: a later model that succeeds retires both
/// (`a_successful_call_clears_the_provider_pacing_window` is that half). Here
/// nothing succeeds, so both effects survive the turn and shape the next one.
#[tokio::test]
async fn a_model_429_parks_the_provider_and_arms_a_strike() {
    let primary = ScriptProvider::new("anthropic", vec![Err("HTTP 429 too many requests".into())]);
    let pacing = ProviderCooldown::default();
    let health = FailoverHealth::default();
    let fp = build_with_health(
        primary as Arc<dyn AiProvider>,
        vec![("anthropic", vec!["opus"])],
        vec![],
        // rust-doctor-disable-next-line excessive-clone
        health.clone(),
    )
    .with_model_cooldown(ModelCooldown::default())
    // rust-doctor-disable-next-line excessive-clone
    .with_provider_cooldown(pacing.clone());

    let msgs = [UnifiedMessage::user("hi")];
    assert!(fp.process(RequestPayload::new(&msgs)).await.is_err());

    assert!(
        pacing.remaining("anthropic").await.is_some(),
        "the 429 must pace the whole provider, so the next turn waits instead \
         of earning a fresh 429"
    );
    let view = health.snapshot().await;
    let anthropic = view
        .iter()
        .find(|h| h.provider == "anthropic")
        .expect("the breaker must have seen this provider");
    assert_eq!(
        anthropic.failure_count, 1,
        "a ladder whose every model was rate-limited costs the provider a strike"
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
    // rust-doctor-disable-next-line unwrap-in-production
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
    // rust-doctor-disable-next-line unwrap-in-production
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
        // rust-doctor-disable-next-line excessive-clone
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
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(resp.text_content(), "openai");
    assert_eq!(primary.models(), vec![Some("gpt-4o".to_string())]);
}

#[tokio::test]
async fn pinned_request_model_overrides_primary_catalog() {
    // The explicit model directive (`select_model`, a model/request override, or
    // `BrainRef::Strict`) reaches the failover as a stamped `payload.model`.
    // On the primary/default slot it must win over the provider's static
    // catalog — otherwise the model the LLM explicitly chose is silently
    // discarded (the bug this guards).
    let primary = ScriptProvider::ok("anthropic");
    let fp = build(
        // rust-doctor-disable-next-line excessive-clone
        primary.clone(),
        vec![("anthropic", vec!["opus", "sonnet"])],
        vec![],
    );

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs).with_model(Some("gpt-5".to_string()));
    // rust-doctor-disable-next-line unwrap-in-production
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
        // rust-doctor-disable-next-line excessive-clone
        primary.clone(),
        vec![("anthropic", vec!["opus"])],
        vec![FailoverNode::new(
            "openai".to_string(),
            vec!["gpt-4o".to_string()],
            fallback.clone(),
        )],
    );

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs).with_model(Some("gpt-5".to_string()));
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(payload).await.unwrap();
    assert_eq!(resp.text_content(), "openai");
    // The primary honoured the pin; the fallback kept its own catalog.
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
        // rust-doctor-disable-next-line excessive-clone
        primary.clone(),
        vec![],
        vec![node("fallback", fallback.clone())],
    );

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(resp.text_content(), "primary");
    assert_eq!(primary.call_count(), 2);
    assert_eq!(fallback.call_count(), 0);
}

#[tokio::test]
async fn kimi_overloaded_fails_over_after_one_retry() {
    // End-to-end: a Kimi-shaped 429 overload with a healthy fallback must not
    // retry the primary more than once before failing over. This is the core
    // "stuck thinking" regression: previously a high `max_retries` kept the
    // request on the overloaded primary for many attempts.
    let primary = ScriptProvider::err(
        "kimi",
        "Rate limit error: Anthropic API rate limited (429): \
         {\"error\":{\"type\":\"rate_limit_error\",\"message\":\"The engine is currently \
         overloaded, please try again later\"},\"type\":\"error\"}",
    );
    let fallback = ScriptProvider::ok("fallback");
    let fp = build(
        // rust-doctor-disable-next-line excessive-clone
        primary.clone(),
        vec![],
        vec![node("fallback", fallback.clone())],
    );

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(resp.text_content(), "fallback");
    // Initial attempt + one overload retry.
    assert_eq!(primary.call_count(), 2);
    assert_eq!(fallback.call_count(), 1);
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
    // rust-doctor-disable-next-line unwrap-in-production
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
    // rust-doctor-disable-next-line excessive-clone
    let fp = build(dead.clone(), vec![], vec![node("healthy", healthy.clone())]);

    let msgs = [UnifiedMessage::user("hi")];
    // First request: dead is dialed once, trips its circuit, healthy serves.
    // rust-doctor-disable-next-line unwrap-in-production
    let _ = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(dead.call_count(), 1);
    // Second request: dead's circuit is open and a later candidate exists,
    // so it is skipped — call_count stays at 1.
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(resp.text_content(), "healthy");
    assert_eq!(dead.call_count(), 1);
}

#[tokio::test]
async fn a_silent_primary_reaches_the_fallback_within_one_idle_window() {
    // A black-holing endpoint (proxy accepts the connection and never answers)
    // surfaces as a typed `Timeout` only AFTER a full TTFB/idle window has been
    // paid. Re-dialing it in place paid that window again, twice more, before
    // the chain moved — long enough for a configured per-turn watchdog to kill
    // the run first, so the healthy fallback was never reached, no strike was
    // ever recorded, and the dead endpoint's circuit remained closed.
    let silent = SilentProvider::new("silent");
    let fb = ScriptProvider::ok("fb");
    // rust-doctor-disable-next-line excessive-clone
    let fp = build(silent.clone(), vec![], vec![node("fb", fb.clone())]);

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(resp.text_content(), "fb");
    assert_eq!(
        silent.call_count(),
        1,
        "the spent window must not be paid a second time in place",
    );
    assert_eq!(fb.call_count(), 1);

    // …and the attempt was charged to the breaker, so the silent endpoint is
    // shed on the ordinary schedule instead of being re-dialed every turn.
    // One walk = one strike, so the remaining walks reach the threshold.
    for _ in 1..CIRCUIT_OPEN_THRESHOLD {
        // rust-doctor-disable-next-line unwrap-in-production
        let _ = fp.process(RequestPayload::new(&msgs)).await;
    }
    assert!(fp.circuit_open("silent").await);
    assert_eq!(silent.call_count(), CIRCUIT_OPEN_THRESHOLD as usize);
}

#[tokio::test]
async fn a_lone_silent_provider_still_spends_its_in_place_retries() {
    // The other direction of the same gate: with no later candidate, advancing
    // the chain means failing the request with zero attempts left. A
    // single-provider deployment — the commonest personal-runtime shape — must
    // keep the full `max_retries` in-place budget on a timeout, exactly as it
    // did before the spent-window rule existed.
    let solo = SilentProvider::new("solo");
    // rust-doctor-disable-next-line excessive-clone
    let fp = build(solo.clone(), vec![], vec![]);

    let msgs = [UnifiedMessage::user("hi")];
    assert!(fp.process(RequestPayload::new(&msgs)).await.is_err());
    assert_eq!(
        solo.call_count(),
        FailoverConfig::default().max_retries as usize + 1,
        "a lone provider keeps its in-place retries; the rule must not widen here",
    );
}

#[tokio::test]
async fn lone_candidate_attempted_even_with_open_circuit() {
    // A single provider that keeps failing must still be retried after
    // its circuit opens — there is nowhere else to fail over to.
    let primary = ScriptProvider::err("solo", "HTTP 429 rate limit");
    // rust-doctor-disable-next-line excessive-clone
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
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(resp.text_content(), "anthropic");
    assert_eq!(dup.call_count(), 0);
}

// ===========================================================================
// Slot semantics: an explicitly pinned model belongs to the slot the caller
// chose, and to no other.
// ===========================================================================

#[tokio::test]
async fn pinned_model_reaches_the_primary_slot() {
    // An explicit model on the primary slot replaces its configured catalog.
    // Fallback slots still use their own models if that primary fails.
    let primary = ScriptProvider::ok("anthropic");
    let fp = build(
        // rust-doctor-disable-next-line excessive-clone
        primary.clone() as Arc<dyn AiProvider>,
        vec![("anthropic", vec!["claude-sonnet-5", "claude-haiku-5"])],
        vec![],
    );

    let msgs = [UnifiedMessage::user("hi")];
    let mut payload = RequestPayload::new(&msgs);
    payload.model = Some("claude-opus-5".to_string());
    // rust-doctor-disable-next-line unwrap-in-production
    fp.process(payload).await.unwrap();
    assert_eq!(
        primary.models(),
        vec![Some("claude-opus-5".to_string())],
        "the pinned model must reach the wire, not the catalog head"
    );
}

#[tokio::test]
async fn a_fallback_is_never_dialed_with_the_primarys_pinned_model() {
    // A registry-derived fallback uses its own default model when it has no
    // catalog. The primary's explicit model must never leak to another provider.
    let primary = ScriptProvider::err("primary", "HTTP 429 too many requests");
    let fb = ScriptProvider::ok("fb1");
    let pool = LivePool::new(
        // rust-doctor-disable-next-line excessive-clone
        primary.clone() as Arc<dyn AiProvider>,
        vec![
            ("primary", primary as Arc<dyn AiProvider>),
            // rust-doctor-disable-next-line excessive-clone
            ("fb1", fb.clone() as Arc<dyn AiProvider>),
        ],
    );
    let fp = FailoverProvider::new(
        pool,
        vec![],
        HashMap::new(),
        FailoverHealth::default(),
        FailoverConfig::default(),
    )
    .with_live_fallback_derivation();

    let msgs = [UnifiedMessage::user("hi")];
    let mut payload = RequestPayload::new(&msgs);
    payload.model = Some("kimi-k2.6".to_string());
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(payload).await.unwrap();
    assert_eq!(resp.text_content(), "fb1");
    assert_eq!(
        fb.models(),
        vec![None],
        "a fallback with no catalog gets its own default, never the primary's model"
    );
}

// ===========================================================================
// Chain membership is described once (`effective_fallback_names`).
// ===========================================================================

#[test]
fn membership_matches_how_the_chain_was_assembled() {
    let live = vec!["primary".to_string(), "a".to_string(), "b".to_string()];
    let configured = vec!["b".to_string(), "gone".to_string()];

    // No live registry → the configured order verbatim.
    assert_eq!(
        effective_fallback_names(&[], "primary", &configured, false),
        vec!["b".to_string(), "gone".to_string()]
    );
    // Auto-derived → everything registered, minus the primary.
    assert_eq!(
        effective_fallback_names(&live, "primary", &configured, true),
        vec!["a".to_string(), "b".to_string()]
    );
    // Explicit chain → operator order, minus entries no longer registered.
    assert_eq!(
        effective_fallback_names(&live, "primary", &configured, false),
        vec!["b".to_string()]
    );
}

// ===========================================================================
// Streaming seam: the chain is in the middle of every production stack, so if
// it does not carry the sink, nothing downstream of it can stream.
// ===========================================================================

/// Provider that emits real deltas, then optionally fails.
struct StreamingProvider {
    name: String,
    chunks: Vec<&'static str>,
    fail_after_stream: Option<String>,
    /// Reports an in-band fault (`ProviderDelta::Error`) after the chunks and
    /// still returns `Ok` — exactly the shape `HttpProvider::execute_once`
    /// produces when a provider faults mid-stream with content already sent.
    report_error: Option<String>,
    /// Streams `ToolCallStart` + `ToolCallArgDelta` INSTEAD of text, then
    /// fails — the shape `HttpProvider` produces on a truncated tool call.
    /// Content reached the sink (so the walk is terminal) but nothing reached
    /// the user's transcript (so the marker must stay off).
    tool_call_only: bool,
    calls: AtomicUsize,
}

impl StreamingProvider {
    fn ok(name: &str, chunks: Vec<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_string(),
            chunks,
            fail_after_stream: None,
            report_error: None,
            tool_call_only: false,
            calls: AtomicUsize::new(0),
        })
    }

    fn fails_mid_stream(name: &str, chunks: Vec<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_string(),
            chunks,
            fail_after_stream: Some("HTTP 429 too many requests".to_string()),
            report_error: None,
            tool_call_only: false,
            calls: AtomicUsize::new(0),
        })
    }

    /// Emits `chunks`, then an in-band `ProviderDelta::Error`, then returns
    /// `Ok(partial)` carrying the fault on `provider_error`.
    fn emits_then_reports_error(name: &str, chunks: Vec<&'static str>, msg: &str) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_string(),
            chunks,
            fail_after_stream: None,
            report_error: Some(msg.to_string()),
            tool_call_only: false,
            calls: AtomicUsize::new(0),
        })
    }

    /// Streams only tool-call deltas, then fails — a truncated tool call.
    fn fails_mid_tool_call(name: &str) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_string(),
            chunks: Vec::new(),
            fail_after_stream: Some("Request timed out".to_string()),
            report_error: None,
            tool_call_only: true,
            calls: AtomicUsize::new(0),
        })
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AiProvider for StreamingProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        // rust-doctor-disable-next-line excessive-clone
        let name = self.name.clone();
        // rust-doctor-disable-next-line excessive-clone
        let fail = self.fail_after_stream.clone();
        Box::pin(async move {
            match fail {
                Some(msg) => Err(AlephError::provider(msg)),
                None => Ok(ProviderResponse::text_only(name)),
            }
        })
    }

    fn execute_streaming_dyn<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
        sink: &'a dyn crate::providers::DeltaSink,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.tool_call_only {
                sink.on_delta(&crate::providers::ProviderDelta::ToolCallStart {
                    id: "t1".to_string(),
                    name: "file_write".to_string(),
                    signature: None,
                })
                .await;
                sink.on_delta(&crate::providers::ProviderDelta::ToolCallArgDelta {
                    id: "t1".to_string(),
                    delta: "{\"path\":\"big".to_string(),
                })
                .await;
            }
            for c in &self.chunks {
                sink.on_delta(&crate::providers::ProviderDelta::TextDelta(
                    (*c).to_string(),
                ))
                .await;
            }
            if let Some(msg) = &self.report_error {
                // rust-doctor-disable-next-line excessive-clone
                sink.on_delta(&crate::providers::ProviderDelta::Error(msg.clone()))
                    .await;
                return Ok(ProviderResponse {
                    text: Some(self.chunks.concat()),
                    // rust-doctor-disable-next-line excessive-clone
                    provider_error: Some(msg.clone()),
                    ..Default::default()
                });
            }
            match &self.fail_after_stream {
                Some(msg) => Err(AlephError::provider(msg.clone())),
                // rust-doctor-disable-next-line excessive-clone
                None => Ok(ProviderResponse::text_only(self.name.clone())),
            }
        })
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.name
    }
    fn color(&self) -> &str {
        "#000"
    }
}

/// Collects every text delta it is handed.
#[derive(Default)]
struct RecordingSink(Mutex<Vec<String>>);

impl RecordingSink {
    fn text(&self) -> String {
        // rust-doctor-disable-next-line unwrap-in-production
        self.0.lock().unwrap().join("")
    }
}

#[async_trait::async_trait]
impl crate::providers::DeltaSink for RecordingSink {
    async fn on_delta(&self, delta: &crate::providers::ProviderDelta) {
        if let crate::providers::ProviderDelta::TextDelta(t) = delta {
            // rust-doctor-disable-next-line unwrap-in-production
            self.0.lock().unwrap().push(t.clone());
        }
    }
}

#[tokio::test]
async fn the_chain_reports_the_streaming_capability_of_the_slot_it_dials() {
    // The harness gates live streaming on this answer. `FailoverProvider`
    // implemented neither this nor `execute_streaming_dyn`, so the gate — which
    // used to ask `as_http_provider()` — was false on every production stack
    // and `stream_llm_call` was unreachable code.
    let streaming = build(
        StreamingProvider::ok("p", vec!["a"]) as Arc<dyn AiProvider>,
        vec![],
        vec![],
    );
    assert!(streaming.supports_streaming());

    let plain = build(
        ScriptProvider::ok("p") as Arc<dyn AiProvider>,
        vec![],
        vec![],
    );
    assert!(!plain.supports_streaming());
}

#[tokio::test]
async fn the_chain_forwards_live_deltas_from_the_serving_candidate() {
    let primary = StreamingProvider::ok("p", vec!["hel", "lo"]);
    let fp = build(primary as Arc<dyn AiProvider>, vec![], vec![]);
    let sink = RecordingSink::default();

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    fp.execute_streaming_dyn(RequestPayload::new(&msgs), &sink)
        .await
        .unwrap();
    assert_eq!(sink.text(), "hello");
}

#[tokio::test]
async fn a_non_streaming_candidate_still_delivers_its_answer_to_the_sink() {
    // The trait default replays rather than dropping the sink. Without that,
    // a caller which suppressed its own once-per-turn emit (the harness does)
    // would show the user nothing at all whenever the serving provider could
    // not stream.
    let fp = build(
        ScriptProvider::ok("plain") as Arc<dyn AiProvider>,
        vec![],
        vec![],
    );
    let sink = RecordingSink::default();

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    fp.execute_streaming_dyn(RequestPayload::new(&msgs), &sink)
        .await
        .unwrap();
    assert_eq!(sink.text(), "plain");
}

#[tokio::test]
async fn a_failure_after_partial_output_is_terminal_instead_of_restarting() {
    // The user has already seen text. Advancing the chain would append a second
    // answer to a half-written one, so the error surfaces instead — even though
    // a 429 would normally fail over.
    let primary = StreamingProvider::fails_mid_stream("p", vec!["partial "]);
    let fb = ScriptProvider::ok("fb");
    let fp = build(
        primary as Arc<dyn AiProvider>,
        vec![],
        // rust-doctor-disable-next-line excessive-clone
        vec![node("fb", fb.clone() as Arc<dyn AiProvider>)],
    );

    let msgs = [UnifiedMessage::user("hi")];
    assert!(!fp.circuit_open("p").await);
    for _ in 0..CIRCUIT_OPEN_THRESHOLD {
        let sink = RecordingSink::default();
        let out = fp
            .execute_streaming_dyn(RequestPayload::new(&msgs), &sink)
            .await;
        // rust-doctor-disable-next-line unwrap-in-production
        let err = out.expect_err("must not silently restart on another candidate");
        assert_eq!(sink.text(), "partial ");
        // The gateway's outer loop reads `Display`, so the fact that a half
        // answer is already on screen has to be *in* the rendered message —
        // the 429 wording alone would read as retryable up there.
        assert!(
            err.to_string().contains(PARTIAL_OUTPUT_EMITTED),
            "the terminal verdict must survive Display: {err}"
        );
        assert_eq!(fb.call_count(), 0, "the fallback must not double-answer");
    }

    // A provider whose proxy cuts every long stream used to stay
    // `circuit: closed, failure_count: 0` forever — it kept leading every walk
    // and the prober, which only dials open circuits, never saw it. Assert the
    // effect (the breaker), then that the walk acts on it: the next call is
    // served by the fallback with `p` sidelined.
    assert!(
        fp.circuit_open("p").await,
        "a post-emission failure must count against the provider's circuit"
    );
    let sink = RecordingSink::default();
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp
        .execute_streaming_dyn(RequestPayload::new(&msgs), &sink)
        .await
        .expect("the sidelined provider must yield to the fallback");
    assert_eq!(resp.text_content(), "fb");
    assert_eq!(fb.call_count(), 1);
}

/// The negative half of the marker's gate — without it nothing pins that
/// `PARTIAL_OUTPUT_EMITTED` does not over-fire.
///
/// A truncated tool call reaches the walk with `EmissionGuard::has_emitted`
/// already latched (its deltas were forwarded), so it is chain-terminal exactly
/// like a cut answer. But the user's screen is still blank — the only
/// production sink drops every non-text/thinking delta — so the harm the marker
/// exists to prevent (a second answer landing under a visible first one) cannot
/// occur, and the gateway's fresh attempt is the correct recovery for precisely
/// the case the site's own diagnostic names: a large file write crossing a
/// proxy timeout.
#[tokio::test]
async fn a_tool_call_only_cut_stays_chain_terminal_without_claiming_the_user_saw_anything() {
    let primary = StreamingProvider::fails_mid_tool_call("p");
    let fb = ScriptProvider::ok("fb");
    let fp = build(
        primary as Arc<dyn AiProvider>,
        vec![],
        // rust-doctor-disable-next-line excessive-clone
        vec![node("fb", fb.clone() as Arc<dyn AiProvider>)],
    );
    let sink = RecordingSink::default();

    let msgs = [UnifiedMessage::user("hi")];
    let out = fp
        .execute_streaming_dyn(RequestPayload::new(&msgs), &sink)
        .await;
    // rust-doctor-disable-next-line unwrap-in-production
    let err = out.expect_err("content reached the sink, so the chain must not advance");

    // Terminal for the chain — the wide bit is unchanged by the split.
    assert_eq!(fb.call_count(), 0, "the walk must not advance the chain");
    assert!(sink.text().is_empty(), "no text was ever streamed");

    // ...but the narrow bit is false, so the marker must be absent and the
    // provider's own retryable wording must survive intact for the gateway.
    assert!(
        !err.to_string().contains(PARTIAL_OUTPUT_EMITTED),
        "nothing user-visible was shown; the marker must not over-fire: {err}"
    );
    assert!(
        err.to_string().contains("timed out"),
        "the original diagnostic must reach the gateway unwrapped: {err}"
    );
}

#[tokio::test]
async fn a_failure_before_any_output_still_fails_over_while_streaming() {
    // The guard must not turn streaming into "no failover": nothing was
    // emitted, so the ordinary chain walk applies.
    let primary = StreamingProvider::fails_mid_stream("p", vec![]);
    let fb = ScriptProvider::ok("fb");
    let fp = build(
        primary as Arc<dyn AiProvider>,
        vec![],
        // rust-doctor-disable-next-line excessive-clone
        vec![node("fb", fb.clone() as Arc<dyn AiProvider>)],
    );
    let sink = RecordingSink::default();

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp
        .execute_streaming_dyn(RequestPayload::new(&msgs), &sink)
        .await
        .unwrap();
    assert_eq!(resp.text_content(), "fb");
    assert_eq!(
        sink.text(),
        "fb",
        "the replayed answer still reaches the sink"
    );
}

#[tokio::test]
async fn an_in_stream_provider_error_after_content_is_not_a_healthy_success() {
    // A provider that emits a little text and then reports a fault in-band used
    // to be indistinguishable from a clean answer: the collector drops the
    // `Error` delta, `execute_once` only promotes it to `Err` when *nothing*
    // came through, and the walk's `Ok` arm then marked the provider healthy.
    // A provider failing this way on every request stayed `circuit: closed,
    // failure_count: 0` forever. The fault now rides on `provider_error` and
    // earns the same strike a pre-emission failure would have.
    let primary = StreamingProvider::emits_then_reports_error(
        "p",
        vec!["par", "tial"],
        "upstream connection error",
    );
    // rust-doctor-disable-next-line excessive-clone
    let fp = build(primary.clone() as Arc<dyn AiProvider>, vec![], vec![]);

    let msgs = [UnifiedMessage::user("hi")];
    assert!(!fp.circuit_open("p").await);
    for _ in 0..CIRCUIT_OPEN_THRESHOLD {
        let sink = RecordingSink::default();
        // rust-doctor-disable-next-line unwrap-in-production
        let resp = fp
            .execute_streaming_dyn(RequestPayload::new(&msgs), &sink)
            .await
            .unwrap();
        // The user still gets what was already streamed — the strike is
        // bookkeeping, not a change of the request outcome.
        assert_eq!(resp.text_content(), "partial");
        assert_eq!(sink.text(), "partial");
    }
    assert_eq!(primary.call_count(), CIRCUIT_OPEN_THRESHOLD as usize);
    assert!(
        fp.circuit_open("p").await,
        "an in-stream fault must count against the provider's circuit"
    );
}

#[tokio::test(start_paused = true)]
async fn an_in_stream_provider_error_leaves_the_pacing_window_parked() {
    // `pc.clear` retires the 429 pacing window because "we just went through
    // it and succeeded". A faulted turn is not that evidence, so the window
    // must survive — otherwise a provider that 429s and then faults mid-stream
    // is un-paced by its own failure and re-dialed at full rate next turn.
    // Asserting the *effect* (`remaining` still `Some`), not the call.
    let primary = StreamingProvider::emits_then_reports_error("p", vec!["hi"], "overloaded");
    let cooldown = ProviderCooldown::default();
    cooldown
        .cool("p", std::time::Duration::from_secs(600))
        .await;

    let fp = build(primary as Arc<dyn AiProvider>, vec![], vec![])
        // rust-doctor-disable-next-line excessive-clone
        .with_provider_cooldown(cooldown.clone());

    let msgs = [UnifiedMessage::user("hi")];
    let sink = RecordingSink::default();
    // The lone candidate is paced (a virtual sleep under `start_paused`) and
    // then dialed.
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp
        .execute_streaming_dyn(RequestPayload::new(&msgs), &sink)
        .await
        .unwrap();
    assert_eq!(resp.text_content(), "hi");
    assert!(
        cooldown.remaining("p").await.is_some(),
        "a faulted turn must not retire the provider's rate-pacing window"
    );
}

// ===========================================================================
// Health as a routing signal, not just a mid-walk skip.
// ===========================================================================

#[tokio::test]
async fn a_cooling_provider_is_skipped_while_a_healthy_sibling_remains() {
    // Pacing exists so a single paid primary is not bounced to a fallback on
    // every 429. But it used to *sleep* on the parked candidate even when a
    // healthy one was next in the chain, blocking the turn for up to two
    // minutes to insist on a provider that is throttled anyway. The rule now
    // mirrors the circuit breaker's: skip while a later candidate remains.
    let primary = ScriptProvider::ok("primary");
    let fb = ScriptProvider::ok("fb");
    let cooldown = ProviderCooldown::default();
    cooldown
        .cool("primary", std::time::Duration::from_secs(90))
        .await;

    let fp = build(
        // rust-doctor-disable-next-line excessive-clone
        primary.clone() as Arc<dyn AiProvider>,
        vec![],
        vec![node("fb", fb as Arc<dyn AiProvider>)],
    )
    .with_provider_cooldown(cooldown);

    let msgs = [UnifiedMessage::user("hi")];
    let started = Instant::now();
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(resp.text_content(), "fb");
    assert_eq!(
        primary.call_count(),
        0,
        "the parked primary must not be dialed"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the turn must not block on the pacing window"
    );
}

#[tokio::test]
async fn a_cooling_last_resort_is_still_waited_out() {
    // With nothing else to try, waiting is right: the window is short relative
    // to the value of keeping the operator's only provider in use. Capped so a
    // turn never blocks unboundedly.
    let primary = ScriptProvider::ok("solo");
    let cooldown = ProviderCooldown::default();
    cooldown
        .cool("solo", std::time::Duration::from_millis(150))
        .await;

    let fp = build(
        // rust-doctor-disable-next-line excessive-clone
        primary.clone() as Arc<dyn AiProvider>,
        vec![],
        vec![],
    )
    .with_provider_cooldown(cooldown);

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(resp.text_content(), "solo");
    assert_eq!(primary.call_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_successful_call_clears_the_provider_pacing_window() {
    // A model-scoped 429 parks the whole provider. When a sibling model (or a
    // later turn) then answers, the park is stale evidence: the request just
    // went through the window it claims to be protecting. Before this, the
    // provider stayed parked and the next turn deferred it.
    //
    // Time is paused so the walk's "last candidate waits out its window" sleep
    // is free. The window itself is tracked on `std::time::Instant`, which
    // tokio's clock does not touch — so it is still minutes from expiring when
    // the assertion runs, and only the clear can explain its absence.
    let primary = ScriptProvider::ok("primary");
    let pacing = ProviderCooldown::default();
    pacing
        .cool("primary", std::time::Duration::from_secs(600))
        .await;
    assert!(pacing.remaining("primary").await.is_some());

    let fp = FailoverProvider::new(
        Arc::new(StaticDefault::new(primary)),
        vec![],
        HashMap::new(),
        FailoverHealth::default(),
        FailoverConfig::default(),
    )
    // rust-doctor-disable-next-line excessive-clone
    .with_provider_cooldown(pacing.clone());

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(resp.text_content(), "primary");
    assert!(
        pacing.remaining("primary").await.is_none(),
        "a completed call must retire the pacing window it just passed through"
    );
}

// =============================================================================
// Route witness — the walk reporting which endpoint actually answered
// =============================================================================
//
// These pin the seam that replaced the old parallel health table on
// `MultiProviderRegistry`. That table *predicted* a candidate before the
// request, the prediction never reached the wire, and it was the sole producer
// of the `is_fallback` flag every user-visible fallback notice gates on — so a
// real migration lit nothing. The walk is the only honest source; if this seam
// breaks, the banner goes quiet again, silently. Hence tests, not trust.

/// Session keys are namespaced per test: the witness map is process-global and
/// these run concurrently with everything else in the binary.
fn witness_payload<'a>(msgs: &'a [UnifiedMessage], session: &str) -> RequestPayload<'a> {
    let mut meta = HashMap::new();
    meta.insert("session_id".to_string(), session.to_string());
    RequestPayload::new(msgs).with_metadata(Some(meta))
}

#[tokio::test]
async fn a_run_served_entirely_by_the_fallback_still_reads_as_a_migration() {
    let session = "agent:witness-test:provider-migration";
    crate::providers::route_witness::clear(session);

    let primary = ScriptProvider::err("primary", "HTTP 429 too many requests");
    let fb = ScriptProvider::ok("fallback");
    let fp = build(primary, vec![], vec![node("fallback", fb)]);

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(witness_payload(&msgs, session)).await.unwrap();
    assert_eq!(resp.text_content(), "fallback");

    let w = crate::providers::route_witness::take(session)
        .expect("a successful dial must be witnessed");
    // The commonest migration of all: the primary is down for the whole run, so
    // every *success* is already on the fallback. `first` anchors on the first
    // ATTEMPT, which is why this reads as a deviation instead of looking clean.
    assert_eq!(w.first.provider, "primary");
    assert_eq!(w.served.provider, "fallback");
    assert!(
        w.deviated(),
        "a run served entirely by the fallback is the case the notice exists for"
    );
}

#[tokio::test]
async fn a_later_turn_falling_over_deviates_from_the_first_turn() {
    // The shape the banner exists for: turn 1 is served by the primary, a later
    // turn migrates. `first` is pinned at turn 1, `served` follows the latest.
    let session = "agent:witness-test:later-turn";
    crate::providers::route_witness::clear(session);

    let primary = ScriptProvider::new(
        "primary",
        vec![Ok(()), Err("HTTP 429 too many requests".to_string())],
    );
    let fb = ScriptProvider::ok("fallback");
    let fp = build(primary, vec![], vec![node("fallback", fb)]);

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let first = fp.process(witness_payload(&msgs, session)).await.unwrap();
    assert_eq!(first.text_content(), "primary");
    // rust-doctor-disable-next-line unwrap-in-production
    let second = fp.process(witness_payload(&msgs, session)).await.unwrap();
    assert_eq!(second.text_content(), "fallback");

    let w = crate::providers::route_witness::take(session).expect("witnessed");
    assert_eq!(w.first.provider, "primary");
    assert_eq!(w.served.provider, "fallback");
    assert!(
        w.deviated(),
        "a run that ended elsewhere must read as deviated"
    );
}

#[tokio::test]
async fn the_witness_records_the_model_the_walk_actually_asked_for() {
    // A model-level walk within one provider: the endpoint is unchanged but the
    // user did not get the model the walk first chose, which still counts.
    let session = "agent:witness-test:model-walk";
    crate::providers::route_witness::clear(session);

    let primary = ScriptProvider::new(
        "primary",
        vec![Ok(()), Err("HTTP 404 model not found".to_string()), Ok(())],
    );
    let fp = build(
        primary,
        vec![("primary", vec!["model-a", "model-b"])],
        vec![],
    );

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let _ = fp.process(witness_payload(&msgs, session)).await.unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    let _ = fp.process(witness_payload(&msgs, session)).await.unwrap();

    let w = crate::providers::route_witness::take(session).expect("witnessed");
    assert_eq!(w.first.model_id(), Some("model-a"));
    assert_eq!(w.served.model_id(), Some("model-b"));
    assert!(
        w.deviated(),
        "a sibling-model migration is still a deviation"
    );
}

#[tokio::test]
async fn a_payload_without_a_session_id_is_simply_not_witnessed() {
    // Non-gateway callers build payloads without metadata. They must not panic,
    // and must not write under some invented key.
    let primary = ScriptProvider::ok("primary");
    let fp = build(primary, vec![], vec![]);

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(resp.text_content(), "primary");
}

#[tokio::test]
async fn the_nested_chain_sentinel_never_names_itself_as_the_endpoint() {
    // An explicit provider route is `[pin, <the whole global chain>]`.
    // The sentinel is not an endpoint; recording it would publish a provider
    // name the operator never configured. The inner chain speaks for itself.
    let session = "agent:witness-test:nested";
    crate::providers::route_witness::clear(session);

    let inner_primary = ScriptProvider::ok("global-primary");
    let global = Arc::new(build(inner_primary, vec![], vec![]));

    let pinned = ScriptProvider::err("pinned", "HTTP 429 too many requests");
    let fp = build(
        pinned,
        vec![],
        vec![node(
            super::NESTED_CHAIN_NODE,
            global as Arc<dyn AiProvider>,
        )],
    );

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(witness_payload(&msgs, session)).await.unwrap();
    assert_eq!(resp.text_content(), "global-primary");

    let w = crate::providers::route_witness::take(session).expect("witnessed");
    assert_eq!(
        w.served.provider,
        "global-primary",
        "the real endpoint must be reported, never `{}`",
        super::NESTED_CHAIN_NODE
    );
}

// --- round-6: the anchor is the plan's head, not the first dial -------------
//
// Every cheap reason to pass a candidate over — an open circuit or a 429
// pacing park — is settled BEFORE the walk
// builds its first request. `first_attempt` used to be assigned inside the
// model loop, i.e. after all of those `continue`s, so from the second run of an
// outage onward the first *dial* was already the fallback: `first == served`,
// `deviated()` false, and the `ModelResolved{is_fallback}` banner — the only
// user-visible fallback signal on all four surfaces — went dark for exactly the
// case it exists for. A 429 parks a provider for a minute; three strikes open
// its breaker for five.

#[tokio::test]
async fn an_open_primary_that_is_skipped_still_reads_as_a_migration() {
    let session = "agent:witness-test:skipped-open-circuit";
    crate::providers::route_witness::clear(session);

    let primary = ScriptProvider::ok("primary");
    let fb = ScriptProvider::ok("fallback");
    let health = FailoverHealth::default();
    // As if three strikes had already tripped it on an earlier run.
    health
        .open_for_test("primary", Duration::from_secs(300))
        .await;
    let fp = build_with_health(
        // rust-doctor-disable-next-line excessive-clone
        primary.clone() as Arc<dyn AiProvider>,
        vec![],
        vec![node("fallback", fb as Arc<dyn AiProvider>)],
        health,
    );

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(witness_payload(&msgs, session)).await.unwrap();
    assert_eq!(resp.text_content(), "fallback");
    assert_eq!(
        primary.call_count(),
        0,
        "the open circuit must skip the primary before any dial — which is the \
         whole point of this test"
    );

    let w = crate::providers::route_witness::take(session).expect("witnessed");
    assert_eq!(
        w.first.provider, "primary",
        "the run set out for the primary; being passed over pre-dial is not the \
         same as never having chosen it"
    );
    assert_eq!(w.served.provider, "fallback");
    assert!(
        w.deviated(),
        "an outage's second run is still a migration, and the banner is the only \
         way the user learns which endpoint answered"
    );
}

#[tokio::test]
async fn a_paced_primary_that_is_skipped_still_reads_as_a_migration() {
    // The twin of the breaker case, through the other pre-dial gate: a 429
    // parks the provider for `DEFAULT_MODEL_COOLDOWN`, and the walk skips a
    // cooling candidate outright while a later one remains.
    let session = "agent:witness-test:skipped-paced";
    crate::providers::route_witness::clear(session);

    let primary = ScriptProvider::ok("primary");
    let fb = ScriptProvider::ok("fallback");
    let cooldown = ProviderCooldown::default();
    cooldown
        .cool("primary", std::time::Duration::from_secs(90))
        .await;
    let fp = build(
        // rust-doctor-disable-next-line excessive-clone
        primary.clone() as Arc<dyn AiProvider>,
        vec![],
        vec![node("fallback", fb as Arc<dyn AiProvider>)],
    )
    .with_provider_cooldown(cooldown);

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(witness_payload(&msgs, session)).await.unwrap();
    assert_eq!(resp.text_content(), "fallback");
    assert_eq!(primary.call_count(), 0, "the parked primary is not dialed");

    let w = crate::providers::route_witness::take(session).expect("witnessed");
    assert_eq!(w.first.provider, "primary");
    assert_eq!(w.served.provider, "fallback");
    assert!(w.deviated());
}

#[tokio::test]
async fn a_pinned_chain_reports_the_pin_as_the_original() {
    // An explicit provider route is `[pin, <the whole global chain>]`.
    // The outer walk cannot record the sentinel and the inner walk records its
    // OWN first attempt, so before the plan-time anchor a failed pin plus a
    // failed global primary published `original_model = global-primary` — an
    // endpoint the user never selected. A wrong report, not an under-report.
    let session = "agent:witness-test:pinned-original";
    crate::providers::route_witness::clear(session);

    let inner_primary = ScriptProvider::err("global-primary", "HTTP 429 too many requests");
    let inner_fb = ScriptProvider::ok("global-fallback");
    let global = Arc::new(build(
        inner_primary,
        vec![],
        vec![node("global-fallback", inner_fb as Arc<dyn AiProvider>)],
    ));

    let pinned = ScriptProvider::err("pinned", "HTTP 429 too many requests");
    let fp = build(
        pinned,
        vec![],
        vec![node(
            super::NESTED_CHAIN_NODE,
            global as Arc<dyn AiProvider>,
        )],
    );

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(witness_payload(&msgs, session)).await.unwrap();
    assert_eq!(resp.text_content(), "global-fallback");

    let w = crate::providers::route_witness::take(session).expect("witnessed");
    assert_eq!(
        w.first.provider, "pinned",
        "the run set out for the pin the caller chose, not for whatever the \
         nested global chain happened to try first"
    );
    assert_eq!(w.served.provider, "global-fallback");
    assert!(w.deviated());
}

#[tokio::test]
async fn the_anchor_never_names_a_model_the_walk_filtered_out() {
    // The anchor names the ENDPOINT and leaves the model unresolved on purpose.
    // Seeding it from the catalog head would duplicate a derivation the walk
    // performs later (the capability floor, then the cooling sideline): with
    // `model-a` cooling, the seed would read `(primary, model-a)` while the walk
    // serves `(primary, model-b)`, and `deviated()` — which compares the model
    // too — would light a same-provider banner naming a model that was never
    // dialed. The dial refines the anchor instead, so the two cannot disagree.
    let session = "agent:witness-test:anchor-model";
    crate::providers::route_witness::clear(session);

    let primary = ScriptProvider::ok("primary");
    let cd = ModelCooldown::default();
    cd.cool("primary", "model-a", Duration::from_secs(300))
        .await;
    let fp = build(
        // rust-doctor-disable-next-line excessive-clone
        primary.clone() as Arc<dyn AiProvider>,
        vec![("primary", vec!["model-a", "model-b"])],
        vec![],
    )
    .with_model_cooldown(cd);

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let _ = fp.process(witness_payload(&msgs, session)).await.unwrap();
    assert_eq!(
        primary.models(),
        vec![Some("model-b".to_string())],
        "the cooling head is sidelined, so `model-b` is what actually got dialed"
    );

    let w = crate::providers::route_witness::take(session).expect("witnessed");
    assert_eq!(
        w.first.model_id(),
        Some("model-b"),
        "the anchor must adopt the model that was dialed, never the catalog head"
    );
    assert!(
        !w.deviated(),
        "the run got exactly what the walk chose; a banner here would be a wrong \
         report naming a model nothing dialed"
    );
}

#[tokio::test]
async fn a_run_that_never_gets_an_answer_announces_nothing() {
    // The anchor is written before any dial, so a walk that then fails outright
    // leaves a record behind. It must read as "nothing deviated": a run that got
    // no answer at all was not "served by a fallback".
    let session = "agent:witness-test:anchored-then-failed";
    crate::providers::route_witness::clear(session);

    let primary = ScriptProvider::err("primary", "HTTP 500 upstream exploded");
    let fp = build(primary, vec![], vec![]);

    let msgs = [UnifiedMessage::user("hi")];
    assert!(fp.process(witness_payload(&msgs, session)).await.is_err());

    let w = crate::providers::route_witness::take(session)
        .expect("the anchor is written before the dial, so a record exists");
    assert_eq!(w.first.provider, "primary");
    assert!(
        !w.deviated(),
        "an anchored run with no answer must not manufacture a migration"
    );
}

#[tokio::test]
async fn a_provider_whose_whole_ladder_404s_trips_after_three_walks() {
    // `Decision::NextModel` was the one failure exit that recorded no strike at
    // all, so "in what state does this breaker go red for a provider that 404s
    // everything?" answered *never*: circuit closed forever and every turn
    // paying one wasted round-trip per catalog model.
    let primary = ScriptProvider::err("dead_gateway", "HTTP 404 model not found");
    let fp = build(
        // rust-doctor-disable-next-line excessive-clone
        primary.clone() as Arc<dyn AiProvider>,
        vec![("dead_gateway", vec!["m1", "m2"])],
        vec![],
    );

    let msgs = [UnifiedMessage::user("hi")];
    for walk in 1..=CIRCUIT_OPEN_THRESHOLD {
        let _ = fp.process(RequestPayload::new(&msgs)).await;
        let open = fp.circuit_open("dead_gateway").await;
        assert_eq!(
            open,
            walk == CIRCUIT_OPEN_THRESHOLD,
            "walk {walk}: transient strikes must still take {CIRCUIT_OPEN_THRESHOLD} \
             walks to open the breaker"
        );
    }
    assert_eq!(
        primary.call_count(),
        2 * CIRCUIT_OPEN_THRESHOLD as usize,
        "both catalog models are tried on each walk"
    );
}

#[tokio::test]
async fn a_pinned_model_404_does_not_shed_the_provider() {
    // The negative half, and the reason the strike is derived from where the
    // model list was CHOSEN rather than from the 404 alone: a single model the
    // caller named — a `select_model` pick, an explicit model/request override,
    // a `BrainRef::Strict` id, or plain typo — indicts that name, never the
    // endpoint. Shedding the provider would let one bad id take out a healthy
    // one for five minutes.
    let primary = ScriptProvider::err("anthropic", "HTTP 404 model not found");
    let fp = build(
        // rust-doctor-disable-next-line excessive-clone
        primary.clone() as Arc<dyn AiProvider>,
        vec![("anthropic", vec!["opus", "sonnet"])],
        vec![],
    );

    let msgs = [UnifiedMessage::user("hi")];
    for _ in 0..CIRCUIT_OPEN_THRESHOLD {
        let payload = RequestPayload::new(&msgs).with_model(Some("opsu-typo".to_string()));
        let _ = fp.process(payload).await;
    }
    assert!(
        !fp.circuit_open("anthropic").await,
        "a model id the caller named must not shed the endpoint that answered it"
    );
    assert_eq!(
        primary.call_count(),
        CIRCUIT_OPEN_THRESHOLD as usize,
        "the pin replaces the catalog ladder, so one dial per walk"
    );
}

#[tokio::test(start_paused = true)]
async fn a_pacing_window_longer_than_the_cap_is_not_waited_out_at_all() {
    // The last candidate ALWAYS dials — that invariant is not up for
    // negotiation here. What changed is the wait in front of the dial: it was
    // `remaining.min(MAX_OVERLOAD_RETRY_DELAY)`, so a 300s server `Retry-After`
    // bought a 120s sleep that provably could not outlast the window and then
    // dialed *inside* it anyway. `aleph-server` caps a turn at 120s of
    // wall-clock by default (`build_stability_triple`), so that sleep alone
    // spent the entire turn budget and the dial it was preparing for never
    // happened. A wait that cannot finish the job is not spent.
    let solo = ScriptProvider::ok("solo");
    let pacing = ProviderCooldown::default();
    pacing.cool("solo", Duration::from_secs(300)).await;

    let fp = build(
        // rust-doctor-disable-next-line excessive-clone
        solo.clone() as Arc<dyn AiProvider>,
        vec![],
        vec![],
    )
    .with_provider_cooldown(pacing);

    let msgs = [UnifiedMessage::user("hi")];
    let started = tokio::time::Instant::now();
    // rust-doctor-disable-next-line unwrap-in-production
    let resp = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    let elapsed = started.elapsed();

    assert_eq!(resp.text_content(), "solo");
    assert_eq!(solo.call_count(), 1, "the last candidate still dials");
    assert!(
        elapsed < Duration::from_secs(1),
        "a window longer than the cap must not be slept on at all; waited {elapsed:?}"
    );
}

#[tokio::test]
async fn a_ladder_narrowed_by_a_cooling_sibling_does_not_indict_the_provider() {
    // The exhaustion strike must compare against what the CATALOG stated, not
    // against the survivor set the request narrowed it to. Here `m1` is parked
    // by an earlier 429 and never dialed, `m2` 404s — reading the equality off
    // the survivor list makes that one 404 say "this provider serves none of
    // its catalog models", and three such walks open the breaker on an endpoint
    // whose only real fault is one stale catalog entry plus a throttle.
    let health = FailoverHealth::default();
    let cd = ModelCooldown::default();
    cd.cool("gw", "m1", Duration::from_secs(60)).await;
    let primary = ScriptProvider::err("gw", "HTTP 404 model not found");

    let fp = build_with_health(
        // rust-doctor-disable-next-line excessive-clone
        primary.clone() as Arc<dyn AiProvider>,
        vec![("gw", vec!["m1", "m2"])],
        vec![],
        // rust-doctor-disable-next-line excessive-clone
        health.clone(),
    )
    .with_model_cooldown(cd);

    let msgs = [UnifiedMessage::user("hi")];
    let _ = fp.process(RequestPayload::new(&msgs)).await;

    assert_eq!(
        primary.models(),
        vec![Some("m2".to_string())],
        "the cooling rung is withheld, so only one of the two catalog models is dialed"
    );
    assert!(
        !fp.circuit_open("gw").await,
        "one 404 out of a two-rung catalog must not shed the endpoint"
    );
    // `circuit_allows` seeds a default entry for every candidate it gates, so
    // the question is not "is there a row" but "did that row take a strike".
    let counts: Vec<(String, u32)> = health
        .snapshot()
        .await
        .into_iter()
        .map(|v| (v.provider, v.failure_count))
        .collect();
    assert_eq!(
        counts,
        vec![("gw".to_string(), 0)],
        "a rung the walk never dialed cannot testify that the provider serves nothing; \
         no strike may be recorded"
    );
}

#[tokio::test]
async fn a_missing_model_is_sidelined_so_the_next_walk_stops_paying_for_it() {
    // The other half of the `NextModel` bookkeeping: besides the strike, the
    // 404'd model is sidelined for `DEFAULT_MODEL_COOLDOWN`, which is the whole
    // "60s, not 600s" justification. Without a `ModelCooldown` attached that
    // write is unobservable, so this is the test that makes deleting the
    // `cd.cool(...)` line go red.
    let cd = ModelCooldown::default();
    let primary = ScriptProvider::new(
        "gw",
        vec![Err("HTTP 404 model not found".to_string()), Ok(())],
    );
    let fp = build(
        // rust-doctor-disable-next-line excessive-clone
        primary.clone() as Arc<dyn AiProvider>,
        vec![("gw", vec!["m1", "m2"])],
        vec![],
    )
    // rust-doctor-disable-next-line excessive-clone
    .with_model_cooldown(cd.clone());

    let msgs = [UnifiedMessage::user("hi")];
    // rust-doctor-disable-next-line unwrap-in-production
    let first = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(first.text_content(), "gw");
    assert_eq!(primary.call_count(), 2, "walk 1 pays for the 404, then m2");
    assert!(
        cd.is_cooling("gw", "m1").await,
        "the model that answered 404 is sidelined"
    );
    assert!(
        !cd.is_cooling("gw", "m2").await,
        "the model that answered is not"
    );

    // rust-doctor-disable-next-line unwrap-in-production
    let second = fp.process(RequestPayload::new(&msgs)).await.unwrap();
    assert_eq!(second.text_content(), "gw");
    assert_eq!(
        primary.call_count(),
        3,
        "walk 2 dials only the surviving rung — the saving the sideline buys"
    );
    assert_eq!(
        primary.models(),
        vec![
            Some("m1".to_string()),
            Some("m2".to_string()),
            Some("m2".to_string())
        ],
    );
}
