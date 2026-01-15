//! Cowork FFI bindings
//!
//! This module provides FFI-safe wrapper types for the Cowork task orchestration system.
//! These types are designed to work with UniFFI for Swift/Kotlin interop.

use std::sync::Arc;

use crate::cowork::{CoworkConfig, ExecutionState};
use crate::cowork::monitor::{ProgressEvent, ProgressSubscriber};
use crate::cowork::types::{
    ExecutionSummary, Task, TaskDependency, TaskGraph, TaskStatus, TaskType,
};

// ============================================================================
// FFI Enums
// ============================================================================

/// Execution state for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoworkExecutionState {
    Idle,
    Planning,
    AwaitingConfirmation,
    Executing,
    Paused,
    Cancelled,
    Completed,
}

impl From<ExecutionState> for CoworkExecutionState {
    fn from(state: ExecutionState) -> Self {
        match state {
            ExecutionState::Idle => CoworkExecutionState::Idle,
            ExecutionState::Planning => CoworkExecutionState::Planning,
            ExecutionState::AwaitingConfirmation => CoworkExecutionState::AwaitingConfirmation,
            ExecutionState::Executing => CoworkExecutionState::Executing,
            ExecutionState::Paused => CoworkExecutionState::Paused,
            ExecutionState::Cancelled => CoworkExecutionState::Cancelled,
            ExecutionState::Completed => CoworkExecutionState::Completed,
        }
    }
}

/// Task status state for FFI (simplified)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoworkTaskStatusState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl From<&TaskStatus> for CoworkTaskStatusState {
    fn from(status: &TaskStatus) -> Self {
        match status {
            TaskStatus::Pending => CoworkTaskStatusState::Pending,
            TaskStatus::Running { .. } => CoworkTaskStatusState::Running,
            TaskStatus::Completed { .. } => CoworkTaskStatusState::Completed,
            TaskStatus::Failed { .. } => CoworkTaskStatusState::Failed,
            TaskStatus::Cancelled => CoworkTaskStatusState::Cancelled,
        }
    }
}

/// Task type category for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoworkTaskTypeCategory {
    FileOperation,
    CodeExecution,
    DocumentGeneration,
    AppAutomation,
    AiInference,
}

impl From<&TaskType> for CoworkTaskTypeCategory {
    fn from(task_type: &TaskType) -> Self {
        match task_type {
            TaskType::FileOperation(_) => CoworkTaskTypeCategory::FileOperation,
            TaskType::CodeExecution(_) => CoworkTaskTypeCategory::CodeExecution,
            TaskType::DocumentGeneration(_) => CoworkTaskTypeCategory::DocumentGeneration,
            TaskType::AppAutomation(_) => CoworkTaskTypeCategory::AppAutomation,
            TaskType::AiInference(_) => CoworkTaskTypeCategory::AiInference,
        }
    }
}

/// Progress event type for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoworkProgressEventType {
    TaskStarted,
    TaskProgress,
    TaskCompleted,
    TaskFailed,
    TaskCancelled,
    GraphProgress,
    GraphCompleted,
}

// ============================================================================
// FFI Dictionaries (Structs)
// ============================================================================

/// Cowork configuration for FFI
#[derive(Debug, Clone)]
pub struct CoworkConfigFFI {
    pub enabled: bool,
    pub require_confirmation: bool,
    pub max_parallelism: u32,
    pub dry_run: bool,
}

impl From<CoworkConfig> for CoworkConfigFFI {
    fn from(config: CoworkConfig) -> Self {
        Self {
            enabled: config.enabled,
            require_confirmation: config.require_confirmation,
            max_parallelism: config.max_parallelism as u32,
            dry_run: config.dry_run,
        }
    }
}

impl From<CoworkConfigFFI> for CoworkConfig {
    fn from(config: CoworkConfigFFI) -> Self {
        Self {
            enabled: config.enabled,
            require_confirmation: config.require_confirmation,
            max_parallelism: config.max_parallelism as usize,
            dry_run: config.dry_run,
        }
    }
}

