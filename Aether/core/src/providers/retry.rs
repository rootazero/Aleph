/// Retry logic with exponential backoff for AI provider requests
///
/// This module provides utilities for retrying failed requests with
/// exponential backoff strategy.
use crate::error::AetherError;
use std::future::Future;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Maximum number of retry attempts
const MAX_RETRIES: u32 = 3;

/// Initial backoff duration (1 second)
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Determines if an error is retryable
///
/// Retryable errors:
/// - Network errors
/// - Server errors (5xx)
/// - Timeout errors
///
/// Non-retryable errors:
/// - Authentication errors (401)
/// - Rate limit errors (429)
/// - Invalid configuration
/// - Provider-specific errors
fn is_retryable(error: &AetherError) -> bool {
    match error {
        AetherError::NetworkError(_) => true,
        AetherError::Timeout => true,
        AetherError::ProviderError(msg) => {
            // Retry on server errors (5xx)
            msg.contains("500") || msg.contains("502") || msg.contains("503") || msg.contains("504")
        }
        // Don't retry these errors
        AetherError::AuthenticationError(_) => false,
        AetherError::RateLimitError(_) => false,
        AetherError::InvalidConfig(_) => false,
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
/// ```no_run
/// use aether_core::providers::retry::retry_with_backoff;
///
/// async fn fetch_data() -> Result<String, aether_core::error::AetherError> {
///     // ... network request
///     Ok("data".to_string())
/// }
///
/// let result = retry_with_backoff(|| fetch_data(), None).await;
/// ```
pub async fn retry_with_backoff<F, Fut, T>(
    mut operation: F,
    max_retries: Option<u32>,
) -> Result<T, AetherError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, AetherError>>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_is_retryable() {
        // Retryable errors
        assert!(is_retryable(&AetherError::NetworkError(
            "connection failed".into()
        )));
        assert!(is_retryable(&AetherError::Timeout));
        assert!(is_retryable(&AetherError::ProviderError(
            "500 Internal Server Error".into()
        )));
        assert!(is_retryable(&AetherError::ProviderError(
            "503 Service Unavailable".into()
        )));

        // Non-retryable errors
        assert!(!is_retryable(&AetherError::AuthenticationError(
            "invalid key".into()
        )));
        assert!(!is_retryable(&AetherError::RateLimitError(
            "quota exceeded".into()
        )));
        assert!(!is_retryable(&AetherError::InvalidConfig(
            "bad config".into()
        )));
        assert!(!is_retryable(&AetherError::ProviderError(
            "400 Bad Request".into()
        )));
    }

    #[tokio::test]
    async fn test_retry_success_first_attempt() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = retry_with_backoff(
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

        let result = retry_with_backoff(
            || {
                let counter = counter_clone.clone();
                async move {
                    let count = counter.fetch_add(1, Ordering::SeqCst);
                    if count < 2 {
                        Err(AetherError::NetworkError("temporary failure".into()))
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

        let result = retry_with_backoff(
            || {
                let counter = counter_clone.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err::<String, _>(AetherError::NetworkError("persistent failure".into()))
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

        let result = retry_with_backoff(
            || {
                let counter = counter_clone.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err::<String, _>(AetherError::AuthenticationError("invalid key".into()))
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

        let result = retry_with_backoff(
            || {
                let counter = counter_clone.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err::<String, _>(AetherError::NetworkError("failure".into()))
                }
            },
            Some(5),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 5);
    }
}
