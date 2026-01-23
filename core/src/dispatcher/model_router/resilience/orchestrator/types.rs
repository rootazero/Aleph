//! Execution Types
//!
//! This module contains the core types for orchestrated execution:
//! - ExecutionRequest: Input parameters for execution
//! - ExecutionResult: Output with success/failure details
//! - AttemptRecord: Record of a single execution attempt
//! - ExecutionError: Error types that can occur during execution

use super::super::budget::BudgetScope;
use super::super::retry::{BackoffStrategy, RetryPolicy};
use crate::dispatcher::model_router::{CallOutcome, TaskIntent};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use thiserror::Error;

// =============================================================================
// Execution Request
// =============================================================================

/// Request for orchestrated execution
#[derive(Debug, Clone)]
pub struct ExecutionRequest {
    /// Unique request ID
    pub id: String,

    /// Preferred model ID
    pub preferred_model: String,

    /// Task intent for routing
    pub intent: TaskIntent,

    /// Input token count (for cost estimation)
    pub input_tokens: u32,

    /// Estimated output tokens (for cost estimation)
    pub estimated_output_tokens: u32,

    /// Budget scope for this request
    pub budget_scope: BudgetScope,

    /// Custom retry policy (overrides orchestrator default)
    pub retry_policy: Option<RetryPolicy>,

    /// Custom backoff strategy (overrides orchestrator default)
    pub backoff_strategy: Option<BackoffStrategy>,

    /// Whether to allow failover to other models
    pub allow_failover: bool,

    /// Request metadata
    pub metadata: HashMap<String, String>,
}

impl ExecutionRequest {
    /// Create a new execution request
    pub fn new(
        id: impl Into<String>,
        preferred_model: impl Into<String>,
        intent: TaskIntent,
    ) -> Self {
        Self {
            id: id.into(),
            preferred_model: preferred_model.into(),
            intent,
            input_tokens: 0,
            estimated_output_tokens: 500,
            budget_scope: BudgetScope::Global,
            retry_policy: None,
            backoff_strategy: None,
            allow_failover: true,
            metadata: HashMap::new(),
        }
    }

    /// Builder: set input tokens
    pub fn with_input_tokens(mut self, tokens: u32) -> Self {
        self.input_tokens = tokens;
        self
    }

    /// Builder: set estimated output tokens
    pub fn with_estimated_output_tokens(mut self, tokens: u32) -> Self {
        self.estimated_output_tokens = tokens;
        self
    }

    /// Builder: set budget scope
    pub fn with_budget_scope(mut self, scope: BudgetScope) -> Self {
        self.budget_scope = scope;
        self
    }

    /// Builder: set custom retry policy
    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = Some(policy);
        self
    }

    /// Builder: set custom backoff strategy
    pub fn with_backoff_strategy(mut self, strategy: BackoffStrategy) -> Self {
        self.backoff_strategy = Some(strategy);
        self
    }

    /// Builder: disable failover
    pub fn without_failover(mut self) -> Self {
        self.allow_failover = false;
        self
    }

    /// Builder: add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

// =============================================================================
// Attempt Record
// =============================================================================

/// Record of a single execution attempt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptRecord {
    /// Attempt number (1-indexed)
    pub attempt_number: u32,

    /// Model ID used for this attempt
    pub model_id: String,

    /// Duration of this attempt
    pub duration_ms: u64,

    /// Outcome of this attempt
    pub outcome: CallOutcome,

    /// Error detail if failed
    pub error_detail: Option<String>,

    /// Whether this was a failover attempt
    pub is_failover: bool,

    /// Backoff delay before this attempt (ms)
    pub backoff_delay_ms: Option<u64>,
}

impl AttemptRecord {
    /// Create a new attempt record
    pub fn new(attempt_number: u32, model_id: impl Into<String>) -> Self {
        Self {
            attempt_number,
            model_id: model_id.into(),
            duration_ms: 0,
            outcome: CallOutcome::Unknown,
            error_detail: None,
            is_failover: false,
            backoff_delay_ms: None,
        }
    }

    /// Set duration
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration_ms = duration.as_millis() as u64;
        self
    }

    /// Set outcome
    pub fn with_outcome(mut self, outcome: CallOutcome) -> Self {
        self.outcome = outcome;
        self
    }

    /// Set error detail
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error_detail = Some(error.into());
        self
    }

    /// Mark as failover attempt
    pub fn as_failover(mut self) -> Self {
        self.is_failover = true;
        self
    }

    /// Set backoff delay
    pub fn with_backoff(mut self, delay: Duration) -> Self {
        self.backoff_delay_ms = Some(delay.as_millis() as u64);
        self
    }
}

