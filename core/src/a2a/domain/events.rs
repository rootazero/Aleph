use serde::{Deserialize, Serialize};
use serde_json::Map;

use super::message::Artifact;
use super::task::TaskStatus;

/// Event emitted when a task's status changes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusUpdateEvent {
    pub task_id: String,
    pub context_id: String,
    pub status: TaskStatus,
    #[serde(rename = "final")]
    pub is_final: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, serde_json::Value>>,
}

/// Event emitted when a task produces or updates an artifact
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskArtifactUpdateEvent {
    pub task_id: String,
    pub context_id: String,
    pub artifact: Artifact,
    #[serde(default)]
    pub append: bool,
    #[serde(default)]
    pub last_chunk: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, serde_json::Value>>,
}

/// Unified update event — tagged by kind
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum UpdateEvent {
    StatusUpdate(TaskStatusUpdateEvent),
    ArtifactUpdate(TaskArtifactUpdateEvent),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::domain::message::Part;
    use crate::a2a::domain::task::TaskState;
    use chrono::Utc;

    #[test]
    fn status_update_event_serde_roundtrip() {
        let event = TaskStatusUpdateEvent {
            task_id: "task-1".to_string(),
            context_id: "ctx-1".to_string(),
            status: TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: Utc::now(),
            },
            is_final: false,
            metadata: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: TaskStatusUpdateEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.task_id, "task-1");
        assert!(!back.is_final);
    }

    #[test]
    fn status_update_final_field_renamed() {
        let event = TaskStatusUpdateEvent {
            task_id: "t1".to_string(),
            context_id: "c1".to_string(),
            status: TaskStatus {
                state: TaskState::Completed,
                message: None,
                timestamp: Utc::now(),
            },
            is_final: true,
            metadata: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        // "is_final" should be serialized as "final"
        assert_eq!(json["final"], true);
        assert!(json.get("is_final").is_none());
    }

    #[test]
    fn artifact_update_event_serde_roundtrip() {
        let event = TaskArtifactUpdateEvent {
            task_id: "task-1".to_string(),
            context_id: "ctx-1".to_string(),
            artifact: Artifact {
                artifact_id: "art-1".to_string(),
                kind: "code".to_string(),
                parts: vec![Part::Text {
                    text: "fn main() {}".to_string(),
                    metadata: None,
                }],
                metadata: None,
            },
            append: false,
            last_chunk: true,
            metadata: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: TaskArtifactUpdateEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.task_id, "task-1");
        assert!(back.last_chunk);
        assert!(!back.append);
    }

    #[test]
    fn update_event_tagged_enum() {
        let status_event = UpdateEvent::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "t1".to_string(),
            context_id: "c1".to_string(),
            status: TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: Utc::now(),
            },
            is_final: false,
            metadata: None,
        });
        let json = serde_json::to_value(&status_event).unwrap();
        assert_eq!(json["kind"], "status-update");

        let artifact_event = UpdateEvent::ArtifactUpdate(TaskArtifactUpdateEvent {
            task_id: "t1".to_string(),
            context_id: "c1".to_string(),
            artifact: Artifact {
                artifact_id: "a1".to_string(),
                kind: "text".to_string(),
                parts: vec![],
                metadata: None,
            },
            append: false,
            last_chunk: false,
            metadata: None,
        });
        let json = serde_json::to_value(&artifact_event).unwrap();
        assert_eq!(json["kind"], "artifact-update");
    }
}
