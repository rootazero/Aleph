//! Retry Orchestrator
//!
//! This module provides the `RetryOrchestrator` which orchestrates retry and
//! failover logic for resilient API call execution. It integrates with:
//! - RetryPolicy for retry decisions
//! - BackoffStrategy for delay calculation
//! - FailoverChain for alternative model selection
//! - HealthManager for circuit breaker checks
//! - BudgetManager for cost control
//! - MetricsCollector for observability

use super::budget::{BudgetCheckResult, BudgetManager, BudgetScope, CostEstimate};
use super::failover::FailoverChain;
use super::health::HealthStatus;
use super::health_manager::HealthManager;
use super::metrics::CallOutcome;
use super::profiles::ModelProfile;
use super::retry::{BackoffStrategy, RetryPolicy};
use super::TaskIntent;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::RwLock;

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
#[derive(Debug)]
pub struct ExecutionResult<T> {
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

impl<T> ExecutionResult<T> {
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
    pub fn budget_exceeded(check_result: BudgetCheckResult) -> Self {
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

// =============================================================================
// Orchestrator Event
// =============================================================================

/// Events emitted by the orchestrator
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrchestratorEvent {
    /// Execution started
    ExecutionStarted {
        request_id: String,
        preferred_model: String,
    },

    /// Retry attempt starting
    RetryAttempt {
        request_id: String,
        attempt: u32,
        model_id: String,
        reason: String,
        backoff_ms: u64,
    },

    /// Failover to different model
    Failover {
        request_id: String,
        from_model: String,
        to_model: String,
        reason: String,
    },

    /// Execution completed successfully
    ExecutionSuccess {
        request_id: String,
        model_id: String,
        attempts: u32,
        duration_ms: u64,
    },

    /// Execution failed
    ExecutionFailed {
        request_id: String,
        error: String,
        attempts: u32,
        duration_ms: u64,
    },

    /// Circuit breaker skipped model
    CircuitBreakerSkip {
        request_id: String,
        model_id: String,
    },

    /// Budget warning
    BudgetWarning {
        request_id: String,
        message: String,
    },
}

// =============================================================================
// Retry Orchestrator
// =============================================================================

/// Type alias for async executor function
pub type ExecutorFn<T> = Box<
    dyn Fn(String, ExecutionRequest) -> Pin<Box<dyn Future<Output = Result<T, CallOutcome>> + Send>>
        + Send
        + Sync,
>;

/// Configuration for the retry orchestrator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    /// Default retry policy
    #[serde(default)]
    pub default_retry_policy: RetryPolicy,

    /// Default backoff strategy
    #[serde(default)]
    pub default_backoff: BackoffStrategy,

    /// Whether to enable budget checks
    #[serde(default = "default_true")]
    pub budget_checks_enabled: bool,

    /// Whether to enable health checks
    #[serde(default = "default_true")]
    pub health_checks_enabled: bool,

    /// Whether to emit events
    #[serde(default = "default_true")]
    pub events_enabled: bool,

    /// Reset retry count when failing over to a new model
    #[serde(default)]
    pub reset_retries_on_failover: bool,
}

fn default_true() -> bool {
    true
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            default_retry_policy: RetryPolicy::default(),
            default_backoff: BackoffStrategy::default(),
            budget_checks_enabled: true,
            health_checks_enabled: true,
            events_enabled: true,
            reset_retries_on_failover: false,
        }
    }
}

/// Orchestrates retry and failover logic for resilient API execution
pub struct RetryOrchestrator {
    /// Configuration
    config: OrchestratorConfig,

    /// Health manager for circuit breaker checks
    health_manager: Option<Arc<HealthManager>>,

    /// Budget manager for cost control
    budget_manager: Option<Arc<BudgetManager>>,

    /// Model profiles for cost estimation
    profiles: Arc<RwLock<HashMap<String, ModelProfile>>>,

    /// Event sender
    event_tx: Option<tokio::sync::broadcast::Sender<OrchestratorEvent>>,
}

