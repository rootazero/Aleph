/// Retry logic with exponential backoff for AI provider requests
///
/// This module provides utilities for retrying failed requests with
/// exponential backoff strategy. Inspired by `OpenCode`'s retry.ts.
use crate::config::RetryPolicy;
use crate::error::{AlephError, Result};
use crate::tool_metadata::DEFAULT_MAX_RETRIES;
use rand::RngExt;
use std::future::Future;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Constants matching `OpenCode`'s retry.ts
pub const RETRY_INITIAL_DELAY_MS: u64 = 2000; // 2 seconds
pub const RETRY_BACKOFF_FACTOR: f64 = 2.0;
pub const RETRY_MAX_DELAY_NO_HEADERS_MS: u64 = 30_000; // 30 seconds
pub const RETRY_MAX_DELAY_WITH_HEADERS_MS: u64 = i32::MAX as u64; // ~24 days (matches JS max setTimeout)

/// Initial backoff duration (1 second) (default, used when no policy provided)
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Default jitter factor for `retry_with_backoff` — matches `RetryPolicy::default()`.
/// Equal-jitter shape: each backoff is widened by `[0, base * 0.25]`.
const DEFAULT_JITTER_FACTOR: f64 = 0.25;

/// Add a random jitter on top of a deterministic backoff duration.
///
/// `factor` is the maximum jitter as a fraction of `base` — `0.0` disables
/// jitter (returns `base` unchanged), `0.25` widens to `[base, base * 1.25]`,
/// `1.0` widens to `[base, base * 2.0]`. Values are clamped to `[0.0, 1.0]`
/// — larger factors do not produce wider spread.
///
/// Why this exists: concurrent agents that share a rate-limited provider
/// (e.g. multiple subagents under the same Anthropic key) otherwise retry
/// in lockstep — a thundering herd that defeats the very rate limit they
/// just hit. Spreading retries across a window of `factor * base` decorrelates
/// the storm without ever sleeping *less* than the deterministic backoff,
/// so the "at least exponential" contract is preserved.
#[must_use]
pub fn apply_jitter(base: Duration, factor: f64) -> Duration {
    if factor <= 0.0 {
        return base;
    }
    let base_ms = u64::try_from(base.as_millis()).unwrap_or(u64::MAX);
    if base_ms == 0 {
        return base;
    }
    let factor = factor.min(1.0);
    let max_extra_ms = ((base_ms as f64) * factor) as u64;
    if max_extra_ms == 0 {
        return base;
    }
    let extra = rand::rng().random_range(0..=max_extra_ms);
    Duration::from_millis(base_ms.saturating_add(extra))
}

/// Determines if an error is retryable using default policy.
fn is_retryable(error: &AlephError) -> bool {
    let default_policy = RetryPolicy::default();
    is_retryable_with_policy(error, &default_policy)
}

/// Determines if an error is retryable using provided policy.
///
/// Retryable errors:
/// - Network errors (if `retry_on_network_error` is true)
/// - Timeout errors (if `retry_on_timeout` is true)
/// - Server errors (matching status codes in policy)
///
/// Non-retryable errors:
/// - Authentication errors (401)
/// - Rate limit errors (429) - UNLESS it's an overloaded error
/// - Invalid configuration
/// - Provider-specific errors not matching policy
fn is_retryable_with_policy(error: &AlephError, policy: &RetryPolicy) -> bool {
    match error {
        AlephError::NetworkError { .. } => policy.retry_on_network_error,
        AlephError::Timeout { .. } => policy.retry_on_timeout,
        AlephError::ProviderError { message, .. } => {
            // Check for overloaded messages (retryable like OpenCode)
            if is_overloaded_message(message) {
                return true;
            }
            // Check if message contains any retryable status code
            policy
                .retryable_status_codes
                .iter()
                .any(|code| message.contains(&code.to_string()))
        }
        // Rate limit errors are NOT retryable — retrying amplifies the problem.
        AlephError::RateLimitError { .. } => false,
        // Don't retry these errors
        AlephError::AuthenticationError { .. } => false,
        AlephError::InvalidConfig { .. } => false,
        _ => false,
    }
}

/// Check if error message indicates an overloaded condition (retryable)
///
/// Matches server-side overload conditions that are transient.
/// NOTE: "rate limit" and "`too_many_requests`" are intentionally excluded —
/// rate limits indicate quota exhaustion and retrying amplifies the problem.
fn is_overloaded_message(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("overloaded") || lower.contains("exhausted") || lower.contains("capacity")
}

/// Extended retryable check that returns the reason if retryable
///
/// Matches `OpenCode`'s `retryable()` function signature.
#[must_use]
pub fn retryable_reason(error: &AlephError) -> Option<String> {
    let default_policy = RetryPolicy::default();
    if is_retryable_with_policy(error, &default_policy) {
        Some(format!("{error}"))
    } else {
        None
    }
}