// =============================================================================
// Execution Result
// =============================================================================

/// Result of orchestrated execution
#[derive(Debug, Clone)]
pub struct ExecutionResult<T>
where
    T: Clone,
{
    /// Final result (success value or error)
    pub result: Result<T, ExecutionError>,

    /// Total attempts made
    pub attempts: u32,

    /// Models tried in order
    pub models_tried: Vec<String>,

    /// Total time spent (ms)
    pub total_duration_ms: u64,

    /// Detailed attempt log
    pub attempt_log: Vec<AttemptRecord>,

    /// Final model ID (if successful)
    pub final_model: Option<String>,

    /// Estimated cost (if available)
    pub estimated_cost: Option<f64>,
}

impl<T: Clone> ExecutionResult<T> {
    /// Create a successful result
    pub fn success(
        value: T,
        attempts: u32,
        models_tried: Vec<String>,
        duration: Duration,
        attempt_log: Vec<AttemptRecord>,
    ) -> Self {
        let final_model = models_tried.last().cloned();
        Self {
            result: Ok(value),
            attempts,
            models_tried,
            total_duration_ms: duration.as_millis() as u64,
            attempt_log,
            final_model,
            estimated_cost: None,
        }
    }

    /// Create a failed result
    pub fn failure(
        error: ExecutionError,
        attempts: u32,
        models_tried: Vec<String>,
        duration: Duration,
        attempt_log: Vec<AttemptRecord>,
    ) -> Self {
        Self {
            result: Err(error),
            attempts,
            models_tried,
            total_duration_ms: duration.as_millis() as u64,
            attempt_log,
            final_model: None,
            estimated_cost: None,
        }
    }

    /// Create a budget exceeded result
    pub fn budget_exceeded(check_result: super::super::budget::BudgetCheckResult) -> Self {
        Self {
            result: Err(ExecutionError::BudgetExceeded {
                message: check_result.message(),
            }),
            attempts: 0,
            models_tried: vec![],
            total_duration_ms: 0,
            attempt_log: vec![],
            final_model: None,
            estimated_cost: None,
        }
    }

    /// Set estimated cost
    pub fn with_estimated_cost(mut self, cost: f64) -> Self {
        self.estimated_cost = Some(cost);
        self
    }

    /// Check if execution was successful
    pub fn is_success(&self) -> bool {
        self.result.is_ok()
    }

    /// Check if execution failed
    pub fn is_failure(&self) -> bool {
        self.result.is_err()
    }

    /// Get the successful value (if any)
    pub fn ok(self) -> Option<T> {
        self.result.ok()
    }

    /// Get the error (if any)
    pub fn err(&self) -> Option<&ExecutionError> {
        self.result.as_ref().err()
    }
}

// =============================================================================
// Execution Error
// =============================================================================

/// Errors that can occur during orchestrated execution
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionError {
    /// Budget check failed
    #[error("Budget exceeded: {message}")]
    BudgetExceeded { message: String },

    /// All models in the failover chain are unavailable
    #[error("All models unavailable: tried {models_tried:?}")]
    AllModelsUnavailable { models_tried: Vec<String> },

    /// Maximum retry attempts exceeded
    #[error("Max attempts ({attempts}) exceeded, last error: {last_outcome:?}")]
    MaxAttemptsExceeded {
        attempts: u32,
        last_outcome: CallOutcome,
    },

    /// Total timeout exceeded
    #[error("Total timeout exceeded after {elapsed_ms}ms")]
    TotalTimeoutExceeded { elapsed_ms: u64 },

    /// Circuit breaker is open for the model
    #[error("Circuit breaker open for model: {model_id}")]
    CircuitOpen { model_id: String },

    /// No healthy model available
    #[error("No healthy model available in failover chain")]
    NoHealthyModel,

    /// Request was cancelled
    #[error("Execution cancelled: {reason}")]
    Cancelled { reason: String },

    /// Internal error
    #[error("Internal error: {message}")]
    Internal { message: String },
}

impl ExecutionError {
    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::MaxAttemptsExceeded { .. } | Self::TotalTimeoutExceeded { .. }
        )
    }

    /// Check if this error is due to budget
    pub fn is_budget_error(&self) -> bool {
        matches!(self, Self::BudgetExceeded { .. })
    }

    /// Check if this error is due to health issues
    pub fn is_health_error(&self) -> bool {
        matches!(
            self,
            Self::CircuitOpen { .. } | Self::NoHealthyModel | Self::AllModelsUnavailable { .. }
        )
    }
}
