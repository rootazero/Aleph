//! Unified executor implementation
//!
//! This module implements the `UnifiedExecutor` that executes plans
//! produced by the unified planner.
//!
//! # Architecture
//!
//! The executor takes an `ExecutionPlan` and produces an `ExecutionResult`:
//!
//! - **Conversational**: Invokes the AI agent for direct response
//! - **SingleAction**: Executes a single tool call
//! - **TaskGraph**: Orchestrates multi-step execution with DAG scheduling

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::types::{
    ExecutionContext, ExecutionResult, ExecutorError, TaskExecutionResult, ToolCallRecord,
};
use crate::agent::RigAgentManager;
use crate::cowork::executor::ExecutionContext as CoworkExecutionContext;
use crate::cowork::executor::ExecutorRegistry;
use crate::cowork::scheduler::{DagScheduler, SchedulerConfig, TaskScheduler};
use crate::cowork::types::{TaskGraph, TaskStatus};
use crate::ffi::AetherEventHandler;
use crate::planner::{ExecutionPlan, PlannedTask};

// =============================================================================
// Executor Configuration
// =============================================================================

/// Configuration for the unified executor
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Maximum number of tasks to run in parallel (default: 4)
    pub max_parallelism: usize,
    /// Timeout for individual task execution in seconds (default: 300)
    pub task_timeout_seconds: u64,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_parallelism: 4,
            task_timeout_seconds: 300,
        }
    }
}

impl ExecutorConfig {
    /// Create a new config with custom parallelism
    pub fn with_parallelism(mut self, max_parallelism: usize) -> Self {
        self.max_parallelism = max_parallelism;
        self
    }

    /// Create a new config with custom timeout
    pub fn with_timeout(mut self, timeout_seconds: u64) -> Self {
        self.task_timeout_seconds = timeout_seconds;
        self
    }
}

// =============================================================================
// Unified Executor
// =============================================================================

/// Unified executor that executes plans produced by the planner
///
/// The executor handles three types of plans:
/// - Conversational: Direct AI response
/// - SingleAction: Single tool execution
/// - TaskGraph: Multi-step DAG execution
pub struct UnifiedExecutor {
    /// Agent manager for AI interactions
    agent_manager: Arc<RigAgentManager>,
    /// DAG scheduler for task graph execution
    dag_scheduler: Arc<RwLock<DagScheduler>>,
    /// Registry of task executors
    executor_registry: Arc<ExecutorRegistry>,
    /// Event handler for callbacks
    event_handler: Arc<dyn AetherEventHandler>,
    /// Executor configuration
    config: ExecutorConfig,
}

impl UnifiedExecutor {
    /// Create a new executor with default configuration
    pub fn new(
        agent_manager: Arc<RigAgentManager>,
        executor_registry: Arc<ExecutorRegistry>,
        event_handler: Arc<dyn AetherEventHandler>,
    ) -> Self {
        let config = ExecutorConfig::default();
        let scheduler_config = SchedulerConfig {
            max_parallelism: config.max_parallelism,
        };

        Self {
            agent_manager,
            dag_scheduler: Arc::new(RwLock::new(DagScheduler::with_config(scheduler_config))),
            executor_registry,
            event_handler,
            config,
        }
    }

    /// Create a new executor with custom configuration
    pub fn with_config(
        agent_manager: Arc<RigAgentManager>,
        executor_registry: Arc<ExecutorRegistry>,
        event_handler: Arc<dyn AetherEventHandler>,
        config: ExecutorConfig,
    ) -> Self {
        let scheduler_config = SchedulerConfig {
            max_parallelism: config.max_parallelism,
        };

        Self {
            agent_manager,
            dag_scheduler: Arc::new(RwLock::new(DagScheduler::with_config(scheduler_config))),
            executor_registry,
            event_handler,
            config,
        }
    }

