//! Failure classification: map one failed `process()` attempt to a [`Decision`].

use std::time::Duration;

use crate::error::{AlephError, ErrorClass};
use crate::providers::llm_retry::{
    classify, classify_exhausted, extract_retry_after_str, has_status_code, is_transient_overload,
    RetryVerdict,
};

use super::{DEFAULT_TRANSIENT_DELAY, OVERLOAD_RETRY_BUDGET};

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
// rust-doctor-disable-next-line high-cyclomatic-complexity
pub(crate) fn decide(err: &AlephError, attempt: u32, max_retries: u32) -> Decision {
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
