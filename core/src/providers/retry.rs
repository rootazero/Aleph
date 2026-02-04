/// Retry logic with exponential backoff for AI provider requests
///
/// This module provides utilities for retrying failed requests with
/// exponential backoff strategy. Inspired by OpenCode's retry.ts.
use crate::config::RetryPolicy;
use crate::dispatcher::DEFAULT_MAX_RETRIES;
use crate::error::{AlephError, Result};
use std::future::Future;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Constants matching OpenCode's retry.ts
pub const RETRY_INITIAL_DELAY_MS: u64 = 2000; // 2 seconds
pub const RETRY_BACKOFF_FACTOR: f64 = 2.0;
pub const RETRY_MAX_DELAY_NO_HEADERS_MS: u64 = 30_000; // 30 seconds
pub const RETRY_MAX_DELAY_WITH_HEADERS_MS: u64 = i32::MAX as u64; // ~24 days (matches JS max setTimeout)

/// Initial backoff duration (1 second) (default, used when no policy provided)
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Determines if an error is retryable using default policy.
fn is_retryable(error: &AlephError) -> bool {
    let default_policy = RetryPolicy::default();
    is_retryable_with_policy(error, &default_policy)
}

/// Determines if an error is retryable using provided policy.
///
/// Retryable errors:
/// - Network errors (if retry_on_network_error is true)
/// - Timeout errors (if retry_on_timeout is true)
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
        // Rate limit with retry-after is potentially retryable
        AlephError::RateLimitError { message, .. } => {
            // Match OpenCode: retry on "too_many_requests" or overloaded
            is_overloaded_message(message)
        }
        // Don't retry these errors
        AlephError::AuthenticationError { .. } => false,
        AlephError::InvalidConfig { .. } => false,
        _ => false,
    }
}

/// Check if error message indicates an overloaded condition (retryable)
///
/// Matches OpenCode's detection of overloaded providers.
fn is_overloaded_message(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("overloaded")
        || lower.contains("too_many_requests")
        || lower.contains("exhausted")
        || lower.contains("capacity")
        || lower.contains("rate limit")
}

/// Extended retryable check that returns the reason if retryable
///
/// Matches OpenCode's retryable() function signature.
pub fn retryable_reason(error: &AlephError) -> Option<String> {
    let default_policy = RetryPolicy::default();
    if is_retryable_with_policy(error, &default_policy) {
        Some(format!("{}", error))
    } else {
        None
    }
}

/// Calculate delay for a retry attempt
///
/// This matches OpenCode's delay() function from retry.ts.
/// Priority:
/// 1. Use retry_after_ms if provided (from Retry-After-Ms header)
/// 2. Use retry_after_secs if provided (from Retry-After header, parsed)
/// 3. Fall back to exponential backoff
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
        return Duration::from_millis(ms.min(max_delay));
    }

    // Exponential backoff: initial * factor^(attempt-1)
    let delay_ms = (RETRY_INITIAL_DELAY_MS as f64) * RETRY_BACKOFF_FACTOR.powi((attempt - 1) as i32);
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
pub fn parse_retry_after(value: &str) -> Option<u64> {
    // Try parsing as seconds first
    if let Ok(secs) = value.parse::<u64>() {
        return Some(secs * 1000);
    }

    // Try parsing as HTTP date
    if let Ok(date) = httpdate::parse_http_date(value) {
        let now = std::time::SystemTime::now();
        if let Ok(duration) = date.duration_since(now) {
            return Some(duration.as_millis() as u64);
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
    let mut last_error = None;

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
                    return Err(last_error.unwrap_or(error));
                }

                // Calculate backoff duration (exponential: 1s, 2s, 4s)
                let backoff = INITIAL_BACKOFF * 2_u32.pow(attempt - 1);

                warn!(
                    attempt,
                    error = ?error,
                    backoff_ms = backoff.as_millis(),
                    "Attempt failed, retrying with backoff"
                );

                // Wait before retrying
                tokio::time::sleep(backoff).await;

                last_error = Some(error);
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
    let mut last_error = None;

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
                    return Err(last_error.unwrap_or(error));
                }

                // Calculate backoff duration using policy multiplier
                let backoff_secs =
                    initial_backoff.as_secs_f64() * multiplier.powi(attempt as i32 - 1);
                let backoff = Duration::from_secs_f64(backoff_secs);

                warn!(
                    attempt,
                    error = ?error,
                    backoff_ms = backoff.as_millis(),
                    "Attempt failed, retrying with policy-based backoff"
                );

                // Wait before retrying
                tokio::time::sleep(backoff).await;

                last_error = Some(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

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
        assert!(is_overloaded_message("too_many_requests"));
        assert!(is_overloaded_message("Rate limit exceeded"));
        assert!(is_overloaded_message("Resource exhausted"));
        assert!(is_overloaded_message("At capacity"));
        assert!(!is_overloaded_message("Invalid request"));
        assert!(!is_overloaded_message("Authentication failed"));
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
        assert_eq!(calculate_delay(5, None, false), Duration::from_millis(30000));
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
}
