//! Agent FFI Types
//!
//! Contains Agent-related FFI types:
//! - AgentExecutionState, AgentTaskStatusState, AgentTaskTypeCategory, AgentProgressEventType
//! - AgentTaskFFI, AgentTaskDependencyFFI, AgentTaskGraphFFI
//! - AgentExecutionSummaryFFI, AgentProgressEventFFI
//! - AgentProgressHandler trait, FfiProgressSubscriber

use std::sync::Arc;

use crate::dispatcher::agent_types::{
    ExecutionSummary, Task, TaskDependency, TaskGraph, TaskStatus, TaskType,
};
use crate::dispatcher::monitor::{ProgressEvent, ProgressSubscriber};
use crate::dispatcher::ExecutionState;

// ============================================================================
// FFI Enums
// ============================================================================

/// Execution state for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentExecutionState {
    Idle,
    Planning,
    AwaitingConfirmation,
    Executing,
    Paused,
    Cancelled,
    Completed,
}

impl From<ExecutionState> for AgentExecutionState {
    fn from(state: ExecutionState) -> Self {
        match state {
            ExecutionState::Idle => AgentExecutionState::Idle,
            ExecutionState::Planning => AgentExecutionState::Planning,
            ExecutionState::AwaitingConfirmation => AgentExecutionState::AwaitingConfirmation,
            ExecutionState::Executing => AgentExecutionState::Executing,
            ExecutionState::Paused => AgentExecutionState::Paused,
            ExecutionState::Cancelled => AgentExecutionState::Cancelled,
            ExecutionState::Completed => AgentExecutionState::Completed,
        }
    }
}

/// Task status state for FFI (simplified)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTaskStatusState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl From<&TaskStatus> for AgentTaskStatusState {
    fn from(status: &TaskStatus) -> Self {
        match status {
            TaskStatus::Pending => AgentTaskStatusState::Pending,
            TaskStatus::Running { .. } => AgentTaskStatusState::Running,
            TaskStatus::Completed { .. } => AgentTaskStatusState::Completed,
            TaskStatus::Failed { .. } => AgentTaskStatusState::Failed,
            TaskStatus::Cancelled => AgentTaskStatusState::Cancelled,
        }
    }
}

/// Task type category for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTaskTypeCategory {
    FileOperation,
    CodeExecution,
    DocumentGeneration,
    AppAutomation,
    AiInference,
    ImageGeneration,
    VideoGeneration,
    AudioGeneration,
}

impl From<&TaskType> for AgentTaskTypeCategory {
    fn from(task_type: &TaskType) -> Self {
        match task_type {
            TaskType::FileOperation(_) => AgentTaskTypeCategory::FileOperation,
            TaskType::CodeExecution(_) => AgentTaskTypeCategory::CodeExecution,
            TaskType::DocumentGeneration(_) => AgentTaskTypeCategory::DocumentGeneration,
            TaskType::AppAutomation(_) => AgentTaskTypeCategory::AppAutomation,
            TaskType::AiInference(_) => AgentTaskTypeCategory::AiInference,
            TaskType::ImageGeneration(_) => AgentTaskTypeCategory::ImageGeneration,
            TaskType::VideoGeneration(_) => AgentTaskTypeCategory::VideoGeneration,
            TaskType::AudioGeneration(_) => AgentTaskTypeCategory::AudioGeneration,
        }
    }
}

/// Progress event type for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentProgressEventType {
    TaskStarted,
    TaskProgress,
    TaskCompleted,
    TaskFailed,
    TaskCancelled,
    GraphProgress,
    GraphCompleted,
}

// ============================================================================
// Task FFI Structs
// ============================================================================

/// Cowork task for FFI (simplified)
#[derive(Debug, Clone)]
pub struct AgentTaskFFI {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub task_type: AgentTaskTypeCategory,
    pub status: AgentTaskStatusState,
    pub progress: f32,
    pub error_message: Option<String>,
}

impl From<&Task> for AgentTaskFFI {
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
            task_type: AgentTaskTypeCategory::from(&task.task_type),
            status: AgentTaskStatusState::from(&task.status),
            progress: task.progress(),
            error_message,
        }
    }
}

/// Cowork task dependency for FFI
#[derive(Debug, Clone)]
pub struct AgentTaskDependencyFFI {
    pub from_task_id: String,
    pub to_task_id: String,
}

impl From<&TaskDependency> for AgentTaskDependencyFFI {
    fn from(dep: &TaskDependency) -> Self {
        Self {
            from_task_id: dep.from.clone(),
            to_task_id: dep.to.clone(),
        }
    }
}