impl RetryOrchestrator {
    /// Create a new retry orchestrator with configuration
    pub fn new(config: OrchestratorConfig) -> Self {
        let (event_tx, _) = tokio::sync::broadcast::channel(100);

        Self {
            config,
            health_manager: None,
            budget_manager: None,
            profiles: Arc::new(RwLock::new(HashMap::new())),
            event_tx: Some(event_tx),
        }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(OrchestratorConfig::default())
    }

    /// Builder: set health manager
    pub fn with_health_manager(mut self, manager: Arc<HealthManager>) -> Self {
        self.health_manager = Some(manager);
        self
    }

    /// Builder: set budget manager
    pub fn with_budget_manager(mut self, manager: Arc<BudgetManager>) -> Self {
        self.budget_manager = Some(manager);
        self
    }

    /// Builder: set model profiles
    pub fn with_profiles(mut self, profiles: Vec<ModelProfile>) -> Self {
        let map: HashMap<_, _> = profiles.into_iter().map(|p| (p.id.clone(), p)).collect();
        self.profiles = Arc::new(RwLock::new(map));
        self
    }

    /// Subscribe to orchestrator events
    pub fn subscribe(&self) -> Option<tokio::sync::broadcast::Receiver<OrchestratorEvent>> {
        self.event_tx.as_ref().map(|tx| tx.subscribe())
    }

    /// Update configuration
    pub fn set_config(&mut self, config: OrchestratorConfig) {
        self.config = config;
    }

    /// Get current configuration
    pub fn config(&self) -> &OrchestratorConfig {
        &self.config
    }

    /// Add or update a model profile
    pub async fn add_profile(&self, profile: ModelProfile) {
        let mut profiles = self.profiles.write().await;
        profiles.insert(profile.id.clone(), profile);
    }

    // =========================================================================
    // Core Execution
    // =========================================================================