/// Code execution configuration for FFI
#[derive(Debug, Clone)]
pub struct CodeExecConfigFFI {
    /// Enable code execution (disabled by default for security)
    pub enabled: bool,
    /// Default runtime (shell, python, node)
    pub default_runtime: String,
    /// Execution timeout in seconds
    pub timeout_seconds: u64,
    /// Enable sandboxed execution
    pub sandbox_enabled: bool,
    /// Allow network access in sandbox
    pub allow_network: bool,
    /// Allowed runtimes (empty = all)
    pub allowed_runtimes: Vec<String>,
    /// Working directory for executions
    pub working_directory: Option<String>,
    /// Environment variables to pass
    pub pass_env: Vec<String>,
    /// Blocked command patterns
    pub blocked_commands: Vec<String>,
}

impl Default for CodeExecConfigFFI {
    fn default() -> Self {
        Self {
            enabled: false,
            default_runtime: "shell".to_string(),
            timeout_seconds: 60,
            sandbox_enabled: true,
            allow_network: false,
            allowed_runtimes: Vec::new(),
            working_directory: None,
            pass_env: vec!["PATH".to_string(), "HOME".to_string(), "USER".to_string()],
            blocked_commands: Vec::new(),
        }
    }
}

impl From<crate::config::types::cowork::CodeExecConfigToml> for CodeExecConfigFFI {
    fn from(config: crate::config::types::cowork::CodeExecConfigToml) -> Self {
        Self {
            enabled: config.enabled,
            default_runtime: config.default_runtime,
            timeout_seconds: config.timeout_seconds,
            sandbox_enabled: config.sandbox_enabled,
            allow_network: config.allow_network,
            allowed_runtimes: config.allowed_runtimes,
            working_directory: config.working_directory,
            pass_env: config.pass_env,
            blocked_commands: config.blocked_commands,
        }
    }
}

impl From<CodeExecConfigFFI> for crate::config::types::cowork::CodeExecConfigToml {
    fn from(config: CodeExecConfigFFI) -> Self {
        Self {
            enabled: config.enabled,
            default_runtime: config.default_runtime,
            timeout_seconds: config.timeout_seconds,
            sandbox_enabled: config.sandbox_enabled,
            allow_network: config.allow_network,
            allowed_runtimes: config.allowed_runtimes,
            working_directory: config.working_directory,
            pass_env: config.pass_env,
            blocked_commands: config.blocked_commands,
        }
    }
}

/// Cowork task for FFI (simplified)
#[derive(Debug, Clone)]
pub struct CoworkTaskFFI {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub task_type: CoworkTaskTypeCategory,
    pub status: CoworkTaskStatusState,
    pub progress: f32,
    pub error_message: Option<String>,
}

impl From<&Task> for CoworkTaskFFI {
    fn from(task: &Task) -> Self {
        let error_message = if let TaskStatus::Failed { error, .. } = &task.status {
            Some(error.clone())
        } else {
            None
        };

        Self {
            id: task.id.clone(),
            name: task.name.clone(),
            description: task.description.clone(),
            task_type: CoworkTaskTypeCategory::from(&task.task_type),
            status: CoworkTaskStatusState::from(&task.status),
            progress: task.progress(),
            error_message,
        }
    }
}

/// Cowork task dependency for FFI
#[derive(Debug, Clone)]
pub struct CoworkTaskDependencyFFI {
    pub from_task_id: String,
    pub to_task_id: String,
}

impl From<&TaskDependency> for CoworkTaskDependencyFFI {
    fn from(dep: &TaskDependency) -> Self {
        Self {
            from_task_id: dep.from.clone(),
            to_task_id: dep.to.clone(),
        }
    }
}

/// Cowork task graph for FFI
#[derive(Debug, Clone)]
pub struct CoworkTaskGraphFFI {
    pub id: String,
    pub title: String,
    pub original_request: Option<String>,
    pub tasks: Vec<CoworkTaskFFI>,
    pub edges: Vec<CoworkTaskDependencyFFI>,
}

impl From<&TaskGraph> for CoworkTaskGraphFFI {
    fn from(graph: &TaskGraph) -> Self {
        Self {
            id: graph.id.clone(),
            title: graph.metadata.title.clone(),
            original_request: graph.metadata.original_request.clone(),
            tasks: graph.tasks.iter().map(CoworkTaskFFI::from).collect(),
            edges: graph.edges.iter().map(CoworkTaskDependencyFFI::from).collect(),
        }
    }
}

