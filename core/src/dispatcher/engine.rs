//! AgentEngine - unified API for task orchestration
//!
//! Renamed from CoworkEngine to reflect the agent-centric architecture.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::agent_types::{ExecutionSummary, Task, TaskGraph, TaskResult, TaskStatus, TaskType};
use super::executor::{ExecutionContext, ExecutorRegistry, NoopExecutor};
use super::model_router::{
    ModelMatcher, ModelProfile, ModelRouter, ModelRoutingRules, TaskContextManager,
};
use super::monitor::{ProgressMonitor, ProgressSubscriber, TaskMonitor};
use super::planner::{GenerationProviders, LlmTaskPlanner, TaskPlanner};
use super::scheduler::{DagScheduler, SchedulerConfig, TaskScheduler};
use crate::error::{AetherError, Result};
use crate::providers::AiProvider;

// Hardcoded agent configuration constants (security-enforced, not user-configurable)
/// Whether to require user confirmation before execution (always true for safety)
pub const REQUIRE_CONFIRMATION: bool = true;
/// Maximum number of tasks to run in parallel
pub const MAX_PARALLELISM: usize = 4;
/// Maximum number of retry attempts for failed tasks
pub const MAX_TASK_RETRIES: u32 = 3;

// Security boundary constants for file operations and code execution
/// Maximum file size for file operations (100MB)
pub const DEFAULT_MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;
/// Whether sandbox is enabled by default for code execution
pub const DEFAULT_SANDBOX_ENABLED: bool = true;
/// Whether network access is allowed by default in sandbox
pub const DEFAULT_ALLOW_NETWORK: bool = false;
/// Default timeout for code execution in seconds
pub const DEFAULT_CODE_EXEC_TIMEOUT: u64 = 60;
/// Whether to require confirmation for write operations
pub const DEFAULT_REQUIRE_CONFIRMATION_FOR_WRITE: bool = true;
/// Whether to require confirmation for delete operations
pub const DEFAULT_REQUIRE_CONFIRMATION_FOR_DELETE: bool = true;
/// Whether file operations are enabled by default
pub const DEFAULT_FILE_OPS_ENABLED: bool = true;
/// Whether code execution is enabled by default (false for security)
pub const DEFAULT_CODE_EXEC_ENABLED: bool = false;
/// Default runtime for code execution
pub const DEFAULT_CODE_EXEC_RUNTIME: &str = "shell";
/// Default environment variables to pass to executed code
pub const DEFAULT_PASS_ENV: &[&str] = &["PATH", "HOME", "USER"];

// Code execution output limits
/// Maximum stdout capture size (10MB)
pub const MAX_STDOUT_SIZE: usize = 10 * 1024 * 1024;
/// Maximum stderr capture size (1MB)
pub const MAX_STDERR_SIZE: usize = 1024 * 1024;

// AI model defaults
/// Default max tokens for AI model responses
pub const DEFAULT_MAX_TOKENS: u32 = 4096;

// Retry defaults
/// Default maximum retry attempts for operations
pub const DEFAULT_MAX_RETRIES: u32 = 3;

// Timeout defaults (in seconds)
/// Default confirmation timeout
pub const DEFAULT_CONFIRMATION_TIMEOUT_SECS: u64 = 30;
/// Default connection timeout
pub const DEFAULT_CONNECTION_TIMEOUT_SECS: u64 = 30;

/// Configuration for the Agent engine
///
/// Renamed from CoworkConfig to reflect the agent-centric architecture.
/// Note: Core execution parameters (confirmation, parallelism, retries) are hardcoded
/// for security and stability. Only model routing settings are configurable.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Model profiles for multi-model routing
    pub model_profiles: Vec<ModelProfile>,

    /// Model routing rules (contains enable_pipelines flag)
    pub routing_rules: Option<ModelRoutingRules>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model_profiles: Vec::new(),
            routing_rules: None,
        }
    }
}

impl AgentConfig {
    /// Create config with model routing enabled
    pub fn with_model_routing(
        mut self,
        profiles: Vec<ModelProfile>,
        rules: ModelRoutingRules,
    ) -> Self {
        self.model_profiles = profiles;
        self.routing_rules = Some(rules);
        self
    }

    /// Check if pipelines are enabled
    pub fn pipelines_enabled(&self) -> bool {
        self.routing_rules
            .as_ref()
            .is_some_and(|r| r.enable_pipelines)
    }
}

/// Current execution state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionState {
    /// Not executing
    Idle,
    /// Planning a task
    Planning,
    /// Waiting for user confirmation
    AwaitingConfirmation,
    /// Executing tasks
    Executing,
    /// Execution paused
    Paused,
    /// Execution cancelled
    Cancelled,
    /// Execution completed
    Completed,
}

