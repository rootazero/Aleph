//! Retry Policy and Backoff Strategy
//!
//! This module provides configuration types for retry behavior and backoff
//! delay calculation strategies for resilient API call execution.

use super::CallOutcome;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::time::Duration;

// =============================================================================
// Retry Policy
// =============================================================================

/// Configuration for retry behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of attempts (including initial)
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,

    /// Timeout for each individual attempt in milliseconds
    #[serde(default = "default_attempt_timeout_ms")]
    pub attempt_timeout_ms: u64,

    /// Total timeout across all attempts in milliseconds (optional)
    #[serde(default)]
    pub total_timeout_ms: Option<u64>,

    /// Error types that trigger retry
    #[serde(default = "default_retryable_outcomes")]
    pub retryable_outcomes: Vec<RetryableOutcome>,

    /// Whether to use failover on non-retryable errors
    #[serde(default = "default_true")]
    pub failover_on_non_retryable: bool,
}

fn default_max_attempts() -> u32 {
    3
}

fn default_attempt_timeout_ms() -> u64 {
    30_000 // 30 seconds
}

fn default_true() -> bool {
    true
}

fn default_retryable_outcomes() -> Vec<RetryableOutcome> {
    vec![
        RetryableOutcome::Timeout,
        RetryableOutcome::RateLimited,
        RetryableOutcome::NetworkError,
    ]
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: default_max_attempts(),
            attempt_timeout_ms: default_attempt_timeout_ms(),
            total_timeout_ms: Some(90_000), // 90 seconds
            retryable_outcomes: default_retryable_outcomes(),
            failover_on_non_retryable: true,
        }
    }
}

impl RetryPolicy {
    /// Create a new retry policy with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: set maximum attempts
    pub fn with_max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts.max(1);
        self
    }

    /// Builder: set attempt timeout
    pub fn with_attempt_timeout(mut self, timeout: Duration) -> Self {
        self.attempt_timeout_ms = timeout.as_millis() as u64;
        self
    }

    /// Builder: set attempt timeout in milliseconds
    pub fn with_attempt_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.attempt_timeout_ms = timeout_ms;
        self
    }

    /// Builder: set total timeout
    pub fn with_total_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.total_timeout_ms = timeout.map(|t| t.as_millis() as u64);
        self
    }

    /// Builder: set total timeout in milliseconds
    pub fn with_total_timeout_ms(mut self, timeout_ms: Option<u64>) -> Self {
        self.total_timeout_ms = timeout_ms;
        self
    }

    /// Builder: set retryable outcomes
    pub fn with_retryable_outcomes(mut self, outcomes: Vec<RetryableOutcome>) -> Self {
        self.retryable_outcomes = outcomes;
        self
    }

    /// Builder: set failover behavior
    pub fn with_failover_on_non_retryable(mut self, failover: bool) -> Self {
        self.failover_on_non_retryable = failover;
        self
    }

    /// Get attempt timeout as Duration
    pub fn attempt_timeout(&self) -> Duration {
        Duration::from_millis(self.attempt_timeout_ms)
    }

    /// Get total timeout as Duration (if set)
    pub fn total_timeout(&self) -> Option<Duration> {
        self.total_timeout_ms.map(Duration::from_millis)
    }

    /// Check if an outcome should trigger a retry
    pub fn should_retry(&self, outcome: &CallOutcome) -> bool {
        let retryable = match outcome {
            CallOutcome::Success => return false,
            CallOutcome::Timeout => RetryableOutcome::Timeout,
            CallOutcome::RateLimited => RetryableOutcome::RateLimited,
            CallOutcome::NetworkError => RetryableOutcome::NetworkError,
            CallOutcome::ApiError { status_code } => {
                // 5xx errors are generally retryable
                if *status_code >= 500 && *status_code < 600 {
                    RetryableOutcome::ServerError
                } else {
                    return false;
                }
            }
            CallOutcome::ContentFiltered => return false,
            CallOutcome::ContextOverflow => return false,
            CallOutcome::Unknown => return false,
        };

        self.retryable_outcomes.contains(&retryable)
    }

    /// Check if we should failover after this outcome
    pub fn should_failover(&self, outcome: &CallOutcome) -> bool {
        if self.should_retry(outcome) {
            // Retryable errors can failover after exhausting retries
            true
        } else {
            // Non-retryable errors failover based on config
            self.failover_on_non_retryable
        }
    }

    /// Create a policy that never retries
    pub fn no_retry() -> Self {
        Self {
            max_attempts: 1,
            attempt_timeout_ms: default_attempt_timeout_ms(),
            total_timeout_ms: None,
            retryable_outcomes: vec![],
            failover_on_non_retryable: false,
        }
    }

    /// Create an aggressive retry policy for critical requests
    pub fn aggressive() -> Self {
        Self {
            max_attempts: 5,
            attempt_timeout_ms: 60_000,
            total_timeout_ms: Some(300_000),
            retryable_outcomes: vec![
                RetryableOutcome::Timeout,
                RetryableOutcome::RateLimited,
                RetryableOutcome::NetworkError,
                RetryableOutcome::ServerError,
            ],
            failover_on_non_retryable: true,
        }
    }
}

