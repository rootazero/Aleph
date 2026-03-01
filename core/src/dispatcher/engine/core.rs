//! Core AgentEngine implementation

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::config::{AgentConfig, ExecutionState};
use super::constants::{MAX_PARALLELISM, MAX_TASK_RETRIES, REQUIRE_CONFIRMATION};
use crate::dispatcher::agent_types::{ExecutionSummary, TaskGraph, TaskStatus};
use crate::dispatcher::executor::{ExecutionContext, ExecutorRegistry, NoopExecutor};
use crate::dispatcher::model_router::{ModelMatcher, ModelProfile, ModelRouter};
use crate::dispatcher::monitor::{ProgressMonitor, ProgressSubscriber, TaskMonitor};
use crate::dispatcher::planner::{GenerationProviders, LlmTaskPlanner, TaskPlanner};
use crate::dispatcher::scheduler::{DagScheduler, SchedulerConfig, TaskScheduler};
use crate::error::Result;
use crate::providers::AiProvider;

/// The main Agent engine
///
/// Provides a unified API for planning and executing task graphs.
/// Renamed from CoworkEngine to reflect the agent-centric architecture.
pub struct AgentEngine {
    /// Configuration (stored for future model routing access, core parameters are hardcoded)
    pub(crate) _config: AgentConfig,
    pub(crate) planner: Arc<dyn TaskPlanner>,
    pub(crate) scheduler: RwLock<DagScheduler>,
    pub(crate) executors: ExecutorRegistry,
    pub(crate) monitor: Arc<ProgressMonitor>,
    pub(crate) state: RwLock<ExecutionState>,
    pub(crate) paused: AtomicBool,
    pub(crate) cancelled: AtomicBool,
    /// Model matcher for multi-model routing
    pub(crate) model_matcher: Option<Arc<ModelMatcher>>,
    /// AI provider for model execution
    pub(crate) provider: Option<Arc<dyn AiProvider>>,
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
            _config: config,
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
            _config: config,
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
            _config: config,
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

        // Mark remaining tasks as Cancelled if execution was cancelled
        if self.cancelled.load(Ordering::SeqCst) {
            for task in &mut graph.tasks {
                if task.is_pending() || task.is_running() {
                    task.status = TaskStatus::Cancelled;
                }
            }
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

        // Don't overwrite Cancelled state with Completed
        if !self.cancelled.load(Ordering::SeqCst) {
            *self.state.write().await = ExecutionState::Completed;
        }
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

    /// Get model profiles
    pub fn model_profiles(&self) -> Vec<ModelProfile> {
        self.model_matcher
            .as_ref()
            .map(|m| m.profiles().to_vec())
            .unwrap_or_default()
    }
}
