//! Orchestrated Router
//!
//! This module provides the `OrchestratedRouter` which combines intelligent routing
//! with resilient execution through retry, failover, and budget management.
//!
//! # Architecture
//!
//! ```text
//!                                    ┌──────────────────────┐
//!                                    │  OrchestratedRouter  │
//!                                    └──────────┬───────────┘
//!                                               │
//!           ┌───────────────┬──────────────────┼──────────────────┬───────────────┐
//!           │               │                  │                  │               │
//!   ┌───────▼───────┐ ┌─────▼─────┐ ┌──────────▼──────────┐ ┌─────▼─────┐ ┌───────▼───────┐
//!   │  ModelMatcher │ │ HealthMgr │ │  RetryOrchestrator  │ │ BudgetMgr │ │ MetricsCollec │
//!   └───────────────┘ └───────────┘ └─────────────────────┘ └───────────┘ └───────────────┘
//! ```

use super::budget::{BudgetCheckResult, BudgetManager, BudgetScope};
use super::failover::{FailoverChain, FailoverSelectionMode};
use super::orchestrator::{
    ExecutionError, ExecutionRequest, ExecutionResult, OrchestratorConfig, OrchestratorEvent,
    RetryOrchestrator,
};
use super::retry::{BackoffStrategy, RetryPolicy};
use crate::dispatcher::model_router::{
    CallOutcome, Capability, DynamicScorer, HealthManager, MetricsCollector, ModelMatcher,
    ModelProfile, ModelRouter, RoutingError, TaskIntent,
};
use std::collections::HashMap;
use std::future::Future;
use crate::sync_primitives::Arc;
use tokio::sync::{broadcast, RwLock};

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the orchestrated router
#[derive(Debug, Clone)]
pub struct OrchestratedRouterConfig {
    /// Default retry policy
    pub default_retry_policy: RetryPolicy,

    /// Default backoff strategy
    pub default_backoff: BackoffStrategy,

    /// Default failover selection mode
    pub default_failover_mode: FailoverSelectionMode,

    /// Whether to enable budget checking
    pub enable_budget_check: bool,

    /// Whether to enable health checking
    pub enable_health_check: bool,

    /// Whether to auto-build failover chains from matcher
    pub auto_build_failover_chain: bool,

    /// Event channel buffer size
    pub event_buffer_size: usize,
}

impl Default for OrchestratedRouterConfig {
    fn default() -> Self {
        Self {
            default_retry_policy: RetryPolicy::default(),
            default_backoff: BackoffStrategy::ExponentialJitter {
                initial_ms: 100,
                max_ms: 5000,
                jitter_factor: 0.2,
            },
            default_failover_mode: FailoverSelectionMode::HealthPriority,
            enable_budget_check: true,
            enable_health_check: true,
            auto_build_failover_chain: true,
            event_buffer_size: 64,
        }
    }
}

// =============================================================================
// Router Events
// =============================================================================

/// Events emitted by the orchestrated router
#[derive(Debug, Clone)]
pub enum RouterEvent {
    /// Routing started
    RoutingStarted {
        request_id: String,
        intent: TaskIntent,
        preferred_model: String,
    },

    /// Budget check completed
    BudgetChecked {
        request_id: String,
        result: BudgetCheckResultSummary,
    },

    /// Model selected for execution
    ModelSelected {
        request_id: String,
        model_id: String,
        reason: String,
    },

    /// Execution completed
    ExecutionCompleted {
        request_id: String,
        success: bool,
        attempts: u32,
        models_tried: Vec<String>,
        duration_ms: u64,
    },

    /// Cost recorded
    CostRecorded {
        request_id: String,
        model_id: String,
        cost_usd: f64,
        remaining_usd: f64,
    },

    /// Forwarded from RetryOrchestrator
    OrchestratorEvent(OrchestratorEvent),
}

/// Summary of budget check result for events
#[derive(Debug, Clone)]
pub enum BudgetCheckResultSummary {
    Allowed { remaining_usd: f64 },
    Warning { message: String },
    Blocked { reason: String },
}

// =============================================================================
// Routing Request
// =============================================================================

/// High-level routing request with full context
#[derive(Debug, Clone)]
pub struct RoutingRequest {
    /// Unique request ID
    pub id: String,

    /// Task intent
    pub intent: TaskIntent,

    /// Preferred model (if any)
    pub preferred_model: Option<String>,

    /// Input token count (for cost estimation)
    pub input_tokens: u32,

