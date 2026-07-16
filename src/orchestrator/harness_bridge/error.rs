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
    const AUTH_MARKERS: &[&str] = &["401", "403", "Unauthorized"];
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
    has_any_marker(msg, NETWORK_MARKERS)
        || has_any_marker(msg, AUTH_MARKERS)
        || contains_http_status(msg, 500)
        || contains_http_status(msg, 502)
        || contains_http_status(msg, 503)
        || contains_http_status(msg, 429)
        || has_any_marker(msg, RATE_LIMIT_MARKERS)
}

fn has_any_marker(msg: &str, markers: &[&str]) -> bool {
    markers.iter().any(|m| msg.contains(m))
}

fn contains_http_status(msg: &str, code: u16) -> bool {
    let code_str = code.to_string();
    let mut search_from = 0;
    while let Some(pos) = msg[search_from..].find(&code_str) {
        let abs_pos = search_from + pos;
        let before_ok = abs_pos == 0 || !msg.as_bytes()[abs_pos - 1].is_ascii_digit();
        let after_pos = abs_pos + code_str.len();
        let after_ok = after_pos >= msg.len() || !msg.as_bytes()[after_pos].is_ascii_digit();
        if before_ok && after_ok {
            return true;
        }
        search_from = abs_pos + code_str.len();
    }
    false
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
}
