use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
    pub metadata: Option<Value>,
    /// Tokens the LLM call that produced this message was billed for. Zero on
    /// user / tool / system rows, which no call produced.
    ///
    /// (There were `model` / `model_provider` fields here too. The `messages`
    /// table has never had those columns — no INSERT wrote them and every SELECT
    /// filled them with `None` — so they were struct-shaped decoration. The
    /// serving model is recorded where it has a column: `sessions.model`.)
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
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

    /// Origin channel of this session derived from identity metadata.
    ///
    /// Returns `None` for the synthetic `""`/`"unknown"` sentinel so callers
    /// (`sessions.list`, `sessions.changed`) omit a meaningless origin badge.
    /// Single source of truth for the "what counts as a real origin" rule,
    /// shared by the `SessionInfo` builder and the session-changed event.
    #[must_use]
    pub fn origin_channel(&self) -> Option<String> {
        let im = self.identity_meta.as_ref()?;
        let c = im.source_channel.trim();
        (!c.is_empty() && c != "unknown").then(|| c.to_string())
    }

    /// Origin conversation id captured alongside the origin channel on the
    /// first inbound message (e.g. the Telegram chat id). Drives cross-surface
    /// reply fan-out (sub-gap (b)): a run continued from the Panel can deliver
    /// its final reply back to `(origin_channel, origin_conversation)`.
    pub fn origin_conversation(&self) -> Option<String> {
        self.identity_meta
            .as_ref()?
            .custom
            .get(ORIGIN_CONVERSATION_KEY)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    }
}

/// Identity-metadata custom key under which a session's origin conversation id
/// is persisted. Written by `SessionManager::set_source_channel`, read by
/// `SessionMetadata::origin_conversation`.
pub const ORIGIN_CONVERSATION_KEY: &str = "origin_conversation";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_record_tool_fields_default_none_and_roundtrip() {
        // Old JSON (no tool fields) deserializes → None
        let legacy =
            r#"{"id":"1","role":"assistant","content":"hi","timestamp":1,"metadata":null}"#;
        let rec: MessageRecord = serde_json::from_str(legacy).unwrap();
        assert!(rec.tool_call_id.is_none());
        assert!(rec.tool_name.is_none());
        // With tool fields round-trip
        let tool = MessageRecord {
            id: "2".into(),
            role: "tool".into(),
            content: "{}".into(),
            timestamp: 2,
            metadata: None,
            input_tokens: 0,
            output_tokens: 0,
            tool_call_id: Some("call_1".into()),
            tool_name: Some("bash_exec".into()),
        };
        let back: MessageRecord =
            serde_json::from_str(&serde_json::to_string(&tool).unwrap()).unwrap();
        assert_eq!(back.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(back.tool_name.as_deref(), Some("bash_exec"));
    }
}

#[cfg(test)]
mod origin_channel_tests {
    use super::*;
    use crate::gateway::session_manager::SessionIdentityMeta;

    fn meta_with_channel(channel: &str) -> SessionMetadata {
        SessionMetadata {
            identity_meta: Some(SessionIdentityMeta::owner(channel)),
            ..Default::default()
        }
    }

    #[test]
    fn origin_channel_none_when_no_identity() {
        assert_eq!(SessionMetadata::default().origin_channel(), None);
    }

    #[test]
    fn origin_channel_none_for_unknown_sentinel() {
        assert_eq!(meta_with_channel("unknown").origin_channel(), None);
        assert_eq!(meta_with_channel("  ").origin_channel(), None);
    }

    #[test]
    fn origin_channel_some_for_real_channel() {
        assert_eq!(
            meta_with_channel("telegram").origin_channel(),
            Some("telegram".to_string())
        );
        assert_eq!(
            meta_with_channel("gui:chat").origin_channel(),
            Some("gui:chat".to_string())
        );
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

/// Default retention for the user-facing `/compact` command and the
/// `session.compact` RPC: keep this many most-recent messages, dropping the
/// older ones. The transcript is a working buffer — the memory layer's
/// hierarchical summaries (`SessionCompactor::post_turn_compress`) already
/// distilled the dropped turns into recallable notes, so this trim is not a
/// context loss. Single-sourced so the `session_compact` tool and the
/// `session.compact` RPC handler can never diverge on the retention count.
pub const SESSION_COMPACT_KEEP_LAST_N: usize = 50;

/// Outcome of `SessionStore::truncate_messages`.
#[derive(Debug, Clone, Default)]
pub struct TruncateResult {
    /// Number of messages that were removed from the session.
    pub messages_removed: usize,
    /// Rough estimate of the prompt+completion tokens that were dropped.
    pub tokens_removed_estimate: u64,
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