    /// Execute a plan and return the result
    ///
    /// This is the main entry point for plan execution. It dispatches
    /// to the appropriate execution method based on plan type.
    pub async fn execute(
        &self,
        plan: ExecutionPlan,
        context: ExecutionContext,
    ) -> Result<ExecutionResult, ExecutorError> {
        let start = Instant::now();
        info!(plan_type = plan.plan_type(), "Executing plan");

        let result = match plan {
            ExecutionPlan::Conversational { enhanced_prompt } => {
                self.execute_conversation(enhanced_prompt, context).await
            }
            ExecutionPlan::SingleAction {
                tool_name,
                parameters,
                ..
            } => self.execute_single_action(tool_name, parameters, context).await,
            ExecutionPlan::TaskGraph {
                tasks,
                dependencies,
                ..
            } => {
                self.execute_task_graph(tasks, dependencies, context)
                    .await
            }
        };

        let elapsed = start.elapsed();
        match &result {
            Ok(r) => {
                info!(
                    success = r.success,
                    execution_time_ms = elapsed.as_millis() as u64,
                    tool_calls = r.tool_calls.len(),
                    "Plan execution completed"
                );
            }
            Err(e) => {
                error!(
                    error = %e,
                    execution_time_ms = elapsed.as_millis() as u64,
                    "Plan execution failed"
                );
            }
        }

        result
    }

    /// Execute a conversational plan
    ///
    /// This invokes the AI agent to generate a response, optionally
    /// using an enhanced prompt.
    pub async fn execute_conversation(
        &self,
        enhanced_prompt: Option<String>,
        _context: ExecutionContext,
    ) -> Result<ExecutionResult, ExecutorError> {
        let start = Instant::now();
        debug!("Executing conversational plan");

        // Notify thinking started
        self.event_handler.on_thinking();

        // Prepare the prompt
        let prompt = enhanced_prompt.unwrap_or_default();
        if prompt.is_empty() {
            // No prompt means nothing to process
            return Ok(ExecutionResult::success("")
                .with_execution_time(start.elapsed()));
        }

        // Process through the agent (streaming is handled internally by agent)
        let response = self
            .agent_manager
            .process(&prompt)
            .await
            .map_err(|e| ExecutorError::execution_failed(e.to_string()))?;

        // Build tool call records from agent response
        // Note: AgentResponse.tools_called only contains tool names, not full details
        let tool_calls: Vec<ToolCallRecord> = response
            .tools_called
            .iter()
            .map(|tool_name| {
                ToolCallRecord::success(
                    tool_name,
                    serde_json::Value::Null,
                    "", // No result details available
                    0,  // No duration available
                )
            })
            .collect();

        // Notify completion
        self.event_handler.on_complete(response.content.clone());

        Ok(ExecutionResult::success(&response.content)
            .with_tool_calls(tool_calls)
            .with_execution_time(start.elapsed()))
    }

    /// Execute a single action plan
    ///
    /// This executes a single tool call with the given parameters.
    /// The execution is done by asking the AI to use the specified tool.
    pub async fn execute_single_action(
        &self,
        tool_name: String,
        parameters: serde_json::Value,
        _context: ExecutionContext,
    ) -> Result<ExecutionResult, ExecutorError> {
        let start = Instant::now();
        debug!(tool_name = %tool_name, "Executing single action");

        // Generate a unique call ID
        let call_id = uuid::Uuid::new_v4().to_string();

        // Notify tool start
        self.event_handler.on_tool_start(tool_name.clone());
        self.event_handler
            .on_tool_call_started(call_id.clone(), tool_name.clone());

        // Build a prompt that instructs the AI to use the specific tool
        // This leverages the agent's tool calling capability
        let params_str = serde_json::to_string_pretty(&parameters).unwrap_or_default();
        let prompt = format!(
            "Use the {} tool with the following parameters:\n{}",
            tool_name, params_str
        );

        let tool_start = Instant::now();
        let result = self.agent_manager.process(&prompt).await;

        let tool_duration = tool_start.elapsed();

        match result {
            Ok(response) => {
                let output = response.content;
                let _was_tool_called = response.tools_called.contains(&tool_name);

                // Notify success
                self.event_handler
                    .on_tool_result(tool_name.clone(), output.clone());
                self.event_handler
                    .on_tool_call_completed(call_id, output.clone());

                let tool_record = ToolCallRecord::success(
                    &tool_name,
                    parameters,
                    &output,
                    tool_duration.as_millis() as u64,
                );

                Ok(ExecutionResult::success(&output)
                    .with_tool_calls(vec![tool_record])
                    .with_execution_time(start.elapsed()))
            }
            Err(e) => {
                let error_msg = e.to_string();
                let is_retryable = true; // Most tool errors are retryable

                // Notify failure
                self.event_handler
                    .on_tool_call_failed(call_id, error_msg.clone(), is_retryable);

                let _tool_record = ToolCallRecord::failure(
                    &tool_name,
                    parameters,
                    &error_msg,
                    tool_duration.as_millis() as u64,
                );

                Err(ExecutorError::tool_error(error_msg))
            }
        }
    }