    /// Estimated output tokens (for cost estimation)
    pub estimated_output_tokens: u32,

    /// Budget scope for this request
    pub budget_scope: BudgetScope,

    /// Override retry policy
    pub retry_policy: Option<RetryPolicy>,

    /// Override backoff strategy
    pub backoff_strategy: Option<BackoffStrategy>,

    /// Custom failover chain (if not auto-building)
    pub failover_chain: Option<FailoverChain>,

    /// Required capabilities (filters alternatives)
    pub required_capabilities: Vec<Capability>,
}

impl RoutingRequest {
    /// Create a new routing request with minimal parameters
    pub fn new(id: impl Into<String>, intent: TaskIntent) -> Self {
        Self {
            id: id.into(),
            intent,
            preferred_model: None,
            input_tokens: 0,
            estimated_output_tokens: 1000, // Default estimate
            budget_scope: BudgetScope::Global,
            retry_policy: None,
            backoff_strategy: None,
            failover_chain: None,
            required_capabilities: Vec::new(),
        }
    }

    pub fn with_preferred_model(mut self, model: impl Into<String>) -> Self {
        self.preferred_model = Some(model.into());
        self
    }

    pub fn with_tokens(mut self, input: u32, estimated_output: u32) -> Self {
        self.input_tokens = input;
        self.estimated_output_tokens = estimated_output;
        self
    }

    pub fn with_budget_scope(mut self, scope: BudgetScope) -> Self {
        self.budget_scope = scope;
        self
    }

    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = Some(policy);
        self
    }

    pub fn with_failover_chain(mut self, chain: FailoverChain) -> Self {
        self.failover_chain = Some(chain);
        self
    }

    pub fn with_required_capabilities(mut self, capabilities: Vec<Capability>) -> Self {
        self.required_capabilities = capabilities;
        self
    }
}

// =============================================================================
// Routing Result
// =============================================================================

/// High-level result combining routing and execution
#[derive(Debug, Clone)]
pub struct RoutingResult<T: Clone> {
    /// The final result
    pub result: Result<T, RoutingExecutionError>,

    /// Selected primary model
    pub primary_model: String,

    /// Execution details
    pub execution: ExecutionResult<T>,

    /// Budget status after execution
    pub budget_remaining_usd: Option<f64>,
}

/// Combined error type for routing and execution
#[derive(Debug, Clone)]
pub enum RoutingExecutionError {
    /// Routing failed
    Routing(String),

    /// Budget exceeded before execution
    BudgetExceeded { message: String },

    /// Execution failed after retries
    Execution(ExecutionError),
}

impl std::fmt::Display for RoutingExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Routing(msg) => write!(f, "Routing error: {}", msg),
            Self::BudgetExceeded { message } => write!(f, "Budget exceeded: {}", message),
            Self::Execution(e) => write!(f, "Execution error: {}", e),
        }
    }
}

impl std::error::Error for RoutingExecutionError {}

// =============================================================================
// Orchestrated Router
// =============================================================================

/// Router that combines intelligent routing with resilient execution
pub struct OrchestratedRouter {
    /// Configuration
    config: OrchestratedRouterConfig,

    /// Model matcher for routing decisions
    matcher: Arc<ModelMatcher>,

    /// Health manager for circuit breaker
    health_manager: Arc<HealthManager>,

    /// Retry orchestrator for resilient execution
    orchestrator: RetryOrchestrator,

    /// Budget manager (optional)
    budget_manager: Option<Arc<BudgetManager>>,

    /// Metrics collector
    _collector: Arc<dyn MetricsCollector + Send + Sync>,

    /// Dynamic scorer
    _scorer: Arc<DynamicScorer>,

    /// Model profiles for failover chain building
    profiles: Arc<RwLock<HashMap<String, ModelProfile>>>,

    /// Event sender
    event_tx: broadcast::Sender<RouterEvent>,
}

impl OrchestratedRouter {
    /// Create a new orchestrated router
    pub fn new(
        matcher: Arc<ModelMatcher>,
        health_manager: Arc<HealthManager>,
        collector: Arc<dyn MetricsCollector + Send + Sync>,
        scorer: Arc<DynamicScorer>,
    ) -> Self {
        let config = OrchestratedRouterConfig::default();
        let (event_tx, _) = broadcast::channel(config.event_buffer_size);

        // Build profiles map from matcher
        let profiles: HashMap<String, ModelProfile> = matcher
            .profiles()
            .iter()
            .map(|p| (p.id.clone(), p.clone()))
            .collect();

        // Create orchestrator
        let profile_vec: Vec<ModelProfile> = profiles.values().cloned().collect();
        let orchestrator = RetryOrchestrator::new(OrchestratorConfig::default())
            .with_health_manager(Arc::clone(&health_manager))
            .with_profiles(profile_vec);

        Self {
            config,
            matcher,
            health_manager,
            orchestrator,
            budget_manager: None,
            _collector: collector,
            _scorer: scorer,
            profiles: Arc::new(RwLock::new(profiles)),
            event_tx,
        }
    }

