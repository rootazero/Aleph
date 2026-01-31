//! Core types for resilient task execution.
//!
//! Defines task outcomes, degradation strategies, and execution context.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Outcome of a task execution
#[derive(Debug, Clone)]
pub enum TaskOutcome<T> {
    /// Task succeeded with primary result
    Success(T),
    /// Task degraded but produced fallback result
    Degraded {
        result: T,
        reason: DegradationReason,
        attempts: u32,
    },
    /// Task failed completely
    Failed {
        error: String,
        attempts: u32,
        last_attempt_duration: Duration,
    },
}

impl<T> TaskOutcome<T> {
    /// Check if task succeeded (either primary or degraded)
    pub fn is_ok(&self) -> bool {
        matches!(self, TaskOutcome::Success(_) | TaskOutcome::Degraded { .. })
    }

    /// Check if task failed completely
    pub fn is_failed(&self) -> bool {
        matches!(self, TaskOutcome::Failed { .. })
    }

    /// Get the result if available
    pub fn result(&self) -> Option<&T> {
        match self {
            TaskOutcome::Success(r) => Some(r),
            TaskOutcome::Degraded { result, .. } => Some(result),
            TaskOutcome::Failed { .. } => None,
        }
    }

    /// Map the result value
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> TaskOutcome<U> {
        match self {
            TaskOutcome::Success(t) => TaskOutcome::Success(f(t)),
            TaskOutcome::Degraded {
                result,
                reason,
                attempts,
            } => TaskOutcome::Degraded {
                result: f(result),
                reason,
                attempts,
            },
            TaskOutcome::Failed {
                error,
                attempts,
                last_attempt_duration,
            } => TaskOutcome::Failed {
                error,
                attempts,
                last_attempt_duration,
            },
        }
    }
}

/// Reason for degradation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DegradationReason {
    /// Primary method timed out
    Timeout { elapsed: Duration, limit: Duration },
    /// Primary method failed after retries
    RetriesExhausted { attempts: u32, last_error: String },
    /// External service unavailable
    ServiceUnavailable { service: String },
    /// Rate limited
    RateLimited { retry_after: Option<Duration> },
    /// Resource quota exceeded
    QuotaExceeded { resource: String },
    /// Manual degradation requested
    Manual { reason: String },
}

/// Strategy for handling degradation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradationStrategy {
    /// Skip the task entirely
    Skip,
    /// Use a simpler fallback method
    Fallback { fallback_id: String },
    /// Return partial results
    PartialResult,
    /// Return cached result if available
    UseCached { max_age_secs: u64 },
    /// Notify and fail
    NotifyAndFail { notify_channels: Vec<String> },
}

impl Default for DegradationStrategy {
    fn default() -> Self {
        DegradationStrategy::NotifyAndFail {
            notify_channels: vec![],
        }
    }
}

/// Configuration for resilient task execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResilienceConfig {
    /// Maximum retry attempts (including initial)
    pub max_attempts: u32,
    /// Initial backoff delay in milliseconds
    pub initial_backoff_ms: u64,
    /// Backoff multiplier for exponential backoff
    pub backoff_multiplier: f64,
    /// Maximum backoff delay in milliseconds
    pub max_backoff_ms: u64,
    /// Add jitter to backoff (recommended)
    pub use_jitter: bool,
    /// Jitter factor (0.0-1.0)
    pub jitter_factor: f64,
    /// Task timeout in milliseconds
    pub timeout_ms: u64,
    /// Degradation strategy when retries exhausted
    pub degradation_strategy: DegradationStrategy,
    /// Whether to retry on timeout
    pub retry_on_timeout: bool,
}

impl Default for ResilienceConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff_ms: 1000,
            backoff_multiplier: 2.0,
            max_backoff_ms: 30000,
            use_jitter: true,
            jitter_factor: 0.2,
            timeout_ms: 60000,
            degradation_strategy: DegradationStrategy::default(),
            retry_on_timeout: true,
        }
    }
}

impl ResilienceConfig {
    /// Create a config for critical tasks (more retries, longer timeouts)
    pub fn critical() -> Self {
        Self {
            max_attempts: 5,
            initial_backoff_ms: 2000,
            backoff_multiplier: 2.0,
            max_backoff_ms: 60000,
            use_jitter: true,
            jitter_factor: 0.2,
            timeout_ms: 300000, // 5 minutes
            degradation_strategy: DegradationStrategy::NotifyAndFail {
                notify_channels: vec!["telegram".to_string()],
            },
            retry_on_timeout: true,
        }
    }

    /// Create a config for best-effort tasks (fewer retries, quick fallback)
    pub fn best_effort() -> Self {
        Self {
            max_attempts: 2,
            initial_backoff_ms: 500,
            backoff_multiplier: 2.0,
            max_backoff_ms: 5000,
            use_jitter: true,
            jitter_factor: 0.2,
            timeout_ms: 30000,
            degradation_strategy: DegradationStrategy::Skip,
            retry_on_timeout: false,
        }
    }

