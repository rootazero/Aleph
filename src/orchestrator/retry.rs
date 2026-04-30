//! Retry utilities for Orchestrator layer.
//!
//! Provides exponential backoff computation for transient error retries.
//! Formula: min(base_delay * 2^min(attempt-1, 10), max_delay)
//!
//! Note: The LLM layer (`providers/llm_retry.rs`) has its own retry implementation.
//! This module serves the Orchestrator/Gateway layer retry logic.

use std::time::Duration;

/// Default base delay for retry (10 seconds).
const DEFAULT_BASE_DELAY_MS: u64 = 10_000;
/// Default max delay cap (5 minutes).
const DEFAULT_MAX_DELAY_MS: u64 = 300_000;
/// Default max retry attempts.
const DEFAULT_MAX_ATTEMPTS: u32 = 6;

/// Retry configuration for Orchestrator layer.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Base delay for first retry attempt.
    pub base_delay: Duration,
    /// Maximum delay cap.
    pub max_delay: Duration,
    /// Maximum number of retry attempts.
    pub max_attempts: u32,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            base_delay: Duration::from_millis(DEFAULT_BASE_DELAY_MS),
            max_delay: Duration::from_millis(DEFAULT_MAX_DELAY_MS),
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }
}

impl RetryConfig {
    /// Create a new RetryConfig with custom values.
    pub fn new(base_delay: Duration, max_delay: Duration, max_attempts: u32) -> Self {
        Self {
            base_delay,
            max_delay,
            max_attempts,
        }
    }

    /// Create a RetryConfig with only base_delay customized.
    pub fn with_base_delay(base_delay: Duration) -> Self {
        Self {
            base_delay,
            ..Default::default()
        }
    }

    /// Create a RetryConfig with only max_delay customized.
    pub fn with_max_delay(max_delay: Duration) -> Self {
        Self {
            max_delay,
            ..Default::default()
        }
    }
}

/// Compute the retry delay for a given attempt number.
///
/// Formula: min(base_delay * 2^min(attempt-1, 10), max_delay)
///
/// | Attempt | Delay (default config) |
/// |---------|----------------------|
/// | 1       | 10s                  |
/// | 2       | 20s                  |
/// | 3       | 40s                  |
/// | 4       | 80s                  |
/// | 5       | 160s                 |
/// | 6       | 300s (capped)       |
///
/// # Arguments
/// * `attempt` - The retry attempt number (1-indexed, first retry = 1)
/// * `config` - The retry configuration
///
/// # Examples
/// ```
/// use alephcore::orchestrator::retry::{compute_retry_delay, RetryConfig};
///
/// let config = RetryConfig::default();
/// assert_eq!(compute_retry_delay(1, &config), std::time::Duration::from_secs(10));
/// assert_eq!(compute_retry_delay(2, &config), std::time::Duration::from_secs(20));
/// ```
pub fn compute_retry_delay(attempt: u32, config: &RetryConfig) -> Duration {
    if attempt == 0 {
        return config.base_delay;
    }

    // Cap exponent at 10 to prevent overflow (2^10 = 1024)
    // Use attempt-1 so that attempt=1 gives 2^0=1 (no backoff on first retry)
    let exp = attempt.saturating_sub(1).min(10);

    // Use checked arithmetic to prevent overflow
    let multiplier = 1u64.checked_shl(exp).unwrap_or(u64::MAX);
    let delay_ms = config.base_delay.as_millis() as u64 * multiplier;

    // Cap at max_delay
    config.max_delay.min(Duration::from_millis(delay_ms))
}

/// Determine if another retry attempt should be made.
///
/// Returns `true` if `attempt < max_attempts`.
///
/// # Arguments
/// * `attempt` - The current attempt number (0-indexed, before first attempt = 0)
/// * `config` - The retry configuration
pub fn should_retry(attempt: u32, config: &RetryConfig) -> bool {
    attempt < config.max_attempts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_delay_exponential() {
        let config = RetryConfig::default();

        // Attempt 1: base_delay * 2^0 = 10s
        assert_eq!(compute_retry_delay(1, &config), Duration::from_secs(10));

        // Attempt 2: base_delay * 2^1 = 20s
        assert_eq!(compute_retry_delay(2, &config), Duration::from_secs(20));

        // Attempt 3: base_delay * 2^2 = 40s
        assert_eq!(compute_retry_delay(3, &config), Duration::from_secs(40));

        // Attempt 4: base_delay * 2^3 = 80s
        assert_eq!(compute_retry_delay(4, &config), Duration::from_secs(80));
    }

    #[test]
    fn test_retry_delay_capped() {
        let config = RetryConfig::default();

        // Attempt 6: base_delay * 2^5 = 320s, but capped at 300s
        assert_eq!(compute_retry_delay(6, &config), Duration::from_secs(300));

        // Attempt 10+: still capped at 300s
        assert_eq!(compute_retry_delay(10, &config), Duration::from_secs(300));
        assert_eq!(compute_retry_delay(100, &config), Duration::from_secs(300));
    }

    #[test]
    fn test_retry_delay_zero_attempt() {
        let config = RetryConfig::default();
        // Attempt 0 returns base_delay
        assert_eq!(compute_retry_delay(0, &config), Duration::from_secs(10));
    }

    #[test]
    fn test_should_retry() {
        let config = RetryConfig::default();

        // Attempt 0, 1, 2, 3, 4, 5 should retry (max_attempts = 6)
        assert!(should_retry(0, &config));
        assert!(should_retry(1, &config));
        assert!(should_retry(2, &config));
        assert!(should_retry(5, &config));

        // Attempt 6+ should not retry
        assert!(!should_retry(6, &config));
        assert!(!should_retry(100, &config));
    }

    #[test]
    fn test_retry_config_custom() {
        let config = RetryConfig::new(
            Duration::from_secs(5),  // base_delay = 5s
            Duration::from_secs(60), // max_delay = 60s
            3,                       // max_attempts = 3
        );

        assert_eq!(compute_retry_delay(1, &config), Duration::from_secs(5));
        assert_eq!(compute_retry_delay(2, &config), Duration::from_secs(10));
        assert_eq!(compute_retry_delay(3, &config), Duration::from_secs(20));
        // Attempt 4: 5 * 2^3 = 40s (not capped, 40s < 60s)
        assert_eq!(compute_retry_delay(4, &config), Duration::from_secs(40));
        // Attempt 5: 5 * 2^4 = 80s, capped to 60s
        assert_eq!(compute_retry_delay(5, &config), Duration::from_secs(60));

        assert!(should_retry(0, &config));
        assert!(should_retry(1, &config));
        assert!(should_retry(2, &config));
        assert!(!should_retry(3, &config));
    }
}
