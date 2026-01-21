//! DAG-based task scheduler

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::join_all;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::{SchedulerConfig, TaskScheduler};
use crate::dispatcher::callback::{DagTaskPlan, ExecutionCallback, UserDecision};
use crate::dispatcher::context::{TaskContext, TaskOutput};
use crate::dispatcher::agent_types::{Task, TaskGraph, TaskResult, TaskStatus};
use crate::dispatcher::risk::RiskEvaluator;
use crate::error::Result;

// ============================================================================
// ExecutionResult - Result of graph execution
// ============================================================================

/// Result of graph execution
#[derive(Debug)]
pub struct ExecutionResult {
    /// Graph identifier
    pub graph_id: String,
    /// List of successfully completed task IDs
    pub completed_tasks: Vec<String>,
    /// List of failed task IDs
    pub failed_tasks: Vec<String>,
    /// Whether execution was cancelled by user
    pub cancelled: bool,
    /// Task context containing full outputs (for accessing complete results)
    pub context: Option<TaskContext>,
}

impl ExecutionResult {
    /// Create a new execution result
    pub fn new(graph_id: impl Into<String>) -> Self {
        Self {
            graph_id: graph_id.into(),
            completed_tasks: Vec::new(),
            failed_tasks: Vec::new(),
            cancelled: false,
            context: None,
        }
    }

    /// Check if execution was successful (no failures, not cancelled)
    pub fn is_success(&self) -> bool {
        !self.cancelled && self.failed_tasks.is_empty()
    }

    /// Get total executed tasks count
    pub fn total_executed(&self) -> usize {
        self.completed_tasks.len() + self.failed_tasks.len()
    }

    /// Get detailed summary including task results
    ///
    /// Returns a formatted string with each task's name and full output.
    /// This is useful for displaying the complete results to the user.
    pub fn detailed_summary(&self) -> String {
        let mut parts = Vec::new();

        if let Some(ctx) = &self.context {
            // Build task results using completed_tasks order and variables for full content
            // This approach works even if history has been trimmed
            for task_id in &self.completed_tasks {
                if let Some(output) = ctx.variables().get(task_id) {
                    let full_content = match &output.value {
                        serde_json::Value::String(s) => s.clone(),
                        v => v.to_string(),
                    };
                    // Use summary as title if available, otherwise use task_id
                    let title = output.summary.as_deref().unwrap_or(task_id);
                    // Truncate title to first line or 50 chars for readability (UTF-8 safe)
                    let title_short = title.lines().next().unwrap_or(title);
                    let title_short = if title_short.chars().count() > 50 {
                        let truncated: String = title_short.chars().take(47).collect();
                        format!("{}...", truncated)
                    } else {
                        title_short.to_string()
                    };
                    parts.push(format!("### {}\n\n{}", title_short, full_content));
                }
            }
        }

        if parts.is_empty() {
            format!(
                "完成 {} 个任务，失败 {} 个",
                self.completed_tasks.len(),
                self.failed_tasks.len()
            )
        } else {
            parts.join("\n\n---\n\n")
        }
    }
}

// ============================================================================
// GraphTaskExecutor - Task executor trait
// ============================================================================

/// Task executor trait for graph execution
///
/// Implementors provide the actual task execution logic.
/// The executor receives the task and context, and returns the output.
#[async_trait]
pub trait GraphTaskExecutor: Send + Sync {
    /// Execute a single task
    ///
    /// # Arguments
    /// * `task` - The task to execute
    /// * `context` - Prompt context built from dependencies
    ///
    /// # Returns
    /// TaskOutput on success, or error on failure
    async fn execute(&self, task: &Task, context: &str) -> Result<TaskOutput>;
}

/// DAG-based task scheduler
///
/// Schedules tasks based on dependency graph, executing independent
/// tasks in parallel up to a configured limit.
pub struct DagScheduler {
    config: SchedulerConfig,
    /// Tasks that have been marked as completed
    completed: HashSet<String>,
    /// Tasks that have been marked as failed
    failed: HashSet<String>,
    /// Tasks currently being executed
    running: HashSet<String>,
}

impl DagScheduler {
    /// Create a new DAG scheduler with default configuration
    pub fn new() -> Self {
        Self::with_config(SchedulerConfig::default())
    }

    /// Create a new DAG scheduler with custom configuration
    pub fn with_config(config: SchedulerConfig) -> Self {
        Self {
            config,
            completed: HashSet::new(),
            failed: HashSet::new(),
            running: HashSet::new(),
        }
    }

