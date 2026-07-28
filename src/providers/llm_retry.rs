//! LLM error classification and backoff shape.
//!
//! The single answer to "is this error worth retrying, and how long should the
//! next attempt wait" — consumed by the failover walk (`failover::decision`,
//! `failover::provider`) and the reactive-compaction rescue. Classification is
//! by error *shape* (HTTP status, provider error text), never by anything the
//! user wrote, so it stays infrastructure.
//!
//! It does **not** drive a retry loop of its own. It used to also carry
//! `retry_async`, a standalone cancellation-aware driver with no callers: the
//! failover walk owns the loop (it has to — only it knows the candidate chain,
//! the breaker and the per-provider cooldowns), and a second implementation of
//! the same backoff was one more place for the two to disagree.

use std::time::Duration;

/// Outcome of classifying an error for retry decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryVerdict {
    /// The error is transient; wait `delay` before retrying.
    Retry { delay: Duration },
    /// The error is permanent; do not retry.
    Fatal,
    /// The request is too large — compact messages and retry.
    CompactAndRetry { token_gap: Option<usize> },
    /// Primary model unavailable — switch to fallback if available.
    Fallback { reason: String },
}

/// Whether `msg` mentions HTTP status `code` as a *status*, not as a fragment of
/// some longer number.
///
/// Every classifier below keys on three-digit codes, and a bare
/// `msg.contains("429")` also fires on any digit run that happens to contain
/// them — which provider error bodies are full of: token counts (`"Limit
/// 40000, Used 40123"` contains `401`), request ids (`req_4045…` contains
/// `404`), byte sizes, epoch timestamps. The consequences are not cosmetic:
/// a `401` read out of a token count tags the failure
/// [`Permanent`](crate::providers::failover) and opens the circuit breaker on
/// the FIRST strike with the full ten-minute cooldown; a `404` read out of a
/// request id is classified "model not found" and quietly burns the provider's
/// whole model list. Same defect shape as the `Retry-After` HTTP-date that was
/// read as a day-of-month: the digits were right there, nobody checked what
/// they were attached to.
///
/// The test is deliberately narrow — the neighbouring characters must not be
/// ASCII digits. Everything a real message puts around a status code (spaces,
/// `(`, `:`, `=`, `,`, `/`, end of string) still matches, so no genuine
/// classification is lost.
#[must_use]
pub fn has_status_code(msg: &str, code: u16) -> bool {
    let needle = code.to_string();
    let bytes = msg.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = msg[from..].find(&needle) {
        let start = from + rel;
        let end = start + needle.len();
        let left_ok = start == 0 || !bytes[start - 1].is_ascii_digit();
        let right_ok = end == bytes.len() || !bytes[end].is_ascii_digit();
        if left_ok && right_ok {
            return true;
        }
        // Overlapping matches are impossible for a fixed-width numeric needle,
        // but advancing by one keeps the scan correct for any future needle.
        from = start + 1;
    }
    false
}

/// Extract token gap from "prompt is too long: X tokens > Y maximum" error messages.
#[must_use]
pub fn parse_token_gap_str(msg: &str) -> Option<usize> {
    let lower = msg.to_lowercase();
    if !lower.contains("prompt is too long") && !lower.contains("prompt_too_long") {
        return None;
    }
    // Find "N tokens > M" pattern manually. Slice `lower` (not `msg`):
    // `to_lowercase` can change byte lengths for some Unicode chars, so an
    // index found in `lower` may not be a char boundary in `msg` — slicing
    // `msg` with it could panic on a provider-supplied error string. The
    // digits we extract are ASCII and identical in both strings.
    let tokens_idx = lower.find("tokens")?;
    let before_tokens = lower[..tokens_idx].trim_end();
    let actual_str = before_tokens.rsplit(|c: char| !c.is_ascii_digit()).next()?;
    if actual_str.is_empty() {
        return None;
    }
    let actual: usize = actual_str.parse().ok()?;

    let after_tokens = &lower[tokens_idx..];
    let gt_idx = after_tokens.find('>')?;
    let after_gt = after_tokens[gt_idx + 1..].trim_start();
    let limit_str: String = after_gt
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if limit_str.is_empty() {
        return None;
    }
    let limit: usize = limit_str.parse().ok()?;

    Some(actual.saturating_sub(limit))
}

