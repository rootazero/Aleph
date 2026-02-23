//! Retry Orchestrator Engine
//!
//! This module provides the `RetryOrchestrator` which orchestrates retry and
//! failover logic for resilient API call execution. It integrates with:
//! - RetryPolicy for retry decisions
//! - BackoffStrategy for delay calculation
//! - FailoverChain for alternative model selection
//! - HealthManager for circuit breaker checks
//! - BudgetManager for cost control
//! - MetricsCollector for observability

use super::super::budget::{BudgetCheckResult, BudgetManager, CostEstimate, PricingSource};
use super::super::failover::FailoverChain;
use super::super::retry::{BackoffStrategy, RetryPolicy};
use super::events::OrchestratorEvent;
use super::types::{AttemptRecord, ExecutionError, ExecutionRequest, ExecutionResult};
use crate::dispatcher::model_router::{CallOutcome, CostTier, HealthManager, HealthStatus, ModelProfile};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

// =============================================================================
// Executor Type Alias
// =============================================================================

/// Type alias for async executor function
pub type ExecutorFn<T> = Box<
    dyn Fn(String, ExecutionRequest) -> Pin<Box<dyn Future<Output = Result<T, CallOutcome>> + Send>>
        + Send
        + Sync,
>;

// =============================================================================
// Orchestrator Configuration
// =============================================================================

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

// =============================================================================
// Retry Orchestrator
// =============================================================================

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
        T: Clone,
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
                let check = budget_mgr
                    .check_budget(&request.budget_scope, &estimate)
                    .await;

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

            let result =
                tokio::time::timeout(timeout, executor(current_model.clone(), request.clone()))
                    .await;

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
                let delay =
                    backoff.delay_for_attempt(retries_for_current_model - 1, rate_limit_hint);

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
        T: Clone,
        F: Fn(String, ExecutionRequest) -> Fut,
        Fut: Future<Output = Result<T, CallOutcome>>,
    {
        // Create a single-model failover chain
        let chain = FailoverChain::new(&request.preferred_model);
        self.execute(request.without_failover(), &chain, executor)
            .await
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
                pricing_source: PricingSource::Default,
            }
        }
    }

    /// Get health statuses for models in failover chain
    async fn get_health_statuses(&self, chain: &FailoverChain) -> HashMap<String, HealthStatus> {
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
    async fn get_cost_tiers(&self, chain: &FailoverChain) -> HashMap<String, CostTier> {
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