    /// Execute a task graph plan
    ///
    /// This orchestrates multi-step execution using DAG scheduling.
    /// Tasks are executed in dependency order with parallel execution
    /// where possible.
    pub async fn execute_task_graph(
        &self,
        tasks: Vec<PlannedTask>,
        dependencies: Vec<(usize, usize)>,
        context: ExecutionContext,
    ) -> Result<ExecutionResult, ExecutorError> {
        let start = Instant::now();
        info!(
            task_count = tasks.len(),
            dependency_count = dependencies.len(),
            "Executing task graph"
        );

        if tasks.is_empty() {
            return Ok(ExecutionResult::success("No tasks to execute")
                .with_execution_time(start.elapsed()));
        }

        // Build TaskGraph from PlannedTasks
        let mut graph = TaskGraph::new(
            uuid::Uuid::new_v4().to_string(),
            "Execution Plan",
        );

        // Add tasks to graph
        for planned in &tasks {
            let task = planned.to_task();
            graph.add_task(task);
        }

        // Add dependencies (convert from index-based to id-based)
        for (dependent_idx, dependency_idx) in &dependencies {
            let from_id = format!("planned_task_{}", dependency_idx);
            let to_id = format!("planned_task_{}", dependent_idx);
            graph.add_dependency(&from_id, &to_id);
        }

        // Validate the graph
        if let Err(e) = graph.validate() {
            return Err(ExecutorError::execution_failed(format!(
                "Invalid task graph: {}",
                e
            )));
        }

        // Notify plan created
        let step_descriptions: Vec<String> = tasks
            .iter()
            .map(|t| t.description.clone())
            .collect();
        self.event_handler.on_plan_created(
            graph.id.clone(),
            step_descriptions,
        );

        // Convert executor context
        let cowork_ctx = self.convert_context(&context, &graph.id);

        // Execute using scheduler
        let mut task_results: Vec<TaskExecutionResult> = Vec::new();
        let mut tool_calls: Vec<ToolCallRecord> = Vec::new();
        let mut all_succeeded = true;

        // Reset scheduler for new execution
        {
            let mut scheduler = self.dag_scheduler.write().await;
            scheduler.reset();
        }

        // Main execution loop
        loop {
            // Get next batch of ready tasks
            let ready_tasks = {
                let scheduler = self.dag_scheduler.read().await;
                let ready = scheduler.next_ready(&graph);
                ready.iter().map(|t| t.id.clone()).collect::<Vec<_>>()
            };

            if ready_tasks.is_empty() {
                // Check if execution is complete
                let scheduler = self.dag_scheduler.read().await;
                if scheduler.is_complete(&graph) {
                    break;
                }
                // No ready tasks but not complete - might be waiting for running tasks
                // In a real implementation, we'd use async task spawning
                // For now, break to avoid infinite loop
                if graph.tasks.iter().all(|t| t.is_finished()) {
                    break;
                }
                warn!("No ready tasks available but execution not complete");
                break;
            }

            // Mark tasks as running
            {
                let mut scheduler = self.dag_scheduler.write().await;
                for task_id in &ready_tasks {
                    scheduler.mark_running(task_id);
                }
            }

            // Execute tasks (sequentially for simplicity; parallel execution can be added)
            for task_id in ready_tasks {
                let task_start = Instant::now();
                let task = match graph.get_task(&task_id) {
                    Some(t) => t.clone(),
                    None => continue,
                };

                debug!(task_id = %task_id, task_name = %task.name, "Executing task");

                // Notify task start
                let call_id = uuid::Uuid::new_v4().to_string();
                self.event_handler
                    .on_tool_call_started(call_id.clone(), task.name.clone());

                // Execute the task
                let result = self.executor_registry.execute(&task, &cowork_ctx).await;
                let task_duration = task_start.elapsed();

                match result {
                    Ok(task_result) => {
                        // Update graph and scheduler
                        if let Some(t) = graph.get_task_mut(&task_id) {
                            t.status = TaskStatus::completed(task_result.clone());
                        }
                        {
                            let mut scheduler = self.dag_scheduler.write().await;
                            scheduler.mark_completed(&task_id);
                        }

                        // Extract output from TaskResult
                        // TaskResult.output is serde_json::Value, convert to string
                        let output = match &task_result.output {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Null => String::new(),
                            other => other.to_string(),
                        };
                        self.event_handler
                            .on_tool_call_completed(call_id, output.clone());

                        task_results.push(
                            TaskExecutionResult::success(&task_id, &task.name, &output)
                                .with_execution_time(task_duration),
                        );

                        // Add tool call record
                        tool_calls.push(ToolCallRecord::success(
                            &task.name,
                            task.parameters.clone(),
                            &output,
                            task_duration.as_millis() as u64,
                        ));
                    }
                    Err(e) => {
                        let error_msg = e.to_string();

                        // Update graph and scheduler
                        if let Some(t) = graph.get_task_mut(&task_id) {
                            t.status = TaskStatus::failed(&error_msg);
                        }
                        {
                            let mut scheduler = self.dag_scheduler.write().await;
                            scheduler.mark_failed(&task_id, &error_msg);
                        }

                        // Record failure
                        self.event_handler
                            .on_tool_call_failed(call_id, error_msg.clone(), true);

                        task_results.push(
                            TaskExecutionResult::failure(&task_id, &task.name, &error_msg)
                                .with_execution_time(task_duration),
                        );

                        // Add tool call record
                        tool_calls.push(ToolCallRecord::failure(
                            &task.name,
                            task.parameters.clone(),
                            &error_msg,
                            task_duration.as_millis() as u64,
                        ));

                        all_succeeded = false;
                    }
                }
            }
        }

        // Build summary
        let successful_count = task_results.iter().filter(|r| r.success).count();
        let failed_count = task_results.iter().filter(|r| !r.success).count();
        let summary = format!(
            "Executed {} tasks: {} succeeded, {} failed",
            task_results.len(),
            successful_count,
            failed_count
        );

        if all_succeeded {
            Ok(ExecutionResult::success(&summary)
                .with_tool_calls(tool_calls)
                .with_task_results(task_results)
                .with_execution_time(start.elapsed()))
        } else {
            // Return partial success with error information
            let mut result = ExecutionResult::success(&summary)
                .with_tool_calls(tool_calls)
                .with_task_results(task_results)
                .with_execution_time(start.elapsed());
            result.success = false;
            result.error = Some(format!("{} tasks failed", failed_count));
            Ok(result)
        }
    }

