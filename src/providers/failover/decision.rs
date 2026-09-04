//! Failure classification: map one failed `process()` attempt to a [`Decision`].

use std::time::Duration;

use crate::error::{AlephError, ErrorClass};
use crate::providers::llm_retry::{
    classify, classify_exhausted, extract_retry_after_str, has_status_code, is_transient_overload,
    RetryVerdict,
};

use super::{DEFAULT_MODEL_COOLDOWN, DEFAULT_TRANSIENT_DELAY, MAX_COOLDOWN, OVERLOAD_RETRY_BUDGET};

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
pub(crate) enum FailureKind {
    /// Recoverable soon (rate limit, overload, network): strike-then-probe.
    Transient,
    /// Won't recover this session (bad/expired key, forbidden): shed at once.
    Permanent,
}

/// What to do after one failed `process()` attempt.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Retry the same provider + model after `delay`.
    RetrySame(Duration),
    /// Advance to the next model of the same provider.
    NextModel,
    /// This model hit a *model-specific* 429. Record a per-model cooldown
    /// (`Some(d)` carries the server `Retry-After`), then prefer a sibling
    /// model before advancing providers.
    ///
    /// "Model-specific" describes which model is *sidelined*, not how far the
    /// consequences reach. The walk's arm also parks the whole provider
    /// (`ProviderCooldown::cool`, so the NEXT turn paces itself before
    /// re-dialing it) and arms `tripped = Transient`, so the circuit does trip
    /// once every model of that provider is exhausted. Sibling models stay live
    /// only *within this walk*: a later model that answers retires both effects
    /// (`ProviderCooldown::clear` on success, and `tripped` is dropped). Pinned
    /// by `failover::tests::a_model_429_parks_the_provider_and_arms_a_strike`
    /// and its opposite half `a_successful_call_clears_the_provider_pacing_window`.
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

/// The pacing window a rate-limited attempt earns: the server hint when it gave
/// one, else the default, always capped.
///
/// Single derivation of that default-and-cap so the two writers of the window —
/// [`decide`]'s `RateLimited` verdict and [`strike_for`] — cannot disagree about
/// how long a 429 parks a provider (criterion: order/unit/boundary derived in
/// one place).
pub(crate) fn cooldown_window(hint: Option<Duration>) -> Duration {
    hint.unwrap_or(DEFAULT_MODEL_COOLDOWN).min(MAX_COOLDOWN)
}

/// The bookkeeping an attempt earns when it failed *after* the user has already
/// been shown part of an answer.
///
/// Two arms of the walk end that way and must not drift apart:
/// * the streaming `Err` arm guarded by `EmissionGuard::has_emitted`, and
/// * the `Ok(resp)` arm where the provider reported an in-band fault mid-stream
///   ([`ProviderResponse::provider_error`](crate::providers::adapter::ProviderResponse::provider_error)).
///
/// Neither may retry or advance — appending a second answer to a half-written
/// one is worse than the fault — so the *routing* half of a verdict is already
/// settled and only the bookkeeping half is open. [`Decision`] cannot answer it:
/// its variants encode **where to go next**, so consuming one here would have to
/// invent bookkeeping for `NextModel` / `Stop` / `RetrySame`, which name no
/// strike at all. This derives the two facts that are actually needed —
/// how the breaker should count it, and whether anything told us to wait —
/// directly from the error, in one place.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Strike {
    /// How the circuit breaker counts this failure.
    pub(crate) kind: FailureKind,
    /// Pacing window to park on the model *and* its provider, set only when the
    /// fault was a rate limit. `None` for every other fault: nothing told us to
    /// wait, and inventing a window would sideline a provider on a blip.
    pub(crate) cooldown: Option<Duration>,
}

/// Derive the [`Strike`] one already-emitted failed attempt deserves.
pub(crate) fn strike_for(err: &AlephError) -> Strike {
    let msg = err.to_string();
    // Same permanent/transient split `decide` tags its `NextProvider` with, read
    // from the same predicate — a dead credential is shed at once, everything
    // else needs `CIRCUIT_OPEN_THRESHOLD` strikes.
    let kind = if crate::providers::llm_retry::is_permanent_failure(&msg) {
        FailureKind::Permanent
    } else {
        FailureKind::Transient
    };
    // Same classifier and same hint precedence `decide` uses for
    // `Decision::RateLimited`: typed `suggestion` first, then the `Retry-After`
    // the message body itself states.
    let cooldown = match classify_exhausted(&msg) {
        RetryVerdict::Fallback { reason } if reason.starts_with("rate limited") => {
            Some(cooldown_window(
                retry_after_from_suggestion(err).or_else(|| extract_retry_after_str(&msg)),
            ))
        }
        _ => None,
    };
    Strike { kind, cooldown }
}