/// Cowork execution summary for FFI
#[derive(Debug, Clone)]
pub struct CoworkExecutionSummaryFFI {
    pub graph_id: String,
    pub total_tasks: u32,
    pub completed_tasks: u32,
    pub failed_tasks: u32,
    pub cancelled_tasks: u32,
    pub total_duration_ms: u64,
    pub errors: Vec<String>,
}

impl From<ExecutionSummary> for CoworkExecutionSummaryFFI {
    fn from(summary: ExecutionSummary) -> Self {
        Self {
            graph_id: summary.graph_id,
            total_tasks: summary.total_tasks as u32,
            completed_tasks: summary.completed_tasks as u32,
            failed_tasks: summary.failed_tasks as u32,
            cancelled_tasks: summary.cancelled_tasks as u32,
            total_duration_ms: summary.total_duration.as_millis() as u64,
            errors: summary.errors,
        }
    }
}

/// Cowork progress event for FFI
#[derive(Debug, Clone)]
pub struct CoworkProgressEventFFI {
    pub event_type: CoworkProgressEventType,
    pub task_id: Option<String>,
    pub task_name: Option<String>,
    pub progress: f32,
    pub message: Option<String>,
    pub error: Option<String>,
}

impl From<&ProgressEvent> for CoworkProgressEventFFI {
    fn from(event: &ProgressEvent) -> Self {
        match event {
            ProgressEvent::TaskStarted { task_id, task_name } => Self {
                event_type: CoworkProgressEventType::TaskStarted,
                task_id: Some(task_id.clone()),
                task_name: Some(task_name.clone()),
                progress: 0.0,
                message: None,
                error: None,
            },
            ProgressEvent::Progress {
                task_id,
                progress,
                message,
            } => Self {
                event_type: CoworkProgressEventType::TaskProgress,
                task_id: Some(task_id.clone()),
                task_name: None,
                progress: *progress,
                message: message.clone(),
                error: None,
            },
            ProgressEvent::TaskCompleted {
                task_id,
                task_name,
                ..
            } => Self {
                event_type: CoworkProgressEventType::TaskCompleted,
                task_id: Some(task_id.clone()),
                task_name: Some(task_name.clone()),
                progress: 1.0,
                message: None,
                error: None,
            },
            ProgressEvent::TaskFailed {
                task_id,
                task_name,
                error,
            } => Self {
                event_type: CoworkProgressEventType::TaskFailed,
                task_id: Some(task_id.clone()),
                task_name: Some(task_name.clone()),
                progress: 0.0,
                message: None,
                error: Some(error.clone()),
            },
            ProgressEvent::TaskCancelled { task_id, task_name } => Self {
                event_type: CoworkProgressEventType::TaskCancelled,
                task_id: Some(task_id.clone()),
                task_name: Some(task_name.clone()),
                progress: 0.0,
                message: None,
                error: None,
            },
            ProgressEvent::GraphProgress {
                graph_id,
                overall_progress,
                running_tasks,
                pending_tasks,
            } => Self {
                event_type: CoworkProgressEventType::GraphProgress,
                task_id: Some(graph_id.clone()),
                task_name: None,
                progress: *overall_progress,
                message: Some(format!(
                    "Running: {}, Pending: {}",
                    running_tasks, pending_tasks
                )),
                error: None,
            },
            ProgressEvent::GraphCompleted {
                graph_id,
                total_tasks,
                completed_tasks,
                failed_tasks,
            } => Self {
                event_type: CoworkProgressEventType::GraphCompleted,
                task_id: Some(graph_id.clone()),
                task_name: None,
                progress: 1.0,
                message: Some(format!(
                    "Total: {}, Completed: {}, Failed: {}",
                    total_tasks, completed_tasks, failed_tasks
                )),
                error: None,
            },
        }
    }
}

// ============================================================================
// FFI Callback Interface
// ============================================================================

/// Progress handler callback interface for FFI
pub trait CoworkProgressHandler: Send + Sync {
    /// Called when a progress event occurs
    fn on_progress_event(&self, event: CoworkProgressEventFFI);
}