    /// Convert executor context to cowork execution context
    fn convert_context(&self, _ctx: &ExecutionContext, graph_id: &str) -> CoworkExecutionContext {
        CoworkExecutionContext::new(graph_id)
            .with_dry_run(false)
    }

    /// Get the current configuration
    pub fn config(&self) -> &ExecutorConfig {
        &self.config
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // ExecutorConfig Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_executor_config_default() {
        let config = ExecutorConfig::default();
        assert_eq!(config.max_parallelism, 4);
        assert_eq!(config.task_timeout_seconds, 300);
    }

    #[test]
    fn test_executor_config_with_parallelism() {
        let config = ExecutorConfig::default().with_parallelism(8);
        assert_eq!(config.max_parallelism, 8);
        assert_eq!(config.task_timeout_seconds, 300);
    }

    #[test]
    fn test_executor_config_with_timeout() {
        let config = ExecutorConfig::default().with_timeout(600);
        assert_eq!(config.max_parallelism, 4);
        assert_eq!(config.task_timeout_seconds, 600);
    }

    #[test]
    fn test_executor_config_builder_chain() {
        let config = ExecutorConfig::default()
            .with_parallelism(2)
            .with_timeout(120);
        assert_eq!(config.max_parallelism, 2);
        assert_eq!(config.task_timeout_seconds, 120);
    }