    /// Configure the router
    pub fn with_config(mut self, config: OrchestratedRouterConfig) -> Self {
        self.config = config;
        self
    }

    /// Add budget manager
    pub fn with_budget_manager(mut self, manager: Arc<BudgetManager>) -> Self {
        self.budget_manager = Some(Arc::clone(&manager));
        self.orchestrator = self.orchestrator.with_budget_manager(manager);
        self
    }

    /// Subscribe to router events
    pub fn subscribe_events(&self) -> broadcast::Receiver<RouterEvent> {
        self.event_tx.subscribe()
    }

    /// Get current configuration
    pub fn config(&self) -> &OrchestratedRouterConfig {
        &self.config
    }

    /// Get health manager reference
    pub fn health_manager(&self) -> &Arc<HealthManager> {
        &self.health_manager
    }

    /// Get budget manager reference (if configured)
    pub fn budget_manager(&self) -> Option<&Arc<BudgetManager>> {
        self.budget_manager.as_ref()
    }

    // =========================================================================
    // Core Execution
    // =========================================================================

    /// Execute a request with full retry, failover, and budget support
    ///
    /// This is the main entry point that:
    /// 1. Routes to select primary model
    /// 2. Pre-checks budget
    /// 3. Builds failover chain
    /// 4. Executes with retry orchestrator
    /// 5. Records cost on success
    pub async fn execute<T, F, Fut>(&self, request: RoutingRequest, executor: F) -> RoutingResult<T>
    where
        T: Clone + Send + 'static,
        F: Fn(String, ExecutionRequest) -> Fut + Send + Sync,
        Fut: Future<Output = Result<T, CallOutcome>> + Send,
    {
        // Emit start event
        let _ = self.event_tx.send(RouterEvent::RoutingStarted {
            request_id: request.id.clone(),
            intent: request.intent.clone(),
            preferred_model: request.preferred_model.clone().unwrap_or_default(),
        });

        // 1. Route to select primary model
        let primary_model = match self.select_primary_model(&request).await {
            Ok(model) => model,
            Err(e) => {
                let err_msg = e.to_string();
                return RoutingResult {
                    result: Err(RoutingExecutionError::Routing(err_msg.clone())),
                    primary_model: String::new(),
                    execution: ExecutionResult {
                        result: Err(ExecutionError::Internal { message: err_msg }),
                        attempts: 0,
                        models_tried: vec![],
                        total_duration_ms: 0,
                        attempt_log: vec![],
                        final_model: None,
                        estimated_cost: None,
                    },
                    budget_remaining_usd: None,
                };
            }
        };

        let _ = self.event_tx.send(RouterEvent::ModelSelected {
            request_id: request.id.clone(),
            model_id: primary_model.clone(),
            reason: "Primary model selected".to_string(),
        });

        // 2. Estimate cost and pre-check budget
        if self.config.enable_budget_check {
            if let Some(budget_mgr) = &self.budget_manager {
                let estimate = budget_mgr.estimate_cost(
                    &primary_model,
                    request.input_tokens,
                    request.estimated_output_tokens,
                );

                let check_result = budget_mgr
                    .check_budget(&request.budget_scope, &estimate)
                    .await;

                // Emit budget check event
                let summary = match &check_result {
                    BudgetCheckResult::Allowed { remaining_usd } => {
                        BudgetCheckResultSummary::Allowed {
                            remaining_usd: *remaining_usd,
                        }
                    }
                    BudgetCheckResult::Warning { message, .. } => {
                        BudgetCheckResultSummary::Warning {
                            message: message.clone(),
                        }
                    }
                    BudgetCheckResult::SoftBlocked { .. }
                    | BudgetCheckResult::HardBlocked { .. } => BudgetCheckResultSummary::Blocked {
                        reason: check_result.message(),
                    },
                };

                let _ = self.event_tx.send(RouterEvent::BudgetChecked {
                    request_id: request.id.clone(),
                    result: summary,
                });

                // Block on hard limit
                if let BudgetCheckResult::HardBlocked { .. } = check_result {
                    return RoutingResult {
                        result: Err(RoutingExecutionError::BudgetExceeded {
                            message: check_result.message(),
                        }),
                        primary_model,
                        execution: ExecutionResult {
                            result: Err(ExecutionError::BudgetExceeded {
                                message: check_result.message(),
                            }),
                            attempts: 0,
                            models_tried: vec![],
                            total_duration_ms: 0,
                            attempt_log: vec![],
                            final_model: None,
                            estimated_cost: None,
                        },
                        budget_remaining_usd: None,
                    };
                }
            }
        }

        // 3. Build failover chain
        let failover_chain = self.build_failover_chain(&request, &primary_model).await;

        // 4. Build execution request
        let exec_request = ExecutionRequest {
            id: request.id.clone(),
            preferred_model: primary_model.clone(),
            intent: request.intent.clone(),
            input_tokens: request.input_tokens,
            estimated_output_tokens: request.estimated_output_tokens,
            budget_scope: request.budget_scope.clone(),
            retry_policy: request
                .retry_policy
                .or(Some(self.config.default_retry_policy.clone())),
            backoff_strategy: request
                .backoff_strategy
                .or(Some(self.config.default_backoff.clone())),
            allow_failover: true,
            metadata: HashMap::new(),
        };

        // 5. Execute with orchestrator
        let execution = self
            .orchestrator
            .execute(exec_request, &failover_chain, executor)
            .await;

        // 6. Emit completion event
        let _ = self.event_tx.send(RouterEvent::ExecutionCompleted {
            request_id: request.id.clone(),
            success: execution.result.is_ok(),
            attempts: execution.attempts,
            models_tried: execution.models_tried.clone(),
            duration_ms: execution.total_duration_ms,
        });

        // 7. Record cost if successful
        let budget_remaining = if execution.result.is_ok() {
            if let Some(budget_mgr) = &self.budget_manager {
                // Use the last successful model from attempt log
                let actual_model = execution
                    .attempt_log
                    .last()
                    .map(|a| a.model_id.clone())
                    .unwrap_or(primary_model.clone());

                // Estimate actual cost (would need actual tokens from response in real impl)
                let estimate = budget_mgr.estimate_cost(
                    &actual_model,
                    request.input_tokens,
                    request.estimated_output_tokens,
                );

                budget_mgr
                    .record_cost_direct(&request.budget_scope, estimate.base_cost_usd)
                    .await;

                // Get remaining budget
                let states = budget_mgr.get_status(&request.budget_scope).await;
                let remaining = states.first().map(|s| s.remaining_usd);

                if let Some(rem) = remaining {
                    let _ = self.event_tx.send(RouterEvent::CostRecorded {
                        request_id: request.id.clone(),
                        model_id: actual_model,
                        cost_usd: estimate.base_cost_usd,
                        remaining_usd: rem,
                    });
                }

                remaining
            } else {
                None
            }
        } else {
            None
        };

        // 8. Build final result
        let result = match &execution.result {
            Ok(v) => Ok(v.clone()),
            Err(e) => Err(RoutingExecutionError::Execution(e.clone())),
        };

        RoutingResult {
            result,
            primary_model,
            execution,
            budget_remaining_usd: budget_remaining,
        }
    }

