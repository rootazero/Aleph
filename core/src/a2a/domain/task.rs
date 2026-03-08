use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Map;

use super::message::{A2AMessage, Artifact};
use crate::domain::{AggregateRoot, Entity};

/// A2A task state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    #[serde(rename = "submitted")]
    Submitted,
    #[serde(rename = "working")]
    Working,
    #[serde(rename = "input-required")]
    InputRequired,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "canceled")]
    Canceled,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "rejected")]
    Rejected,
    #[serde(rename = "auth-required")]
    AuthRequired,
}

impl TaskState {
    /// Returns true if the task is in a terminal state (no further transitions)
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Canceled | Self::Failed | Self::Rejected
        )
    }

    /// Returns true if the task can be canceled from this state
    pub fn is_cancelable(&self) -> bool {
        matches!(self, Self::Submitted | Self::Working | Self::InputRequired)
    }
}

/// Current status of a task
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<A2AMessage>,
    pub timestamp: DateTime<Utc>,
}

/// A2A Task — the aggregate root of the A2A bounded context
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2ATask {
    pub id: String,
    pub context_id: String,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<A2AMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, serde_json::Value>>,
    pub kind: String,
}

impl Entity for A2ATask {
    type Id = String;
    fn id(&self) -> &Self::Id {
        &self.id
    }
}

impl AggregateRoot for A2ATask {}

impl A2ATask {
    /// Create a new task with default Submitted state
    pub fn new(id: impl Into<String>, context_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            context_id: context_id.into(),
            status: TaskStatus {
                state: TaskState::Submitted,
                message: None,
                timestamp: Utc::now(),
            },
            artifacts: Vec::new(),
            history: Vec::new(),
            metadata: None,
            kind: "task".to_string(),
        }
    }
}

/// Parameters for listing tasks
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTasksParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_filter: Option<Vec<TaskState>>,
}

/// Result of listing tasks
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTasksResult {
    pub tasks: Vec<A2ATask>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_state_is_terminal() {
        assert!(!TaskState::Submitted.is_terminal());
        assert!(!TaskState::Working.is_terminal());
        assert!(!TaskState::InputRequired.is_terminal());
        assert!(TaskState::Completed.is_terminal());
        assert!(TaskState::Canceled.is_terminal());
        assert!(TaskState::Failed.is_terminal());
        assert!(TaskState::Rejected.is_terminal());
        assert!(!TaskState::AuthRequired.is_terminal());
    }

    #[test]
    fn task_state_is_cancelable() {
        assert!(TaskState::Submitted.is_cancelable());
        assert!(TaskState::Working.is_cancelable());
        assert!(TaskState::InputRequired.is_cancelable());
        assert!(!TaskState::Completed.is_cancelable());
        assert!(!TaskState::Canceled.is_cancelable());
        assert!(!TaskState::Failed.is_cancelable());
        assert!(!TaskState::Rejected.is_cancelable());
        assert!(!TaskState::AuthRequired.is_cancelable());
    }

    #[test]
    fn task_state_serde_roundtrip() {
        let states = [
            TaskState::Submitted,
            TaskState::Working,
            TaskState::InputRequired,
            TaskState::Completed,
            TaskState::Canceled,
            TaskState::Failed,
            TaskState::Rejected,
            TaskState::AuthRequired,
        ];
        for state in states {
            let json = serde_json::to_string(&state).unwrap();
            let back: TaskState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, back);
        }
    }

    #[test]
    fn task_state_json_values() {
        assert_eq!(
            serde_json::to_string(&TaskState::Submitted).unwrap(),
            "\"submitted\""
        );
        assert_eq!(
            serde_json::to_string(&TaskState::InputRequired).unwrap(),
            "\"input-required\""
        );
        assert_eq!(
            serde_json::to_string(&TaskState::AuthRequired).unwrap(),
            "\"auth-required\""
        );
    }

    #[test]
    fn a2a_task_new_defaults() {
        let task = A2ATask::new("task-123", "ctx-456");
        assert_eq!(task.id, "task-123");
        assert_eq!(task.context_id, "ctx-456");
        assert_eq!(task.status.state, TaskState::Submitted);
        assert!(task.status.message.is_none());
        assert!(task.artifacts.is_empty());
        assert!(task.history.is_empty());
        assert!(task.metadata.is_none());
        assert_eq!(task.kind, "task");
    }

    #[test]
    fn a2a_task_entity_trait() {
        let task = A2ATask::new("task-abc", "ctx-def");
        assert_eq!(task.id(), "task-abc");
    }

    #[test]
    fn a2a_task_serde_roundtrip() {
        let task = A2ATask::new("task-1", "ctx-1");
        let json = serde_json::to_string(&task).unwrap();
        let back: A2ATask = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "task-1");
        assert_eq!(back.context_id, "ctx-1");
        assert_eq!(back.kind, "task");
        assert_eq!(back.status.state, TaskState::Submitted);
    }

    #[test]
    fn a2a_task_camelcase_fields() {
        let task = A2ATask::new("t1", "c1");
        let json = serde_json::to_value(&task).unwrap();
        assert!(json.get("id").is_some());
        assert!(json.get("contextId").is_some());
        assert!(json.get("status").is_some());
        assert!(json.get("kind").is_some());
    }

    #[test]
    fn list_tasks_params_default() {
        let params = ListTasksParams::default();
        assert!(params.context_id.is_none());
        assert!(params.limit.is_none());
        assert!(params.cursor.is_none());
        assert!(params.state_filter.is_none());
    }

    #[test]
    fn list_tasks_params_serde() {
        let params = ListTasksParams {
            context_id: Some("ctx-1".to_string()),
            limit: Some(10),
            cursor: None,
            state_filter: Some(vec![TaskState::Working, TaskState::Submitted]),
        };
        let json = serde_json::to_string(&params).unwrap();
        let back: ListTasksParams = serde_json::from_str(&json).unwrap();
        assert_eq!(back.context_id.unwrap(), "ctx-1");
        assert_eq!(back.limit.unwrap(), 10);
        assert_eq!(back.state_filter.unwrap().len(), 2);
    }

    #[test]
    fn list_tasks_result_serde() {
        let result = ListTasksResult {
            tasks: vec![A2ATask::new("t1", "c1")],
            next_cursor: Some("cursor-abc".to_string()),
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: ListTasksResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tasks.len(), 1);
        assert_eq!(back.next_cursor.unwrap(), "cursor-abc");
    }
}
