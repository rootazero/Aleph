//! Retry behavior policies
//!
//! Configurable retry parameters for network operations including
//! backoff strategy and retryable error conditions.

use crate::dispatcher::DEFAULT_MAX_RETRIES;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Policy for retry behavior in network operations
///
/// Controls retry attempts, backoff timing, and which errors should
/// trigger retries.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RetryPolicy {
    /// Maximum retry attempts
    /// Default: 3
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Initial backoff duration in milliseconds
    /// Default: 1000
    #[serde(default = "default_initial_backoff_ms")]
    pub initial_backoff_ms: u64,

    /// Backoff multiplier for exponential backoff
    /// Default: 2.0
    #[serde(default = "default_backoff_multiplier")]
    pub backoff_multiplier: f64,

    /// Maximum backoff duration in milliseconds (cap)
    /// Default: 32000
    #[serde(default = "default_max_backoff_ms")]
    pub max_backoff_ms: u64,

    /// HTTP status codes that should trigger retry
    /// Default: [500, 502, 503, 504]
    #[serde(default = "default_retryable_status_codes")]
    pub retryable_status_codes: Vec<u16>,

    /// Whether to retry on timeout errors
    /// Default: true
    #[serde(default = "default_retry_on_timeout")]
    pub retry_on_timeout: bool,

    /// Whether to retry on network/connection errors
    /// Default: true
    #[serde(default = "default_retry_on_network_error")]
    pub retry_on_network_error: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            initial_backoff_ms: default_initial_backoff_ms(),
            backoff_multiplier: default_backoff_multiplier(),
            max_backoff_ms: default_max_backoff_ms(),
            retryable_status_codes: default_retryable_status_codes(),
            retry_on_timeout: default_retry_on_timeout(),
            retry_on_network_error: default_retry_on_network_error(),
        }
    }
}

fn default_max_retries() -> u32 {
    DEFAULT_MAX_RETRIES
}

fn default_initial_backoff_ms() -> u64 {
    1000
}

fn default_backoff_multiplier() -> f64 {
    2.0
}

fn default_max_backoff_ms() -> u64 {
    32000
}

fn default_retryable_status_codes() -> Vec<u16> {
    vec![500, 502, 503, 504]
}

fn default_retry_on_timeout() -> bool {
    true
}

fn default_retry_on_network_error() -> bool {
    true
}

impl RetryPolicy {
    /// Get initial backoff as std::time::Duration
    pub fn initial_backoff_duration(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.initial_backoff_ms)
    }

    /// Get max backoff as std::time::Duration
    pub fn max_backoff_duration(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.max_backoff_ms)
    }

    /// Calculate backoff duration for a given attempt (0-indexed)
    pub fn backoff_for_attempt(&self, attempt: u32) -> std::time::Duration {
        let backoff_ms =
            (self.initial_backoff_ms as f64 * self.backoff_multiplier.powi(attempt as i32)) as u64;
        let capped = backoff_ms.min(self.max_backoff_ms);
        std::time::Duration::from_millis(capped)
    }

    /// Check if a status code should trigger retry
    pub fn should_retry_status(&self, status_code: u16) -> bool {
        self.retryable_status_codes.contains(&status_code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.initial_backoff_ms, 1000);
        assert_eq!(policy.backoff_multiplier, 2.0);
        assert!(policy.retryable_status_codes.contains(&500));
        assert!(policy.retryable_status_codes.contains(&503));
    }

    #[test]
    fn test_backoff_calculation() {
        let policy = RetryPolicy::default();
        // Attempt 0: 1000ms
        assert_eq!(
            policy.backoff_for_attempt(0),
            std::time::Duration::from_millis(1000)
        );
        // Attempt 1: 2000ms
        assert_eq!(
            policy.backoff_for_attempt(1),
            std::time::Duration::from_millis(2000)
        );
        // Attempt 2: 4000ms
        assert_eq!(
            policy.backoff_for_attempt(2),
            std::time::Duration::from_millis(4000)
        );
    }

    #[test]
    fn test_backoff_cap() {
        let mut policy = RetryPolicy {
            max_backoff_ms: 5000,
            ..RetryPolicy::default()
        };
        // Would be 8000ms without cap, but capped at 5000
        assert_eq!(
            policy.backoff_for_attempt(3),
            std::time::Duration::from_millis(5000)
        );
    }

    #[test]
    fn test_status_code_check() {
        let policy = RetryPolicy::default();
        assert!(policy.should_retry_status(500));
        assert!(policy.should_retry_status(503));
        assert!(!policy.should_retry_status(400));
        assert!(!policy.should_retry_status(401));
    }

    #[test]
    fn test_partial_deserialization() {
        let toml = r#"
            max_retries = 5
            initial_backoff_ms = 500
        "#;
        let policy: RetryPolicy = toml::from_str(toml).unwrap();
        assert_eq!(policy.max_retries, 5);
        assert_eq!(policy.initial_backoff_ms, 500);
        // Defaults for unspecified
        assert_eq!(policy.backoff_multiplier, 2.0);
        assert!(policy.retry_on_timeout);
    }
}