/// The main Agent engine
///
/// Provides a unified API for planning and executing task graphs.
/// Renamed from CoworkEngine to reflect the agent-centric architecture.
pub struct AgentEngine {
    /// Configuration (stored for model routing access, core parameters are hardcoded)
    #[allow(dead_code)]
    config: AgentConfig,
    planner: Arc<dyn TaskPlanner>,
    scheduler: RwLock<DagScheduler>,
    executors: ExecutorRegistry,
    monitor: Arc<ProgressMonitor>,
    state: RwLock<ExecutionState>,
    paused: AtomicBool,
    cancelled: AtomicBool,
    /// Model matcher for multi-model routing
    model_matcher: Option<Arc<ModelMatcher>>,
    /// AI provider for model execution
    provider: Option<Arc<dyn AiProvider>>,
}

impl AgentEngine {
    /// Create a new AgentEngine
    pub fn new(config: AgentConfig, provider: Arc<dyn AiProvider>) -> Self {
        let scheduler_config = SchedulerConfig {
            max_parallelism: MAX_PARALLELISM,
            max_task_retries: MAX_TASK_RETRIES,
        };

        let mut executors = ExecutorRegistry::new();
        // Register the NoopExecutor for testing
        executors.register("noop", Arc::new(NoopExecutor::new()));

        // Initialize model matcher if pipelines are enabled
        let model_matcher = if config.pipelines_enabled() && !config.model_profiles.is_empty() {
            let rules = config.routing_rules.clone().unwrap_or_default();
            Some(Arc::new(ModelMatcher::new(
                config.model_profiles.clone(),
                rules,
            )))
        } else {
            None
        };

        Self {
            config,
            planner: Arc::new(LlmTaskPlanner::new(provider.clone())),
            scheduler: RwLock::new(DagScheduler::with_config(scheduler_config)),
            executors,
            monitor: Arc::new(ProgressMonitor::new()),
            state: RwLock::new(ExecutionState::Idle),
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            model_matcher,
            provider: Some(provider),
        }
    }

    /// Create an AgentEngine with a custom planner
    pub fn with_planner(config: AgentConfig, planner: Arc<dyn TaskPlanner>) -> Self {
        let scheduler_config = SchedulerConfig {
            max_parallelism: MAX_PARALLELISM,
            max_task_retries: MAX_TASK_RETRIES,
        };

        let mut executors = ExecutorRegistry::new();
        executors.register("noop", Arc::new(NoopExecutor::new()));

        // Initialize model matcher if pipelines are enabled
        let model_matcher = if config.pipelines_enabled() && !config.model_profiles.is_empty() {
            let rules = config.routing_rules.clone().unwrap_or_default();
            Some(Arc::new(ModelMatcher::new(
                config.model_profiles.clone(),
                rules,
            )))
        } else {
            None
        };

        Self {
            config,
            planner,
            scheduler: RwLock::new(DagScheduler::with_config(scheduler_config)),
            executors,
            monitor: Arc::new(ProgressMonitor::new()),
            state: RwLock::new(ExecutionState::Idle),
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            model_matcher,
            provider: None,
        }
    }

    /// Create an AgentEngine with a custom planner and provider
    pub fn with_planner_and_provider(
        config: AgentConfig,
        planner: Arc<dyn TaskPlanner>,
        provider: Arc<dyn AiProvider>,
    ) -> Self {
        let scheduler_config = SchedulerConfig {
            max_parallelism: MAX_PARALLELISM,
            max_task_retries: MAX_TASK_RETRIES,
        };

        let mut executors = ExecutorRegistry::new();
        executors.register("noop", Arc::new(NoopExecutor::new()));

        // Initialize model matcher if pipelines are enabled
        let model_matcher = if config.pipelines_enabled() && !config.model_profiles.is_empty() {
            let rules = config.routing_rules.clone().unwrap_or_default();
            Some(Arc::new(ModelMatcher::new(
                config.model_profiles.clone(),
                rules,
            )))
        } else {
            None
        };

        Self {
            config,
            planner,
            scheduler: RwLock::new(DagScheduler::with_config(scheduler_config)),
            executors,
            monitor: Arc::new(ProgressMonitor::new()),
            state: RwLock::new(ExecutionState::Idle),
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            model_matcher,
            provider: Some(provider),
        }
    }

    /// Get the executor registry for registering custom executors
    pub fn executors_mut(&mut self) -> &mut ExecutorRegistry {
        &mut self.executors
    }