/// Retryable outcome type for configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryableOutcome {
    /// Request timed out
    Timeout,
    /// Rate limited (429)
    RateLimited,
    /// Network/connection error
    NetworkError,
    /// Server error (5xx)
    ServerError,
}

impl RetryableOutcome {
    /// Get display name
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::RateLimited => "rate_limited",
            Self::NetworkError => "network_error",
            Self::ServerError => "server_error",
        }
    }
}

// =============================================================================
// Backoff Strategy
// =============================================================================

/// Backoff calculation strategy
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BackoffStrategy {
    /// Fixed delay between attempts
    Constant {
        /// Delay in milliseconds
        delay_ms: u64,
    },

    /// Exponential backoff: initial * multiplier^attempt
    Exponential {
        /// Initial delay in milliseconds
        initial_ms: u64,
        /// Maximum delay in milliseconds
        max_ms: u64,
        /// Multiplier for each attempt
        #[serde(default = "default_multiplier")]
        multiplier: f64,
    },

    /// Exponential with random jitter
    ExponentialJitter {
        /// Initial delay in milliseconds
        initial_ms: u64,
        /// Maximum delay in milliseconds
        max_ms: u64,
        /// Jitter factor (0.0 - 1.0)
        #[serde(default = "default_jitter_factor")]
        jitter_factor: f64,
    },

    /// Respect Retry-After header from rate limits
    /// Fallback uses exponential jitter with default settings
    RateLimitAware {
        /// Initial delay in milliseconds for fallback
        #[serde(default = "default_fallback_initial_ms")]
        fallback_initial_ms: u64,
        /// Maximum delay in milliseconds for fallback
        #[serde(default = "default_fallback_max_ms")]
        fallback_max_ms: u64,
    },
}

fn default_fallback_initial_ms() -> u64 {
    100
}

fn default_fallback_max_ms() -> u64 {
    5000
}

fn default_multiplier() -> f64 {
    2.0
}

fn default_jitter_factor() -> f64 {
    0.2
}

impl Default for BackoffStrategy {
    fn default() -> Self {
        Self::ExponentialJitter {
            initial_ms: 100,
            max_ms: 5000,
            jitter_factor: 0.2,
        }
    }
}

impl BackoffStrategy {
    /// Create constant backoff strategy
    pub fn constant(delay: Duration) -> Self {
        Self::Constant {
            delay_ms: delay.as_millis() as u64,
        }
    }

    /// Create constant backoff strategy from milliseconds
    pub fn constant_ms(delay_ms: u64) -> Self {
        Self::Constant { delay_ms }
    }

    /// Create exponential backoff strategy
    pub fn exponential(initial: Duration, max: Duration) -> Self {
        Self::Exponential {
            initial_ms: initial.as_millis() as u64,
            max_ms: max.as_millis() as u64,
            multiplier: 2.0,
        }
    }

    /// Create exponential backoff strategy from milliseconds
    pub fn exponential_ms(initial_ms: u64, max_ms: u64) -> Self {
        Self::Exponential {
            initial_ms,
            max_ms,
            multiplier: 2.0,
        }
    }

    /// Create exponential backoff with jitter
    pub fn exponential_jitter(initial: Duration, max: Duration, jitter_factor: f64) -> Self {
        Self::ExponentialJitter {
            initial_ms: initial.as_millis() as u64,
            max_ms: max.as_millis() as u64,
            jitter_factor: jitter_factor.clamp(0.0, 1.0),
        }
    }

    /// Create exponential backoff with jitter from milliseconds
    pub fn exponential_jitter_ms(initial_ms: u64, max_ms: u64, jitter_factor: f64) -> Self {
        Self::ExponentialJitter {
            initial_ms,
            max_ms,
            jitter_factor: jitter_factor.clamp(0.0, 1.0),
        }
    }

    /// Create rate-limit aware backoff with default fallback
    pub fn rate_limit_aware() -> Self {
        Self::RateLimitAware {
            fallback_initial_ms: 100,
            fallback_max_ms: 5000,
        }
    }

    /// Create rate-limit aware backoff with custom fallback settings
    pub fn rate_limit_aware_with_fallback(initial_ms: u64, max_ms: u64) -> Self {
        Self::RateLimitAware {
            fallback_initial_ms: initial_ms,
            fallback_max_ms: max_ms,
        }
    }

