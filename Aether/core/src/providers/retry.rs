/// Retry logic with exponential backoff for AI provider requests
///
/// This module provides utilities for retrying failed requests with
/// exponential backoff strategy.
use crate::config::RetryPolicy;
use crate::error::{AetherError, Result};
use std::future::Future;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Maximum number of retry attempts (default, used when no policy provided)
const MAX_RETRIES: u32 = 3;

/// Initial backoff duration (1 second) (default, used when no policy provided)
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Determines if an error is retryable using default policy.
fn is_retryable(error: &AetherError) -> bool {
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
/// - Rate limit errors (429)
/// - Invalid configuration
/// - Provider-specific errors not matching policy
fn is_retryable_with_policy(error: &AetherError, policy: &RetryPolicy) -> bool {
    match error {
        AetherError::NetworkError { .. } => policy.retry_on_network_error,
        AetherError::Timeout { .. } => policy.retry_on_timeout,
        AetherError::ProviderError { message, .. } => {
            // Check if message contains any retryable status code
            policy.retryable_status_codes.iter().any(|code| {
                message.contains(&code.to_string())
            })
        }
        // Don't retry these errors
        AetherError::AuthenticationError { .. } => false,
        AetherError::RateLimitError { .. } => false,
        AetherError::InvalidConfig { .. } => false,
        _ => false,
    }
}

/// Retry a future with exponential backoff
///
/// # Arguments
/// * `operation` - The async operation to retry
/// * `max_retries` - Maximum number of retry attempts (default: 3)
///
/// # Returns
/// * `Ok(T)` - If operation succeeds
/// * `Err(AetherError)` - If all retry attempts fail
///
/// # Retry Strategy
/// - Attempt 1: Immediate
/// - Attempt 2: Wait 1s
/// - Attempt 3: Wait 2s
/// - Attempt 4: Wait 4s
///
/// # Example
/// ```rust,ignore
/// use aethecore::providers::retry::retry_with_backoff;
///
/// async fn fetch_data() -> Result<String, aethecore::error::AetherError> {
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
    let max_retries = max_retries.unwrap_or(MAX_RETRIES);
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
/// * `Err(AetherError)` - If all retry attempts fail
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
                let backoff_secs = initial_backoff.as_secs_f64() * multiplier.powi(attempt as i32 - 1);
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
        assert!(is_retryable(&AetherError::network("connection failed")));
        assert!(is_retryable(&AetherError::Timeout { suggestion: None }));
        assert!(is_retryable(&AetherError::provider(
            "500 Internal Server Error"
        )));
        assert!(is_retryable(&AetherError::provider(
            "503 Service Unavailable"
        )));

        // Non-retryable errors
        assert!(!is_retryable(&AetherError::authentication(
            "Test",
            "invalid key"
        )));
        assert!(!is_retryable(&AetherError::rate_limit("quota exceeded")));
        assert!(!is_retryable(&AetherError::invalid_config("bad config")));
        assert!(!is_retryable(&AetherError::provider("400 Bad Request")));
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
                    Ok::<_, AetherError>("success".to_string())
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
                        Err(AetherError::network("temporary failure"))
                    } else {
                        Ok::<_, AetherError>("success".to_string())
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
                    Err(AetherError::network("persistent failure"))
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
                    Err(AetherError::authentication("OpenAI", "invalid key"))
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
                    Err(AetherError::network("failure"))
                }
            },
            Some(5),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 5);
    }
}
