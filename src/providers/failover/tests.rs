use super::decision::{decide, Decision, FailureKind};
use crate::config::types::{LoadBalanceStrategy, RouteMode};
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::load_stats::LoadStats;
use crate::providers::route_handle::RouteHandle;
use crate::providers::route_policy::EndpointTier;
use crate::providers::AiProvider;
use crate::sandbox::exec_approval::gate::ApprovalRequester;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

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
