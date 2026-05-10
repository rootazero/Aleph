//! Error classification logic for harness bridge.

use crate::orchestrator::errors::FlowError;
use crate::harness::trait_def::HarnessError;

/// Classify a non-cancelled `HarnessError` as either a provider-transient
/// failure (retryable by Gateway's outer fallback loop) or an internal error.
///
/// Transient indicators (per Gateway's existing classification in the
/// retiring `run_loop.rs::run_agent_loop`): HTTP 5xx (500/502/503), network
/// failures, connection drops, timeouts, and 401/403 auth errors that the
/// fallback loop used to treat as "try another provider".
///
/// Intentionally message-based — `HarnessError` wraps `AlephError` but the
/// specific AlephError variant isn't propagated structurally through
/// `HarnessError::Llm(AlephError)` in a way that survives the async trait
/// boundary without widening the public API. Message matching here mirrors
/// the exact classification the retiring run_loop did (see §5 behaviour
/// parity in the resolution design).
///
/// TODO(phase6c): replace with structural matching once HarnessError
/// surfaces a `Transient(AlephError)` variant directly.
pub(super) fn classify_harness_error(
    err: HarnessError,
    provider: &str,
) -> FlowError {
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
    // Network / connection.
    let is_network = msg.contains("Network error")
        || msg.contains("error sending request")
        || msg.contains("connection")
        || msg.contains("dns")
        || msg.contains("timed out");
    // Auth — Gateway treats 401/403 as retryable (switch provider).
    let is_auth = msg.contains("401") || msg.contains("403") || msg.contains("Unauthorized");
    // Server — match 500/502/503 with word boundaries to avoid matching "4500".
    let is_server = contains_http_status(msg, 500)
        || contains_http_status(msg, 502)
        || contains_http_status(msg, 503);
    // Rate-limited responses are NOT treated as retryable here (mirrors the
    // retiring run_loop.rs which explicitly skips retry for rate limits).
    is_network || is_auth || is_server
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