    /// Execute with retry and failover support
    ///
    /// # Arguments
    /// * `request` - The execution request
    /// * `failover_chain` - Chain of models to try on failure
    /// * `executor` - Async function that performs the actual API call
    ///
    /// # Returns
    /// ExecutionResult containing either the successful value or error details
    pub async fn execute<T, F, Fut>(
        &self,
        request: ExecutionRequest,
        failover_chain: &FailoverChain,
        executor: F,
    ) -> ExecutionResult<T>
    where
        F: Fn(String, ExecutionRequest) -> Fut,
        Fut: Future<Output = Result<T, CallOutcome>>,
    {
        let start = Instant::now();
        let mut attempts: u32 = 0;
        let mut models_tried: Vec<String> = vec![];
        let mut attempt_log: Vec<AttemptRecord> = vec![];
        let mut current_model = request.preferred_model.clone();
        let mut rate_limit_hint: Option<Duration> = None;
        let mut retries_for_current_model: u32 = 0;

        // Get effective policies
        let retry_policy = request
            .retry_policy
            .clone()
            .unwrap_or_else(|| self.config.default_retry_policy.clone());
        let backoff = request
            .backoff_strategy
            .clone()
            .unwrap_or_else(|| self.config.default_backoff.clone());

        // Emit start event
        self.emit_event(OrchestratorEvent::ExecutionStarted {
            request_id: request.id.clone(),
            preferred_model: request.preferred_model.clone(),
        });

        // Budget pre-check
        if self.config.budget_checks_enabled {
            if let Some(budget_mgr) = &self.budget_manager {
                let estimate = self.estimate_cost(&current_model, &request).await;
                let check = budget_mgr.check_budget(&request.budget_scope, &estimate).await;

                match &check {
                    BudgetCheckResult::HardBlocked { .. } => {
                        return ExecutionResult::budget_exceeded(check);
                    }
                    BudgetCheckResult::SoftBlocked { .. } => {
                        return ExecutionResult::budget_exceeded(check);
                    }
                    BudgetCheckResult::Warning { message, .. } => {
                        self.emit_event(OrchestratorEvent::BudgetWarning {
                            request_id: request.id.clone(),
                            message: message.clone(),
                        });
                    }
                    BudgetCheckResult::Allowed { .. } => {}
                }
            }
        }

        loop {
            attempts += 1;
            retries_for_current_model += 1;

            // Check total timeout
            if let Some(total_timeout) = retry_policy.total_timeout() {
                if start.elapsed() >= total_timeout {
                    return ExecutionResult::failure(
                        ExecutionError::TotalTimeoutExceeded {
                            elapsed_ms: start.elapsed().as_millis() as u64,
                        },
                        attempts,
                        models_tried,
                        start.elapsed(),
                        attempt_log,
                    );
                }
            }

            // Health/circuit breaker check
            if self.config.health_checks_enabled {
                if let Some(health_mgr) = &self.health_manager {
                    let status = health_mgr.get_status(&current_model).await;

                    if !status.can_call() && !status.can_call_for_recovery() {
                        // Circuit is open, emit event and try failover
                        self.emit_event(OrchestratorEvent::CircuitBreakerSkip {
                            request_id: request.id.clone(),
                            model_id: current_model.clone(),
                        });

                        if request.allow_failover {
                            // Get health statuses for failover selection
                            let health_map = self.get_health_statuses(failover_chain).await;
                            let cost_map = self.get_cost_tiers(failover_chain).await;

                            if let Some(next) = failover_chain.next_model_sync(
                                &models_tried,
                                &health_map,
                                &cost_map,
                            ) {
                                self.emit_event(OrchestratorEvent::Failover {
                                    request_id: request.id.clone(),
                                    from_model: current_model.clone(),
                                    to_model: next.clone(),
                                    reason: "circuit_breaker_open".to_string(),
                                });

                                models_tried.push(current_model.clone());
                                current_model = next;

                                if self.config.reset_retries_on_failover {
                                    retries_for_current_model = 0;
                                }
                                continue;
                            }
                        }

                        return ExecutionResult::failure(
                            ExecutionError::CircuitOpen {
                                model_id: current_model,
                            },
                            attempts,
                            models_tried,
                            start.elapsed(),
                            attempt_log,
                        );
                    }
                }
            }

            // Track this model
            if !models_tried.contains(&current_model) {
                models_tried.push(current_model.clone());
            }

            // Create attempt record
            let mut attempt_record = AttemptRecord::new(attempts, &current_model);
            if attempts > 1 {
                if let Some(delay) = rate_limit_hint {
                    attempt_record = attempt_record.with_backoff(delay);
                }
            }
            if models_tried.len() > 1 && models_tried.last() == Some(&current_model) {
                attempt_record = attempt_record.as_failover();
            }

            // Execute with timeout
            let attempt_start = Instant::now();
            let timeout = retry_policy.attempt_timeout();

            let result = tokio::time::timeout(timeout, executor(current_model.clone(), request.clone())).await;

            let attempt_duration = attempt_start.elapsed();

            // Process result
            let (outcome, error_detail) = match &result {
                Ok(Ok(_)) => (CallOutcome::Success, None),
                Ok(Err(outcome)) => (*outcome, Some(outcome.error_type().to_string())),
                Err(_) => (CallOutcome::Timeout, Some("attempt timeout".to_string())),
            };

            attempt_record = attempt_record
                .with_duration(attempt_duration)
                .with_outcome(outcome);

            if let Some(err) = &error_detail {
                attempt_record = attempt_record.with_error(err);
            }

            attempt_log.push(attempt_record);

            // Success - return result
            if let Ok(Ok(value)) = result {
                self.emit_event(OrchestratorEvent::ExecutionSuccess {
                    request_id: request.id.clone(),
                    model_id: current_model.clone(),
                    attempts,
                    duration_ms: start.elapsed().as_millis() as u64,
                });

                return ExecutionResult::success(
                    value,
                    attempts,
                    models_tried,
                    start.elapsed(),
                    attempt_log,
                );
            }

            // Extract rate limit hint if available
            if outcome == CallOutcome::RateLimited {
                // In a real implementation, we'd parse Retry-After header
                // For now, use a default hint
                rate_limit_hint = Some(Duration::from_secs(5));
            }

            // Check if we should retry on same model
            let should_retry = retry_policy.should_retry(&outcome)
                && retries_for_current_model < retry_policy.max_attempts;

            if should_retry {
                // Calculate backoff delay
                let delay = backoff.delay_for_attempt(retries_for_current_model - 1, rate_limit_hint);

                self.emit_event(OrchestratorEvent::RetryAttempt {
                    request_id: request.id.clone(),
                    attempt: attempts + 1,
                    model_id: current_model.clone(),
                    reason: outcome.error_type().to_string(),
                    backoff_ms: delay.as_millis() as u64,
                });

                // Wait for backoff
                tokio::time::sleep(delay).await;
                continue;
            }

            // Check if we should failover
            let should_failover = request.allow_failover && retry_policy.should_failover(&outcome);

            if should_failover {
                // Get health statuses for failover selection
                let health_map = self.get_health_statuses(failover_chain).await;
                let cost_map = self.get_cost_tiers(failover_chain).await;

                if let Some(next) =
                    failover_chain.next_model_sync(&models_tried, &health_map, &cost_map)
                {
                    self.emit_event(OrchestratorEvent::Failover {
                        request_id: request.id.clone(),
                        from_model: current_model.clone(),
                        to_model: next.clone(),
                        reason: outcome.error_type().to_string(),
                    });

                    current_model = next;

                    if self.config.reset_retries_on_failover {
                        retries_for_current_model = 0;
                    }
                    continue;
                }
            }

            // No more options - return failure
            self.emit_event(OrchestratorEvent::ExecutionFailed {
                request_id: request.id.clone(),
                error: outcome.error_type().to_string(),
                attempts,
                duration_ms: start.elapsed().as_millis() as u64,
            });

            return ExecutionResult::failure(
                ExecutionError::MaxAttemptsExceeded {
                    attempts,
                    last_outcome: outcome,
                },
                attempts,
                models_tried,
                start.elapsed(),
                attempt_log,
            );
        }
    }