    /// Check if all dependencies of a task are satisfied
    fn dependencies_satisfied(&self, task: &Task, graph: &TaskGraph) -> bool {
        let predecessors = graph.get_predecessors(&task.id);

        for pred_id in predecessors {
            // Check if predecessor is completed
            if !self.completed.contains(pred_id) {
                // Also check the actual task status in case we missed an update
                if let Some(pred_task) = graph.get_task(pred_id) {
                    if !pred_task.is_completed() {
                        return false;
                    }
                } else {
                    return false;
                }
            }
        }

        true
    }

    /// Check if a task should be blocked due to failed dependencies
    fn has_failed_dependency(&self, task: &Task, graph: &TaskGraph) -> bool {
        let predecessors = graph.get_predecessors(&task.id);

        for pred_id in predecessors {
            if self.failed.contains(pred_id) {
                return true;
            }
            if let Some(pred_task) = graph.get_task(pred_id) {
                if pred_task.is_failed() {
                    return true;
                }
            }
        }

        false
    }

    /// Mark a task as currently running
    pub fn mark_running(&mut self, task_id: &str) {
        self.running.insert(task_id.to_string());
        debug!("Task '{}' marked as running", task_id);
    }

    /// Get the number of currently running tasks
    pub fn running_count(&self) -> usize {
        self.running.len()
    }

    /// Get available parallelism slots
    pub fn available_slots(&self) -> usize {
        self.config
            .max_parallelism
            .saturating_sub(self.running.len())
    }
}

impl Default for DagScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskScheduler for DagScheduler {
    fn next_ready<'a>(&self, graph: &'a TaskGraph) -> Vec<&'a Task> {
        let available = self.available_slots();

        if available == 0 {
            return Vec::new();
        }

        let ready: Vec<&Task> = graph
            .tasks
            .iter()
            // Only pending tasks
            .filter(|t| t.is_pending())
            // Not already running
            .filter(|t| !self.running.contains(&t.id))
            // Not blocked by failed dependency
            .filter(|t| !self.has_failed_dependency(t, graph))
            // All dependencies satisfied
            .filter(|t| self.dependencies_satisfied(t, graph))
            // Take up to available slots
            .take(available)
            .collect();

        if !ready.is_empty() {
            debug!(
                "Scheduler found {} ready tasks: {:?}",
                ready.len(),
                ready.iter().map(|t| &t.id).collect::<Vec<_>>()
            );
        }

        ready
    }

    fn mark_completed(&mut self, task_id: &str) {
        self.running.remove(task_id);
        self.completed.insert(task_id.to_string());
        info!("Task '{}' completed", task_id);
    }

    fn mark_failed(&mut self, task_id: &str, error: &str) {
        self.running.remove(task_id);
        self.failed.insert(task_id.to_string());
        info!("Task '{}' failed: {}", task_id, error);
    }

    fn is_complete(&self, graph: &TaskGraph) -> bool {
        graph.tasks.iter().all(|t| t.is_finished())
    }

    fn reset(&mut self) {
        self.completed.clear();
        self.failed.clear();
        self.running.clear();
        debug!("Scheduler state reset");
    }
}

// ============================================================================
// execute_graph - Full DAG execution with retry and callbacks
// ============================================================================