/// Adapter to bridge FFI callback to internal ProgressSubscriber
pub struct FfiProgressSubscriber {
    handler: Arc<dyn CoworkProgressHandler>,
}

impl FfiProgressSubscriber {
    /// Create a new FFI progress subscriber
    pub fn new(handler: Arc<dyn CoworkProgressHandler>) -> Self {
        Self { handler }
    }
}

impl ProgressSubscriber for FfiProgressSubscriber {
    fn on_event(&self, event: ProgressEvent) {
        let ffi_event = CoworkProgressEventFFI::from(&event);
        self.handler.on_progress_event(ffi_event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cowork::types::{FileOp, TaskResult};
    use std::path::PathBuf;

    #[test]
    fn test_execution_state_conversion() {
        assert_eq!(
            CoworkExecutionState::from(ExecutionState::Idle),
            CoworkExecutionState::Idle
        );
        assert_eq!(
            CoworkExecutionState::from(ExecutionState::Executing),
            CoworkExecutionState::Executing
        );
    }

    #[test]
    fn test_task_status_conversion() {
        assert_eq!(
            CoworkTaskStatusState::from(&TaskStatus::Pending),
            CoworkTaskStatusState::Pending
        );
        assert_eq!(
            CoworkTaskStatusState::from(&TaskStatus::running(0.5)),
            CoworkTaskStatusState::Running
        );
        assert_eq!(
            CoworkTaskStatusState::from(&TaskStatus::completed(TaskResult::default())),
            CoworkTaskStatusState::Completed
        );
        assert_eq!(
            CoworkTaskStatusState::from(&TaskStatus::failed("error")),
            CoworkTaskStatusState::Failed
        );
    }

    #[test]
    fn test_config_conversion() {
        let config = CoworkConfig {
            enabled: true,
            require_confirmation: false,
            max_parallelism: 8,
            dry_run: true,
        };

        let ffi_config = CoworkConfigFFI::from(config.clone());
        assert_eq!(ffi_config.enabled, true);
        assert_eq!(ffi_config.max_parallelism, 8);
        assert_eq!(ffi_config.dry_run, true);

        let converted_back = CoworkConfig::from(ffi_config);
        assert_eq!(converted_back.enabled, config.enabled);
        assert_eq!(converted_back.max_parallelism, config.max_parallelism);
    }

    #[test]
    fn test_task_conversion() {
        let task = Task::new(
            "task_1",
            "Test Task",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        )
        .with_description("A test task");

        let ffi_task = CoworkTaskFFI::from(&task);
        assert_eq!(ffi_task.id, "task_1");
        assert_eq!(ffi_task.name, "Test Task");
        assert_eq!(ffi_task.description, Some("A test task".to_string()));
        assert_eq!(ffi_task.task_type, CoworkTaskTypeCategory::FileOperation);
        assert_eq!(ffi_task.status, CoworkTaskStatusState::Pending);
        assert_eq!(ffi_task.progress, 0.0);
    }

    #[test]
    fn test_graph_conversion() {
        let mut graph = TaskGraph::new("graph_1", "Test Graph");
        graph.metadata.original_request = Some("Do something".to_string());

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

        let ffi_graph = CoworkTaskGraphFFI::from(&graph);
        assert_eq!(ffi_graph.id, "graph_1");
        assert_eq!(ffi_graph.title, "Test Graph");
        assert_eq!(ffi_graph.original_request, Some("Do something".to_string()));
        assert_eq!(ffi_graph.tasks.len(), 2);
        assert_eq!(ffi_graph.edges.len(), 1);
        assert_eq!(ffi_graph.edges[0].from_task_id, "task_1");
        assert_eq!(ffi_graph.edges[0].to_task_id, "task_2");
    }

    #[test]
    fn test_progress_event_conversion() {
        let event = ProgressEvent::TaskStarted {
            task_id: "task_1".to_string(),
            task_name: "Test Task".to_string(),
        };

        let ffi_event = CoworkProgressEventFFI::from(&event);
        assert_eq!(ffi_event.event_type, CoworkProgressEventType::TaskStarted);
        assert_eq!(ffi_event.task_id, Some("task_1".to_string()));
        assert_eq!(ffi_event.task_name, Some("Test Task".to_string()));
    }
}