    /// Register the FileOpsExecutor with the given configuration
    ///
    /// This should be called after creating the engine to enable file operations.
    pub fn register_file_ops_executor(
        &mut self,
        config: crate::config::types::agent::FileOpsConfigToml,
    ) {
        if config.enabled {
            let executor = config.create_executor();
            self.executors.register("file_ops", Arc::new(executor));
            info!("Registered FileOpsExecutor");
        } else {
            debug!("FileOpsExecutor disabled by configuration");
        }
    }

    /// Register the CodeExecutor with the given configuration
    ///
    /// This should be called after creating the engine to enable code execution.
    /// Code execution is disabled by default for security reasons.
    pub fn register_code_executor(
        &mut self,
        config: crate::config::types::agent::CodeExecConfigToml,
        file_ops_config: &crate::config::types::agent::FileOpsConfigToml,
    ) {
        if config.enabled {
            // Create permission checker from file_ops config (shared permission model)
            let permission_checker = crate::dispatcher::executor::PathPermissionChecker::new(
                file_ops_config.allowed_paths.clone(),
                file_ops_config.denied_paths.clone(),
                file_ops_config.max_file_size,
            );
            let executor = config.create_executor(permission_checker);
            self.executors.register("code_exec", Arc::new(executor));
            info!("Registered CodeExecutor");
        } else {
            debug!("CodeExecutor disabled by configuration (default)");
        }
    }

    /// Subscribe to progress events
    pub fn subscribe(&self, subscriber: Arc<dyn ProgressSubscriber>) {
        self.monitor.subscribe(subscriber);
    }

    /// Get the current execution state
    pub async fn state(&self) -> ExecutionState {
        *self.state.read().await
    }

    /// Plan a task from a natural language request
    ///
    /// # Arguments
    ///
    /// * `request` - The user's natural language request
    ///
    /// # Returns
    ///
    /// * `Ok(TaskGraph)` - The planned task graph
    /// * `Err` - If planning fails
    pub async fn plan(&self, request: &str) -> Result<TaskGraph> {
        info!("Planning task: {}", request);
        *self.state.write().await = ExecutionState::Planning;

        let result = self.planner.plan(request).await;

        if result.is_err() {
            *self.state.write().await = ExecutionState::Idle;
        } else if REQUIRE_CONFIRMATION {
            *self.state.write().await = ExecutionState::AwaitingConfirmation;
        }

        result
    }

    /// Plan a task with available generation providers
    ///
    /// This method should be used when the user has configured image, video, or audio
    /// generation providers. The providers will be passed to the LLM planner so it can
    /// correctly route generation tasks to the appropriate providers.
    ///
    /// # Arguments
    ///
    /// * `request` - The user's natural language request
    /// * `providers` - Available generation providers (image, video, audio)
    ///
    /// # Returns
    ///
    /// * `Ok(TaskGraph)` - The planned task graph
    /// * `Err` - If planning fails
    pub async fn plan_with_providers(
        &self,
        request: &str,
        providers: &GenerationProviders,
    ) -> Result<TaskGraph> {
        info!("Planning task with providers: {}", request);
        *self.state.write().await = ExecutionState::Planning;

        let result = self.planner.plan_with_providers(request, providers).await;

        if result.is_err() {
            *self.state.write().await = ExecutionState::Idle;
        } else if REQUIRE_CONFIRMATION {
            *self.state.write().await = ExecutionState::AwaitingConfirmation;
        }

        result
    }

