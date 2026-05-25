//! Retry behavior policies
//!
//! Configurable retry parameters for network operations including
//! backoff strategy and retryable error conditions.

use crate::tool_metadata::DEFAULT_MAX_RETRIES;
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

    /// Jitter factor for backoff randomisation in range [0.0, 1.0].
    ///
    /// 0.0 disables jitter (deterministic exponential backoff). Positive
    /// values spread retry storms across concurrent callers: each backoff
    /// duration is widened by a random amount in `[0, base * factor]` —
    /// so the floor stays at the computed exponential value and the
    /// ceiling grows by up to `factor * 100 %`. Hermes-style protection
    /// against thundering-herd on shared rate-limited providers.
    ///
    /// Default: 0.25 (±25 % spread, AWS-recommended equal-jitter shape).
    #[serde(default = "default_jitter_factor")]
    pub jitter_factor: f64,
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
            jitter_factor: default_jitter_factor(),
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

fn default_jitter_factor() -> f64 {
    0.25
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
        // Guard against invalid multiplier values that could produce NaN/Infinity
        if !self.backoff_multiplier.is_finite() || self.backoff_multiplier < 0.0 {
            return std::time::Duration::from_millis(self.initial_backoff_ms);
        }

        let backoff_ms = (self.initial_backoff_ms as f64
            * self
                .backoff_multiplier
                .powi(attempt.try_into().unwrap_or(i32::MAX)))
        .clamp(0.0, f64::MAX) as u64;
        let capped = backoff_ms.min(self.max_backoff_ms);
        std::time::Duration::from_millis(capped.max(self.initial_backoff_ms))
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
        // Jitter default — equal-jitter shape, +/-25 %.
        assert!((policy.jitter_factor - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn test_jitter_factor_deserialises_legacy_toml() {
        // Existing config files have no `jitter_factor` key; serde default must fill it.
        let toml = r#"
            max_retries = 4
            initial_backoff_ms = 500
            backoff_multiplier = 2.0
            max_backoff_ms = 32000
            retryable_status_codes = [500]
            retry_on_timeout = true
            retry_on_network_error = true
        "#;
        let policy: RetryPolicy = toml::from_str(toml).unwrap();
        assert!((policy.jitter_factor - 0.25).abs() < f64::EPSILON);
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
        let policy = RetryPolicy {
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
