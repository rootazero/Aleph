//! Part ID trait and update types for UI message flow

use serde::{Deserialize, Serialize};

use super::parts::SessionPart;

/// Trait for getting unique part ID
pub trait PartId {
    /// Get the unique identifier for this part
    fn part_id(&self) -> String;
}

impl PartId for SessionPart {
    fn part_id(&self) -> String {
        match self {
            SessionPart::UserInput(p) => format!("user_input_{}", p.timestamp),
            SessionPart::AiResponse(p) => format!("ai_response_{}", p.timestamp),
            SessionPart::ToolCall(p) => p.id.clone(),
            SessionPart::Reasoning(p) => format!("reasoning_{}", p.timestamp),
            SessionPart::PlanCreated(p) => p.plan_id.clone(),
            SessionPart::SubAgentCall(p) => format!("subagent_{}", p.agent_id),
            SessionPart::Summary(p) => format!("summary_{}", p.compacted_at),
            SessionPart::CompactionMarker(p) => format!("compaction_marker_{}", p.timestamp),
            SessionPart::SystemReminder(p) => format!("reminder_{}", p.timestamp),
            SessionPart::StepStart(p) => format!("step_start_{}", p.step_id),
            SessionPart::StepFinish(p) => format!("step_finish_{}", p.step_id),
            SessionPart::Snapshot(p) => p.snapshot_id.clone(),
            SessionPart::Patch(p) => p.patch_id.clone(),
            SessionPart::StreamingText(p) => p.part_id.clone(),
        }
    }
}

/// Part event type for UI updates
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartEventType {
    /// Part was added to the session
    Added,
    /// Part was updated (e.g., tool call status changed)
    Updated,
    /// Part was removed (e.g., compaction)
    Removed,
}

impl std::fmt::Display for PartEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PartEventType::Added => write!(f, "added"),
            PartEventType::Updated => write!(f, "updated"),
            PartEventType::Removed => write!(f, "removed"),
        }
    }
}

/// Part update event data for UI rendering
///
/// This structure contains all information needed for the UI to render
/// a part update (add, update, or remove).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartUpdateData {
    /// Session ID this part belongs to
    pub session_id: String,
    /// Unique part identifier
    pub part_id: String,
    /// Part type name (e.g., "tool_call", "ai_response")
    pub part_type: String,
    /// Event type (Added, Updated, Removed)
    pub event_type: PartEventType,
    /// Serialized part data as JSON
    pub part_json: String,
    /// Delta content for streaming updates (text chunks)
    pub delta: Option<String>,
    /// Timestamp when the event occurred
    pub timestamp: i64,
}

impl PartUpdateData {
    /// Create a new PartUpdateData for an added part
    pub fn added(session_id: &str, part: &SessionPart) -> Self {
        Self {
            session_id: session_id.to_string(),
            part_id: part.part_id(),
            part_type: part.type_name().to_string(),
            event_type: PartEventType::Added,
            part_json: serde_json::to_string(part).unwrap_or_default(),
            delta: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Create a new PartUpdateData for an updated part
    pub fn updated(session_id: &str, part: &SessionPart, delta: Option<String>) -> Self {
        Self {
            session_id: session_id.to_string(),
            part_id: part.part_id(),
            part_type: part.type_name().to_string(),
            event_type: PartEventType::Updated,
            part_json: serde_json::to_string(part).unwrap_or_default(),
            delta,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Create a new PartUpdateData for a removed part
    pub fn removed(session_id: &str, part_id: &str, part_type: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            part_id: part_id.to_string(),
            part_type: part_type.to_string(),
            event_type: PartEventType::Removed,
            part_json: String::new(),
            delta: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Create update for streaming text delta
    pub fn text_delta(session_id: &str, part_id: &str, part_type: &str, delta: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            part_id: part_id.to_string(),
            part_type: part_type.to_string(),
            event_type: PartEventType::Updated,
            part_json: String::new(),
            delta: Some(delta.to_string()),
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }
}