    /// Calculate delay for given attempt number (0-indexed)
    ///
    /// # Arguments
    /// * `attempt` - The attempt number (0 for first retry after initial failure)
    /// * `rate_limit_hint` - Optional delay from Retry-After header
    pub fn delay_for_attempt(&self, attempt: u32, rate_limit_hint: Option<Duration>) -> Duration {
        match self {
            Self::Constant { delay_ms } => Duration::from_millis(*delay_ms),

            Self::Exponential {
                initial_ms,
                max_ms,
                multiplier,
            } => {
                let delay_ms = (*initial_ms as f64) * multiplier.powi(attempt as i32);
                let clamped = delay_ms.min(*max_ms as f64) as u64;
                Duration::from_millis(clamped)
            }

            Self::ExponentialJitter {
                initial_ms,
                max_ms,
                jitter_factor,
            } => {
                let base_ms = (*initial_ms as f64) * 2.0_f64.powi(attempt as i32);
                let jitter = rand::thread_rng().gen::<f64>() * jitter_factor * base_ms;
                let total_ms = (base_ms + jitter).min(*max_ms as f64) as u64;
                Duration::from_millis(total_ms)
            }

            Self::RateLimitAware {
                fallback_initial_ms,
                fallback_max_ms,
            } => {
                // Use rate limit hint if available, otherwise use exponential jitter fallback
                rate_limit_hint.unwrap_or_else(|| {
                    let base_ms = (*fallback_initial_ms as f64) * 2.0_f64.powi(attempt as i32);
                    let jitter = rand::thread_rng().gen::<f64>() * 0.2 * base_ms;
                    let total_ms = (base_ms + jitter).min(*fallback_max_ms as f64) as u64;
                    Duration::from_millis(total_ms)
                })
            }
        }
    }

