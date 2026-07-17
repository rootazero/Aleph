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

impl SessionSnapshot {
    /// Render this snapshot as prompt text for LLM consumption.
    ///
    /// Empty sections are omitted to avoid noise.
    #[must_use]
    pub fn to_prompt_text(&self) -> String {
        let mut out = String::with_capacity(512);
        out.push_str("# Previous Session Context\n\n");
        out.push_str("**Summary:** ");
        out.push_str(&self.summary);
        out.push('\n');

        if !self.key_decisions.is_empty() {
            out.push_str("\n**Key decisions:**\n");
            for d in &self.key_decisions {
                out.push_str("- ");
                out.push_str(d);
                out.push('\n');
            }
        }

        if !self.active_files.is_empty() {
            out.push_str("\n**Active files:**\n");
            for f in &self.active_files {
                out.push_str("- ");
                out.push_str(f);
                out.push('\n');
            }
        }

        if !self.pending_tasks.is_empty() {
            out.push_str("\n**Pending tasks:**\n");
            for t in &self.pending_tasks {
                out.push_str("- ");
                out.push_str(t);
                out.push('\n');
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_prompt_text_renders_all_sections() {
        let snapshot = SessionSnapshot {
            session_id: "s1".into(),
            agent_id: "main".into(),
            created_at: Utc::now(),
            summary: "Implemented feature X.".into(),
            key_decisions: vec!["Use trait objects".into()],
            active_files: vec!["src/lib.rs".into()],
            tool_state: None,
            pending_tasks: vec!["Write tests".into()],
        };
        let text = snapshot.to_prompt_text();
        assert!(text.contains("# Previous Session Context"));
        assert!(text.contains("**Summary:** Implemented feature X."));
        assert!(text.contains("**Key decisions:**"));
        assert!(text.contains("- Use trait objects"));
        assert!(text.contains("**Active files:**"));
        assert!(text.contains("- src/lib.rs"));
        assert!(text.contains("**Pending tasks:**"));
        assert!(text.contains("- Write tests"));
    }

    #[test]
    fn to_prompt_text_skips_empty_sections() {
        let snapshot = SessionSnapshot {
            session_id: "s2".into(),
            agent_id: "main".into(),
            created_at: Utc::now(),
            summary: "Quick fix.".into(),
            key_decisions: vec![],
            active_files: vec![],
            tool_state: None,
            pending_tasks: vec![],
        };
        let text = snapshot.to_prompt_text();
        assert!(text.contains("**Summary:**"));
        assert!(!text.contains("**Key decisions:**"));
        assert!(!text.contains("**Active files:**"));
        assert!(!text.contains("**Pending tasks:**"));
    }

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
