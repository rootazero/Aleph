//! Retry configuration types
//!
//! Contains RetryConfigToml for retry and failover behavior.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::backoff::BackoffConfigToml;

// =============================================================================
// RetryConfigToml - Retry and Failover Configuration
// =============================================================================

/// Configuration for retry and failover behavior
///
/// # Example TOML
///
/// ```toml
/// [model_router.retry]
/// enabled = true
/// max_attempts = 3
/// attempt_timeout_ms = 30000
/// total_timeout_ms = 90000
/// failover_on_non_retryable = true
/// retryable_errors = ["timeout", "rate_limited", "network_error", "server_error"]
///
/// [model_router.retry.backoff]
/// strategy = "exponential_jitter"
/// initial_ms = 100
/// max_ms = 5000
/// jitter_factor = 0.2
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RetryConfigToml {
    /// Whether retry is enabled (default: true)
    #[serde(default = "default_retry_enabled")]
    pub enabled: bool,

    /// Maximum number of attempts (including initial) (default: 3)
    #[serde(default = "default_retry_max_attempts")]
    pub max_attempts: u32,

    /// Timeout for each individual attempt in milliseconds (default: 30000)
    #[serde(default = "default_retry_attempt_timeout")]
    pub attempt_timeout_ms: u64,

    /// Total timeout across all attempts in milliseconds (default: 90000)
    /// Set to 0 to disable total timeout
    #[serde(default = "default_retry_total_timeout")]
    pub total_timeout_ms: u64,

    /// Whether to use failover on non-retryable errors (default: true)
    #[serde(default = "default_retry_failover_on_non_retryable")]
    pub failover_on_non_retryable: bool,

    /// Error types that trigger retry (default: timeout, rate_limited, network_error, server_error)
    #[serde(default = "default_retry_retryable_errors")]
    pub retryable_errors: Vec<String>,

    /// Backoff configuration
    #[serde(default)]
    pub backoff: BackoffConfigToml,
}

fn default_retry_enabled() -> bool {
    true
}

fn default_retry_max_attempts() -> u32 {
    3
}

fn default_retry_attempt_timeout() -> u64 {
    30000 // 30 seconds
}

fn default_retry_total_timeout() -> u64 {
    90000 // 90 seconds
}

fn default_retry_failover_on_non_retryable() -> bool {
    true
}

fn default_retry_retryable_errors() -> Vec<String> {
    vec![
        "timeout".to_string(),
        "rate_limited".to_string(),
        "network_error".to_string(),
        "server_error".to_string(),
    ]
}

impl Default for RetryConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_retry_enabled(),
            max_attempts: default_retry_max_attempts(),
            attempt_timeout_ms: default_retry_attempt_timeout(),
            total_timeout_ms: default_retry_total_timeout(),
            failover_on_non_retryable: default_retry_failover_on_non_retryable(),
            retryable_errors: default_retry_retryable_errors(),
            backoff: BackoffConfigToml::default(),
        }
    }
}

impl RetryConfigToml {
    /// Validate the retry configuration
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.max_attempts == 0 {
            return Err("retry.max_attempts must be > 0".to_string());
        }
        if self.max_attempts > 10 {
            warn!(
                max_attempts = self.max_attempts,
                "retry.max_attempts > 10 may cause excessive retry loops"
            );
        }

        if self.attempt_timeout_ms == 0 {
            return Err("retry.attempt_timeout_ms must be > 0".to_string());
        }

        if self.total_timeout_ms > 0 && self.total_timeout_ms < self.attempt_timeout_ms {
            warn!(
                total = self.total_timeout_ms,
                attempt = self.attempt_timeout_ms,
                "retry.total_timeout_ms < attempt_timeout_ms may prevent retries"
            );
        }

        self.backoff.validate()?;
        Ok(())
    }

    /// Convert to internal RetryPolicy
    pub fn to_retry_policy(&self) -> crate::dispatcher::model_router::RetryPolicy {
        use crate::dispatcher::model_router::{RetryPolicy, RetryableOutcome};

        let retryable_outcomes: Vec<RetryableOutcome> = self
            .retryable_errors
            .iter()
            .filter_map(|s| match s.as_str() {
                "timeout" => Some(RetryableOutcome::Timeout),
                "rate_limited" => Some(RetryableOutcome::RateLimited),
                "network_error" => Some(RetryableOutcome::NetworkError),
                "server_error" => Some(RetryableOutcome::ServerError),
                _ => None,
            })
            .collect();

        RetryPolicy {
            max_attempts: self.max_attempts,
            attempt_timeout_ms: self.attempt_timeout_ms,
            total_timeout_ms: if self.total_timeout_ms > 0 {
                Some(self.total_timeout_ms)
            } else {
                None
            },
            retryable_outcomes,
            failover_on_non_retryable: self.failover_on_non_retryable,
        }
    }
}