impl DagScheduler {
    /// Execute entire TaskGraph with callbacks
    ///
    /// This is the main entry point for DAG execution. It orchestrates:
    /// 1. Risk evaluation and user confirmation flow
    /// 2. DAG-based task scheduling with parallelism
    /// 3. Retry logic for transient failures
    /// 4. Progress callbacks for UI updates
    ///
    /// # Arguments
    /// * `graph` - The task graph to execute
    /// * `executor` - Task executor implementation
    /// * `callback` - UI callback for progress updates
    /// * `context` - Task context for inter-task communication
    ///
    /// # Returns
    /// ExecutionResult containing completed/failed task lists
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let graph = TaskGraph::new("plan_1", "My Plan");
    /// let executor = Arc::new(MyExecutor);
    /// let callback = Arc::new(NoOpCallback);
    /// let context = TaskContext::new("User request");
    ///
    /// let result = DagScheduler::execute_graph(
    ///     graph, executor, callback, context, None
    /// ).await?;
    /// ```
    pub async fn execute_graph(
        mut graph: TaskGraph,
        executor: Arc<dyn GraphTaskExecutor>,
        callback: Arc<dyn ExecutionCallback>,
        mut context: TaskContext,
        config: Option<SchedulerConfig>,
    ) -> Result<ExecutionResult> {
        let graph_id = graph.id.clone();
        let mut result = ExecutionResult::new(&graph_id);

        info!("Starting execution of graph '{}' with {} tasks", graph_id, graph.tasks.len());

        // 1. Risk evaluation - check if confirmation is required
        let risk_evaluator = RiskEvaluator::new();
        let requires_confirmation = risk_evaluator.evaluate_graph(&graph);

        // 2. Create DagTaskPlan and notify UI
        let plan = DagTaskPlan::from_graph(&graph, requires_confirmation);
        callback.on_plan_ready(&plan).await;

        // 3. If high-risk tasks exist, request user confirmation
        if requires_confirmation {
            info!("Graph contains high-risk tasks, requesting user confirmation");
            let decision = callback.on_confirmation_required(&plan).await;

            if decision == UserDecision::Cancelled {
                info!("User cancelled execution of graph '{}'", graph_id);
                callback.on_cancelled().await;
                result.cancelled = true;
                return Ok(result);
            }

            info!("User confirmed execution of graph '{}'", graph_id);
        }

        // 4. Create scheduler with Arc<Mutex> for async context
        let scheduler_config = config.unwrap_or_default();
        let max_task_retries = scheduler_config.max_task_retries;
        let scheduler = Arc::new(Mutex::new(DagScheduler::with_config(scheduler_config)));

        // 5. DAG scheduling loop
        loop {
            // Get ready tasks
            let ready_tasks: Vec<(String, String, Vec<String>)> = {
                let sched = scheduler.lock().await;
                let ready = sched.next_ready(&graph);

                if ready.is_empty() {
                    // Check if we're done
                    if sched.is_complete(&graph) {
                        break;
                    }
                    // No tasks ready but not complete - check for deadlock
                    if sched.running_count() == 0 {
                        warn!("Graph '{}' has no ready tasks and none running - possible deadlock", graph_id);
                        break;
                    }
                }

                ready
                    .into_iter()
                    .map(|t| {
                        let deps: Vec<String> = graph
                            .get_predecessors(&t.id)
                            .into_iter()
                            .map(|s| s.to_string())
                            .collect();
                        (t.id.clone(), t.name.clone(), deps)
                    })
                    .collect()
            };

            if ready_tasks.is_empty() {
                // Wait a bit if tasks are still running
                let running = scheduler.lock().await.running_count();
                if running > 0 {
                    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                    continue;
                }
                break;
            }

            // Mark tasks as running
            {
                let mut sched = scheduler.lock().await;
                for (task_id, _, _) in &ready_tasks {
                    sched.mark_running(task_id);
                }
            }

            // Update graph status to Running
            for (task_id, _, _) in &ready_tasks {
                if let Some(task) = graph.get_task_mut(task_id) {
                    task.status = TaskStatus::running(0.0);
                }
            }

            // Execute tasks in parallel
            let futures: Vec<_> = ready_tasks
                .into_iter()
                .map(|(task_id, task_name, deps)| {
                    let executor = Arc::clone(&executor);
                    let callback = Arc::clone(&callback);
                    let task = graph.get_task(&task_id).cloned();
                    let dep_refs: Vec<&str> = deps.iter().map(|s| s.as_str()).collect();
                    let prompt_context = context.build_prompt_context(&task_id, &dep_refs);

                    async move {
                        if let Some(task) = task {
                            // Notify task start
                            callback.on_task_start(&task_id, &task_name).await;

                            // Execute with retry
                            let exec_result = execute_with_retry(
                                &task,
                                &executor,
                                &callback,
                                &prompt_context,
                                max_task_retries,
                            )
                            .await;

                            (task_id, task_name, exec_result)
                        } else {
                            (
                                task_id.clone(),
                                task_name,
                                Err(crate::error::AetherError::NotFound(format!(
                                    "Task '{}' not found in graph",
                                    task_id
                                ))),
                            )
                        }
                    }
                })
                .collect();

            let results = join_all(futures).await;

            // Process results
            for (task_id, task_name, exec_result) in results {
                let mut sched = scheduler.lock().await;

                match exec_result {
                    Ok(output) => {
                        // Record output in context
                        context.record_output_with_name(&task_id, &task_name, output.clone());

                        // Update task status
                        if let Some(task) = graph.get_task_mut(&task_id) {
                            task.status = TaskStatus::completed(
                                TaskResult::with_string(output.get_summary())
                                    .with_summary(output.get_summary())
                            );
                        }

                        // Mark completed in scheduler
                        sched.mark_completed(&task_id);
                        result.completed_tasks.push(task_id.clone());

                        // Notify callback
                        callback
                            .on_task_complete(&task_id, &context.get_output(&task_id).map(|o| o.get_summary()).unwrap_or_default())
                            .await;

                        info!("Task '{}' completed successfully", task_id);
                    }
                    Err(e) => {
                        let error_msg = e.to_string();

                        // Update task status
                        if let Some(task) = graph.get_task_mut(&task_id) {
                            task.status = TaskStatus::failed(&error_msg);
                        }

                        // Mark failed in scheduler
                        sched.mark_failed(&task_id, &error_msg);
                        result.failed_tasks.push(task_id.clone());

                        // Notify callback
                        callback.on_task_failed(&task_id, &error_msg).await;

                        warn!("Task '{}' failed: {}", task_id, error_msg);
                    }
                }
            }
        }

        // 6. Store context in result for accessing full outputs
        result.context = Some(context);

        // 7. Generate summary and notify completion
        let summary = format!(
            "Completed: {}, Failed: {}, Total: {}",
            result.completed_tasks.len(),
            result.failed_tasks.len(),
            result.total_executed()
        );
        callback.on_all_complete(&summary).await;

        info!(
            "Graph '{}' execution finished: {}",
            graph_id, summary
        );

        Ok(result)
    }
}

