//! Session data types returned by database handlers.

use serde::{Deserialize, Serialize};

/// Session information returned by list handlers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Session key string
    pub key: String,
    /// Agent ID
    pub agent_id: String,
    /// Session type (main, peer, task, ephemeral)
    pub session_type: String,
    /// Message count in session
    pub message_count: u32,
    /// Created timestamp (ISO 8601)
    pub created_at: String,
    /// Last activity timestamp (ISO 8601)
    pub last_active_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Session status (e.g. "closed")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Current lifecycle state
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// User-facing label
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Input tokens consumed
    pub input_tokens: u64,
    /// Output tokens consumed
    pub output_tokens: u64,
    /// Model used
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Model provider used
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    /// Parent session key
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_key: Option<String>,
    /// Number of compactions performed
    pub compaction_count: u64,
    /// Originating channel ("telegram", "gui:chat", ...) derived from session
    /// identity metadata. `None` for legacy/unknown-origin sessions. Lets the
    /// Panel distinguish channel-originated conversations from its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
}

/// Session history message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessage {
    /// Message role (user, assistant, system)
    pub role: String,
    /// Message content
    pub content: String,
    /// Timestamp (ISO 8601)
    pub timestamp: String,
    /// Optional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}
