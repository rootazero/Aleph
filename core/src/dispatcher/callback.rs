//! Execution callback interface for UI feedback
//!
//! This module provides the callback interface for reporting task execution
//! progress to the UI layer (Swift/C#). It enables real-time updates during
//! DAG task execution.

use async_trait::async_trait;

use crate::dispatcher::cowork_types::TaskGraph;
use crate::dispatcher::risk::{RiskEvaluator, RiskLevel};

// ============================================================================
// TaskDisplayStatus - UI-friendly task status
// ============================================================================

/// Task status for UI display
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum TaskDisplayStatus {
    /// Task is waiting to be executed
    Pending,
    /// Task is currently running
    Running,
    /// Task completed successfully
    Completed,
    /// Task failed
    Failed,
    /// Task was cancelled
    Cancelled,
}

impl std::fmt::Display for TaskDisplayStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskDisplayStatus::Pending => write!(f, "pending"),
            TaskDisplayStatus::Running => write!(f, "running"),
            TaskDisplayStatus::Completed => write!(f, "completed"),
            TaskDisplayStatus::Failed => write!(f, "failed"),
            TaskDisplayStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

// ============================================================================
// TaskInfo - UI-friendly task information
// ============================================================================

/// Task information for UI display
#[derive(Debug, Clone)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct TaskInfo {
    /// Unique task identifier
    pub id: String,
    /// Human-readable task name
    pub name: String,
    /// Current status
    pub status: TaskDisplayStatus,
    /// Risk level ("low" or "high")
    pub risk_level: String,
    /// IDs of tasks this task depends on
    pub dependencies: Vec<String>,
}

impl TaskInfo {
    /// Create a new TaskInfo
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        status: TaskDisplayStatus,
        risk_level: RiskLevel,
        dependencies: Vec<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            status,
            risk_level: match risk_level {
                RiskLevel::Low => "low".to_string(),
                RiskLevel::High => "high".to_string(),
            },
            dependencies,
        }
    }
}

// ============================================================================
// TaskPlan - UI-friendly execution plan
// ============================================================================

/// Execution plan for UI display
#[derive(Debug, Clone)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct TaskPlan {
    /// Unique plan identifier
    pub id: String,
    /// Human-readable plan title
    pub title: String,
    /// List of tasks in the plan
    pub tasks: Vec<TaskInfo>,
    /// Whether user confirmation is required before execution
    pub requires_confirmation: bool,
}

impl TaskPlan {
    /// Create a TaskPlan from a TaskGraph
    ///
    /// # Arguments
    /// * `graph` - The task graph to convert
    /// * `requires_confirmation` - Whether user confirmation is required
    ///
    /// # Returns
    /// A TaskPlan suitable for UI display
    pub fn from_graph(graph: &TaskGraph, requires_confirmation: bool) -> Self {
        let evaluator = RiskEvaluator::new();

        // Build a map of task dependencies
        let tasks: Vec<TaskInfo> = graph
            .tasks
            .iter()
            .map(|task| {
                // Get dependencies for this task
                let dependencies: Vec<String> = graph
                    .get_predecessors(&task.id)
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect();

                // Convert status
                let status = match &task.status {
                    crate::dispatcher::cowork_types::TaskStatus::Pending => {
                        TaskDisplayStatus::Pending
                    }
                    crate::dispatcher::cowork_types::TaskStatus::Running { .. } => {
                        TaskDisplayStatus::Running
                    }
                    crate::dispatcher::cowork_types::TaskStatus::Completed { .. } => {
                        TaskDisplayStatus::Completed
                    }
                    crate::dispatcher::cowork_types::TaskStatus::Failed { .. } => {
                        TaskDisplayStatus::Failed
                    }
                    crate::dispatcher::cowork_types::TaskStatus::Cancelled => {
                        TaskDisplayStatus::Cancelled
                    }
                };

                // Evaluate risk
                let risk_level = evaluator.evaluate(task);

                TaskInfo::new(&task.id, &task.name, status, risk_level, dependencies)
            })
            .collect();

        Self {
            id: graph.id.clone(),
            title: graph.metadata.title.clone(),
            tasks,
            requires_confirmation,
        }
    }

    /// Get the number of tasks in the plan
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Check if any task has high risk
    pub fn has_high_risk_tasks(&self) -> bool {
        self.tasks.iter().any(|t| t.risk_level == "high")
    }
}

// ============================================================================
// UserDecision - User's response to confirmation request
// ============================================================================

/// User's decision on whether to proceed with execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserDecision {
    /// User confirmed, proceed with execution
    Confirmed,
    /// User cancelled, abort execution
    Cancelled,
}

// ============================================================================
// ExecutionCallback - Async callback trait for UI updates
// ============================================================================