/// Execute a task with retry logic
///
/// Attempts to execute the task up to `max_retries` times.
/// On each failure, notifies the callback with retry attempt info.
/// After all retries exhausted, notifies with "deciding" state.
///
/// # Arguments
/// * `task` - The task to execute
/// * `executor` - Task executor
/// * `callback` - UI callback for progress updates
/// * `context` - Prompt context for the task
/// * `max_retries` - Maximum number of retry attempts
///
/// # Returns
/// TaskOutput on success, or error after all retries failed
async fn execute_with_retry(
    task: &Task,
    executor: &Arc<dyn GraphTaskExecutor>,
    callback: &Arc<dyn ExecutionCallback>,
    context: &str,
    max_retries: u32,
) -> Result<TaskOutput> {
    let mut last_error = None;

    for attempt in 1..=max_retries {
        debug!("Executing task '{}' attempt {}/{}", task.id, attempt, max_retries);

        match executor.execute(task, context).await {
            Ok(output) => {
                return Ok(output);
            }
            Err(e) => {
                let error_msg = e.to_string();
                last_error = Some(e);

                if attempt < max_retries {
                    // Notify retry
                    callback.on_task_retry(&task.id, attempt, &error_msg).await;
                    warn!(
                        "Task '{}' failed on attempt {}/{}: {}, retrying...",
                        task.id, attempt, max_retries, error_msg
                    );

                    // Brief delay before retry
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                } else {
                    // All retries exhausted - notify deciding state
                    callback.on_task_deciding(&task.id, &error_msg).await;
                    warn!(
                        "Task '{}' failed after {} attempts: {}",
                        task.id, max_retries, error_msg
                    );
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        crate::error::AetherError::Other {
            message: format!("Task '{}' failed with unknown error", task.id),
            suggestion: None,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::agent_types::{FileOp, TaskResult, TaskStatus, TaskType};
    use std::path::PathBuf;

    fn create_task(id: &str) -> Task {
        Task::new(
            id,
            format!("Task {}", id),
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        )
    }

    #[test]
    fn test_scheduler_basic() {
        let mut scheduler = DagScheduler::new();
        let mut graph = TaskGraph::new("test", "Test Graph");

        graph.add_task(create_task("a"));
        graph.add_task(create_task("b"));
        graph.add_task(create_task("c"));

        // a -> b -> c
        graph.add_dependency("a", "b");
        graph.add_dependency("b", "c");

        // Initially only 'a' should be ready
        let ready = scheduler.next_ready(&graph);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "a");

        // Mark 'a' as running then completed
        scheduler.mark_running("a");
        assert_eq!(scheduler.running_count(), 1);

        scheduler.mark_completed("a");
        assert_eq!(scheduler.running_count(), 0);

        // Update task status in graph
        graph.get_task_mut("a").unwrap().status = TaskStatus::completed(TaskResult::default());

        // Now 'b' should be ready
        let ready = scheduler.next_ready(&graph);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "b");
    }

    #[test]
    fn test_scheduler_parallel() {
        let scheduler = DagScheduler::with_config(SchedulerConfig { max_parallelism: 4, ..Default::default() });
        let mut graph = TaskGraph::new("test", "Parallel Test");

        // Four independent tasks
        graph.add_task(create_task("a"));
        graph.add_task(create_task("b"));
        graph.add_task(create_task("c"));
        graph.add_task(create_task("d"));

        // All four should be ready at once
        let ready = scheduler.next_ready(&graph);
        assert_eq!(ready.len(), 4);
    }

    #[test]
    fn test_scheduler_parallelism_limit() {
        let scheduler = DagScheduler::with_config(SchedulerConfig { max_parallelism: 2, ..Default::default() });
        let mut graph = TaskGraph::new("test", "Limited Parallel Test");

        graph.add_task(create_task("a"));
        graph.add_task(create_task("b"));
        graph.add_task(create_task("c"));
        graph.add_task(create_task("d"));

        // Only 2 should be returned due to limit
        let ready = scheduler.next_ready(&graph);
        assert_eq!(ready.len(), 2);
    }

    #[test]
    fn test_scheduler_failed_dependency() {
        let mut scheduler = DagScheduler::new();
        let mut graph = TaskGraph::new("test", "Failed Dependency Test");

        graph.add_task(create_task("a"));
        graph.add_task(create_task("b"));
        graph.add_dependency("a", "b");

        // Start and fail 'a'
        scheduler.mark_running("a");
        scheduler.mark_failed("a", "Test failure");

        // Update graph
        graph.get_task_mut("a").unwrap().status = TaskStatus::failed("Test failure");

        // 'b' should not be ready because 'a' failed
        let ready = scheduler.next_ready(&graph);
        assert!(ready.is_empty());
    }

    #[test]
    fn test_scheduler_diamond_dependency() {
        let mut scheduler = DagScheduler::with_config(SchedulerConfig { max_parallelism: 4, ..Default::default() });
        let mut graph = TaskGraph::new("test", "Diamond Test");

        //     a
        //    / \
        //   b   c
        //    \ /
        //     d
        graph.add_task(create_task("a"));
        graph.add_task(create_task("b"));
        graph.add_task(create_task("c"));
        graph.add_task(create_task("d"));

        graph.add_dependency("a", "b");
        graph.add_dependency("a", "c");
        graph.add_dependency("b", "d");
        graph.add_dependency("c", "d");

        // Only 'a' should be ready initially
        let ready = scheduler.next_ready(&graph);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "a");

        // Complete 'a'
        scheduler.mark_running("a");
        scheduler.mark_completed("a");
        graph.get_task_mut("a").unwrap().status = TaskStatus::completed(TaskResult::default());

        // 'b' and 'c' should be ready in parallel
        let ready = scheduler.next_ready(&graph);
        assert_eq!(ready.len(), 2);

        // Complete 'b' and 'c'
        scheduler.mark_running("b");
        scheduler.mark_running("c");
        scheduler.mark_completed("b");
        scheduler.mark_completed("c");
        graph.get_task_mut("b").unwrap().status = TaskStatus::completed(TaskResult::default());
        graph.get_task_mut("c").unwrap().status = TaskStatus::completed(TaskResult::default());

        // Now 'd' should be ready
        let ready = scheduler.next_ready(&graph);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "d");
    }

    #[test]
    fn test_scheduler_reset() {
        let mut scheduler = DagScheduler::new();

        scheduler.mark_running("a");
        scheduler.mark_completed("a");
        scheduler.mark_failed("b", "error");

        assert_eq!(scheduler.completed.len(), 1);
        assert_eq!(scheduler.failed.len(), 1);

        scheduler.reset();

        assert!(scheduler.completed.is_empty());
        assert!(scheduler.failed.is_empty());
        assert!(scheduler.running.is_empty());
    }

    // ========================================================================
    // execute_graph tests
    // ========================================================================

    use crate::dispatcher::callback::NoOpCallback;
    use crate::dispatcher::context::TaskContext;

    /// Mock task executor that always succeeds
    struct MockTaskExecutor;

    #[async_trait]
    impl GraphTaskExecutor for MockTaskExecutor {
        async fn execute(&self, task: &Task, _context: &str) -> crate::error::Result<TaskOutput> {
            Ok(TaskOutput::text(format!("Result for task: {}", task.id)))
        }
    }

    /// Mock task executor that always fails
    struct FailingExecutor;

    #[async_trait]
    impl GraphTaskExecutor for FailingExecutor {
        async fn execute(&self, task: &Task, _context: &str) -> crate::error::Result<TaskOutput> {
            Err(crate::error::AetherError::Other {
                message: format!("Task '{}' intentionally failed", task.id),
                suggestion: None,
            })
        }
    }

    #[tokio::test]
    async fn test_execute_graph_basic() {
        // Create a simple linear graph: a -> b -> c
        let mut graph = TaskGraph::new("test_basic", "Basic Test Graph");
        graph.add_task(create_task("a"));
        graph.add_task(create_task("b"));
        graph.add_task(create_task("c"));
        graph.add_dependency("a", "b");
        graph.add_dependency("b", "c");

        let executor = Arc::new(MockTaskExecutor);
        let callback = Arc::new(NoOpCallback);
        let context = TaskContext::new("Test user request");

        let result = DagScheduler::execute_graph(graph, executor, callback, context, None)
            .await
            .unwrap();

        // All tasks should complete successfully
        assert!(result.is_success());
        assert_eq!(result.completed_tasks.len(), 3);
        assert!(result.failed_tasks.is_empty());
        assert!(!result.cancelled);
    }

    #[tokio::test]
    async fn test_execute_graph_parallel() {
        // Create a graph with parallel tasks: a, b, c all independent
        let mut graph = TaskGraph::new("test_parallel", "Parallel Test Graph");
        graph.add_task(create_task("a"));
        graph.add_task(create_task("b"));
        graph.add_task(create_task("c"));
        // No dependencies - all can run in parallel

        let executor = Arc::new(MockTaskExecutor);
        let callback = Arc::new(NoOpCallback);
        let context = TaskContext::new("Test parallel execution");

        let result = DagScheduler::execute_graph(graph, executor, callback, context, None)
            .await
            .unwrap();

        assert!(result.is_success());
        assert_eq!(result.completed_tasks.len(), 3);
    }

    #[tokio::test]
    async fn test_execute_graph_with_failure() {
        // Create a simple graph
        let mut graph = TaskGraph::new("test_failure", "Failure Test Graph");
        graph.add_task(create_task("a"));
        graph.add_task(create_task("b"));
        graph.add_dependency("a", "b");

        let executor = Arc::new(FailingExecutor);
        let callback = Arc::new(NoOpCallback);
        let context = TaskContext::new("Test failure handling");

        let result = DagScheduler::execute_graph(graph, executor, callback, context, None)
            .await
            .unwrap();

        // Task 'a' should fail, 'b' should not be executed (dependency failed)
        assert!(!result.is_success());
        assert_eq!(result.failed_tasks.len(), 1);
        assert_eq!(result.failed_tasks[0], "a");
        // 'b' is never executed because 'a' failed
        assert!(result.completed_tasks.is_empty());
    }

    #[tokio::test]
    async fn test_execute_graph_diamond() {
        // Diamond pattern: a -> b, a -> c, b -> d, c -> d
        let mut graph = TaskGraph::new("test_diamond", "Diamond Test Graph");
        graph.add_task(create_task("a"));
        graph.add_task(create_task("b"));
        graph.add_task(create_task("c"));
        graph.add_task(create_task("d"));
        graph.add_dependency("a", "b");
        graph.add_dependency("a", "c");
        graph.add_dependency("b", "d");
        graph.add_dependency("c", "d");

        let executor = Arc::new(MockTaskExecutor);
        let callback = Arc::new(NoOpCallback);
        let context = TaskContext::new("Test diamond execution");

        let result = DagScheduler::execute_graph(graph, executor, callback, context, None)
            .await
            .unwrap();

        assert!(result.is_success());
        assert_eq!(result.completed_tasks.len(), 4);
    }

    #[test]
    fn test_execution_result() {
        let mut result = ExecutionResult::new("test_graph");
        assert!(result.is_success());
        assert_eq!(result.total_executed(), 0);

        result.completed_tasks.push("a".to_string());
        assert!(result.is_success());
        assert_eq!(result.total_executed(), 1);

        result.failed_tasks.push("b".to_string());
        assert!(!result.is_success());
        assert_eq!(result.total_executed(), 2);

        result.failed_tasks.clear();
        result.cancelled = true;
        assert!(!result.is_success());
    }
}