/// Classify an error AFTER all retries are exhausted.
///
/// Final verdict once the in-place retry budget is spent.
///
/// Reclassifies transient errors as `Fallback` since retrying did not help.
/// 413 stays `CompactAndRetry` (the turn driver owns that recovery); 400 stays
/// `Fatal` (the request itself is broken). Called by `FailoverProvider` on
/// `AlephError` display strings.
#[must_use]
// rust-doctor-disable-next-line high-cyclomatic-complexity
pub fn classify_exhausted(raw: &str) -> RetryVerdict {
    let base = classify(raw);

    // 413 → still CompactAndRetry (main loop handles compression)
    if matches!(base, RetryVerdict::CompactAndRetry { .. }) {
        return base;
    }

    let msg = raw.to_lowercase();

    // D3: "overloaded" beats 429 here as well (see `classify`). After local
    // retries have been exhausted, escalate transient overload to Fallback —
    // the local backoff didn't help, so a sibling provider is the next bet.
    let account_patterns = ["account", "organization", "billing", "quota exceeded"];
    let is_account_scoped = account_patterns.iter().any(|p| msg.contains(p));
    if !is_account_scoped && (msg.contains("overloaded") || has_status_code(&msg, 529)) {
        return RetryVerdict::Fallback {
            reason: format!("provider overloaded after retries: {raw}"),
        };
    }

    // 429 rate limit → classify as model-specific (Fallback) vs account-wide (Fatal).
    if has_status_code(&msg, 429) || msg.contains("rate limit") || msg.contains("rate_limit") {
        return classify_rate_limit(raw);
    }

    // 400 → Fatal (request is malformed, fallback won't help)
    if has_status_code(&msg, 400) && (msg.contains("bad request") || msg.contains("invalid")) {
        return RetryVerdict::Fatal;
    }

    // 404 model not found → Fallback
    if has_status_code(&msg, 404) || msg.contains("not found") {
        return RetryVerdict::Fallback {
            reason: "model not found".into(),
        };
    }

    // 401/403 auth errors → Fallback (but caller should notify user)
    if has_status_code(&msg, 401)
        || has_status_code(&msg, 403)
        || msg.contains("unauthorized")
        || msg.contains("forbidden")
    {
        return RetryVerdict::Fallback {
            reason: "authentication failed — check your API key".into(),
        };
    }

    // Transient errors (overloaded, network) that exhausted retries → Fallback
    if matches!(base, RetryVerdict::Retry { .. }) {
        return RetryVerdict::Fallback {
            reason: format!("primary model unavailable: {raw}"),
        };
    }

    // Everything else stays Fatal
    RetryVerdict::Fatal
}

/// Normalise a raw `Retry-After` **header value** to whole seconds.
///
/// RFC 7231 allows two forms and providers use both: a delay in seconds
/// (`"120"`) and an HTTP-date (`"Wed, 21 Oct 2026 07:28:00 GMT"`). The adapters
/// interpolate this value into a `"Rate limited. Retry after {v} seconds."`
/// suggestion that the failover layer parses back out, so a date reaching that
/// format string is not merely ugly — [`extract_retry_after_str`] takes the
/// first digit run it finds and reads the *day of month* as a delay. A server
/// asking for several hours would be re-dialed 21 seconds later, every 21
/// seconds. Normalising here means only one shape ever reaches the formatter.
///
/// A date already in the past yields `0` (retry immediately), never a negative
/// or wrapped value.
#[must_use]
pub fn retry_after_header_secs(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if let Ok(secs) = trimmed.parse::<u64>() {
        return Some(secs);
    }
    let at = httpdate::parse_http_date(trimmed).ok()?;
    Some(
        at.duration_since(std::time::SystemTime::now())
            .map_or(0, |d| d.as_secs()),
    )
}