    /// Execute a task graph
    ///
    /// # Arguments
    ///
    /// * `graph` - The task graph to execute
    ///
    /// # Returns
    ///
    /// * `Ok(ExecutionSummary)` - Summary of the execution
    /// * `Err` - If execution fails
    pub async fn execute(&self, mut graph: TaskGraph) -> Result<ExecutionSummary> {
        info!(
            "Executing task graph: {} ({})",
            graph.metadata.title, graph.id
        );
        *self.state.write().await = ExecutionState::Executing;

        // Reset state
        self.paused.store(false, Ordering::SeqCst);
        self.cancelled.store(false, Ordering::SeqCst);
        self.scheduler.write().await.reset();

        let start_time = Instant::now();
        let ctx = ExecutionContext::new(&graph.id);

        // Main execution loop
        loop {
            // Check for cancellation
            if self.cancelled.load(Ordering::SeqCst) {
                warn!("Execution cancelled");
                *self.state.write().await = ExecutionState::Cancelled;
                break;
            }

            // Check for pause
            while self.paused.load(Ordering::SeqCst) {
                *self.state.write().await = ExecutionState::Paused;
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                if self.cancelled.load(Ordering::SeqCst) {
                    break;
                }
            }

            if self.cancelled.load(Ordering::SeqCst) {
                break;
            }

            *self.state.write().await = ExecutionState::Executing;

            // Get ready tasks
            let ready_tasks: Vec<String> = {
                let scheduler = self.scheduler.read().await;
                scheduler
                    .next_ready(&graph)
                    .iter()
                    .map(|t| t.id.clone())
                    .collect()
            };

            if ready_tasks.is_empty() {
                // Check if we're done
                let scheduler = self.scheduler.read().await;
                if scheduler.is_complete(&graph) {
                    break;
                }

                // Wait for running tasks to complete
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                continue;
            }

            // Execute ready tasks in parallel
            let mut handles = Vec::new();

            for task_id in ready_tasks {
                let task = match graph.get_task(&task_id) {
                    Some(t) => t.clone(),
                    None => continue,
                };

                // Mark as running
                {
                    let mut scheduler = self.scheduler.write().await;
                    scheduler.mark_running(&task_id);
                }

                if let Some(graph_task) = graph.get_task_mut(&task_id) {
                    graph_task.status = TaskStatus::running(0.0);
                }

                self.monitor.on_task_start(&task);

                // Spawn execution
                let executors = &self.executors;
                let ctx_clone = ctx.clone();

                let result = executors.execute(&task, &ctx_clone).await;

                handles.push((task_id.clone(), task.clone(), result));
            }

            // Process results
            for (task_id, task, result) in handles {
                match result {
                    Ok(task_result) => {
                        debug!("Task {} completed successfully", task_id);

                        {
                            let mut scheduler = self.scheduler.write().await;
                            scheduler.mark_completed(&task_id);
                        }

                        if let Some(graph_task) = graph.get_task_mut(&task_id) {
                            graph_task.status = TaskStatus::completed(task_result.clone());
                        }

                        self.monitor.on_task_complete(&task, &task_result);
                    }
                    Err(e) => {
                        error!("Task {} failed: {}", task_id, e);

                        let error_msg = e.to_string();
                        {
                            let mut scheduler = self.scheduler.write().await;
                            scheduler.mark_failed(&task_id, &error_msg);
                        }

                        if let Some(graph_task) = graph.get_task_mut(&task_id) {
                            graph_task.status = TaskStatus::failed(&error_msg);
                        }

                        self.monitor.on_task_failed(&task, &error_msg);
                    }
                }
            }

            // Update graph progress
            self.monitor.update_graph_progress(&graph);
        }

        // Build summary
        let counts = graph.count_by_status();
        let summary = ExecutionSummary {
            graph_id: graph.id.clone(),
            total_tasks: counts.total(),
            completed_tasks: counts.completed,
            failed_tasks: counts.failed,
            cancelled_tasks: counts.cancelled,
            total_duration: start_time.elapsed(),
            artifacts: Vec::new(), // TODO: collect artifacts
            errors: graph
                .tasks
                .iter()
                .filter_map(|t| {
                    if let TaskStatus::Failed { error, .. } = &t.status {
                        Some(format!("{}: {}", t.name, error))
                    } else {
                        None
                    }
                })
                .collect(),
        };

        *self.state.write().await = ExecutionState::Completed;
        self.monitor.on_graph_complete(&graph);

        info!(
            "Execution completed: {} tasks, {} completed, {} failed",
            summary.total_tasks, summary.completed_tasks, summary.failed_tasks
        );

        Ok(summary)
    }

    /// Pause execution
    ///
    /// Running tasks will complete, but no new tasks will start.
    pub fn pause(&self) {
        info!("Pausing execution");
        self.paused.store(true, Ordering::SeqCst);
    }

    /// Resume execution after pause
    pub fn resume(&self) {
        info!("Resuming execution");
        self.paused.store(false, Ordering::SeqCst);
    }

