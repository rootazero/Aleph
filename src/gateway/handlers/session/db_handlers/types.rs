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
    /// Last-active time as Unix epoch seconds. The Panel expects this (its
    /// SessionEntry.updated_at) for time-grouping & sort; previously only the
    /// RFC3339 `last_active_at` string was sent, so Panel sort/subtitle were dead.
    pub updated_at: i64,
    /// User-chosen project working directory persisted on the session
    /// (`identity_meta.custom["project_root"]`). `None` ⇒ the default
    /// `~/.aleph/workspaces/{agent_id}` workspace. The Panel restores this into
    /// `active_project_root` when the session is reselected after a reload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_root: Option<String>,
    /// Per-session execution-tier override (`identity_meta.custom["exec_tier"]`,
    /// written through `sessions.patch`). `None` ⇒ the session follows the global
    /// `[policies.exec_tier]`. The run loop keeps enforcing a stored tier across
    /// reloads, so the Panel must be able to read it back — without this carrier
    /// the composer's tier pill silently under-reports the gate that is actually
    /// live on the session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exec_tier: Option<String>,
    /// Per-session usage-mode override (`identity_meta.custom["session_mode"]`,
    /// written through `sessions.patch` or stamped from a request-carried
    /// value). `None` ⇒ the session follows the global `[policies] mode`. Same
    /// read-back contract as `exec_tier`: the Panel's mode pill restores it on
    /// session reselect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Plan phase (`identity_meta.custom["plan_phase"]`). `None` ⇒ `building`,
    /// which is where every session that never asked to plan already is — so
    /// absent and `"building"` mean the same thing, and only the interesting
    /// value is ever on the wire.
    ///
    /// A client MUST read this back rather than remembering what it sent: an
    /// approved handoff writes `building` onto the session from the server
    /// side, mid-conversation, with no request of the client's involved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_phase: Option<String>,
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
