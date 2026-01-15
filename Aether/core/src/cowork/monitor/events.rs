//! Progress event definitions

use serde::{Deserialize, Serialize};

use crate::cowork::types::TaskResult;

/// Progress events emitted during task execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProgressEvent {
    /// A task has started executing
    TaskStarted {
        task_id: String,
        task_name: String,
    },

    /// Task progress has been updated
    Progress {
        task_id: String,
        progress: f32,
        message: Option<String>,
    },

    /// A task has completed successfully
    TaskCompleted {
        task_id: String,
        task_name: String,
        result: TaskResult,
    },

    /// A task has failed
    TaskFailed {
        task_id: String,
        task_name: String,
        error: String,
    },

    /// A task was cancelled
    TaskCancelled {
        task_id: String,
        task_name: String,
    },

    /// The entire task graph has completed
    GraphCompleted {
        graph_id: String,
        total_tasks: usize,
        completed_tasks: usize,
        failed_tasks: usize,
    },

    /// Overall progress update for the graph
    GraphProgress {
        graph_id: String,
        overall_progress: f32,
        running_tasks: usize,
        pending_tasks: usize,
    },
}

impl ProgressEvent {
    /// Create a task started event
    pub fn task_started(task_id: impl Into<String>, task_name: impl Into<String>) -> Self {
        Self::TaskStarted {
            task_id: task_id.into(),
            task_name: task_name.into(),
        }
    }

    /// Create a progress event
    pub fn progress(task_id: impl Into<String>, progress: f32) -> Self {
        Self::Progress {
            task_id: task_id.into(),
            progress,
            message: None,
        }
    }

    /// Create a progress event with message
    pub fn progress_with_message(
        task_id: impl Into<String>,
        progress: f32,
        message: impl Into<String>,
    ) -> Self {
        Self::Progress {
            task_id: task_id.into(),
            progress,
            message: Some(message.into()),
        }
    }

    /// Create a task completed event
    pub fn task_completed(
        task_id: impl Into<String>,
        task_name: impl Into<String>,
        result: TaskResult,
    ) -> Self {
        Self::TaskCompleted {
            task_id: task_id.into(),
            task_name: task_name.into(),
            result,
        }
    }

    /// Create a task failed event
    pub fn task_failed(
        task_id: impl Into<String>,
        task_name: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self::TaskFailed {
            task_id: task_id.into(),
            task_name: task_name.into(),
            error: error.into(),
        }
    }

    /// Create a task cancelled event
    pub fn task_cancelled(task_id: impl Into<String>, task_name: impl Into<String>) -> Self {
        Self::TaskCancelled {
            task_id: task_id.into(),
            task_name: task_name.into(),
        }
    }

    /// Create a graph completed event
    pub fn graph_completed(
        graph_id: impl Into<String>,
        total_tasks: usize,
        completed_tasks: usize,
        failed_tasks: usize,
    ) -> Self {
        Self::GraphCompleted {
            graph_id: graph_id.into(),
            total_tasks,
            completed_tasks,
            failed_tasks,
        }
    }

    /// Create a graph progress event
    pub fn graph_progress(
        graph_id: impl Into<String>,
        overall_progress: f32,
        running_tasks: usize,
        pending_tasks: usize,
    ) -> Self {
        Self::GraphProgress {
            graph_id: graph_id.into(),
            overall_progress,
            running_tasks,
            pending_tasks,
        }
    }

    /// Get the task ID if this is a task-related event
    pub fn task_id(&self) -> Option<&str> {
        match self {
            Self::TaskStarted { task_id, .. } => Some(task_id),
            Self::Progress { task_id, .. } => Some(task_id),
            Self::TaskCompleted { task_id, .. } => Some(task_id),
            Self::TaskFailed { task_id, .. } => Some(task_id),
            Self::TaskCancelled { task_id, .. } => Some(task_id),
            Self::GraphCompleted { .. } | Self::GraphProgress { .. } => None,
        }
    }

    /// Get the graph ID if this is a graph-related event
    pub fn graph_id(&self) -> Option<&str> {
        match self {
            Self::GraphCompleted { graph_id, .. } => Some(graph_id),
            Self::GraphProgress { graph_id, .. } => Some(graph_id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_creation() {
        let event = ProgressEvent::task_started("task_1", "Test Task");
        assert_eq!(event.task_id(), Some("task_1"));

        let event = ProgressEvent::progress("task_1", 0.5);
        if let ProgressEvent::Progress { progress, .. } = event {
            assert_eq!(progress, 0.5);
        } else {
            panic!("Wrong event type");
        }
    }

    #[test]
    fn test_event_serialization() {
        let event = ProgressEvent::task_started("task_1", "Test Task");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("task_started"));

        let parsed: ProgressEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.task_id(), Some("task_1"));
    }
}