/// Classify one failed attempt into a [`Decision`].
///
/// Two-stage: the string classifier ([`classify`]) recognises an in-place
/// retry opportunity (529 / network keywords); [`classify_exhausted`] then
/// gives the final verdict. A `Fatal` string verdict is overridden to a
/// provider-level failover when the *typed* error is transient — covering
/// errors whose `Display` carried no HTTP code (e.g. `Timeout` →
/// "Request timed out").
///
/// `has_later_candidate` states whether the walk still has somewhere to go.
/// It is the walk's own `idx + 1 < total`, threaded in rather than re-derived
/// here, so the circuit-breaker gate, the rate-ceiling gate, the 429 pacing
/// gate and this classifier all read the *same* fact (criterion: a boundary
/// is derived in one place). It only ever narrows a verdict towards the
/// chain — when it is `false` every path behaves exactly as it did before.
// rust-doctor-disable-next-line high-cyclomatic-complexity
pub(crate) fn decide(
    err: &AlephError,
    attempt: u32,
    max_retries: u32,
    has_later_candidate: bool,
) -> Decision {
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
        // Cap the overload ride-out so a large `max_retries` config cannot keep
        // the UI stuck waiting while the same overloaded provider is hammered
        // in place. A single extra attempt is enough for a transient spike;
        // after that we escalate to the failover chain (or surface the error
        // if there is no sibling).
        max_retries.min(OVERLOAD_RETRY_BUDGET)
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

    // A typed `Timeout` says a *silence window has already been spent* on this
    // attempt, which no other transient error tells us. Every producer of the
    // variant that reaches this walk is a watchdog that fires only after a
    // whole window of nothing:
    //   * the TTFB guard (`HttpProvider::execute_once`) and the SSE gap guard
    //     (`protocols::stream_idle`) — `effective_idle_secs`, 60s by default;
    //   * reqwest's own `is_timeout` on `send()` in the `HttpProvider` family
    //     (every protocol adapter builds its client through
    //     `protocols::http_client`, via `protocols::registry` or
    //     `protocols::loader`) — that builder sets only a 10s `connect_timeout`
    //     and deliberately no overall request timeout, so there this is the
    //     handshake, not the body;
    //   * reqwest's `is_timeout` in the native `providers::ollama` path, which
    //     is *not* an `HttpProvider`: it builds its own client with an overall
    //     `.timeout(config.timeout_seconds)` and no `connect_timeout`, 300s by
    //     default (`config::types::provider::default_timeout_seconds`). A spent
    //     window a fortiori — the whole request budget elapsed with nothing
    //     returned, so re-dialing in place would buy another 300s of silence;
    //   * a stream the adapter saw cut before its terminal frame
    //     (`openai_chat` / `openai_responses` / `gemini`), and the
    //     truncated-tool-call diagnostic, where re-dialing re-truncates the
    //     same oversized output.
    // In every one of those an in-place retry buys a second full window against
    // an endpoint that has already demonstrated silence, while the per-turn
    // watchdog above us keeps running. The chain, not the socket, is the next
    // bet — so advance and let the breaker count the strike.
    //
    // Keyed on the *typed variant*, never on the words "timed out": that phrase
    // also reaches `classify` from untyped provider bodies via `llm_retry`'s
    // network word list, and there nothing tells us a window was spent.
    //
    // Guarded by `has_later_candidate` because advancing off the terminal
    // candidate is not "try elsewhere", it is "fail the request with zero
    // retries" — the single-provider shape the overload budget already reasons
    // about. There the ordinary `max_retries` in-place budget still applies,
    // unchanged.
    if has_later_candidate && matches!(err, AlephError::Timeout { .. }) {
        return next_provider;
    }

    match classify_exhausted(&msg) {
        // 413 — the turn driver owns this recovery path via
        // `context::compact::rescue::try_reactive_compact_and_retry` (the
        // harness reaches it through `RescueHost`). The failover layer stops so the
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
                // The cooldown hint prefers the typed `suggestion` field; when
                // the error carries none, fall back to the `Retry-After` the
                // message body itself states — `classify_rate_limit` already
                // parsed the same text into the reason, so discarding it here
                // replaced a server-guided wait with the blind default.
                Decision::RateLimited(server_delay.or_else(|| extract_retry_after_str(&msg)))
            } else {
                next_provider
            }
        }
        // `classify_exhausted` never yields `Retry`; handled defensively.
        RetryVerdict::Retry { delay } if can_retry => Decision::RetrySame(delay),
        RetryVerdict::Retry { .. } => next_provider,
        RetryVerdict::Fatal => {
            // `has_status_code`, not a bare substring: provider bodies are full
            // of digit runs that merely *contain* 400 ("used 400123 tokens;
            // invalid"), and a false bad-request here aborts the whole walk on
            // what is really a transient error (see `llm_retry::has_status_code`).
            let explicit_bad_request = has_status_code(&lower, 400)
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