    /// Calculate delay with minimum enforcement
    pub fn delay_for_attempt_min(&self, attempt: u32, min: Duration) -> Duration {
        self.delay_for_attempt(attempt, None).max(min)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_policy_default() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_attempts, 3);
        assert_eq!(policy.attempt_timeout_ms, 30_000);
        assert!(policy.total_timeout_ms.is_some());
        assert!(policy.failover_on_non_retryable);
    }

    #[test]
    fn test_retry_policy_builder() {
        let policy = RetryPolicy::new()
            .with_max_attempts(5)
            .with_attempt_timeout(Duration::from_secs(60))
            .with_total_timeout(None)
            .with_failover_on_non_retryable(false);

        assert_eq!(policy.max_attempts, 5);
        assert_eq!(policy.attempt_timeout_ms, 60_000);
        assert!(policy.total_timeout_ms.is_none());
        assert!(!policy.failover_on_non_retryable);
    }

    #[test]
    fn test_retry_policy_max_attempts_min() {
        let policy = RetryPolicy::new().with_max_attempts(0);
        assert_eq!(policy.max_attempts, 1); // minimum is 1
    }

    #[test]
    fn test_retry_policy_should_retry() {
        let policy = RetryPolicy::default();

        // Retryable
        assert!(policy.should_retry(&CallOutcome::Timeout));
        assert!(policy.should_retry(&CallOutcome::RateLimited));
        assert!(policy.should_retry(&CallOutcome::NetworkError));

        // Not retryable
        assert!(!policy.should_retry(&CallOutcome::Success));
        assert!(!policy.should_retry(&CallOutcome::ContentFiltered));
        assert!(!policy.should_retry(&CallOutcome::ContextOverflow));
        assert!(!policy.should_retry(&CallOutcome::ApiError { status_code: 400 }));
    }

    #[test]
    fn test_retry_policy_server_error() {
        let policy =
            RetryPolicy::new().with_retryable_outcomes(vec![RetryableOutcome::ServerError]);

        assert!(policy.should_retry(&CallOutcome::ApiError { status_code: 500 }));
        assert!(policy.should_retry(&CallOutcome::ApiError { status_code: 502 }));
        assert!(policy.should_retry(&CallOutcome::ApiError { status_code: 503 }));
        assert!(!policy.should_retry(&CallOutcome::ApiError { status_code: 400 }));
        assert!(!policy.should_retry(&CallOutcome::ApiError { status_code: 404 }));
    }

    #[test]
    fn test_retry_policy_should_failover() {
        let policy = RetryPolicy::default();
        assert!(policy.should_failover(&CallOutcome::Timeout)); // retryable
        assert!(policy.should_failover(&CallOutcome::ContentFiltered)); // non-retryable but failover enabled

        let no_failover = RetryPolicy::no_retry();
        assert!(!no_failover.should_failover(&CallOutcome::ContentFiltered));
    }

    #[test]
    fn test_retry_policy_no_retry() {
        let policy = RetryPolicy::no_retry();
        assert_eq!(policy.max_attempts, 1);
        assert!(policy.retryable_outcomes.is_empty());
        assert!(!policy.failover_on_non_retryable);
    }

    #[test]
    fn test_retry_policy_aggressive() {
        let policy = RetryPolicy::aggressive();
        assert_eq!(policy.max_attempts, 5);
        assert!(policy
            .retryable_outcomes
            .contains(&RetryableOutcome::ServerError));
    }

    #[test]
    fn test_retry_policy_timeout_conversion() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.attempt_timeout(), Duration::from_secs(30));
        assert_eq!(policy.total_timeout(), Some(Duration::from_secs(90)));
    }

    #[test]
    fn test_backoff_constant() {
        let backoff = BackoffStrategy::constant_ms(100);

        assert_eq!(
            backoff.delay_for_attempt(0, None),
            Duration::from_millis(100)
        );
        assert_eq!(
            backoff.delay_for_attempt(1, None),
            Duration::from_millis(100)
        );
        assert_eq!(
            backoff.delay_for_attempt(5, None),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn test_backoff_exponential() {
        let backoff = BackoffStrategy::exponential_ms(100, 5000);

        assert_eq!(
            backoff.delay_for_attempt(0, None),
            Duration::from_millis(100)
        );
        assert_eq!(
            backoff.delay_for_attempt(1, None),
            Duration::from_millis(200)
        );
        assert_eq!(
            backoff.delay_for_attempt(2, None),
            Duration::from_millis(400)
        );
        assert_eq!(
            backoff.delay_for_attempt(3, None),
            Duration::from_millis(800)
        );

        // Should cap at max
        assert_eq!(
            backoff.delay_for_attempt(10, None),
            Duration::from_millis(5000)
        );
    }

    #[test]
    fn test_backoff_exponential_jitter() {
        let backoff = BackoffStrategy::exponential_jitter_ms(100, 5000, 0.2);

        // With jitter, delays should vary but be in expected range
        let delay0 = backoff.delay_for_attempt(0, None);
        assert!(delay0 >= Duration::from_millis(100));
        assert!(delay0 <= Duration::from_millis(120)); // base + 20% jitter

        // Should still cap at max
        let delay_high = backoff.delay_for_attempt(100, None);
        assert!(delay_high <= Duration::from_millis(5000));
    }

    #[test]
    fn test_backoff_rate_limit_aware() {
        let backoff = BackoffStrategy::rate_limit_aware_with_fallback(100, 5000);

        // Without hint, use fallback (exponential jitter)
        let delay = backoff.delay_for_attempt(0, None);
        assert!(delay >= Duration::from_millis(100));
        assert!(delay <= Duration::from_millis(120)); // base + jitter

        // With hint, use hint
        let hint = Duration::from_secs(30);
        assert_eq!(
            backoff.delay_for_attempt(0, Some(hint)),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn test_backoff_delay_min() {
        let backoff = BackoffStrategy::constant_ms(50);
        let min = Duration::from_millis(100);

        assert_eq!(
            backoff.delay_for_attempt_min(0, min),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn test_retryable_outcome_as_str() {
        assert_eq!(RetryableOutcome::Timeout.as_str(), "timeout");
        assert_eq!(RetryableOutcome::RateLimited.as_str(), "rate_limited");
        assert_eq!(RetryableOutcome::NetworkError.as_str(), "network_error");
        assert_eq!(RetryableOutcome::ServerError.as_str(), "server_error");
    }

    #[test]
    fn test_retry_policy_serialization() {
        let policy = RetryPolicy::default();
        let json = serde_json::to_string(&policy).unwrap();
        assert!(json.contains("max_attempts"));
        assert!(json.contains("attempt_timeout_ms"));

        let parsed: RetryPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.max_attempts, policy.max_attempts);
    }

    #[test]
    fn test_backoff_serialization() {
        let backoff = BackoffStrategy::ExponentialJitter {
            initial_ms: 100,
            max_ms: 5000,
            jitter_factor: 0.2,
        };

        let json = serde_json::to_string(&backoff).unwrap();
        assert!(json.contains("exponential_jitter"));
        assert!(json.contains("jitter_factor"));

        let parsed: BackoffStrategy = serde_json::from_str(&json).unwrap();
        if let BackoffStrategy::ExponentialJitter { jitter_factor, .. } = parsed {
            assert!((jitter_factor - 0.2).abs() < 0.001);
        } else {
            panic!("Expected ExponentialJitter");
        }
    }
}