/// Calculate delay for a retry attempt
///
/// This matches `OpenCode`'s `delay()` function from retry.ts.
/// Priority:
/// 1. Use `retry_after_ms` if provided (from Retry-After-Ms header)
/// 2. Use `retry_after_secs` if provided (from Retry-After header, parsed)
/// 3. Fall back to exponential backoff
#[must_use]
pub fn calculate_delay(
    attempt: u32,
    retry_after_ms: Option<u64>,
    has_retry_header: bool,
) -> Duration {
    // Check for retry-after header values
    if let Some(ms) = retry_after_ms {
        let max_delay = if has_retry_header {
            RETRY_MAX_DELAY_WITH_HEADERS_MS
        } else {
            RETRY_MAX_DELAY_NO_HEADERS_MS
        };
        // Floor at the initial delay: a `Retry-After: 0` must not collapse the
        // backoff to a zero sleep that immediately re-hammers the provider.
        return Duration::from_millis(ms.max(RETRY_INITIAL_DELAY_MS).min(max_delay));
    }

    // Exponential backoff: initial * factor^(attempt-1)
    let exponent = attempt.saturating_sub(1);
    let delay_ms = (RETRY_INITIAL_DELAY_MS as f64)
        * RETRY_BACKOFF_FACTOR.powi(i32::try_from(exponent).unwrap_or(i32::MAX));
    let capped_ms = (delay_ms as u64).min(RETRY_MAX_DELAY_NO_HEADERS_MS);
    Duration::from_millis(capped_ms)
}

/// Parse Retry-After header value
///
/// The header can be either:
/// - A number of seconds (e.g., "120")
/// - An HTTP date (e.g., "Wed, 21 Oct 2015 07:28:00 GMT")
///
/// Returns the delay in milliseconds.
#[must_use]
pub fn parse_retry_after(value: &str) -> Option<u64> {
    // Try parsing as seconds first
    if let Ok(secs) = value.parse::<u64>() {
        return Some(secs.saturating_mul(1000));
    }

    // Try parsing as HTTP date
    if let Ok(date) = httpdate::parse_http_date(value) {
        let now = std::time::SystemTime::now();
        if let Ok(duration) = date.duration_since(now) {
            return Some(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
        }
    }

    None
}

/// Retry a future with exponential backoff
///
/// # Arguments
/// * `operation` - The async operation to retry
/// * `max_retries` - Maximum number of retry attempts (default: 3)
///
/// # Returns
/// * `Ok(T)` - If operation succeeds
/// * `Err(AlephError)` - If all retry attempts fail
///
/// # Retry Strategy
/// - Attempt 1: Immediate
/// - Attempt 2: Wait 1s
/// - Attempt 3: Wait 2s
/// - Attempt 4: Wait 4s
///
/// # Example
/// ```rust,ignore
/// use alephcore::providers::retry::retry_with_backoff;
///
/// async fn fetch_data() -> Result<String, alephcore::error::AlephError> {
///     // ... network request
///     Ok("data".to_string())
/// }
///
/// let result = retry_with_backoff(|| fetch_data(), None).await;
/// ```
pub async fn retry_with_backoff<F, Fut, T>(mut operation: F, max_retries: Option<u32>) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let max_retries = max_retries.unwrap_or(DEFAULT_MAX_RETRIES);
    let mut attempt = 0;

    loop {
        attempt += 1;

        match operation().await {
            Ok(result) => {
                if attempt > 1 {
                    info!(attempts = attempt, "Operation succeeded after retry");
                }
                return Ok(result);
            }
            Err(error) => {
                // Check if we should retry
                if !is_retryable(&error) {
                    debug!(
                        error = ?error,
                        "Error is not retryable, failing immediately"
                    );
                    return Err(error);
                }

                // Check if we've exhausted retries
                if attempt >= max_retries {
                    warn!(
                        max_retries,
                        attempt,
                        error = ?error,
                        "Max retries exceeded, giving up"
                    );
                    return Err(error);
                }

                // Calculate backoff duration (exponential: 1s, 2s, 4s) with overflow protection
                let backoff_secs = INITIAL_BACKOFF.as_secs_f64() * 2_f64.powi(attempt as i32 - 1);
                let backoff = Duration::from_secs_f64(backoff_secs.min(30.0));
                // Spread retry storms across concurrent callers (see `apply_jitter`).
                let backoff = apply_jitter(backoff, DEFAULT_JITTER_FACTOR);

                warn!(
                    attempt,
                    error = ?error,
                    backoff_ms = backoff.as_millis(),
                    "Attempt failed, retrying with backoff"
                );

                // Wait before retrying
                tokio::time::sleep(backoff).await;
            }
        }
    }
}

