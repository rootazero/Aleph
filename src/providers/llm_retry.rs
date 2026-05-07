//! LLM call retry with error classification and exponential backoff.
//!
//! Classifies errors as transient (rate-limit, overloaded, connection) or fatal,
//! then retries transient failures with exponential backoff. Backoff waits are
//! cancellation-aware via `CancellationToken`.

use std::future::Future;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// Maximum delay cap for exponential backoff.
const MAX_DELAY: Duration = Duration::from_secs(30);

/// Outcome of classifying an error for retry decisions.
#[derive(Debug, Clone, PartialEq)]
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

/// Extract token gap from "prompt is too long: X tokens > Y maximum" error messages.
pub fn parse_token_gap(err: &anyhow::Error) -> Option<usize> {
    let msg = err.to_string();
    let lower = msg.to_lowercase();
    if !lower.contains("prompt is too long") && !lower.contains("prompt_too_long") {
        return None;
    }
    // Find "N tokens > M" pattern manually
    let tokens_idx = lower.find("tokens")?;
    let before_tokens = msg[..tokens_idx].trim_end();
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
/// Called by the main loop when `retry_async` returns its final error.
/// Reclassifies transient errors as `Fallback` since retries didn't help.
/// 413 errors pass through as `CompactAndRetry` (handled separately).
/// 400 errors remain `Fatal` (request itself is broken).
pub fn classify_exhausted_error(err: &anyhow::Error) -> RetryVerdict {
    let base = classify_error(err);

    // 413 → still CompactAndRetry (main loop handles compression)
    if matches!(base, RetryVerdict::CompactAndRetry { .. }) {
        return base;
    }

    let msg = err.to_string().to_lowercase();

    // 429 rate limit → classify as model-specific (Fallback) vs account-wide (Fatal).
    if msg.contains("429") || msg.contains("rate limit") || msg.contains("rate_limit") {
        return classify_rate_limit(err);
    }

    // 400 → Fatal (request is malformed, fallback won't help)
    if msg.contains("400") && (msg.contains("bad request") || msg.contains("invalid")) {
        return RetryVerdict::Fatal;
    }

    // 404 model not found → Fallback
    if msg.contains("404") || msg.contains("not found") {
        return RetryVerdict::Fallback {
            reason: "model not found".into(),
        };
    }

    // 401/403 auth errors → Fallback (but caller should notify user)
    if msg.contains("401")
        || msg.contains("403")
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
            reason: format!("primary model unavailable: {}", err),
        };
    }

    // Everything else stays Fatal
    RetryVerdict::Fatal
}