    /// Create a config with fallback
    pub fn with_fallback(fallback_id: impl Into<String>) -> Self {
        Self {
            degradation_strategy: DegradationStrategy::Fallback {
                fallback_id: fallback_id.into(),
            },
            ..Default::default()
        }
    }

    /// Calculate backoff for a given attempt
    pub fn calculate_backoff(&self, attempt: u32) -> Duration {
        let base_ms = self.initial_backoff_ms as f64
            * self.backoff_multiplier.powi(attempt.saturating_sub(1) as i32);
        let capped_ms = base_ms.min(self.max_backoff_ms as f64);

        let final_ms = if self.use_jitter {
            let jitter = rand::random::<f64>() * self.jitter_factor * 2.0 - self.jitter_factor;
            (capped_ms * (1.0 + jitter)).max(0.0)
        } else {
            capped_ms
        };

        Duration::from_millis(final_ms as u64)
    }
}

/// Context for task execution
#[derive(Debug, Clone)]
pub struct TaskContext {
    /// Task identifier
    pub task_id: String,
    /// Current attempt number (1-based)
    pub attempt: u32,
    /// Total elapsed time
    pub elapsed: Duration,
    /// Previous error if retrying
    pub previous_error: Option<String>,
    /// Whether this is a degraded execution
    pub is_degraded: bool,
}

impl TaskContext {
    /// Create initial context
    pub fn new(task_id: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            attempt: 1,
            elapsed: Duration::ZERO,
            previous_error: None,
            is_degraded: false,
        }
    }

    /// Create context for retry
    pub fn for_retry(&self, error: String, elapsed: Duration) -> Self {
        Self {
            task_id: self.task_id.clone(),
            attempt: self.attempt + 1,
            elapsed,
            previous_error: Some(error),
            is_degraded: false,
        }
    }

    /// Create context for degraded execution
    pub fn for_degradation(&self) -> Self {
        Self {
            task_id: self.task_id.clone(),
            attempt: self.attempt,
            elapsed: self.elapsed,
            previous_error: self.previous_error.clone(),
            is_degraded: true,
        }
    }
}

/// Error classification for retry decisions
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorClass {
    /// Error is transient, should retry
    Transient,
    /// Error is permanent, should not retry
    Permanent,
    /// Error is rate-limit related, should wait
    RateLimit { retry_after: Option<Duration> },
    /// Unknown error type
    Unknown,
}

/// Classify an error for retry decisions
pub fn classify_error(error: &str) -> ErrorClass {
    let lower = error.to_lowercase();

    if lower.contains("timeout") || lower.contains("timed out") {
        return ErrorClass::Transient;
    }

    if lower.contains("rate limit") || lower.contains("too many requests") || lower.contains("429")
    {
        return ErrorClass::RateLimit { retry_after: None };
    }

    if lower.contains("connection") || lower.contains("network") || lower.contains("503") {
        return ErrorClass::Transient;
    }

    if lower.contains("invalid") || lower.contains("not found") || lower.contains("401") {
        return ErrorClass::Permanent;
    }

    ErrorClass::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_outcome_success() {
        let outcome: TaskOutcome<String> = TaskOutcome::Success("result".to_string());
        assert!(outcome.is_ok());
        assert!(!outcome.is_failed());
        assert_eq!(outcome.result(), Some(&"result".to_string()));
    }

    #[test]
    fn test_task_outcome_degraded() {
        let outcome: TaskOutcome<String> = TaskOutcome::Degraded {
            result: "fallback".to_string(),
            reason: DegradationReason::RetriesExhausted {
                attempts: 3,
                last_error: "timeout".to_string(),
            },
            attempts: 3,
        };
        assert!(outcome.is_ok());
        assert_eq!(outcome.result(), Some(&"fallback".to_string()));
    }

    #[test]
    fn test_resilience_config_backoff() {
        let config = ResilienceConfig {
            use_jitter: false, // Disable for deterministic test
            ..Default::default()
        };

        let b1 = config.calculate_backoff(1);
        let b2 = config.calculate_backoff(2);
        let b3 = config.calculate_backoff(3);

        assert_eq!(b1.as_millis(), 1000);
        assert_eq!(b2.as_millis(), 2000);
        assert_eq!(b3.as_millis(), 4000);
    }

    #[test]
    fn test_error_classification() {
        assert_eq!(classify_error("connection timeout"), ErrorClass::Transient);
        assert_eq!(
            classify_error("rate limit exceeded"),
            ErrorClass::RateLimit { retry_after: None }
        );
        assert_eq!(classify_error("invalid request"), ErrorClass::Permanent);
    }
}
