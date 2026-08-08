//! `SessionSnapshot` — the serializable unit of cross-session context.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A snapshot of conversation context for resuming across sessions.
///
/// The payload is exactly one thing: the LLM-written `/end-summary`. There is
/// deliberately NO structured `key_decisions` / `active_files` / `pending_tasks`
/// / `tool_state` breakdown alongside it. The only source those fields could
/// ever have is that same natural-language summary, and scraping it
/// deterministically is precisely what R7/P8 ban — so they could never acquire
/// a legitimate producer, stayed empty forever, and made every rendered
/// snapshot end with three empty labels that contradicted the filled
/// `## Key Decisions` / `## Files & Code` / `## Pending` sections the summary
/// itself is mandated to carry. Legacy `resume.json` files written while those
/// keys existed keep loading unchanged: serde ignores unknown fields by default.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub session_id: String,
    /// Owning agent. Snapshots from all agents share one on-disk directory,
    /// so the reader filters on this to keep agents' contexts isolated.
    /// `#[serde(default)]` keeps legacy agent-less files deserializable —
    /// their empty id never matches a real agent, so they are skipped.
    #[serde(default)]
    pub agent_id: String,
    /// Owning memory **partition** — [`super::snapshot_partition`]'s output for
    /// the session's ambient scope at write time (`main`, `main__u-alice`,
    /// `main__p-x7f2`).
    ///
    /// `agent_id` alone is the base id, which every user of the same agent
    /// shares; it can therefore never separate alice's `/end-summary` from
    /// bob's. This is the dimension that does, and the reader matches on it
    /// exactly.
    ///
    /// `#[serde(default)]` keeps every legacy `resume.json` deserializable.
    /// Their `None` is NOT read as "the base partition" — see
    /// [`super::SnapshotReader::load_latest`] for why that is fail-closed on
    /// purpose.
    #[serde(default)]
    pub scope_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub summary: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization_roundtrip() {
        let snapshot = SessionSnapshot {
            session_id: "s-rt".into(),
            agent_id: "main".into(),
            scope_id: Some("main__u-alice".into()),
            created_at: Utc::now(),
            summary: "Roundtrip test.".into(),
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let restored: SessionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.session_id, snapshot.session_id);
        assert_eq!(restored.agent_id, snapshot.agent_id);
        assert_eq!(restored.scope_id, snapshot.scope_id);
        assert_eq!(restored.summary, snapshot.summary);
    }

    #[test]
    fn legacy_snapshot_without_scope_id_deserializes_to_none() {
        // Files written before the partition dimension existed carry no
        // `scope_id`; they must keep loading, and their `None` must stay
        // distinguishable from a real partition so the reader can fail closed.
        let legacy = r#"{
            "session_id": "old",
            "agent_id": "main",
            "created_at": "2026-01-01T00:00:00Z",
            "summary": "Legacy snapshot."
        }"#;
        let restored: SessionSnapshot = serde_json::from_str(legacy).unwrap();
        assert_eq!(restored.scope_id, None);
    }

    #[test]
    fn legacy_snapshot_without_agent_id_deserializes_to_empty() {
        // Files written before the agent dimension existed have no agent_id
        // key; they must keep loading (empty id = never matches an agent).
        let legacy = r#"{
            "session_id": "old",
            "created_at": "2026-01-01T00:00:00Z",
            "summary": "Legacy snapshot."
        }"#;
        let restored: SessionSnapshot = serde_json::from_str(legacy).unwrap();
        assert_eq!(restored.agent_id, "");
        assert_eq!(restored.summary, "Legacy snapshot.");
    }

    #[test]
    fn legacy_snapshot_with_dropped_structured_fields_still_loads() {
        // Every file written before the four producer-less fields were cut
        // still carries them. Dropping the fields must not orphan those files:
        // serde ignores unknown keys by default, and the summary — the only
        // part anything ever read — survives.
        let legacy = r#"{
            "session_id": "old",
            "agent_id": "main",
            "created_at": "2026-01-01T00:00:00Z",
            "summary": "Legacy snapshot.",
            "key_decisions": ["chose JSON"],
            "active_files": ["a.rs"],
            "tool_state": "active",
            "pending_tasks": ["deploy"]
        }"#;
        let restored: SessionSnapshot = serde_json::from_str(legacy).unwrap();
        assert_eq!(restored.agent_id, "main");
        assert_eq!(restored.summary, "Legacy snapshot.");
    }
}
