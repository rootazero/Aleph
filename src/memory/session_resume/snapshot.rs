//! `SessionSnapshot` — the serializable unit of cross-session context.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A snapshot of conversation context for resuming across sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub session_id: String,
    /// Owning agent. Snapshots from all agents share one on-disk directory,
    /// so the reader filters on this to keep agents' contexts isolated.
    /// `#[serde(default)]` keeps legacy agent-less files deserializable —
    /// their empty id never matches a real agent, so they are skipped.
    #[serde(default)]
    pub agent_id: String,
    pub created_at: DateTime<Utc>,
    pub summary: String,
    pub key_decisions: Vec<String>,
    pub active_files: Vec<String>,
    pub tool_state: Option<String>,
    pub pending_tasks: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization_roundtrip() {
        let snapshot = SessionSnapshot {
            session_id: "s-rt".into(),
            agent_id: "main".into(),
            created_at: Utc::now(),
            summary: "Roundtrip test.".into(),
            key_decisions: vec!["chose JSON".into()],
            active_files: vec!["a.rs".into()],
            tool_state: Some("active".into()),
            pending_tasks: vec!["deploy".into()],
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let restored: SessionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.session_id, snapshot.session_id);
        assert_eq!(restored.agent_id, snapshot.agent_id);
        assert_eq!(restored.summary, snapshot.summary);
        assert_eq!(restored.key_decisions, snapshot.key_decisions);
        assert_eq!(restored.active_files, snapshot.active_files);
        assert_eq!(restored.tool_state, snapshot.tool_state);
        assert_eq!(restored.pending_tasks, snapshot.pending_tasks);
    }

    #[test]
    fn legacy_snapshot_without_agent_id_deserializes_to_empty() {
        // Files written before the agent dimension existed have no agent_id
        // key; they must keep loading (empty id = never matches an agent).
        let legacy = r#"{
            "session_id": "old",
            "created_at": "2026-01-01T00:00:00Z",
            "summary": "Legacy snapshot.",
            "key_decisions": [],
            "active_files": [],
            "tool_state": null,
            "pending_tasks": []
        }"#;
        let restored: SessionSnapshot = serde_json::from_str(legacy).unwrap();
        assert_eq!(restored.agent_id, "");
        assert_eq!(restored.summary, "Legacy snapshot.");
    }
}
