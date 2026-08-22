//! Error classification logic for harness bridge.

use crate::harness::trait_def::HarnessError;
use crate::orchestrator::errors::FlowError;

/// Classify a non-cancelled `HarnessError` as either a provider-transient
/// failure (retryable by Gateway's outer fallback loop) or an internal error.
///
/// Transient indicators (per Gateway's existing classification in the
/// retiring `run_loop.rs::run_agent_loop`): HTTP 5xx (500/502/503), network
/// failures, connection drops, timeouts, and 401/403 auth errors that the
/// fallback loop used to treat as "try another provider".
///
/// Intentionally message-based — `HarnessError` wraps `AlephError` but the
/// specific `AlephError` variant isn't propagated structurally through
/// `HarnessError::Llm(AlephError)` in a way that survives the async trait
/// boundary without widening the public API. Message matching here mirrors
/// the exact classification the retiring `run_loop` did (see §5 behaviour
/// parity in the resolution design).
///
/// TODO(phase6c): replace with structural matching once `HarnessError`
/// surfaces a `Transient(AlephError)` variant directly.
pub(super) fn classify_harness_error(err: HarnessError, provider: &str) -> FlowError {
    let msg = err.to_string();
    if is_transient_harness_message(&msg) {
        FlowError::Transient {
            provider: provider.to_string(),
            message: msg,
        }
    } else {
        FlowError::Internal(format!("harness: {msg}"))
    }
}

fn is_transient_harness_message(msg: &str) -> bool {
    const NETWORK_MARKERS: &[&str] = &[
        "Network error",
        "error sending request",
        "connection",
        "dns",
        "timed out",
    ];
    // Auth is deliberately NOT a bare-substring list. It used to be
    // `["401", "403", "Unauthorized"]` matched with `str::contains`, which
    // fires on any message that merely *embeds* those digits — a token count
    // ("401234 tokens > 200000 maximum"), a request id, a 13-digit epoch. A
    // fatal, deterministic error classified Transient here is re-dispatched by
    // the gateway's outer loop up to `MAX_FALLBACK_ATTEMPTS` times, burning
    // budget and finally surfacing a provider error instead of the real cause.
    // `has_status_code` is the same digit-boundary check the rest of the repo
    // uses; CLAUDE.md §9 names `contains("401")` matching `40123` by hand.
    const AUTH_PHRASE_MARKERS: &[&str] = &["Unauthorized"];
    const RATE_LIMIT_MARKERS: &[&str] = &[
        "rate limit",
        "Rate limit",
        "rate_limit",
        "receiving too many requests",
    ];

    // Rate limit (429). A 429 that escapes here means the FailoverProvider
    // already exhausted its in-place retry budget AND every chained
    // provider/model (deep backoff + per-model cooldown + chain advance). At
    // that point the failure is genuinely transient — the throttle window will
    // pass — so let the Gateway's outer dispatch loop (`MAX_FALLBACK_ATTEMPTS`)
    // take another spaced attempt instead of surfacing a fatal error to the
    // user. The earlier "rate limits are not retryable" stance assumed an empty
    // chain; with chain self-heal a 429 here is a load signal, not a dead end.
    // Status codes all go through the one digit-boundary matcher. This file
    // used to carry `contains_http_status`, a byte-for-byte reimplementation
    // of `llm_retry::has_status_code` (same `find` loop, same left/right digit
    // guards) — two answers to "is this number an HTTP status", one of which
    // the auth arm above was not even using.
    const TRANSIENT_STATUSES: &[u16] = &[500, 502, 503, 429];
    const AUTH_STATUSES: &[u16] = &[401, 403];

    has_any_marker(msg, NETWORK_MARKERS)
        || has_any_marker(msg, AUTH_PHRASE_MARKERS)
        || has_any_status(msg, AUTH_STATUSES)
        || has_any_status(msg, TRANSIENT_STATUSES)
        || has_any_marker(msg, RATE_LIMIT_MARKERS)
}

fn has_any_marker(msg: &str, markers: &[&str]) -> bool {
    markers.iter().any(|m| msg.contains(m))
}

fn has_any_status(msg: &str, codes: &[u16]) -> bool {
    codes
        .iter()
        .any(|c| crate::providers::llm_retry::has_status_code(msg, *c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_429_is_transient() {
        // The exact production string that previously surfaced as a fatal
        // `flow: internal dispatch error` — now classified transient so the
        // Gateway's outer dispatch loop retries instead of hard-failing.
        let msg = "llm error: Rate limit error: Anthropic API rate limited (429): We're \
                   receiving too many requests at the moment. Please wait a moment and try again.";
        assert!(is_transient_harness_message(msg));
    }

    #[test]
    fn plain_internal_error_is_not_transient() {
        // A non-network, non-rate-limit failure must stay fatal so we don't
        // spin the outer loop on a genuine bug.
        assert!(!is_transient_harness_message(
            "tool registry misconfigured: unknown tool"
        ));
    }

    /// A number that merely *contains* an auth status is not an auth status.
    ///
    /// Written against the bug: `AUTH_MARKERS` matched with `str::contains`,
    /// so this message — a deterministic, fatal, retry-proof failure — was
    /// classified `Transient` and re-dispatched three times. Break
    /// `has_any_status` back into `msg.contains("401")` and this goes red at
    /// this line; that is the only reason it exists.
    #[test]
    fn a_number_embedding_an_auth_status_is_not_an_auth_failure() {
        assert!(!is_transient_harness_message(
            "llm error: prompt is too long: 401234 tokens > 200000 maximum"
        ));
        assert!(!is_transient_harness_message(
            "llm error: invalid request id 9940312 rejected by upstream"
        ));
    }

    /// The other half: narrowing the match must not stop recognising real
    /// auth failures. Without this, "fix the false positive" and "delete the
    /// classification" look identical in the suite.
    #[test]
    fn a_real_auth_status_is_still_transient() {
        assert!(is_transient_harness_message(
            "llm error: request failed with status 401: invalid api key"
        ));
        assert!(is_transient_harness_message(
            "llm error: HTTP 403 (forbidden) from provider"
        ));
        assert!(is_transient_harness_message(
            "llm error: Unauthorized — token expired"
        ));
    }
}
