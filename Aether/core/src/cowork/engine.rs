//! CoworkEngine - unified API for task orchestration

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::executor::{ExecutionContext, ExecutorRegistry, NoopExecutor};
use super::monitor::{ProgressMonitor, ProgressSubscriber, TaskMonitor};
use super::planner::{LlmTaskPlanner, TaskPlanner};
use super::scheduler::{DagScheduler, SchedulerConfig, TaskScheduler};
use super::types::{ExecutionSummary, TaskGraph, TaskStatus};
use crate::error::{AetherError, Result};
use crate::providers::AiProvider;

/// Configuration for the Cowork engine
#[derive(Debug, Clone)]
pub struct CoworkConfig {
    /// Whether Cowork is enabled
    pub enabled: bool,

    /// Whether to require user confirmation before execution
    pub require_confirmation: bool,

    /// Maximum number of tasks to run in parallel
    pub max_parallelism: usize,

    /// Whether to run in dry-run mode (no actual execution)
    pub dry_run: bool,
}

impl Default for CoworkConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            require_confirmation: true,
            max_parallelism: 4,
            dry_run: false,
        }
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

/// The main Cowork engine
///
/// Provides a unified API for planning and executing task graphs.
pub struct CoworkEngine {
    config: CoworkConfig,
    planner: Arc<dyn TaskPlanner>,
    scheduler: RwLock<DagScheduler>,
    executors: ExecutorRegistry,
    monitor: Arc<ProgressMonitor>,
    state: RwLock<ExecutionState>,
    paused: AtomicBool,
    cancelled: AtomicBool,
}

impl CoworkEngine {
    /// Create a new CoworkEngine
    pub fn new(config: CoworkConfig, provider: Arc<dyn AiProvider>) -> Self {
        let scheduler_config = SchedulerConfig {
            max_parallelism: config.max_parallelism,
        };

        let mut executors = ExecutorRegistry::new();
        // Register the NoopExecutor for testing
        executors.register("noop", Arc::new(NoopExecutor::new()));

        Self {
            config,
            planner: Arc::new(LlmTaskPlanner::new(provider)),
            scheduler: RwLock::new(DagScheduler::with_config(scheduler_config)),
            executors,
            monitor: Arc::new(ProgressMonitor::new()),
            state: RwLock::new(ExecutionState::Idle),
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
        }
    }

    /// Create a CoworkEngine with a custom planner
    pub fn with_planner(
        config: CoworkConfig,
        planner: Arc<dyn TaskPlanner>,
    ) -> Self {
        let scheduler_config = SchedulerConfig {
            max_parallelism: config.max_parallelism,
        };

        let mut executors = ExecutorRegistry::new();
        executors.register("noop", Arc::new(NoopExecutor::new()));

        Self {
            config,
            planner,
            scheduler: RwLock::new(DagScheduler::with_config(scheduler_config)),
            executors,
            monitor: Arc::new(ProgressMonitor::new()),
            state: RwLock::new(ExecutionState::Idle),
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
        }
    }

    /// Get the executor registry for registering custom executors
    pub fn executors_mut(&mut self) -> &mut ExecutorRegistry {
        &mut self.executors
    }

    /// Register the FileOpsExecutor with the given configuration
    ///
    /// This should be called after creating the engine to enable file operations.
    pub fn register_file_ops_executor(&mut self, config: crate::config::types::cowork::FileOpsConfigToml) {
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
        config: crate::config::types::cowork::CodeExecConfigToml,
        file_ops_config: &crate::config::types::cowork::FileOpsConfigToml,
    ) {
        if config.enabled {
            // Create permission checker from file_ops config (shared permission model)
            let permission_checker = crate::cowork::executor::PathPermissionChecker::new(
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
        if !self.config.enabled {
            return Err(AetherError::config("Cowork is disabled"));
        }

        info!("Planning task: {}", request);
        *self.state.write().await = ExecutionState::Planning;

        let result = self.planner.plan(request).await;

        if result.is_err() {
            *self.state.write().await = ExecutionState::Idle;
        } else if self.config.require_confirmation {
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
        if !self.config.enabled {
            return Err(AetherError::config("Cowork is disabled"));
        }

        info!("Executing task graph: {} ({})", graph.metadata.title, graph.id);
        *self.state.write().await = ExecutionState::Executing;

        // Reset state
        self.paused.store(false, Ordering::SeqCst);
        self.cancelled.store(false, Ordering::SeqCst);
        self.scheduler.write().await.reset();

        let start_time = Instant::now();
        let ctx = ExecutionContext::new(&graph.id).with_dry_run(self.config.dry_run);

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cowork::types::{FileOp, Task, TaskType};
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
        let config = CoworkConfig {
            enabled: true,
            require_confirmation: false,
            max_parallelism: 2,
            dry_run: false,
        };

        // Create engine with mock planner
        let engine = CoworkEngine::with_planner(
            config,
            Arc::new(MockPlanner),
        );

        let graph = create_test_graph();
        let summary = engine.execute(graph).await.unwrap();

        assert_eq!(summary.total_tasks, 2);
        assert_eq!(summary.completed_tasks, 2);
        assert_eq!(summary.failed_tasks, 0);
    }

    #[tokio::test]
    async fn test_engine_progress_events() {
        let config = CoworkConfig::default();
        let engine = CoworkEngine::with_planner(config, Arc::new(MockPlanner));

        let event_count = Arc::new(AtomicUsize::new(0));
        let count_clone = event_count.clone();

        engine.subscribe(Arc::new(
            crate::cowork::monitor::CallbackSubscriber::new(move |_| {
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
        let config = CoworkConfig::default();
        let engine = CoworkEngine::with_planner(config, Arc::new(MockPlanner));

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
        let config = CoworkConfig::default();
        let engine = CoworkEngine::with_planner(config, Arc::new(MockPlanner));

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
}