    /// Simple execute without failover chain (single model only)
    pub async fn execute_simple<T, F, Fut>(
        &self,
        request: ExecutionRequest,
        executor: F,
    ) -> ExecutionResult<T>
    where
        F: Fn(String, ExecutionRequest) -> Fut,
        Fut: Future<Output = Result<T, CallOutcome>>,
    {
        // Create a single-model failover chain
        let chain = FailoverChain::new(&request.preferred_model);
        self.execute(request.without_failover(), &chain, executor).await
    }

    // =========================================================================
    // Helper Methods
    // =========================================================================

    /// Estimate cost for a request
    async fn estimate_cost(&self, model_id: &str, request: &ExecutionRequest) -> CostEstimate {
        if let Some(budget_mgr) = &self.budget_manager {
            let estimator = budget_mgr.estimator().await;
            estimator.estimate(
                model_id,
                request.input_tokens,
                Some(request.estimated_output_tokens),
            )
        } else {
            // Default estimate
            CostEstimate {
                model_id: model_id.to_string(),
                input_tokens: request.input_tokens,
                estimated_output_tokens: request.estimated_output_tokens,
                base_cost_usd: 0.0,
                with_margin_usd: 0.0,
                pricing_source: super::budget::PricingSource::Default,
            }
        }
    }

    /// Get health statuses for models in failover chain
    async fn get_health_statuses(
        &self,
        chain: &FailoverChain,
    ) -> HashMap<String, HealthStatus> {
        let mut statuses = HashMap::new();

        if let Some(health_mgr) = &self.health_manager {
            for model_id in chain.all_models() {
                let status = health_mgr.get_status(model_id).await;
                statuses.insert(model_id.to_string(), status);
            }
        } else {
            // Assume all healthy if no health manager
            for model_id in chain.all_models() {
                statuses.insert(model_id.to_string(), HealthStatus::Unknown);
            }
        }

        statuses
    }

    /// Get cost tiers for models in failover chain
    async fn get_cost_tiers(
        &self,
        chain: &FailoverChain,
    ) -> HashMap<String, super::profiles::CostTier> {
        let profiles = self.profiles.read().await;
        let mut tiers = HashMap::new();

        for model_id in chain.all_models() {
            if let Some(profile) = profiles.get(model_id) {
                tiers.insert(model_id.to_string(), profile.cost_tier);
            }
        }

        tiers
    }