/// Cowork task graph for FFI
#[derive(Debug, Clone)]
pub struct AgentTaskGraphFFI {
    pub id: String,
    pub title: String,
    pub original_request: Option<String>,
    pub tasks: Vec<AgentTaskFFI>,
    pub edges: Vec<AgentTaskDependencyFFI>,
}

impl From<&TaskGraph> for AgentTaskGraphFFI {
    fn from(graph: &TaskGraph) -> Self {
        Self {
            id: graph.id.clone(),
            title: graph.metadata.title.clone(),
            original_request: graph.metadata.original_request.clone(),
            tasks: graph.tasks.iter().map(AgentTaskFFI::from).collect(),
            edges: graph.edges.iter().map(AgentTaskDependencyFFI::from).collect(),
        }
    }
}

/// Cowork execution summary for FFI
#[derive(Debug, Clone)]
pub struct AgentExecutionSummaryFFI {
    pub graph_id: String,
    pub total_tasks: u32,
    pub completed_tasks: u32,
    pub failed_tasks: u32,
    pub cancelled_tasks: u32,
    pub total_duration_ms: u64,
    pub errors: Vec<String>,
}

impl From<&ExecutionSummary> for AgentExecutionSummaryFFI {
    fn from(summary: &ExecutionSummary) -> Self {
        Self {
            graph_id: summary.graph_id.clone(),
            total_tasks: summary.total_tasks as u32,
            completed_tasks: summary.completed_tasks as u32,
            failed_tasks: summary.failed_tasks as u32,
            cancelled_tasks: summary.cancelled_tasks as u32,
            total_duration_ms: summary.total_duration.as_millis() as u64,
            errors: summary.errors.clone(),
        }
    }
}

impl From<ExecutionSummary> for AgentExecutionSummaryFFI {
    fn from(summary: ExecutionSummary) -> Self {
        AgentExecutionSummaryFFI::from(&summary)
    }
}

/// Cowork progress event for FFI
#[derive(Debug, Clone)]
pub struct AgentProgressEventFFI {
    pub event_type: AgentProgressEventType,
    pub task_id: Option<String>,
    pub task_name: Option<String>,
    pub progress: f32,
    pub message: Option<String>,
    pub error: Option<String>,
}

impl From<&ProgressEvent> for AgentProgressEventFFI {
    fn from(event: &ProgressEvent) -> Self {
        match event {
            ProgressEvent::TaskStarted { task_id, task_name } => Self {
                event_type: AgentProgressEventType::TaskStarted,
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
                event_type: AgentProgressEventType::TaskProgress,
                task_id: Some(task_id.clone()),
                task_name: None,
                progress: *progress,
                message: message.clone(),
                error: None,
            },
            ProgressEvent::TaskCompleted {
                task_id,
                task_name,
                result: _,
            } => Self {
                event_type: AgentProgressEventType::TaskCompleted,
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
                event_type: AgentProgressEventType::TaskFailed,
                task_id: Some(task_id.clone()),
                task_name: Some(task_name.clone()),
                progress: 0.0,
                message: None,
                error: Some(error.clone()),
            },
            ProgressEvent::TaskCancelled { task_id, task_name } => Self {
                event_type: AgentProgressEventType::TaskCancelled,
                task_id: Some(task_id.clone()),
                task_name: Some(task_name.clone()),
                progress: 0.0,
                message: None,
                error: None,
            },
            ProgressEvent::GraphProgress {
                graph_id: _,
                overall_progress,
                running_tasks: _,
                pending_tasks: _,
            } => Self {
                event_type: AgentProgressEventType::GraphProgress,
                task_id: None,
                task_name: None,
                progress: *overall_progress,
                message: None,
                error: None,
            },
            ProgressEvent::GraphCompleted {
                graph_id: _,
                total_tasks: _,
                completed_tasks: _,
                failed_tasks: _,
            } => Self {
                event_type: AgentProgressEventType::GraphCompleted,
                task_id: None,
                task_name: None,
                progress: 1.0,
                message: None,
                error: None,
            },
        }
    }
}

// ============================================================================
// FFI Callback Interface
// ============================================================================

/// Callback interface for progress events (UniFFI callback interface)
pub trait AgentProgressHandler: Send + Sync {
    fn on_progress_event(&self, event: AgentProgressEventFFI);
}

/// Wrapper to convert FFI callback to internal ProgressSubscriber
pub struct FfiProgressSubscriber {
    handler: Arc<dyn AgentProgressHandler>,
}

impl FfiProgressSubscriber {
    pub fn new(handler: Arc<dyn AgentProgressHandler>) -> Self {
        Self { handler }
    }
}

impl ProgressSubscriber for FfiProgressSubscriber {
    fn on_event(&self, event: ProgressEvent) {
        self.handler
            .on_progress_event(AgentProgressEventFFI::from(&event));
    }
}
