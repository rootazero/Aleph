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
    /// Per-session reasoning-depth override
    /// (`identity_meta.custom["think_level"]`).
    ///
    /// The third twin, and the one that was missing: `turn_thinking` has
    /// persisted this since it was written — a request-carried level is stamped
    /// onto the session so later turns read it back — but no client surface
    /// reported it, so no pill could show it and no terminal could restore it.
    /// A knob that is enforced but unreadable looks exactly like a knob nobody
    /// set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub think_level: Option<String>,
    /// Per-session memory mode (`identity_meta.custom["memory_mode"]`,
    /// `"on"` / `"off"`). `None` ⇒ follow `[memory] enabled`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_mode: Option<String>,
    /// Model this conversation is pinned to by `select_model`
    /// (`identity_meta.custom["model_pin"]`), if any.
    ///
    /// Distinct from [`Self::model`], which records what last *served*: a pick
    /// applies from the next run, so for one turn the two disagree and a
    /// surface showing only `model` names the model the user just left.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_pin: Option<String>,
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