    /// Emit an event if events are enabled
    fn emit_event(&self, event: OrchestratorEvent) {
        if self.config.events_enabled {
            if let Some(tx) = &self.event_tx {
                let _ = tx.send(event);
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn test_execution_request_builder() {
        let request = ExecutionRequest::new("req-1", "gpt-4o", TaskIntent::CodeGeneration)
            .with_input_tokens(1000)
            .with_estimated_output_tokens(500)
            .with_budget_scope(BudgetScope::project("test"))
            .with_metadata("key", "value")
            .without_failover();

        assert_eq!(request.id, "req-1");
        assert_eq!(request.preferred_model, "gpt-4o");
        assert_eq!(request.input_tokens, 1000);
        assert!(!request.allow_failover);
        assert_eq!(request.metadata.get("key"), Some(&"value".to_string()));
    }

    #[test]
    fn test_attempt_record() {
        let record = AttemptRecord::new(1, "gpt-4o")
            .with_duration(Duration::from_millis(500))
            .with_outcome(CallOutcome::Success)
            .as_failover()
            .with_backoff(Duration::from_millis(100));

        assert_eq!(record.attempt_number, 1);
        assert_eq!(record.model_id, "gpt-4o");
        assert_eq!(record.duration_ms, 500);
        assert!(record.is_failover);
        assert_eq!(record.backoff_delay_ms, Some(100));
    }

    #[test]
    fn test_execution_result_success() {
        let result: ExecutionResult<String> = ExecutionResult::success(
            "output".to_string(),
            1,
            vec!["gpt-4o".to_string()],
            Duration::from_secs(1),
            vec![],
        );

        assert!(result.is_success());
        assert!(!result.is_failure());
        assert_eq!(result.attempts, 1);
        assert_eq!(result.final_model, Some("gpt-4o".to_string()));
    }

    #[test]
    fn test_execution_result_failure() {
        let result: ExecutionResult<String> = ExecutionResult::failure(
            ExecutionError::MaxAttemptsExceeded {
                attempts: 3,
                last_outcome: CallOutcome::Timeout,
            },
            3,
            vec!["gpt-4o".to_string()],
            Duration::from_secs(10),
            vec![],
        );

        assert!(!result.is_success());
        assert!(result.is_failure());
        assert!(result.err().is_some());
    }

    #[test]
    fn test_execution_error_types() {
        let budget_err = ExecutionError::BudgetExceeded {
            message: "exceeded".into(),
        };
        assert!(budget_err.is_budget_error());
        assert!(!budget_err.is_health_error());

        let health_err = ExecutionError::CircuitOpen {
            model_id: "test".into(),
        };
        assert!(health_err.is_health_error());
        assert!(!health_err.is_budget_error());
    }

    #[test]
    fn test_orchestrator_config_default() {
        let config = OrchestratorConfig::default();
        assert!(config.budget_checks_enabled);
        assert!(config.health_checks_enabled);
        assert!(config.events_enabled);
        assert!(!config.reset_retries_on_failover);
    }

    #[tokio::test]
    async fn test_orchestrator_execute_success() {
        let orchestrator = RetryOrchestrator::with_defaults();
        let request = ExecutionRequest::new("req-1", "gpt-4o", TaskIntent::CodeGeneration);
        let chain = FailoverChain::new("gpt-4o");

        let result = orchestrator
            .execute(request, &chain, |_model, _req| async { Ok("success") })
            .await;

        assert!(result.is_success());
        assert_eq!(result.attempts, 1);
        assert_eq!(result.ok(), Some("success"));
    }

    #[tokio::test]
    async fn test_orchestrator_execute_retry_then_success() {
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        let orchestrator = RetryOrchestrator::with_defaults();
        let request = ExecutionRequest::new("req-1", "gpt-4o", TaskIntent::CodeGeneration);
        let chain = FailoverChain::new("gpt-4o");

        let result = orchestrator
            .execute(request, &chain, move |_model, _req| {
                let count = call_count_clone.clone();
                async move {
                    let current = count.fetch_add(1, Ordering::SeqCst);
                    if current < 2 {
                        Err(CallOutcome::Timeout)
                    } else {
                        Ok("success after retries")
                    }
                }
            })
            .await;

        assert!(result.is_success());
        assert_eq!(result.attempts, 3);
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_orchestrator_execute_max_attempts_exceeded() {
        let orchestrator = RetryOrchestrator::with_defaults();
        let request = ExecutionRequest::new("req-1", "gpt-4o", TaskIntent::CodeGeneration)
            .with_retry_policy(RetryPolicy::new().with_max_attempts(2));
        let chain = FailoverChain::new("gpt-4o");

        let result: ExecutionResult<String> = orchestrator
            .execute(request.without_failover(), &chain, |_model, _req| async {
                Err(CallOutcome::Timeout)
            })
            .await;

        assert!(result.is_failure());
        assert_eq!(result.attempts, 2);

        match result.err() {
            Some(ExecutionError::MaxAttemptsExceeded { attempts, .. }) => {
                assert_eq!(*attempts, 2);
            }
            other => panic!("Expected MaxAttemptsExceeded, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_orchestrator_execute_failover() {
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        let orchestrator = RetryOrchestrator::with_defaults();
        let request = ExecutionRequest::new("req-1", "gpt-4o", TaskIntent::CodeGeneration)
            .with_retry_policy(RetryPolicy::new().with_max_attempts(1)); // Only 1 attempt per model

        let chain = FailoverChain::new("gpt-4o")
            .with_alternatives(vec!["claude-sonnet".to_string()]);

        let result = orchestrator
            .execute(request, &chain, move |model, _req| {
                let count = call_count_clone.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    if model == "gpt-4o" {
                        Err(CallOutcome::Timeout)
                    } else {
                        Ok(format!("success from {}", model))
                    }
                }
            })
            .await;

        assert!(result.is_success());
        assert_eq!(result.models_tried.len(), 2);
        assert!(result.models_tried.contains(&"gpt-4o".to_string()));
        assert!(result.models_tried.contains(&"claude-sonnet".to_string()));
        assert_eq!(result.ok(), Some("success from claude-sonnet".to_string()));
    }

    #[tokio::test]
    async fn test_orchestrator_execute_simple() {
        let orchestrator = RetryOrchestrator::with_defaults();
        let request = ExecutionRequest::new("req-1", "gpt-4o", TaskIntent::CodeGeneration);

        let result = orchestrator
            .execute_simple(request, |_model, _req| async { Ok(42) })
            .await;

        assert!(result.is_success());
        assert_eq!(result.ok(), Some(42));
    }

    #[tokio::test]
    async fn test_orchestrator_non_retryable_error() {
        let orchestrator = RetryOrchestrator::with_defaults();
        let request = ExecutionRequest::new("req-1", "gpt-4o", TaskIntent::CodeGeneration)
            .without_failover();
        let chain = FailoverChain::new("gpt-4o");

        let result: ExecutionResult<String> = orchestrator
            .execute(request, &chain, |_model, _req| async {
                Err(CallOutcome::ContentFiltered)
            })
            .await;

        // ContentFiltered is not retryable and failover is disabled
        assert!(result.is_failure());
        assert_eq!(result.attempts, 1);
    }

    #[tokio::test]
    async fn test_orchestrator_event_subscription() {
        let orchestrator = RetryOrchestrator::with_defaults();
        let mut receiver = orchestrator.subscribe().expect("should have subscriber");

        let request = ExecutionRequest::new("req-1", "gpt-4o", TaskIntent::CodeGeneration);
        let chain = FailoverChain::new("gpt-4o");

        // Execute in background
        let orchestrator_clone = Arc::new(orchestrator);
        let handle = tokio::spawn({
            let orch = orchestrator_clone.clone();
            async move {
                orch.execute(request, &chain, |_model, _req| async { Ok("done") })
                    .await
            }
        });

        // Should receive ExecutionStarted event
        let event = receiver.recv().await.expect("should receive event");
        match event {
            OrchestratorEvent::ExecutionStarted { request_id, .. } => {
                assert_eq!(request_id, "req-1");
            }
            _ => panic!("Expected ExecutionStarted event"),
        }

        handle.await.unwrap();
    }
}