/// Callback interface for task execution progress
///
/// This trait is implemented by the UI layer to receive real-time updates
/// during DAG task execution. All methods are async to support non-blocking
/// UI updates.
///
/// # Example
///
/// ```rust,ignore
/// struct MyCallback;
///
/// #[async_trait]
/// impl ExecutionCallback for MyCallback {
///     async fn on_plan_ready(&self, plan: &TaskPlan) {
///         println!("Plan ready: {}", plan.title);
///     }
///     // ... implement other methods
/// }
/// ```
#[async_trait]
pub trait ExecutionCallback: Send + Sync {
    /// Called when the execution plan is ready
    ///
    /// This is called after task planning is complete, before execution begins.
    /// The UI can use this to display the task list to the user.
    async fn on_plan_ready(&self, plan: &TaskPlan);

    /// Called when user confirmation is required
    ///
    /// This is called when the plan contains high-risk tasks that require
    /// user approval before execution. The implementation should display
    /// a confirmation dialog and return the user's decision.
    ///
    /// # Returns
    /// `UserDecision::Confirmed` to proceed, `UserDecision::Cancelled` to abort
    async fn on_confirmation_required(&self, plan: &TaskPlan) -> UserDecision;

    /// Called when a task starts execution
    ///
    /// # Arguments
    /// * `task_id` - The unique identifier of the task
    /// * `task_name` - The human-readable name of the task
    async fn on_task_start(&self, task_id: &str, task_name: &str);

    /// Called when streaming output is available for a task
    ///
    /// This is called for tasks that produce streaming output (e.g., LLM responses).
    /// The UI can use this to display real-time output.
    ///
    /// # Arguments
    /// * `task_id` - The unique identifier of the task
    /// * `chunk` - A chunk of streaming output
    async fn on_task_stream(&self, task_id: &str, chunk: &str);

    /// Called when a task completes successfully
    ///
    /// # Arguments
    /// * `task_id` - The unique identifier of the task
    /// * `summary` - A brief summary of the task result
    async fn on_task_complete(&self, task_id: &str, summary: &str);

    /// Called when a task is being retried
    ///
    /// This is called when a task fails but is being retried according to
    /// the retry policy.
    ///
    /// # Arguments
    /// * `task_id` - The unique identifier of the task
    /// * `attempt` - The current attempt number (1-based)
    /// * `error` - The error message from the previous attempt
    async fn on_task_retry(&self, task_id: &str, attempt: u32, error: &str);

    /// Called when LLM is deciding how to handle an error
    ///
    /// This is called when a task fails and the system is using an LLM
    /// to decide the next action (retry, skip, abort, etc.).
    ///
    /// # Arguments
    /// * `task_id` - The unique identifier of the task
    /// * `error` - The error message being evaluated
    async fn on_task_deciding(&self, task_id: &str, error: &str);

    /// Called when a task fails permanently
    ///
    /// This is called when a task fails and will not be retried.
    ///
    /// # Arguments
    /// * `task_id` - The unique identifier of the task
    /// * `error` - The final error message
    async fn on_task_failed(&self, task_id: &str, error: &str);

    /// Called when all tasks have completed
    ///
    /// This is called at the end of execution, regardless of whether all
    /// tasks succeeded or some failed.
    ///
    /// # Arguments
    /// * `summary` - A summary of the overall execution result
    async fn on_all_complete(&self, summary: &str);

    /// Called when execution is cancelled
    ///
    /// This is called when the user cancels execution or when cancellation
    /// is triggered programmatically.
    async fn on_cancelled(&self);
}

// ============================================================================
// NoOpCallback - No-op implementation for testing
// ============================================================================

/// A no-op callback implementation for testing
///
/// This implementation does nothing for all callback methods, making it
/// suitable for tests that don't need to verify callback behavior.
pub struct NoOpCallback;

#[async_trait]
impl ExecutionCallback for NoOpCallback {
    async fn on_plan_ready(&self, _plan: &TaskPlan) {}

    async fn on_confirmation_required(&self, _plan: &TaskPlan) -> UserDecision {
        // Default to confirmed for testing
        UserDecision::Confirmed
    }

    async fn on_task_start(&self, _task_id: &str, _task_name: &str) {}

    async fn on_task_stream(&self, _task_id: &str, _chunk: &str) {}

    async fn on_task_complete(&self, _task_id: &str, _summary: &str) {}

    async fn on_task_retry(&self, _task_id: &str, _attempt: u32, _error: &str) {}

    async fn on_task_deciding(&self, _task_id: &str, _error: &str) {}

    async fn on_task_failed(&self, _task_id: &str, _error: &str) {}

    async fn on_all_complete(&self, _summary: &str) {}

    async fn on_cancelled(&self) {}
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::cowork_types::{FileOp, Task, TaskGraph, TaskType};
    use std::path::PathBuf;

    fn create_file_task(id: &str, name: &str, file_op: FileOp) -> Task {
        Task::new(id, name, TaskType::FileOperation(file_op))
    }