/// Retry a future with exponential backoff using policy configuration.
///
/// This version uses the provided `RetryPolicy` for all retry behavior,
/// including max retries, backoff timing, and error classification.
///
/// # Arguments
/// * `operation` - The async operation to retry
/// * `policy` - The retry policy configuration
///
/// # Returns
/// * `Ok(T)` - If operation succeeds
/// * `Err(AlephError)` - If all retry attempts fail
pub async fn retry_with_policy<F, Fut, T>(mut operation: F, policy: &RetryPolicy) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let max_retries = policy.max_retries;
    let initial_backoff = Duration::from_millis(policy.initial_backoff_ms);
    let multiplier = policy.backoff_multiplier;

    let mut attempt = 0;

    loop {
        attempt += 1;

        match operation().await {
            Ok(result) => {
                if attempt > 1 {
                    info!(attempts = attempt, "Operation succeeded after retry");
                }
                return Ok(result);
            }
            Err(error) => {
                // Check if we should retry using policy
                if !is_retryable_with_policy(&error, policy) {
                    debug!(
                        error = ?error,
                        "Error is not retryable per policy, failing immediately"
                    );
                    return Err(error);
                }

                // Check if we've exhausted retries
                if attempt >= max_retries {
                    warn!(
                        max_retries,
                        attempt,
                        error = ?error,
                        "Max retries exceeded per policy, giving up"
                    );
                    return Err(error);
                }

                // Calculate backoff duration using policy multiplier (with overflow protection)
                let backoff_secs =
                    initial_backoff.as_secs_f64() * multiplier.powi(attempt as i32 - 1);
                let backoff = Duration::from_secs_f64(backoff_secs.min(300.0));
                // Spread retry storms across concurrent callers (see `apply_jitter`).
                let backoff = apply_jitter(backoff, policy.jitter_factor);

                warn!(
                    attempt,
                    error = ?error,
                    backoff_ms = backoff.as_millis(),
                    "Attempt failed, retrying with policy-based backoff"
                );

                // Wait before retrying
                tokio::time::sleep(backoff).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Arc;
    use crate::sync_primitives::{AtomicU32, Ordering};

    #[test]
    fn test_is_retryable() {
        // Retryable errors
        assert!(is_retryable(&AlephError::network("connection failed")));
        assert!(is_retryable(&AlephError::Timeout { suggestion: None }));
        assert!(is_retryable(&AlephError::provider(
            "500 Internal Server Error"
        )));
        assert!(is_retryable(&AlephError::provider(
            "503 Service Unavailable"
        )));

        // Non-retryable errors
        assert!(!is_retryable(&AlephError::authentication(
            "Test",
            "invalid key"
        )));
        assert!(!is_retryable(&AlephError::rate_limit("quota exceeded")));
        assert!(!is_retryable(&AlephError::invalid_config("bad config")));
        assert!(!is_retryable(&AlephError::provider("400 Bad Request")));
    }

    #[tokio::test]
    async fn test_retry_success_first_attempt() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result: Result<String> = retry_with_backoff(
            || {
                let counter = counter_clone.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, AlephError>("success".to_string())
                }
            },
            Some(3),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "success");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_success_after_failures() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result: Result<String> = retry_with_backoff(
            || {
                let counter = counter_clone.clone();
                async move {
                    let count = counter.fetch_add(1, Ordering::SeqCst);
                    if count < 2 {
                        Err(AlephError::network("temporary failure"))
                    } else {
                        Ok::<_, AlephError>("success".to_string())
                    }
                }
            },
            Some(3),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "success");
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result: Result<String> = retry_with_backoff(
            || {
                let counter = counter_clone.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err(AlephError::network("persistent failure"))
                }
            },
            Some(3),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_non_retryable_error() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result: Result<String> = retry_with_backoff(
            || {
                let counter = counter_clone.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err(AlephError::authentication("OpenAI", "invalid key"))
                }
            },
            Some(3),
        )
        .await;

        assert!(result.is_err());
        // Should fail immediately without retries
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_with_custom_max_retries() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result: Result<String> = retry_with_backoff(
            || {
                let counter = counter_clone.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err(AlephError::network("failure"))
                }
            },
            Some(5),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn test_is_overloaded_message() {
        assert!(is_overloaded_message("Server is overloaded"));
        assert!(is_overloaded_message("Resource exhausted"));
        assert!(is_overloaded_message("At capacity"));
        assert!(!is_overloaded_message("Invalid request"));
        assert!(!is_overloaded_message("Authentication failed"));
        // Rate limit and too_many_requests are NOT overloaded — they should
        // not trigger retries as that amplifies the problem.
        assert!(!is_overloaded_message("too_many_requests"));
        assert!(!is_overloaded_message("Rate limit exceeded"));
    }

    #[test]
    fn test_calculate_delay() {
        // First attempt: 2000ms
        assert_eq!(calculate_delay(1, None, false), Duration::from_millis(2000));

        // Second attempt: 4000ms
        assert_eq!(calculate_delay(2, None, false), Duration::from_millis(4000));

        // Third attempt: 8000ms
        assert_eq!(calculate_delay(3, None, false), Duration::from_millis(8000));

        // Fifth attempt: 32000ms but capped at 30000ms
        assert_eq!(
            calculate_delay(5, None, false),
            Duration::from_millis(30000)
        );
    }

    #[test]
    fn test_calculate_delay_with_retry_after() {
        // Use retry-after value
        assert_eq!(
            calculate_delay(1, Some(5000), false),
            Duration::from_millis(5000)
        );

        // Cap at max when no headers
        assert_eq!(
            calculate_delay(1, Some(60000), false),
            Duration::from_millis(30000)
        );

        // Allow higher values with headers
        assert_eq!(
            calculate_delay(1, Some(60000), true),
            Duration::from_millis(60000)
        );
    }

    #[test]
    fn test_parse_retry_after() {
        // Parse seconds
        assert_eq!(parse_retry_after("120"), Some(120000));
        assert_eq!(parse_retry_after("60"), Some(60000));
        assert_eq!(parse_retry_after("0"), Some(0));

        // Invalid values
        assert!(parse_retry_after("invalid").is_none());
        assert!(parse_retry_after("-1").is_none());
    }

    #[test]
    fn test_retryable_reason() {
        // Retryable
        assert!(retryable_reason(&AlephError::network("connection failed")).is_some());
        assert!(retryable_reason(&AlephError::Timeout { suggestion: None }).is_some());
        assert!(retryable_reason(&AlephError::provider("500 Internal Server Error")).is_some());

        // Not retryable
        assert!(retryable_reason(&AlephError::authentication("Test", "invalid key")).is_none());
        assert!(retryable_reason(&AlephError::invalid_config("bad config")).is_none());
    }

    // --- apply_jitter (thundering-herd protection) -----------------------------

    #[test]
    fn apply_jitter_zero_factor_is_identity() {
        let d = Duration::from_millis(1000);
        assert_eq!(apply_jitter(d, 0.0), d);
    }

    #[test]
    fn apply_jitter_negative_factor_is_identity() {
        let d = Duration::from_millis(1000);
        assert_eq!(apply_jitter(d, -0.5), d);
    }

    #[test]
    fn apply_jitter_zero_duration_is_zero() {
        assert_eq!(apply_jitter(Duration::ZERO, 0.5), Duration::ZERO);
    }

    #[test]
    fn apply_jitter_never_below_base() {
        // 100 trials — every sample must satisfy `result >= base`.
        let base = Duration::from_millis(500);
        for _ in 0..100 {
            let jittered = apply_jitter(base, 0.5);
            assert!(
                jittered >= base,
                "jittered ({:?}) below base ({:?})",
                jittered,
                base
            );
        }
    }

    #[test]
    fn apply_jitter_within_bounded_range() {
        // factor=1.0 → result lives in [base, 2*base].
        let base = Duration::from_millis(1000);
        let upper = base.saturating_mul(2);
        for _ in 0..100 {
            let jittered = apply_jitter(base, 1.0);
            assert!(jittered >= base && jittered <= upper);
        }
    }

    #[test]
    fn apply_jitter_clamps_factor_above_one() {
        // factor > 1.0 must behave identically to factor=1.0 (no unbounded spread).
        let base = Duration::from_millis(1000);
        let upper = base.saturating_mul(2);
        for _ in 0..50 {
            let jittered = apply_jitter(base, 10.0);
            assert!(jittered >= base && jittered <= upper);
        }
    }

    #[test]
    fn apply_jitter_actually_varies() {
        // Statistical sanity: across 50 trials at factor=0.5 with a 200 ms base,
        // at least 5 distinct values must appear. A deterministic implementation
        // would return only 1, so 5 is a generous floor.
        let base = Duration::from_millis(200);
        let mut samples = std::collections::HashSet::new();
        for _ in 0..50 {
            samples.insert(apply_jitter(base, 0.5));
        }
        assert!(
            samples.len() >= 5,
            "apply_jitter looks deterministic: {} distinct samples",
            samples.len()
        );
    }

    #[test]
    fn apply_jitter_tiny_duration_floors_extra_to_zero() {
        // Base = 1 ms, factor 0.25 → max_extra = 0 (int truncation). Identity.
        let base = Duration::from_millis(1);
        assert_eq!(apply_jitter(base, 0.25), base);
    }
}