/// Extract a Retry-After delay from a raw error message.
///
/// Providers embed "Retry after N seconds" in the suggestion/message field;
/// parsing it yields a server-guided delay instead of a hardcoded value.
///
/// `pub` so the failover layer can parse the `Retry-After` the Anthropic
/// adapter stashes in `AlephError::RateLimitError.suggestion` — a field the
/// error's `Display` drops, so the string classifier never sees it.
#[must_use]
pub fn extract_retry_after_str(raw: &str) -> Option<Duration> {
    let msg = raw.to_lowercase();
    // Match patterns like "retry after 30 seconds" or "retry-after: 60"
    let after_idx = msg
        .find("retry after ")
        .or_else(|| msg.find("retry-after: "))?;
    let start = msg[after_idx..].find(|c: char| c.is_ascii_digit())? + after_idx;
    let num_str: String = msg[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let secs: u64 = num_str.parse().ok()?;
    Some(Duration::from_secs(secs))
}

/// Classify a 429 rate limit error into Fallback or Fatal.
///
/// - Model-specific rate limits (error mentions a model name or per-model quota)
///   → `Fallback` — switching to a different provider/model may succeed.
/// - Account-wide rate limits ("account", "organization", "quota")
///   → `Fatal` — switching models won't help, propagate to user.
/// - Default 429 → `Fallback` (conservative: try switching provider).
fn classify_rate_limit(raw: &str) -> RetryVerdict {
    let msg = raw.to_lowercase();

    // Account-wide / org-level → Fatal (switching won't help)
    let account_patterns = ["account", "organization", "billing", "quota exceeded"];
    if account_patterns.iter().any(|p| msg.contains(p)) {
        return RetryVerdict::Fatal;
    }

    // Default: treat as model-specific → Fallback with server-guided delay
    let reason = if let Some(delay) = extract_retry_after_str(raw) {
        format!("rate limited (retry after {}s)", delay.as_secs())
    } else {
        "rate limited".to_string()
    };
    RetryVerdict::Fallback { reason }
}

/// Whether a 429 body describes a *server-side* transient overload rather than
/// an account/quota limit.
///
/// Anthropic (and several Anthropic-protocol providers) return HTTP 429 for
/// transient capacity throttling with a body like
/// `{"type":"rate_limit_error","message":"We're receiving too many requests at
/// the moment. Please wait a moment and try again."}` — semantically the same
/// as a 529 overload, not a per-account quota. The right move is a short
/// backoff-and-retry on the SAME provider, not a failover: in single-provider
/// setups a `Fallback` verdict has no sibling to switch to and degrades to an
/// immediate hard failure with **zero** retries, even though the server
/// explicitly asked the client to "try again" (the failure that crashed
/// scheduled jobs on 2026-05-29). Callers must check `account_patterns` first
/// so genuine quota limits stay Fatal.
#[must_use]
pub fn is_transient_overload(msg_lower: &str) -> bool {
    msg_lower.contains("overloaded")
        || has_status_code(msg_lower, 529)
        || msg_lower.contains("please wait a moment")
        || msg_lower.contains("receiving too many requests at the moment")
}

/// Whether a raw error string is a *permanent* provider failure — one that
/// will not fix itself within a session (a bad/expired API key, a revoked
/// token, or a forbidden credential).
///
/// This is the circuit-breaker's lens, distinct from [`classify`]'s retry
/// lens: a permanent failure means a sibling provider should be preferred for
/// the rest of the session rather than re-probed aggressively. Transient
/// failures (rate limit, server overload, network blips) are deliberately
/// excluded — a momentary "forbidden"-looking overload must not be mistaken
/// for a dead key — and account/quota 429s are already classified `Fatal`
/// upstream so they never reach the breaker.
#[must_use]
pub fn is_permanent_failure(raw: &str) -> bool {
    let msg = raw.to_lowercase();
    // Transient signals win: a 503/529 overload or a network reset is not a
    // permanent credential failure even if the body happens to say "forbidden".
    if is_transient_overload(&msg) {
        return false;
    }
    has_status_code(&msg, 401)
        || has_status_code(&msg, 403)
        || msg.contains("unauthorized")
        || msg.contains("forbidden")
}

/// Every wording a provider uses to say "your prompt does not fit the context
/// window". Matching any of them routes the turn into the reactive-compaction
/// rescue (`CompactAndRetry`) instead of killing the run.
///
/// This list used to hold only the five Anthropic shapes, which made the whole
/// rescue path dead code on OpenAI / vLLM / Ollama / Gemini: an overflow there
/// matched nothing, fell through to `Fatal`, and — because token estimation is
/// a fail-open heuristic that only recalibrates on a *successful* response —
/// bricked the session permanently.
///
/// Every entry must stay NARROW. The overflow check runs *before* the
/// quota-Fatal and 429 arms in [`classify`], so a loose pattern like
/// "too large" or "token limit" would hijack an OpenAI TPM rate-limit into a
/// pointless compact-and-retry against a provider that is throttling us, not
/// out of context.
/// The one status code in this set is tested with [`has_status_code`] rather
/// than as a substring — see the `413` note on [`CONTEXT_OVERFLOW_PATTERNS`].
const CONTEXT_OVERFLOW_STATUS: u16 = 413;

const CONTEXT_OVERFLOW_PATTERNS: &[&str] = &[
    // Anthropic: `prompt_too_long`, `request_too_large`.
    // `model_context_window_exceeded` is the same overflow surfaced as a *stop
    // reason* — the harness synthesizes an error carrying that marker to route
    // a context-exhausted stream into this same rescue.
    //
    // HTTP 413 belongs here too but is matched by [`CONTEXT_OVERFLOW_STATUS`]
    // instead: as a bare substring it also fires on `"4130 tokens"`, and this
    // check runs FIRST, so a false hit hijacks an unrelated failure into a
    // pointless compact-and-retry.
    "prompt is too long",
    "prompt_too_long",
    "request_too_large",
    "model_context_window_exceeded",
    // OpenAI and every OpenAI-compatible endpoint: error code
    // `context_length_exceeded`, message "This model's maximum context length
    // is N tokens. However, your messages resulted in M tokens. Please reduce
    // the length of the messages."
    "context_length_exceeded",
    "maximum context length",
    "context length exceeded",
    "reduce the length of the messages",
    // vLLM / local OpenAI-compatible servers name the window directly:
    // "... is longer than the maximum model length (max_model_len)".
    "max_model_len",
    // Ollama and llama.cpp-backed servers.
    "input is too long",
    // Gemini: "The input token count (N) exceeds the maximum number of tokens
    // allowed (M)."
    "input token count",
];

/// Classify a raw error message and decide whether to retry it in place.
///
/// The single classifier: `FailoverProvider` calls it on `AlephError` display
/// strings, and the reactive-compaction rescue calls it on its own.
#[must_use]
pub fn classify(raw: &str) -> RetryVerdict {
    let msg = raw.to_lowercase();

    // Context overflow → compact and retry (not a transient retry). Must stay
    // ahead of the 429/quota arms below; see [`CONTEXT_OVERFLOW_PATTERNS`] for
    // why the patterns are deliberately narrow.
    if has_status_code(&msg, CONTEXT_OVERFLOW_STATUS)
        || CONTEXT_OVERFLOW_PATTERNS.iter().any(|p| msg.contains(p))
    {
        return RetryVerdict::CompactAndRetry {
            token_gap: parse_token_gap_str(raw),
        };
    }

    // D3: "overloaded" / 529 must beat the 429 branch because some providers
    // (notably kimi-via-anthropic-protocol) return HTTP 429 with the body
    // `{"type":"rate_limit_error","message":"... engine is currently
    // overloaded ..."}`. Treating that as Fallback skips the local backoff +
    // jitter retry that would have ridden out the transient spike. Account /
    // org / quota errors keep their Fatal verdict even if the message text
    // happens to mention overload, since switching providers can't help.
    let account_patterns = ["account", "organization", "billing", "quota exceeded"];
    let is_account_scoped = account_patterns.iter().any(|p| msg.contains(p));

    if !is_account_scoped && is_transient_overload(&msg) {
        let delay = extract_retry_after_str(raw).unwrap_or(Duration::from_secs(2));
        return RetryVerdict::Retry { delay };
    }

    // Rate-limit (429) → classify as model-specific vs account-wide.
    // Model-specific limits benefit from switching providers (Fallback);
    // account-wide limits propagate immediately (Fatal).
    if has_status_code(&msg, 429) || msg.contains("rate limit") || msg.contains("rate_limit") {
        return classify_rate_limit(raw);
    }

    // Transient network errors → 300 ms base
    for pattern in &["connection", "reset", "timeout", "eof", "broken pipe"] {
        if msg.contains(pattern) {
            return RetryVerdict::Retry {
                delay: Duration::from_millis(300),
            };
        }
    }

    RetryVerdict::Fatal
}

/// Compute exponential backoff: `base * 2^attempt`, capped at `max_delay`.
#[must_use]
pub fn backoff_delay(base: Duration, attempt: u32, max_delay: Duration) -> Duration {
    let factor = 2u64.saturating_pow(attempt);
    let delay_ms = u64::try_from(base.as_millis())
        .unwrap_or(u64::MAX)
        .saturating_mul(factor);
    Duration::from_millis(delay_ms.min(u64::try_from(max_delay.as_millis()).unwrap_or(u64::MAX)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_code_matches_every_shape_a_real_message_uses() {
        for msg in [
            "http 429 too many requests",
            "anthropic api rate limited (429): ...",
            "status=429, body=...",
            "{\"code\":429}",
            "upstream returned 429",
            "429",
        ] {
            assert!(has_status_code(msg, 429), "should match in {msg:?}");
        }
    }

    #[test]
    fn status_code_ignores_digits_borrowed_from_a_longer_number() {
        // The bug this predicate exists for: every one of these is a real
        // shape of provider error text, and every one of them used to be
        // classified by the digits it merely *contains*.
        assert!(
            !has_status_code("rate limit reached: limit 40000, used 40123", 401),
            "a token count must not read as an auth failure"
        );
        assert!(
            !has_status_code("request id req_40450 failed", 404),
            "a request id must not read as model-not-found"
        );
        assert!(
            !has_status_code("prompt used 14290 tokens", 429),
            "a token count must not read as a rate limit"
        );
        assert!(
            !has_status_code("context is 4130 tokens over", 413),
            "a token count must not read as a context-overflow status"
        );
        // A later, genuine occurrence still wins over an earlier false one.
        assert!(has_status_code(
            "used 40123 tokens; http 401 unauthorized",
            401
        ));
    }

    #[test]
    fn a_token_count_no_longer_sheds_a_healthy_provider() {
        // End-to-end on the predicate's most expensive consumer: `Permanent`
        // opens the circuit breaker on the first strike with the full
        // ten-minute cooldown, so a false positive here takes a working
        // provider out of the chain for ten minutes.
        assert!(!is_permanent_failure(
            "HTTP 500 internal error (request consumed 40123 tokens)"
        ));
        assert!(is_permanent_failure("HTTP 401 Unauthorized"));
        assert!(is_permanent_failure("HTTP 403 Forbidden: invalid api key"));
    }

    #[test]
    fn test_classify_rate_limit_model_specific_fallback() {
        // Default 429 → Fallback (try switching provider)
        let err = anyhow::anyhow!("HTTP 429 Too Many Requests");
        assert!(matches!(
            classify(&err.to_string()),
            RetryVerdict::Fallback { .. }
        ));
    }

    #[test]
    fn test_classify_rate_limit_message_fallback() {
        let err = anyhow::anyhow!("Rate limit exceeded: 4500 requests per minute");
        assert!(matches!(
            classify(&err.to_string()),
            RetryVerdict::Fallback { .. }
        ));
    }

    #[test]
    fn test_classify_rate_limit_account_wide_fatal() {
        let err = anyhow::anyhow!("429 account quota exceeded");
        assert_eq!(classify(&err.to_string()), RetryVerdict::Fatal);
    }

    #[test]
    fn test_classify_rate_limit_org_fatal() {
        let err = anyhow::anyhow!("429 organization rate limit");
        assert_eq!(classify(&err.to_string()), RetryVerdict::Fatal);
    }

    #[test]
    fn test_anthropic_generic_transient_429_retries() {
        // Anthropic's server-side overload 429 (distinct from account/quota
        // limits) asks the client to wait and retry. It must classify as
        // Retry — same as "overloaded"/529 — so single-provider setups ride
        // out the spike with backoff instead of hard-failing with zero retries
        // (the failure mode that crashed scheduled jobs on 2026-05-29).
        let raw = r#"Rate limit error: Anthropic API rate limited (429): {"error":{"type":"rate_limit_error","message":"We're receiving too many requests at the moment. Please wait a moment and try again."},"type":"error"}"#;
        assert!(
            matches!(classify(raw), RetryVerdict::Retry { .. }),
            "generic transient 429 should retry, got {:?}",
            classify(raw)
        );
    }

    #[test]
    fn test_anthropic_account_429_with_retry_hint_stays_fatal() {
        // Even when the body carries a "please wait" hint, an account/quota
        // limit must stay Fatal — retrying or switching providers won't help.
        let raw = "429 account quota exceeded — please wait a moment and try again";
        assert_eq!(classify(raw), RetryVerdict::Fatal);
    }

    #[test]
    fn test_extract_retry_after_from_error() {
        let delay = extract_retry_after_str("Rate limited. Retry after 30 seconds.");
        assert_eq!(delay, Some(Duration::from_secs(30)));
    }

    #[test]
    fn test_extract_retry_after_none() {
        assert_eq!(extract_retry_after_str("HTTP 429 Too Many Requests"), None);
    }

    #[test]
    fn retry_after_header_reads_the_delay_seconds_form() {
        assert_eq!(retry_after_header_secs("120"), Some(120));
        assert_eq!(retry_after_header_secs(" 0 "), Some(0));
    }

    /// RFC 7231's second form. These assertions used to live on a parser with
    /// no callers (`providers::retry::parse_retry_after`) — which is exactly how
    /// the live path went on reading a date as its day-of-month unnoticed. They
    /// belong on the function production calls.
    ///
    /// The date is built from `now`, so what is pinned is the delta, not a
    /// fixed clock.
    #[test]
    fn retry_after_header_converts_an_http_date_to_a_delay() {
        const THREE_HOURS: u64 = 3 * 3600;
        let at = std::time::SystemTime::now() + Duration::from_secs(THREE_HOURS);
        let secs = retry_after_header_secs(&httpdate::fmt_http_date(at))
            .expect("an HTTP-date is a valid Retry-After");
        assert!(
            (THREE_HOURS - 5..=THREE_HOURS).contains(&secs),
            "expected ~3h, got {secs}s"
        );
    }

    #[test]
    fn retry_after_header_date_in_the_past_means_retry_now() {
        let at = std::time::SystemTime::now() - Duration::from_secs(3600);
        assert_eq!(
            retry_after_header_secs(&httpdate::fmt_http_date(at)),
            Some(0)
        );
    }

    #[test]
    fn retry_after_header_rejects_what_it_cannot_read() {
        // No guess is better than a wrong one: the caller falls back to a
        // suggestion that carries no number at all.
        assert_eq!(retry_after_header_secs("soon"), None);
        assert_eq!(retry_after_header_secs("-1"), None);
        assert_eq!(retry_after_header_secs(""), None);
    }

    #[test]
    fn test_classify_overloaded() {
        let err = anyhow::anyhow!("HTTP 529 overloaded");
        assert_eq!(
            classify(&err.to_string()),
            RetryVerdict::Retry {
                delay: Duration::from_secs(2)
            }
        );
    }

    /// D3: kimi-via-anthropic conflates overloaded → rate_limit_error so the
    /// wire error reads `429 ... rate_limit_error ... engine overloaded`.
    /// Pre-D3 this returned `Fallback` (no retry, switch provider). Post-D3:
    /// `Retry` so we ride out the transient spike with backoff + jitter on
    /// the same provider.
    #[test]
    fn test_classify_kimi_overloaded_429_is_retry() {
        let err = anyhow::anyhow!(
            "Rate limit error: Anthropic API rate limited (429): \
             {{\"error\":{{\"type\":\"rate_limit_error\",\
             \"message\":\"The engine is currently overloaded, please try again later\"}},\
             \"type\":\"error\"}}"
        );
        assert!(
            matches!(classify(&err.to_string()), RetryVerdict::Retry { .. }),
            "kimi-shaped 429+overloaded must be Retry (transient), got {:?}",
            classify(&err.to_string())
        );
    }

    /// D3: a 429 that BOTH names an account quota AND mentions overload is
    /// still Fatal — account-wide limits can't be ridden out, and switching
    /// providers won't help either.
    #[test]
    fn test_classify_account_429_overloaded_still_fatal() {
        let err = anyhow::anyhow!("429 account quota exceeded; server overloaded");
        assert_eq!(classify(&err.to_string()), RetryVerdict::Fatal);
    }

    #[test]
    fn test_classify_connection_error() {
        let err = anyhow::anyhow!("connection reset by peer");
        assert_eq!(
            classify(&err.to_string()),
            RetryVerdict::Retry {
                delay: Duration::from_millis(300)
            }
        );
    }

    #[test]
    fn test_classify_fatal() {
        let err = anyhow::anyhow!("HTTP 401 Unauthorized");
        assert_eq!(classify(&err.to_string()), RetryVerdict::Fatal);
    }

    #[test]
    fn test_is_permanent_failure_auth() {
        // `decide()` feeds this the RAW provider error (which always carries
        // the HTTP status token), not the synthesized "authentication failed"
        // reason string — so the classifier keys off the status codes.
        assert!(is_permanent_failure("HTTP 401 Unauthorized"));
        assert!(is_permanent_failure("HTTP 403 Forbidden"));
        assert!(is_permanent_failure(
            "OpenAI Chat API error (403): Forbidden — invalid api key"
        ));
        assert!(is_permanent_failure("request failed: 401 unauthorized"));
    }

    #[test]
    fn test_is_permanent_failure_excludes_transient() {
        // Rate limit / overload / network are transient — not permanent.
        assert!(!is_permanent_failure("HTTP 429 too many requests"));
        assert!(!is_permanent_failure("HTTP 529 overloaded"));
        assert!(!is_permanent_failure("connection reset by peer"));
        // A 403-looking body that is really a transient overload must not be
        // treated as a dead credential.
        assert!(!is_permanent_failure(
            "403 forbidden: engine is currently overloaded, please try again later"
        ));
    }

    #[test]
    fn test_classify_unknown_fatal() {
        let err = anyhow::anyhow!("something completely unknown went wrong");
        assert_eq!(classify(&err.to_string()), RetryVerdict::Fatal);
    }

    #[test]
    fn test_backoff_delay() {
        let base = Duration::from_millis(100);
        let max = Duration::from_secs(5);

        // attempt 0 → 100ms * 2^0 = 100ms
        assert_eq!(backoff_delay(base, 0, max), Duration::from_millis(100));
        // attempt 1 → 100ms * 2^1 = 200ms
        assert_eq!(backoff_delay(base, 1, max), Duration::from_millis(200));
        // attempt 2 → 100ms * 2^2 = 400ms
        assert_eq!(backoff_delay(base, 2, max), Duration::from_millis(400));
        // attempt 3 → 100ms * 2^3 = 800ms
        assert_eq!(backoff_delay(base, 3, max), Duration::from_millis(800));
        // attempt 10 → 100ms * 1024 = 102400ms, capped at 5000ms
        assert_eq!(backoff_delay(base, 10, max), max);
    }

    // --- parse_token_gap tests ---

    #[test]
    fn test_parse_token_gap_standard_format() {
        let err = anyhow::anyhow!("prompt is too long: 137500 tokens > 135000 maximum");
        assert_eq!(parse_token_gap_str(&err.to_string()), Some(2500));
    }

    #[test]
    fn test_parse_token_gap_no_match() {
        let err = anyhow::anyhow!("HTTP 413 Payload Too Large");
        assert_eq!(parse_token_gap_str(&err.to_string()), None);
    }

    #[test]
    fn test_parse_token_gap_large_gap() {
        let err = anyhow::anyhow!("prompt is too long: 200000 tokens > 128000 maximum");
        assert_eq!(parse_token_gap_str(&err.to_string()), Some(72000));
    }

    // --- classify_error 413/prompt_too_long tests ---

    #[test]
    fn test_classify_413_status() {
        let err = anyhow::anyhow!("HTTP 413 Payload Too Large");
        assert!(matches!(
            classify(&err.to_string()),
            RetryVerdict::CompactAndRetry { token_gap: None }
        ));
    }

    #[test]
    fn test_classify_context_window_exceeded_stop_reason_marker() {
        // The harness synthesizes this message when a stream ends with
        // stop_reason `model_context_window_exceeded` — it must route to the
        // same compaction rescue as prompt_too_long.
        let err = anyhow::anyhow!(
            "Provider error: model_context_window_exceeded: provider stopped \
             because the context window is full"
        );
        assert!(matches!(
            classify(&err.to_string()),
            RetryVerdict::CompactAndRetry { token_gap: None }
        ));
    }

    #[test]
    fn test_classify_prompt_too_long_with_gap() {
        let err = anyhow::anyhow!("prompt is too long: 137500 tokens > 135000 maximum");
        assert!(matches!(
            classify(&err.to_string()),
            RetryVerdict::CompactAndRetry {
                token_gap: Some(2500)
            }
        ));
    }

    #[test]
    fn test_classify_prompt_too_long_no_numbers() {
        let err = anyhow::anyhow!("Error: prompt_too_long");
        assert!(matches!(
            classify(&err.to_string()),
            RetryVerdict::CompactAndRetry { token_gap: None }
        ));
    }

    /// The compaction rescue must be reachable on every provider, not just
    /// Anthropic. Before this list was widened these four bodies all fell
    /// through to `Fatal` and killed the run — and, because usage is only
    /// recalibrated after a *successful* response, the under-counting session
    /// stayed bricked forever.
    #[test]
    fn test_classify_context_overflow_non_anthropic_providers() {
        let bodies = [
            // OpenAI (and every OpenAI-compatible endpoint).
            r#"OpenAI Chat API error (400): {"error":{"message":"This model's maximum context length is 128000 tokens. However, your messages resulted in 131500 tokens. Please reduce the length of the messages.","type":"invalid_request_error","param":"messages","code":"context_length_exceeded"}}"#,
            // vLLM names the window by its config key.
            "Provider error: ValueError: The prompt (9000 tokens) is longer than the model's max_model_len (8192)",
            // Ollama / llama.cpp-backed servers.
            r#"Ollama API error: {"error":"input is too long for this model's context window"}"#,
            // Gemini.
            r#"Gemini API error (400): {"error":{"code":400,"message":"The input token count (1200000) exceeds the maximum number of tokens allowed (1048576).","status":"INVALID_ARGUMENT"}}"#,
        ];
        for body in bodies {
            assert!(
                matches!(classify(body), RetryVerdict::CompactAndRetry { .. }),
                "context overflow must route to compaction, got {:?} for {body}",
                classify(body)
            );
        }
    }

    /// Guards the narrowness of `CONTEXT_OVERFLOW_PATTERNS`. The overflow check
    /// runs BEFORE the quota/429 arms, so widening it to loose words like
    /// "token limit" or "too large" would turn an OpenAI tokens-per-minute
    /// throttle — which needs a backoff or a failover — into a pointless
    /// compact-and-retry against a context that was never too big.
    #[test]
    fn test_openai_tpm_429_is_not_hijacked_into_compaction() {
        let org_scoped = r#"Rate limit error: OpenAI Chat API rate limited (429): {"error":{"message":"Rate limit reached for gpt-4o in organization org-abc123 on tokens per min (TPM): Limit 30000, Used 28500, Requested 5000. Please try again in 6s.","type":"tokens","code":"rate_limit_exceeded"}}"#;
        assert_eq!(
            classify(org_scoped),
            RetryVerdict::Fatal,
            "an org-wide TPM limit must stay Fatal, not compact"
        );

        let model_scoped = r#"Rate limit error: OpenAI Chat API rate limited (429): {"error":{"message":"Rate limit reached on tokens per min (TPM): Limit 30000, Used 28500, Requested 5000. Please try again in 6s.","code":"rate_limit_exceeded"}}"#;
        assert!(
            matches!(classify(model_scoped), RetryVerdict::Fallback { .. }),
            "a model-scoped TPM limit must stay Fallback, not compact — got {:?}",
            classify(model_scoped)
        );
    }

    // --- classify_exhausted_error tests ---

    #[test]
    fn test_classify_exhausted_rate_limit_fallback() {
        // Default 429 → Fallback (model-specific, try another provider)
        let err = anyhow::anyhow!("HTTP 429 Too Many Requests");
        assert!(matches!(
            classify_exhausted(&err.to_string()),
            RetryVerdict::Fallback { .. }
        ));
    }

    #[test]
    fn test_classify_exhausted_rate_limit_chinese_fallback() {
        // Chinese provider error: model-specific rate limit → Fallback
        let err = anyhow::anyhow!(
            "Rate limit error: OpenAI Chat API rate limited (429): 请求频率达到限制：1分钟内最多4500次"
        );
        assert!(matches!(
            classify_exhausted(&err.to_string()),
            RetryVerdict::Fallback { .. }
        ));
    }

    #[test]
    fn test_classify_exhausted_account_rate_limit_is_fatal() {
        let err = anyhow::anyhow!("429 account quota exceeded");
        assert_eq!(classify_exhausted(&err.to_string()), RetryVerdict::Fatal);
    }

    #[test]
    fn test_classify_exhausted_overloaded() {
        let err = anyhow::anyhow!("HTTP 529 overloaded");
        assert!(matches!(
            classify_exhausted(&err.to_string()),
            RetryVerdict::Fallback { .. }
        ));
    }

    #[test]
    fn test_classify_exhausted_network() {
        let err = anyhow::anyhow!("connection reset by peer");
        assert!(matches!(
            classify_exhausted(&err.to_string()),
            RetryVerdict::Fallback { .. }
        ));
    }

    #[test]
    fn test_classify_exhausted_404() {
        let err = anyhow::anyhow!("HTTP 404 model not found");
        assert!(matches!(
            classify_exhausted(&err.to_string()),
            RetryVerdict::Fallback { .. }
        ));
    }

    #[test]
    fn test_classify_exhausted_401() {
        let err = anyhow::anyhow!("HTTP 401 Unauthorized");
        assert!(matches!(
            classify_exhausted(&err.to_string()),
            RetryVerdict::Fallback { .. }
        ));
    }

    #[test]
    fn test_classify_exhausted_400_stays_fatal() {
        let err = anyhow::anyhow!("HTTP 400 Bad Request: invalid parameter");
        assert_eq!(classify_exhausted(&err.to_string()), RetryVerdict::Fatal);
    }

    #[test]
    fn test_classify_exhausted_413_stays_compact() {
        let err = anyhow::anyhow!("HTTP 413 prompt is too long: 137500 tokens > 135000 maximum");
        assert!(matches!(
            classify_exhausted(&err.to_string()),
            RetryVerdict::CompactAndRetry { .. }
        ));
    }
}
