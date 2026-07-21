//! Part ID trait and update types for UI message flow

use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use super::parts::SessionPart;

fn hash_suffix(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Trait for getting unique part ID
pub trait PartId {
    /// Get the unique identifier for this part
    fn part_id(&self) -> String;
}

impl PartId for SessionPart {
    fn part_id(&self) -> String {
        match self {
            Self::UserInput(p) => {
                format!("user_input_{}_{:x}", p.timestamp, hash_suffix(&p.text))
            }
            Self::AiResponse(p) => {
                format!("ai_response_{}_{:x}", p.timestamp, hash_suffix(&p.content))
            }
            Self::ToolCall(p) => p.id.clone(),
            Self::Reasoning(p) => {
                format!("reasoning_{}_{:x}", p.timestamp, hash_suffix(&p.content))
            }
            Self::PlanCreated(p) => p.plan_id.clone(),
            Self::SubAgentCall(p) => {
                format!("subagent_{}_{:x}", p.agent_id, hash_suffix(&p.prompt))
            }
            Self::Summary(p) => {
                format!("summary_{}_{:x}", p.compacted_at, hash_suffix(&p.content))
            }
            Self::CompactionMarker(p) => {
                let trigger = if p.auto { "auto" } else { "manual" };
                format!(
                    "compaction_marker_{}_{:x}",
                    p.timestamp,
                    hash_suffix(trigger)
                )
            }
            Self::SystemReminder(p) => {
                format!("reminder_{}_{:x}", p.timestamp, hash_suffix(&p.content))
            }
            Self::StepStart(p) => format!("step_start_{}", p.step_id),
            Self::StepFinish(p) => format!("step_finish_{}", p.step_id),
            Self::Snapshot(p) => p.snapshot_id.clone(),
            Self::Patch(p) => p.patch_id.clone(),
            Self::StreamingText(p) => p.part_id.clone(),
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
            Self::Added => write!(f, "added"),
            Self::Updated => write!(f, "updated"),
            Self::Removed => write!(f, "removed"),
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
    /// Part type name (e.g., "`tool_call`", "`ai_response`")
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
    /// Create a new `PartUpdateData` for an added part.
    ///
    /// Returns `Err` when the part cannot be serialized to JSON. Callers
    /// previously got a silently-corrupt `part_json = ""` here, which the
    /// UI then misrendered as an empty payload with no diagnostic — better
    /// to surface the error and let the caller drop the event than ship a
    /// misleading payload.
    pub fn added(session_id: &str, part: &SessionPart) -> Result<Self, serde_json::Error> {
        let part_json = serde_json::to_string(part)?;
        Ok(Self {
            session_id: session_id.to_string(),
            part_id: part.part_id(),
            part_type: part.type_name().to_string(),
            event_type: PartEventType::Added,
            part_json,
            delta: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        })
    }

    /// Create a new `PartUpdateData` for an updated part. See
    /// [`Self::added`] for the rationale on `Result` over silent fallback.
    pub fn updated(
        session_id: &str,
        part: &SessionPart,
        delta: Option<String>,
    ) -> Result<Self, serde_json::Error> {
        let part_json = serde_json::to_string(part)?;
        Ok(Self {
            session_id: session_id.to_string(),
            part_id: part.part_id(),
            part_type: part.type_name().to_string(),
            event_type: PartEventType::Updated,
            part_json,
            delta,
            timestamp: chrono::Utc::now().timestamp_millis(),
        })
    }

    /// Create a new `PartUpdateData` for a removed part
    #[must_use]
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
    #[must_use]
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