    #[test]
    fn test_task_display_status() {
        assert_eq!(TaskDisplayStatus::Pending.to_string(), "pending");
        assert_eq!(TaskDisplayStatus::Running.to_string(), "running");
        assert_eq!(TaskDisplayStatus::Completed.to_string(), "completed");
        assert_eq!(TaskDisplayStatus::Failed.to_string(), "failed");
        assert_eq!(TaskDisplayStatus::Cancelled.to_string(), "cancelled");
    }

    #[test]
    fn test_task_info_creation() {
        let info = TaskInfo::new(
            "task_1",
            "Read file",
            TaskDisplayStatus::Pending,
            RiskLevel::Low,
            vec!["task_0".to_string()],
        );

        assert_eq!(info.id, "task_1");
        assert_eq!(info.name, "Read file");
        assert_eq!(info.status, TaskDisplayStatus::Pending);
        assert_eq!(info.risk_level, "low");
        assert_eq!(info.dependencies, vec!["task_0"]);

        let high_risk_info = TaskInfo::new(
            "task_2",
            "Delete file",
            TaskDisplayStatus::Running,
            RiskLevel::High,
            vec![],
        );
        assert_eq!(high_risk_info.risk_level, "high");
    }

    #[test]
    fn test_task_plan_from_graph() {
        // Create a simple graph: task_1 -> task_2 -> task_3
        let mut graph = TaskGraph::new("plan_1", "Test Plan");

        graph.add_task(create_file_task(
            "task_1",
            "Read config",
            FileOp::Read {
                path: PathBuf::from("/etc/config"),
            },
        ));

        graph.add_task(create_file_task(
            "task_2",
            "Process data",
            FileOp::List {
                path: PathBuf::from("/tmp"),
            },
        ));

        graph.add_task(create_file_task(
            "task_3",
            "Write result",
            FileOp::Write {
                path: PathBuf::from("/tmp/result.txt"),
            },
        ));

        graph.add_dependency("task_1", "task_2");
        graph.add_dependency("task_2", "task_3");

        // Convert to TaskPlan
        let plan = TaskPlan::from_graph(&graph, true);

        assert_eq!(plan.id, "plan_1");
        assert_eq!(plan.title, "Test Plan");
        assert_eq!(plan.task_count(), 3);
        assert!(plan.requires_confirmation);

        // Verify task info
        let task_1 = plan.tasks.iter().find(|t| t.id == "task_1").unwrap();
        assert_eq!(task_1.name, "Read config");
        assert_eq!(task_1.status, TaskDisplayStatus::Pending);
        assert_eq!(task_1.risk_level, "low"); // Read is low risk
        assert!(task_1.dependencies.is_empty()); // No predecessors

        let task_2 = plan.tasks.iter().find(|t| t.id == "task_2").unwrap();
        assert_eq!(task_2.dependencies, vec!["task_1"]);

        let task_3 = plan.tasks.iter().find(|t| t.id == "task_3").unwrap();
        assert_eq!(task_3.risk_level, "high"); // Write is high risk
        assert_eq!(task_3.dependencies, vec!["task_2"]);

        // Verify has_high_risk_tasks
        assert!(plan.has_high_risk_tasks());
    }

    #[test]
    fn test_task_plan_no_high_risk() {
        let mut graph = TaskGraph::new("safe_plan", "Safe Plan");

        graph.add_task(create_file_task(
            "read_1",
            "Read file",
            FileOp::Read {
                path: PathBuf::from("/tmp/data.txt"),
            },
        ));

        graph.add_task(create_file_task(
            "list_1",
            "List directory",
            FileOp::List {
                path: PathBuf::from("/tmp"),
            },
        ));

        let plan = TaskPlan::from_graph(&graph, false);

        assert!(!plan.requires_confirmation);
        assert!(!plan.has_high_risk_tasks());
    }

    #[test]
    fn test_user_decision() {
        assert_eq!(UserDecision::Confirmed, UserDecision::Confirmed);
        assert_ne!(UserDecision::Confirmed, UserDecision::Cancelled);
    }

    #[tokio::test]
    async fn test_noop_callback() {
        let callback = NoOpCallback;
        let plan = TaskPlan {
            id: "test".to_string(),
            title: "Test Plan".to_string(),
            tasks: vec![],
            requires_confirmation: false,
        };

        // All methods should complete without error
        callback.on_plan_ready(&plan).await;
        assert_eq!(
            callback.on_confirmation_required(&plan).await,
            UserDecision::Confirmed
        );
        callback.on_task_start("task_1", "Test Task").await;
        callback.on_task_stream("task_1", "output chunk").await;
        callback.on_task_complete("task_1", "completed").await;
        callback.on_task_retry("task_1", 1, "error").await;
        callback.on_task_deciding("task_1", "error").await;
        callback.on_task_failed("task_1", "final error").await;
        callback.on_all_complete("all done").await;
        callback.on_cancelled().await;
    }
}
