use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
    pub metadata: Option<Value>,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub key: String,
    pub agent_id: String,
    pub session_type: String,
    pub created_at: i64,
    pub last_active_at: i64,
    pub message_count: i64,
    pub total_tokens: i64,
    pub auto_reset_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<crate::gateway::session_manager::SessionState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_meta: Option<crate::gateway::session_manager::SessionIdentityMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_key: Option<String>,
    #[serde(default)]
    pub compaction_count: i64,
    /// Derived title from first user message (computed lazily on append).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derived_title: Option<String>,
    /// Preview of the last message content (first N chars, updated on append).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_preview: Option<String>,
    /// Cumulative runtime in milliseconds (updated on session close).
    #[serde(default)]
    pub runtime_ms: i64,
    /// Estimated cost in USD (updated on session close / usage update).
    #[serde(default)]
    pub estimated_cost_usd: f64,
    /// List of compaction checkpoints (file backend only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checkpoints: Vec<CheckpointSummary>,
}

impl SessionMetadata {
    /// Parse topic and status from a raw metadata JSON string for backward compatibility.
    pub fn parse_legacy_metadata_json(
        json: Option<&str>,
    ) -> (
        Option<String>,
        Option<String>,
        Option<crate::gateway::session_manager::SessionIdentityMeta>,
    ) {
        let Some(s) = json else {
            return (None, None, None);
        };
        if let Ok(identity) =
            serde_json::from_str::<crate::gateway::session_manager::SessionIdentityMeta>(s)
        {
            let topic = identity
                .custom
                .get("topic")
                .and_then(|v| v.as_str())
                .map(String::from);
            let status = identity
                .custom
                .get("status")
                .and_then(|v| v.as_str())
                .map(String::from);
            (topic, status, Some(identity))
        } else if let Ok(val) = serde_json::from_str::<Value>(s) {
            let topic = val.get("topic").and_then(|v| v.as_str()).map(String::from);
            let status = val.get("status").and_then(|v| v.as_str()).map(String::from);
            (topic, status, None)
        } else {
            (None, None, None)
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionPreview {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<SessionMetadata>,
    pub messages: Vec<MessageRecord>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    pub agent_id: Option<String>,
    pub limit: Option<usize>,
    pub active_minutes: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct DeleteResult {
    pub deleted: bool,
}

#[derive(Debug, Clone)]
pub enum CompactStrategy {
    KeepLastN { n: usize },
}

#[derive(Debug, Clone)]
pub struct CompactResult {
    pub compacted: bool,
    pub deleted: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointSummary {
    pub checkpoint_id: String,
    pub created_at: i64,
    #[serde(default)]
    pub message_count: i64,
    #[serde(default)]
    pub retained_message_count: i64,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub session_key: String,
    pub agent_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
    pub topic: Option<String>,
}

/// Event payload broadcast when a session changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionChangedEvent {
    pub session_key: String,
    pub reason: String,
    pub ts: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub total_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default)]
    pub compacted: bool,
}