    /// Simple execution without full routing (uses provided model directly)
    pub async fn execute_simple<T, F, Fut>(
        &self,
        model_id: String,
        intent: TaskIntent,
        input_tokens: u32,
        estimated_output_tokens: u32,
        executor: F,
    ) -> ExecutionResult<T>
    where
        T: Clone + Send + 'static,
        F: Fn(String, ExecutionRequest) -> Fut + Send + Sync,
        Fut: Future<Output = Result<T, CallOutcome>> + Send,
    {
        let exec_request = ExecutionRequest {
            id: uuid::Uuid::new_v4().to_string(),
            preferred_model: model_id.clone(),
            intent,
            input_tokens,
            estimated_output_tokens,
            budget_scope: BudgetScope::Global,
            retry_policy: Some(self.config.default_retry_policy.clone()),
            backoff_strategy: Some(self.config.default_backoff.clone()),
            allow_failover: false,
            metadata: HashMap::new(),
        };

        // Build simple failover chain with just the specified model
        let failover_chain = FailoverChain::new(model_id);

        self.orchestrator
            .execute(exec_request, &failover_chain, executor)
            .await
    }

    // =========================================================================
    // Helper Methods
    // =========================================================================

    /// Select primary model based on request
    async fn select_primary_model(&self, request: &RoutingRequest) -> Result<String, RoutingError> {
        // If preferred model specified and healthy, use it
        if let Some(preferred) = &request.preferred_model {
            if !self.config.enable_health_check || self.health_manager.can_call(preferred).await {
                return Ok(preferred.clone());
            }
        }

        // Otherwise route by intent
        let profile = self
            .matcher
            .route_by_intent(&request.intent)
            .ok_or_else(|| RoutingError::NoModelAvailable {
                task_type: request.intent.to_task_type().to_string(),
            })?;

        // If health check enabled, verify model is healthy
        if self.config.enable_health_check && !self.health_manager.can_call(&profile.id).await {
            // Try to find alternative healthy model
            let profiles = self.profiles.read().await;
            for (id, p) in profiles.iter() {
                if id != &profile.id && self.health_manager.can_call(id).await {
                    // Check capability match
                    if let Some(cap) = request.intent.required_capability() {
                        if p.has_capability(cap) {
                            return Ok(id.clone());
                        }
                    } else {
                        return Ok(id.clone());
                    }
                }
            }

            return Err(RoutingError::NoModelAvailable {
                task_type: request.intent.to_task_type().to_string(),
            });
        }

        Ok(profile.id.clone())
    }

