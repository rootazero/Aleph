//! Error classification logic for harness bridge.

use crate::harness::trait_def::HarnessError;
use crate::orchestrator::errors::FlowError;

/// Classify a non-cancelled `HarnessError` as either a provider-transient
/// failure (retryable by Gateway's outer fallback loop) or an internal error.
///
/// Transient indicators: HTTP 5xx (500/502/503), 429, network failures,
/// connection drops and timeouts.
///
/// **A failure that already showed the user part of an answer is not among
/// them**, whatever its wording — see the
/// [`PARTIAL_OUTPUT_EMITTED`](crate::providers::failover::PARTIAL_OUTPUT_EMITTED)
/// gate at the top of [`is_transient_harness_message`].
///
/// **Auth is not among them.** 401/403 used to be classified transient here,
/// on the reasoning that the fallback loop should "try another provider" — but
/// trying another provider is `FailoverProvider`'s job and it happens *inside*
/// one dispatch, so a credential failure that reaches this function has already
/// exhausted the chain and tripped its breakers. See the permanence gate in
/// [`is_transient_harness_message`] for what re-dispatching it cost.
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

    // Content already reached the user's screen. `FailoverProvider`'s walk
    // stops advancing the chain when that happens — a second candidate would
    // append its answer to a half-written one — and states the fact in the
    // error's rendered message. Re-dispatching here would do exactly what the
    // walk refused to do, one layer up and on the same `run_id`, so this is
    // checked FIRST: the constraint is about what the user has already been
    // shown, not about whether the underlying failure is recoverable. A cut
    // stream's own wording ("connection reset", "timed out") matches
    // `NETWORK_MARKERS` below and would otherwise win.
    if msg.contains(crate::providers::failover::PARTIAL_OUTPUT_EMITTED) {
        return false;
    }

    // An expired OAuth access token is the one auth failure this process holds
    // the remedy for: the retry arm in `run_loop/inner.rs` calls
    // `codex_token_refresher` to mint a new one and hot-swap the live provider
    // before re-dispatching. Checked FIRST because such a message also carries
    // a 401, which the permanence gate below would otherwise shed.
    if crate::gateway::codex_token_refresher::is_oauth_token_expired_error(msg) {
        return true;
    }

    // Every other 401/403 is permanent by the repo's one definition of that
    // word, and this layer used to disagree with it.
    //
    // `FailoverProvider` walks the whole chain inside a SINGLE dispatch and
    // tags a credential failure `FailureKind::Permanent`, which sheds that
    // provider on the first strike with a long cooldown. A message that reaches
    // here therefore means the walk is already over and the breakers are
    // already open. Calling it transient made the gateway's outer loop
    // re-dispatch the identical resolution up to `MAX_FALLBACK_ATTEMPTS` times
    // against a chain that now refuses to dial — measured on a real server, the
    // 2nd and 3rd attempts failed instantly with 0 loops and 0 tokens, and each
    // of them broadcast its own terminal frame, so every client keeping the
    // last one reported a run that had spent real work as `0 tools / 0 tokens`.
    //
    // Two layers answering "is this recoverable?" in opposite directions is the
    // defect; `is_permanent_failure` is the answer that already had consumers
    // (the circuit breaker), so it is the one that wins.
    if crate::providers::llm_retry::is_permanent_failure(msg) {
        return false;
    }

    has_any_marker(msg, NETWORK_MARKERS)
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

    /// A failure raised after the user already saw text must never be
    /// re-dispatched, even though its wording is the most transient-looking
    /// text there is.
    ///
    /// Built through the real construction path
    /// (`failover::mark_partial_output_emitted` → `HarnessError::Llm`) rather
    /// than a hand-typed string, so it asserts the marker survives `Display`
    /// — `AlephError::ProviderError` renders `message` and drops `suggestion`,
    /// which is exactly how a marker parked on the wrong field would become a
    /// no-op that reports success.
    #[test]
    fn a_partial_output_error_is_not_transient_even_when_its_message_says_connection() {
        let marked = crate::providers::failover::mark_partial_output_emitted(
            &crate::error::AlephError::provider("connection reset by peer"),
        );
        let rendered = marked.to_string();
        assert!(
            rendered.contains(crate::providers::failover::PARTIAL_OUTPUT_EMITTED),
            "the marker must reach the classifier through Display: {rendered}"
        );
        assert!(
            rendered.contains("connection reset by peer"),
            "the original diagnostic must survive: {rendered}"
        );

        let flow = classify_harness_error(HarnessError::Llm(marked), "p");
        assert!(
            matches!(flow, FlowError::Internal(_)),
            "a half-written answer must not be re-dispatched, got {flow:?}"
        );

        // Without the marker the same wording IS transient — that contrast is
        // what makes the gate above a real gate rather than a restatement of
        // `plain_internal_error_is_not_transient`.
        assert!(is_transient_harness_message(
            "llm error: Provider error: connection reset by peer"
        ));
    }

    /// The gate's negative half: a chain-terminal failure that showed the user
    /// nothing must still be re-dispatchable.
    ///
    /// `EmissionGuard::has_emitted` also latches on tool-call deltas, which the
    /// production sink never renders. If the walk marked off *that* bit instead
    /// of `has_shown_user_output`, a truncated tool call — the exact case its
    /// own diagnostic calls "a large file write crossing a proxy timeout" —
    /// would arrive here as `FlowError::Internal` and hard-fail the run on the
    /// first truncation, silently deleting a safe recovery. Nothing else pins
    /// that the marker does not over-fire.
    #[test]
    fn a_truncated_tool_call_that_showed_no_text_is_still_transient() {
        let unmarked = crate::error::AlephError::Timeout { suggestion: None };
        let rendered = unmarked.to_string();
        assert!(
            !rendered.contains(crate::providers::failover::PARTIAL_OUTPUT_EMITTED),
            "the walk leaves a text-free cut unmarked: {rendered}"
        );

        let flow = classify_harness_error(HarnessError::Llm(unmarked), "p");
        assert!(
            matches!(flow, FlowError::Transient { .. }),
            "a blank screen must keep its re-dispatch, got {flow:?}"
        );
    }

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
    /// Written against the bug where auth was matched with `str::contains`, so
    /// this message — a deterministic, fatal, retry-proof failure — was read as
    /// a credential failure. The digit-boundary check now lives one layer down,
    /// in `llm_retry::has_status_code` via `is_permanent_failure`; break it back
    /// into `msg.contains("401")` and this goes red.
    #[test]
    fn a_number_embedding_an_auth_status_is_not_an_auth_failure() {
        assert!(!is_transient_harness_message(
            "llm error: prompt is too long: 401234 tokens > 200000 maximum"
        ));
        assert!(!is_transient_harness_message(
            "llm error: invalid request id 9940312 rejected by upstream"
        ));
    }

    /// A real credential failure is NOT transient, and this replaces a test
    /// that asserted the opposite.
    ///
    /// The old rule said "401/403 → try another provider". Trying another
    /// provider is `FailoverProvider`'s job and it happens inside one dispatch,
    /// so a 401 arriving here means the walk already finished and shed the
    /// provider as `FailureKind::Permanent`. Re-dispatching from the gateway's
    /// outer loop re-dialed a chain with open breakers: three attempts, two of
    /// them instant, each broadcasting its own terminal frame — which is how a
    /// run that had really spent two loops and 356 tokens ended up reported to
    /// every client as `0 tools / 0 tokens`.
    #[test]
    fn a_real_auth_failure_is_permanent_not_transient() {
        assert!(!is_transient_harness_message(
            "llm error: request failed with status 401: invalid api key"
        ));
        assert!(!is_transient_harness_message(
            "llm error: HTTP 403 (forbidden) from provider"
        ));
        assert!(!is_transient_harness_message(
            "llm error: Unauthorized — check your API key"
        ));
    }

    /// The other half, so "shed permanent auth failures" and "stop classifying
    /// auth at all" do not look identical in the suite: the one auth failure
    /// this process can fix is an expired OAuth token, because the retry arm
    /// refreshes it and hot-swaps the provider before re-dispatching. It is
    /// checked before the permanence gate precisely because it also carries a
    /// 401 — reorder the two and the self-heal path dies silently.
    #[test]
    fn an_expired_oauth_token_is_still_transient() {
        assert!(is_transient_harness_message(
            "llm error: request failed with status 401: {\"code\":\"token_expired\"}"
        ));
        assert!(is_transient_harness_message(
            "llm error: 401 Unauthorized: your authentication token is expired"
        ));
    }

    /// Both layers must answer "will this recover on its own?" the same way.
    ///
    /// Asserted against `is_permanent_failure` itself rather than against a
    /// re-listed set of statuses: a second list here would be the same drift
    /// this test exists to forbid.
    #[test]
    fn the_two_layers_agree_about_what_is_permanent() {
        for msg in [
            "llm error: request failed with status 401: invalid api key",
            "llm error: HTTP 403 (forbidden) from provider",
            "llm error: Unauthorized",
        ] {
            assert!(
                crate::providers::llm_retry::is_permanent_failure(msg),
                "fixture must be permanent by the breaker's lens: {msg}"
            );
            assert!(
                !is_transient_harness_message(msg),
                "the outer retry loop must not re-dispatch what the breaker shed: {msg}"
            );
        }
        // …and the converse, so this is not satisfied by "nothing is ever
        // transient".
        let overload = "llm error: 503 overloaded, please wait a moment";
        assert!(!crate::providers::llm_retry::is_permanent_failure(overload));
        assert!(is_transient_harness_message(overload));
    }
}