    /// Cancel execution
    ///
    /// Running tasks will complete, but no new tasks will start
    /// and the execution will terminate.
    pub fn cancel(&self) {
        info!("Cancelling execution");
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Check if execution is paused
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Check if execution is cancelled
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    // =========================================================================
    // Model Routing Methods
    // =========================================================================

    /// Get the model matcher (if available)
    pub fn model_matcher(&self) -> Option<&Arc<ModelMatcher>> {
        self.model_matcher.as_ref()
    }

    /// Check if model routing is enabled
    pub fn has_model_routing(&self) -> bool {
        self.model_matcher.is_some()
    }

    /// Set the fallback provider for model routing
    ///
    /// This should be called with the system's `default_provider` from config.
    /// When no suitable model is found through normal routing, the fallback
    /// provider will be used.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Set fallback from default_provider config
    /// if let Some(default_provider) = config.general.default_provider.as_ref() {
    ///     engine.set_fallback_provider(default_provider);
    /// }
    /// ```
    pub fn set_fallback_provider(&mut self, provider: &str) {
        if let Some(ref mut matcher) = self.model_matcher {
            // We need to get a mutable reference, so we clone and replace
            let mut new_matcher = (**matcher).clone();
            new_matcher
                .set_fallback_provider(Some(super::model_router::FallbackProvider::new(provider)));
            *matcher = Arc::new(new_matcher);
        } else {
            // If model routing is not enabled, create a minimal matcher with just fallback
            let rules = super::model_router::ModelRoutingRules::default();
            let matcher = ModelMatcher::new(vec![], rules).with_fallback_provider(provider);
            self.model_matcher = Some(Arc::new(matcher));
        }
    }

    /// Set the fallback provider with a specific model
    pub fn set_fallback_provider_with_model(&mut self, provider: &str, model: &str) {
        if let Some(ref mut matcher) = self.model_matcher {
            let mut new_matcher = (**matcher).clone();
            new_matcher.set_fallback_provider(Some(
                super::model_router::FallbackProvider::new(provider).with_model(model),
            ));
            *matcher = Arc::new(new_matcher);
        } else {
            let rules = super::model_router::ModelRoutingRules::default();
            let matcher =
                ModelMatcher::new(vec![], rules).with_fallback_provider_and_model(provider, model);
            self.model_matcher = Some(Arc::new(matcher));
        }
    }

    /// Check if a fallback provider is configured
    pub fn has_fallback_provider(&self) -> bool {
        self.model_matcher
            .as_ref()
            .map(|m| m.has_fallback())
            .unwrap_or(false)
    }

    /// Route a task to the optimal model
    ///
    /// Returns the selected ModelProfile, or an error if routing fails.
    pub fn route_task(&self, task: &Task) -> Result<ModelProfile> {
        let matcher = self
            .model_matcher
            .as_ref()
            .ok_or_else(|| AetherError::config("Model routing is not enabled"))?;

        matcher
            .route(task)
            .map_err(|e| AetherError::config(e.to_string()))
    }

    /// Route by TaskIntent for unified routing integration
    ///
    /// This method bridges the legacy routing rules to the Model Router by:
    /// 1. Converting `intent_type` string to `TaskIntent` enum
    /// 2. Optionally applying `preferred_model` override
    /// 3. Returning the optimal `ModelProfile` for the intent
    ///
    /// # Arguments
    ///
    /// * `intent` - The TaskIntent to route
    /// * `preferred_model` - Optional model ID to override automatic selection
    ///
    /// # Returns
    ///
    /// * `Ok(ModelProfile)` - The selected model profile
    /// * `Err` - If routing fails or model routing is disabled
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use aethecore::dispatcher::model_router::TaskIntent;
    ///
    /// // Route based on matched routing rule
    /// let rule = config.find_matching_rule(input)?;
    /// let intent = rule.get_task_intent();
    /// let preferred = rule.get_preferred_model();
    /// let profile = engine.route_by_intent(&intent, preferred)?;
    /// ```
    pub fn route_by_intent(
        &self,
        intent: &super::model_router::TaskIntent,
        preferred_model: Option<&str>,
    ) -> Result<ModelProfile> {
        let matcher = self
            .model_matcher
            .as_ref()
            .ok_or_else(|| AetherError::config("Model routing is not enabled"))?;

        matcher
            .route_by_intent_with_preference(intent, preferred_model)
            .ok_or_else(|| AetherError::config("No model available for intent"))
    }

    /// Route from a RoutingRuleConfig
    ///
    /// Convenience method that extracts TaskIntent and preferred_model from a rule
    /// and routes to the optimal model.
    ///
    /// # Arguments
    ///
    /// * `rule` - The matched routing rule
    ///
    /// # Returns
    ///
    /// * `Ok(ModelProfile)` - The selected model profile
    /// * `Err` - If routing fails
    pub fn route_from_rule(&self, rule: &crate::config::RoutingRuleConfig) -> Result<ModelProfile> {
        let intent = rule.get_task_intent();
        let preferred = rule.get_preferred_model();
        self.route_by_intent(&intent, preferred)
    }

    /// Execute a task graph with model routing
    ///
    /// This method routes AI tasks to optimal models based on task characteristics
    /// and executes them using the appropriate provider.
    ///
    /// # Arguments
    ///
    /// * `graph` - The task graph to execute
    ///
    /// # Returns
    ///
    /// * `Ok(ExecutionSummary)` - Summary with model routing information
    /// * `Err` - If execution fails
    pub async fn execute_with_routing(&self, mut graph: TaskGraph) -> Result<ExecutionSummary> {
        // If model routing is not enabled, fall back to regular execution
        if !self.has_model_routing() {
            info!("Model routing not enabled, using standard execution");
            return self.execute(graph).await;
        }

        info!(
            "Executing task graph with model routing: {} ({})",
            graph.metadata.title, graph.id
        );
        *self.state.write().await = ExecutionState::Executing;

        // Reset state
        self.paused.store(false, Ordering::SeqCst);
        self.cancelled.store(false, Ordering::SeqCst);
        self.scheduler.write().await.reset();

        let start_time = Instant::now();
        let ctx = ExecutionContext::new(&graph.id);

        // Create context manager for tracking results
        let context_manager = TaskContextManager::new(&graph.id);

        // Main execution loop
        loop {
            // Check for cancellation
            if self.cancelled.load(Ordering::SeqCst) {
                warn!("Execution cancelled");
                *self.state.write().await = ExecutionState::Cancelled;
                break;
            }

            // Check for pause
            while self.paused.load(Ordering::SeqCst) {
                *self.state.write().await = ExecutionState::Paused;
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                if self.cancelled.load(Ordering::SeqCst) {
                    break;
                }
            }

            if self.cancelled.load(Ordering::SeqCst) {
                break;
            }

            *self.state.write().await = ExecutionState::Executing;

            // Get ready tasks
            let ready_tasks: Vec<String> = {
                let scheduler = self.scheduler.read().await;
                scheduler
                    .next_ready(&graph)
                    .iter()
                    .map(|t| t.id.clone())
                    .collect()
            };

            if ready_tasks.is_empty() {
                // Check if we're done
                let scheduler = self.scheduler.read().await;
                if scheduler.is_complete(&graph) {
                    break;
                }

                // Wait for running tasks to complete
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                continue;
            }

            // Execute ready tasks
            for task_id in ready_tasks {
                let task = match graph.get_task(&task_id) {
                    Some(t) => t.clone(),
                    None => continue,
                };

                // Mark as running
                {
                    let mut scheduler = self.scheduler.write().await;
                    scheduler.mark_running(&task_id);
                }

                if let Some(graph_task) = graph.get_task_mut(&task_id) {
                    graph_task.status = TaskStatus::running(0.0);
                }

                self.monitor.on_task_start(&task);

                // Route AI tasks to optimal model
                let (result, model_used) = if matches!(task.task_type, TaskType::AiInference(_)) {
                    self.execute_ai_task_with_routing(&task, &ctx, &context_manager)
                        .await
                } else {
                    // Non-AI tasks use standard execution
                    let result = self.executors.execute(&task, &ctx).await;
                    (result, None)
                };

                match result {
                    Ok(task_result) => {
                        debug!(
                            "Task {} completed successfully{}",
                            task_id,
                            model_used
                                .as_ref()
                                .map(|m| format!(" (model: {})", m))
                                .unwrap_or_default()
                        );

                        // Store result in context manager
                        let _ = context_manager
                            .store_result(
                                &task,
                                task_result.clone(),
                                model_used.as_deref(),
                                None,
                                0,
                            )
                            .await;

                        {
                            let mut scheduler = self.scheduler.write().await;
                            scheduler.mark_completed(&task_id);
                        }

                        if let Some(graph_task) = graph.get_task_mut(&task_id) {
                            graph_task.status = TaskStatus::completed(task_result.clone());
                        }

                        self.monitor.on_task_complete(&task, &task_result);
                    }
                    Err(e) => {
                        error!("Task {} failed: {}", task_id, e);

                        let error_msg = e.to_string();
                        {
                            let mut scheduler = self.scheduler.write().await;
                            scheduler.mark_failed(&task_id, &error_msg);
                        }

                        if let Some(graph_task) = graph.get_task_mut(&task_id) {
                            graph_task.status = TaskStatus::failed(&error_msg);
                        }

                        self.monitor.on_task_failed(&task, &error_msg);
                    }
                }
            }

            // Update graph progress
            self.monitor.update_graph_progress(&graph);
        }

        // Build summary
        let counts = graph.count_by_status();
        let context_summary = context_manager.summary().await;

        let summary = ExecutionSummary {
            graph_id: graph.id.clone(),
            total_tasks: counts.total(),
            completed_tasks: counts.completed,
            failed_tasks: counts.failed,
            cancelled_tasks: counts.cancelled,
            total_duration: start_time.elapsed(),
            artifacts: Vec::new(),
            errors: graph
                .tasks
                .iter()
                .filter_map(|t| {
                    if let TaskStatus::Failed { error, .. } = &t.status {
                        Some(format!("{}: {}", t.name, error))
                    } else {
                        None
                    }
                })
                .collect(),
        };

        *self.state.write().await = ExecutionState::Completed;
        self.monitor.on_graph_complete(&graph);

        info!(
            "Execution with routing completed: {} tasks, {} completed, {} failed, {} tokens",
            summary.total_tasks,
            summary.completed_tasks,
            summary.failed_tasks,
            context_summary.total_tokens
        );

        Ok(summary)
    }

    /// Execute an AI task with model routing
    async fn execute_ai_task_with_routing(
        &self,
        task: &Task,
        _ctx: &ExecutionContext,
        _context_manager: &TaskContextManager,
    ) -> (Result<TaskResult>, Option<String>) {
        // Route task to optimal model
        let profile = match self.route_task(task) {
            Ok(p) => p,
            Err(e) => {
                return (Err(e), None);
            }
        };

        let model_id = profile.id.clone();
        debug!(
            "Routed task '{}' to model '{}' (provider: {})",
            task.name, model_id, profile.provider
        );

        // Execute with provider (if available)
        if let Some(provider) = &self.provider {
            // Extract prompt from AI task
            let prompt = if let TaskType::AiInference(ai_task) = &task.task_type {
                ai_task.prompt.clone()
            } else {
                return (
                    Err(AetherError::config("Task is not an AI inference task")),
                    Some(model_id),
                );
            };

            // Execute with provider
            match provider.process(&prompt, None).await {
                Ok(response) => {
                    let result = TaskResult {
                        output: serde_json::json!({
                            "response": response,
                            "model_used": model_id,
                            "provider": profile.provider,
                        }),
                        artifacts: Vec::new(),
                        duration: std::time::Duration::ZERO, // TODO: track actual duration
                        summary: Some(format!("Completed with model {}", model_id)),
                    };
                    (Ok(result), Some(model_id))
                }
                Err(e) => (Err(e), Some(model_id)),
            }
        } else {
            // No provider available, return placeholder result
            let result = TaskResult {
                output: serde_json::json!({
                    "model_routed": model_id,
                    "provider": profile.provider,
                    "note": "No provider available for execution",
                }),
                artifacts: Vec::new(),
                duration: std::time::Duration::ZERO,
                summary: Some(format!("Routed to model {} (not executed)", model_id)),
            };
            (Ok(result), Some(model_id))
        }
    }

    /// Get model profiles
    pub fn model_profiles(&self) -> Vec<ModelProfile> {
        self.model_matcher
            .as_ref()
            .map(|m| m.profiles().to_vec())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::agent_types::{FileOp, Task, TaskType};
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    fn create_test_graph() -> TaskGraph {
        let mut graph = TaskGraph::new("test_graph", "Test Graph");

        graph.add_task(Task::new(
            "task_1",
            "Task 1",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        ));

        graph.add_task(Task::new(
            "task_2",
            "Task 2",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        ));

        graph.add_dependency("task_1", "task_2");

        graph
    }

    #[tokio::test]
    async fn test_engine_execute() {
        // Create a mock provider (we won't use it for execution)
        let config = AgentConfig::default();

        // Create engine with mock planner
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        let graph = create_test_graph();
        let summary = engine.execute(graph).await.unwrap();

        assert_eq!(summary.total_tasks, 2);
        assert_eq!(summary.completed_tasks, 2);
        assert_eq!(summary.failed_tasks, 0);
    }

    #[tokio::test]
    async fn test_engine_progress_events() {
        let config = AgentConfig::default();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        let event_count = Arc::new(AtomicUsize::new(0));
        let count_clone = event_count.clone();

        engine.subscribe(Arc::new(
            crate::dispatcher::monitor::CallbackSubscriber::new(move |_| {
                count_clone.fetch_add(1, Ordering::SeqCst);
            }),
        ));

        let graph = create_test_graph();
        engine.execute(graph).await.unwrap();

        // Should have received multiple events (start, complete for each task, graph complete, progress updates)
        assert!(event_count.load(Ordering::SeqCst) >= 4);
    }

    #[tokio::test]
    async fn test_engine_cancel() {
        let config = AgentConfig::default();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        // Test cancel mechanism
        assert!(!engine.is_cancelled());
        engine.cancel();
        assert!(engine.is_cancelled());

        // Execute with pre-cancelled state (state gets reset in execute)
        let graph = create_test_graph();
        let summary = engine.execute(graph).await.unwrap();

        // execute() resets state, so tasks complete normally
        // This verifies the engine can be reused after cancel
        assert_eq!(summary.total_tasks, 2);
    }

    #[tokio::test]
    async fn test_engine_pause_resume() {
        let config = AgentConfig::default();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        // Test pause/resume mechanism
        assert!(!engine.is_paused());
        engine.pause();
        assert!(engine.is_paused());
        engine.resume();
        assert!(!engine.is_paused());
    }

    // Mock planner for testing
    struct MockPlanner;

    #[async_trait::async_trait]
    impl TaskPlanner for MockPlanner {
        async fn plan(&self, _request: &str) -> Result<TaskGraph> {
            Ok(create_test_graph())
        }

        fn name(&self) -> &str {
            "MockPlanner"
        }
    }

    // =========================================================================
    // Model Routing Tests
    // =========================================================================

    use crate::dispatcher::agent_types::AiTask;
    use crate::dispatcher::model_router::{Capability, CostTier, LatencyTier};

    fn create_test_profiles() -> Vec<ModelProfile> {
        vec![
            ModelProfile::new("claude-opus", "anthropic", "claude-opus-4")
                .with_capabilities(vec![Capability::Reasoning, Capability::CodeGeneration])
                .with_cost_tier(CostTier::High)
                .with_latency_tier(LatencyTier::Slow),
            ModelProfile::new("claude-sonnet", "anthropic", "claude-sonnet-4")
                .with_capabilities(vec![Capability::TextAnalysis, Capability::CodeGeneration])
                .with_cost_tier(CostTier::Medium)
                .with_latency_tier(LatencyTier::Medium),
            ModelProfile::new("claude-haiku", "anthropic", "claude-haiku-3")
                .with_capabilities(vec![Capability::TextAnalysis])
                .with_cost_tier(CostTier::Low)
                .with_latency_tier(LatencyTier::Fast),
        ]
    }

    fn create_routing_config() -> AgentConfig {
        let profiles = create_test_profiles();
        let rules = ModelRoutingRules::new("claude-sonnet")
            .with_task_type("code_generation", "claude-opus")
            .with_task_type("quick_tasks", "claude-haiku");

        AgentConfig::default().with_model_routing(profiles, rules)
    }

    #[test]
    fn test_engine_model_routing_enabled() {
        let config = create_routing_config();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        assert!(engine.has_model_routing());
        assert!(engine.model_matcher().is_some());
    }

    #[test]
    fn test_engine_model_routing_disabled() {
        let config = AgentConfig::default();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        assert!(!engine.has_model_routing());
        assert!(engine.model_matcher().is_none());
    }

    #[test]
    fn test_engine_route_task() {
        let config = create_routing_config();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        let task = Task::new(
            "t1",
            "Test Task",
            TaskType::AiInference(AiTask {
                prompt: "Test prompt".to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        );

        let profile = engine.route_task(&task).unwrap();
        // Should route to default model (claude-sonnet)
        assert_eq!(profile.id, "claude-sonnet");
    }

    #[test]
    fn test_engine_model_profiles() {
        let config = create_routing_config();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        let profiles = engine.model_profiles();
        assert_eq!(profiles.len(), 3);
    }

    #[tokio::test]
    async fn test_engine_execute_with_routing_fallback() {
        // When model routing is disabled, execute_with_routing should fall back to execute
        let config = AgentConfig::default();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        let graph = create_test_graph();
        let summary = engine.execute_with_routing(graph).await.unwrap();

        assert_eq!(summary.total_tasks, 2);
        assert_eq!(summary.completed_tasks, 2);
    }

    #[test]
    fn test_engine_route_by_intent() {
        use crate::dispatcher::model_router::TaskIntent;

        let config = create_routing_config();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        // Test routing by intent
        let profile = engine
            .route_by_intent(&TaskIntent::CodeGeneration, None)
            .unwrap();
        assert_eq!(profile.id, "claude-opus");

        // Test routing with preferred model override
        let profile = engine
            .route_by_intent(&TaskIntent::CodeGeneration, Some("claude-haiku"))
            .unwrap();
        assert_eq!(profile.id, "claude-haiku");
    }

    #[test]
    fn test_engine_route_from_rule() {
        use crate::config::RoutingRuleConfig;

        let config = create_routing_config();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        // Test routing from a rule with intent_type
        let rule = RoutingRuleConfig::command("^/code", "anthropic", None)
            .with_intent_type("code_generation");
        let profile = engine.route_from_rule(&rule).unwrap();
        assert_eq!(profile.id, "claude-opus");

        // Test routing from a rule with preferred_model
        let rule = RoutingRuleConfig::command("^/quick", "anthropic", None)
            .with_intent_type("code_generation")
            .with_preferred_model("claude-haiku");
        let profile = engine.route_from_rule(&rule).unwrap();
        assert_eq!(profile.id, "claude-haiku");
    }

    #[test]
    fn test_engine_route_by_intent_disabled() {
        use crate::dispatcher::model_router::TaskIntent;

        // When model routing is disabled, should return error
        let config = AgentConfig::default();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        let result = engine.route_by_intent(&TaskIntent::CodeGeneration, None);
        assert!(result.is_err());
    }
}