    // -------------------------------------------------------------------------
    // Context Conversion Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_execution_context_app_description() {
        let ctx = ExecutionContext::new()
            .with_app_context("Safari")
            .with_window_title("Google Search");

        assert_eq!(
            ctx.app_description(),
            Some("Safari - Google Search".to_string())
        );
    }

    #[test]
    fn test_execution_context_no_app_info() {
        let ctx = ExecutionContext::new();
        assert!(ctx.app_description().is_none());
    }

    // -------------------------------------------------------------------------
    // PlannedTask to Task Conversion Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_planned_task_to_task_id_format() {
        use crate::cowork::types::{FileOp, TaskType};
        use std::path::PathBuf;

        let planned = PlannedTask::new(
            42,
            "Test task",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        );

        let task = planned.to_task();
        assert_eq!(task.id, "planned_task_42");
        assert_eq!(task.name, "Test task");
    }

    // -------------------------------------------------------------------------
    // Execution Plan Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_execution_plan_task_count() {
        use crate::cowork::types::{FileOp, TaskType};
        use std::path::PathBuf;

        let plan = ExecutionPlan::conversational();
        assert_eq!(plan.task_count(), 0);

        let plan = ExecutionPlan::single_action(
            "read_file".to_string(),
            serde_json::json!({"path": "/tmp/test"}),
        );
        assert_eq!(plan.task_count(), 1);

        let tasks = vec![
            PlannedTask::new(
                0,
                "Task 1",
                TaskType::FileOperation(FileOp::List {
                    path: PathBuf::from("/tmp"),
                }),
            ),
            PlannedTask::new(
                1,
                "Task 2",
                TaskType::FileOperation(FileOp::List {
                    path: PathBuf::from("/tmp"),
                }),
            ),
        ];
        let plan = ExecutionPlan::task_graph(tasks, vec![]);
        assert_eq!(plan.task_count(), 2);
    }

    // -------------------------------------------------------------------------
    // ExecutionResult Builder Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_execution_result_with_task_results() {
        let task_results = vec![
            TaskExecutionResult::success("t1", "Task 1", "Output 1"),
            TaskExecutionResult::failure("t2", "Task 2", "Error"),
        ];

        let result = ExecutionResult::success("Done")
            .with_task_results(task_results);

        assert!(result.task_results.is_some());
        let tasks = result.task_results.unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks[0].success);
        assert!(!tasks[1].success);
    }

    #[test]
    fn test_tool_call_record_builders() {
        let record = ToolCallRecord::new("test_tool", serde_json::json!({"key": "value"}))
            .with_result("success", true)
            .with_execution_time_ms(100);

        assert_eq!(record.tool_name, "test_tool");
        assert!(record.success);
        assert_eq!(record.execution_time_ms, 100);
    }
}