    /// Build failover chain for request
    async fn build_failover_chain(
        &self,
        request: &RoutingRequest,
        primary_model: &str,
    ) -> FailoverChain {
        // If custom chain provided, use it
        if let Some(chain) = &request.failover_chain {
            return chain.clone();
        }

        // If auto-build disabled, return simple chain
        if !self.config.auto_build_failover_chain {
            return FailoverChain::new(primary_model.to_string());
        }

        // Build from matcher
        let profiles = self.profiles.read().await;
        let primary_profile = profiles.get(primary_model);

        // Get required capabilities
        let required_caps: Vec<Capability> = if !request.required_capabilities.is_empty() {
            request.required_capabilities.clone()
        } else if let Some(cap) = request.intent.required_capability() {
            vec![cap]
        } else if let Some(profile) = primary_profile {
            profile.capabilities.clone()
        } else {
            vec![]
        };

        // Find alternatives with overlapping capabilities
        let alternatives: Vec<String> = profiles
            .iter()
            .filter(|(id, _)| *id != primary_model)
            .filter(|(_, p)| {
                if required_caps.is_empty() {
                    true
                } else {
                    required_caps.iter().any(|c| p.capabilities.contains(c))
                }
            })
            .map(|(id, _)| id.clone())
            .collect();

        FailoverChain {
            primary: primary_model.to_string(),
            alternatives,
            selection_mode: self.config.default_failover_mode,
            required_capabilities: required_caps,
            health_weight: 0.6,
            cost_weight: 0.4,
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::model_router::health::collector::{InMemoryMetricsCollector, MetricsConfig};
    use crate::dispatcher::model_router::health::status::HealthConfig;
    use crate::dispatcher::model_router::core::profiles::{CostTier, LatencyTier};
    use crate::dispatcher::model_router::core::rules::ModelRoutingRules;
    use crate::dispatcher::model_router::core::scoring::ScoringConfig;

    fn create_test_profiles() -> Vec<ModelProfile> {
        vec![
            ModelProfile::new("model-a", "provider-a", "a")
                .with_capabilities(vec![Capability::CodeGeneration])
                .with_cost_tier(CostTier::Low)
                .with_latency_tier(LatencyTier::Fast),
            ModelProfile::new("model-b", "provider-b", "b")
                .with_capabilities(vec![Capability::CodeGeneration, Capability::Reasoning])
                .with_cost_tier(CostTier::Medium)
                .with_latency_tier(LatencyTier::Medium),
            ModelProfile::new("model-c", "provider-c", "c")
                .with_capabilities(vec![Capability::Reasoning])
                .with_cost_tier(CostTier::High)
                .with_latency_tier(LatencyTier::Slow),
        ]
    }

    fn create_router() -> OrchestratedRouter {
        let profiles = create_test_profiles();
        let rules = ModelRoutingRules::new("model-b");
        let matcher = Arc::new(ModelMatcher::new(profiles, rules));
        let health_manager = Arc::new(HealthManager::new(HealthConfig::default()));
        let collector: Arc<dyn MetricsCollector + Send + Sync> =
            Arc::new(InMemoryMetricsCollector::new(MetricsConfig::default()));
        let scorer = Arc::new(DynamicScorer::new(ScoringConfig::default()));

        OrchestratedRouter::new(matcher, health_manager, collector, scorer)
    }

    #[test]
    fn test_routing_request_builder() {
        let request = RoutingRequest::new("req-1", TaskIntent::CodeGeneration)
            .with_preferred_model("model-a")
            .with_tokens(100, 500)
            .with_budget_scope(BudgetScope::Session("s1".to_string()));

        assert_eq!(request.id, "req-1");
        assert_eq!(request.preferred_model, Some("model-a".to_string()));
        assert_eq!(request.input_tokens, 100);
        assert_eq!(request.estimated_output_tokens, 500);
    }

    #[test]
    fn test_orchestrated_router_config_default() {
        let config = OrchestratedRouterConfig::default();
        assert!(config.enable_budget_check);
        assert!(config.enable_health_check);
        assert!(config.auto_build_failover_chain);
        assert_eq!(config.event_buffer_size, 64);
    }

    #[tokio::test]
    async fn test_select_primary_model_preferred() {
        let router = create_router();
        let request = RoutingRequest::new("req-1", TaskIntent::CodeGeneration)
            .with_preferred_model("model-a");

        let result = router.select_primary_model(&request).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "model-a");
    }

    #[tokio::test]
    async fn test_select_primary_model_by_intent() {
        let router = create_router();
        let request = RoutingRequest::new("req-1", TaskIntent::CodeGeneration);

        let result = router.select_primary_model(&request).await;
        assert!(result.is_ok());
        // Should select a model with CodeGeneration capability
    }

    #[tokio::test]
    async fn test_build_failover_chain() {
        let router = create_router();
        let request = RoutingRequest::new("req-1", TaskIntent::CodeGeneration);

        let chain = router.build_failover_chain(&request, "model-a").await;

        assert_eq!(chain.primary, "model-a");
        // Should have alternatives with CodeGeneration capability
        assert!(chain.alternatives.contains(&"model-b".to_string()));
    }

    #[tokio::test]
    async fn test_execute_simple_success() {
        let router = create_router();

        let result = router
            .execute_simple(
                "model-a".to_string(),
                TaskIntent::CodeGeneration,
                100,
                500,
                |_model, _req| async { Ok("response".to_string()) },
            )
            .await;

        assert!(result.result.is_ok());
        assert_eq!(result.attempts, 1);
        assert_eq!(result.models_tried, vec!["model-a"]);
    }

    #[tokio::test]
    async fn test_execute_simple_retry() {
        let router = create_router();

        let attempt_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempt_count_clone = attempt_count.clone();

        let result = router
            .execute_simple(
                "model-a".to_string(),
                TaskIntent::CodeGeneration,
                100,
                500,
                move |_model, _req| {
                    let count = attempt_count_clone.clone();
                    async move {
                        let n = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if n < 2 {
                            Err(CallOutcome::RateLimited)
                        } else {
                            Ok("success".to_string())
                        }
                    }
                },
            )
            .await;

        assert!(result.result.is_ok());
        assert_eq!(result.attempts, 3);
    }

    #[tokio::test]
    async fn test_execute_full_flow() {
        let router = create_router();

        let request = RoutingRequest::new("req-1", TaskIntent::CodeGeneration)
            .with_preferred_model("model-a")
            .with_tokens(100, 500);

        let result = router
            .execute(request, |_model, _req| async { Ok("response".to_string()) })
            .await;

        assert!(result.result.is_ok());
        assert_eq!(result.primary_model, "model-a");
        assert_eq!(result.execution.attempts, 1);
    }

    #[tokio::test]
    async fn test_event_subscription() {
        let router = create_router();
        let mut rx = router.subscribe_events();

        let request = RoutingRequest::new("req-1", TaskIntent::CodeGeneration)
            .with_preferred_model("model-a");

        // Execute directly
        let _ = router
            .execute(request, |_model, _req| async { Ok("response".to_string()) })
            .await;

        // Check that events were emitted (should have at least RoutingStarted)
        let event = rx.try_recv();
        assert!(event.is_ok());
    }

    #[tokio::test]
    async fn test_routing_execution_error_display() {
        let err = RoutingExecutionError::BudgetExceeded {
            message: "Budget limit reached".to_string(),
        };
        assert!(err.to_string().contains("Budget exceeded"));

        let err = RoutingExecutionError::Routing("No model available".to_string());
        assert!(err.to_string().contains("Routing error"));
    }
}