/// Extract a Retry-After delay from an error message.
///
/// Providers embed "Retry after N seconds" in the suggestion/message field.
/// We parse this to use server-guided delays instead of hardcoded values.
fn extract_retry_after(err: &anyhow::Error) -> Option<Duration> {
    let msg = err.to_string().to_lowercase();
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
fn classify_rate_limit(err: &anyhow::Error) -> RetryVerdict {
    let msg = err.to_string().to_lowercase();

    // Account-wide / org-level → Fatal (switching won't help)
    let account_patterns = ["account", "organization", "billing", "quota exceeded"];
    if account_patterns.iter().any(|p| msg.contains(p)) {
        return RetryVerdict::Fatal;
    }

    // Default: treat as model-specific → Fallback with server-guided delay
    let reason = if let Some(delay) = extract_retry_after(err) {
        format!("rate limited (retry after {}s)", delay.as_secs())
    } else {
        "rate limited".to_string()
    };
    RetryVerdict::Fallback { reason }
}

/// Inspect an `anyhow::Error` display string and decide whether to retry.
pub fn classify_error(err: &anyhow::Error) -> RetryVerdict {
    let msg = err.to_string().to_lowercase();

    // Prompt too long / 413 → compact and retry (not a transient retry)
    if msg.contains("413")
        || msg.contains("prompt is too long")
        || msg.contains("prompt_too_long")
        || msg.contains("request_too_large")
    {
        return RetryVerdict::CompactAndRetry {
            token_gap: parse_token_gap(err),
        };
    }

    // Rate-limit (429) → classify as model-specific vs account-wide.
    // Model-specific limits benefit from switching providers (Fallback);
    // account-wide limits propagate immediately (Fatal).
    if msg.contains("429") || msg.contains("rate limit") || msg.contains("rate_limit") {
        return classify_rate_limit(err);
    }

    // Overloaded (529) → retryable with server-guided or default 2s backoff
    if msg.contains("529") || msg.contains("overloaded") {
        let delay = extract_retry_after(err).unwrap_or(Duration::from_secs(2));
        return RetryVerdict::Retry { delay };
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
pub fn backoff_delay(base: Duration, attempt: u32, max_delay: Duration) -> Duration {
    let factor = 2u64.saturating_pow(attempt);
    let delay_ms = (base.as_millis() as u64).saturating_mul(factor);
    Duration::from_millis(delay_ms.min(max_delay.as_millis() as u64))
}

/// Retry an async operation with error classification and exponential backoff.
///
/// Calls `make_future()` up to `max_retries + 1` times. On transient errors the
/// wait is cancellation-aware: if `cancel` fires during the backoff sleep the
/// function returns immediately with a cancellation error.
pub async fn retry_async<F, Fut, T>(
    make_future: F,
    cancel: &CancellationToken,
    max_retries: usize,
) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let mut attempt: u32 = 0;

    loop {
        match make_future().await {
            Ok(val) => return Ok(val),
            Err(err) => {
                let verdict = classify_error(&err);
                let retries_left = max_retries as u32 - attempt;

                match verdict {
                    RetryVerdict::Fatal
                    | RetryVerdict::CompactAndRetry { .. }
                    | RetryVerdict::Fallback { .. } => {
                        return Err(err);
                    }
                    RetryVerdict::Retry { .. } if retries_left == 0 => {
                        return Err(err);
                    }
                    _ => {}
                }

                let base_delay = match &verdict {
                    RetryVerdict::Retry { delay } => *delay,
                    _ => unreachable!(),
                };

                let delay = backoff_delay(base_delay, attempt, MAX_DELAY);
                tracing::warn!(
                    attempt = attempt + 1,
                    delay_ms = delay.as_millis() as u64,
                    error = %err,
                    "Transient error, retrying after backoff"
                );

                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = cancel.cancelled() => {
                        return Err(anyhow::anyhow!("Cancelled during retry backoff"));
                    }
                }

                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_classify_rate_limit_model_specific_fallback() {
        // Default 429 → Fallback (try switching provider)
        let err = anyhow::anyhow!("HTTP 429 Too Many Requests");
        assert!(matches!(
            classify_error(&err),
            RetryVerdict::Fallback { .. }
        ));
    }

    #[test]
    fn test_classify_rate_limit_message_fallback() {
        let err = anyhow::anyhow!("Rate limit exceeded: 4500 requests per minute");
        assert!(matches!(
            classify_error(&err),
            RetryVerdict::Fallback { .. }
        ));
    }

    #[test]
    fn test_classify_rate_limit_account_wide_fatal() {
        let err = anyhow::anyhow!("429 account quota exceeded");
        assert_eq!(classify_error(&err), RetryVerdict::Fatal);
    }

    #[test]
    fn test_classify_rate_limit_org_fatal() {
        let err = anyhow::anyhow!("429 organization rate limit");
        assert_eq!(classify_error(&err), RetryVerdict::Fatal);
    }

    #[test]
    fn test_extract_retry_after_from_error() {
        let err = anyhow::anyhow!("Rate limited. Retry after 30 seconds.");
        let delay = extract_retry_after(&err);
        assert_eq!(delay, Some(Duration::from_secs(30)));
    }

    #[test]
    fn test_extract_retry_after_none() {
        let err = anyhow::anyhow!("HTTP 429 Too Many Requests");
        assert_eq!(extract_retry_after(&err), None);
    }

    #[test]
    fn test_classify_overloaded() {
        let err = anyhow::anyhow!("HTTP 529 overloaded");
        assert_eq!(
            classify_error(&err),
            RetryVerdict::Retry {
                delay: Duration::from_secs(2)
            }
        );
    }

    #[test]
    fn test_classify_connection_error() {
        let err = anyhow::anyhow!("connection reset by peer");
        assert_eq!(
            classify_error(&err),
            RetryVerdict::Retry {
                delay: Duration::from_millis(300)
            }
        );
    }

    #[test]
    fn test_classify_fatal() {
        let err = anyhow::anyhow!("HTTP 401 Unauthorized");
        assert_eq!(classify_error(&err), RetryVerdict::Fatal);
    }

    #[test]
    fn test_classify_unknown_fatal() {
        let err = anyhow::anyhow!("something completely unknown went wrong");
        assert_eq!(classify_error(&err), RetryVerdict::Fatal);
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
        assert_eq!(parse_token_gap(&err), Some(2500));
    }

    #[test]
    fn test_parse_token_gap_no_match() {
        let err = anyhow::anyhow!("HTTP 413 Payload Too Large");
        assert_eq!(parse_token_gap(&err), None);
    }

    #[test]
    fn test_parse_token_gap_large_gap() {
        let err = anyhow::anyhow!("prompt is too long: 200000 tokens > 128000 maximum");
        assert_eq!(parse_token_gap(&err), Some(72000));
    }

    // --- classify_error 413/prompt_too_long tests ---

    #[test]
    fn test_classify_413_status() {
        let err = anyhow::anyhow!("HTTP 413 Payload Too Large");
        assert!(matches!(
            classify_error(&err),
            RetryVerdict::CompactAndRetry { token_gap: None }
        ));
    }

    #[test]
    fn test_classify_prompt_too_long_with_gap() {
        let err = anyhow::anyhow!("prompt is too long: 137500 tokens > 135000 maximum");
        assert!(matches!(
            classify_error(&err),
            RetryVerdict::CompactAndRetry {
                token_gap: Some(2500)
            }
        ));
    }

    #[test]
    fn test_classify_prompt_too_long_no_numbers() {
        let err = anyhow::anyhow!("Error: prompt_too_long");
        assert!(matches!(
            classify_error(&err),
            RetryVerdict::CompactAndRetry { token_gap: None }
        ));
    }

    #[tokio::test]
    async fn test_retry_succeeds_first_try() {
        let cancel = CancellationToken::new();
        let result = retry_async(|| async { Ok::<_, anyhow::Error>(42) }, &cancel, 3).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_transient_failure() {
        let counter = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();

        let c = counter.clone();
        let result = retry_async(
            move || {
                let c = c.clone();
                async move {
                    let n = c.fetch_add(1, Ordering::SeqCst);
                    if n < 2 {
                        Err(anyhow::anyhow!("connection reset"))
                    } else {
                        Ok(99)
                    }
                }
            },
            &cancel,
            5,
        )
        .await;

        assert_eq!(result.unwrap(), 99);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_fatal_no_retry() {
        let counter = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();

        let c = counter.clone();
        let result: anyhow::Result<i32> = retry_async(
            move || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err(anyhow::anyhow!("HTTP 401 Unauthorized"))
                }
            },
            &cancel,
            5,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_cancelled_during_backoff() {
        let cancel = CancellationToken::new();

        // Cancel after 50ms
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        let result: anyhow::Result<i32> = retry_async(
            || async { Err(anyhow::anyhow!("connection timeout")) },
            &cancel,
            10,
        )
        .await;

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Cancelled during retry backoff"),
            "Expected cancellation error, got: {err_msg}"
        );
    }

    // --- classify_exhausted_error tests ---

    #[test]
    fn test_classify_exhausted_rate_limit_fallback() {
        // Default 429 → Fallback (model-specific, try another provider)
        let err = anyhow::anyhow!("HTTP 429 Too Many Requests");
        assert!(matches!(
            classify_exhausted_error(&err),
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
            classify_exhausted_error(&err),
            RetryVerdict::Fallback { .. }
        ));
    }

    #[test]
    fn test_classify_exhausted_account_rate_limit_is_fatal() {
        let err = anyhow::anyhow!("429 account quota exceeded");
        assert_eq!(classify_exhausted_error(&err), RetryVerdict::Fatal);
    }

    #[test]
    fn test_classify_exhausted_overloaded() {
        let err = anyhow::anyhow!("HTTP 529 overloaded");
        assert!(matches!(
            classify_exhausted_error(&err),
            RetryVerdict::Fallback { .. }
        ));
    }

    #[test]
    fn test_classify_exhausted_network() {
        let err = anyhow::anyhow!("connection reset by peer");
        assert!(matches!(
            classify_exhausted_error(&err),
            RetryVerdict::Fallback { .. }
        ));
    }

    #[test]
    fn test_classify_exhausted_404() {
        let err = anyhow::anyhow!("HTTP 404 model not found");
        assert!(matches!(
            classify_exhausted_error(&err),
            RetryVerdict::Fallback { .. }
        ));
    }

    #[test]
    fn test_classify_exhausted_401() {
        let err = anyhow::anyhow!("HTTP 401 Unauthorized");
        assert!(matches!(
            classify_exhausted_error(&err),
            RetryVerdict::Fallback { .. }
        ));
    }

    #[test]
    fn test_classify_exhausted_400_stays_fatal() {
        let err = anyhow::anyhow!("HTTP 400 Bad Request: invalid parameter");
        assert_eq!(classify_exhausted_error(&err), RetryVerdict::Fatal);
    }

    #[test]
    fn test_classify_exhausted_413_stays_compact() {
        let err = anyhow::anyhow!("HTTP 413 prompt is too long: 137500 tokens > 135000 maximum");
        assert!(matches!(
            classify_exhausted_error(&err),
            RetryVerdict::CompactAndRetry { .. }
        ));
    }

    #[tokio::test]
    async fn test_retry_exhausted_rate_limit() {
        let counter = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();

        let c = counter.clone();
        let result: anyhow::Result<i32> = retry_async(
            move || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err(anyhow::anyhow!("429 rate limit exceeded"))
                }
            },
            &cancel,
            2,
        )
        .await;

        assert!(result.is_err());
        // 429 is now Fallback — not retried by retry_async, only 1 attempt
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_exhausted_account_rate_limit() {
        let counter = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();

        let c = counter.clone();
        let result: anyhow::Result<i32> = retry_async(
            move || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err(anyhow::anyhow!("429 account quota exceeded"))
                }
            },
            &cancel,
            2,
        )
        .await;

        assert!(result.is_err());
        // Account-wide 429 is Fatal — no retries, only 1 attempt
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
