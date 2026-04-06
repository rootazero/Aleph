//! SessionSnapshot — the serializable unit of cross-session context.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A snapshot of conversation context for resuming across sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub session_id: String,
    pub created_at: DateTime<Utc>,
    pub summary: String,
    pub key_decisions: Vec<String>,
    pub active_files: Vec<String>,
    pub tool_state: Option<String>,
    pub pending_tasks: Vec<String>,
}

/// Decision marker keywords used by [`SessionSnapshot::extract_decisions`].
const DECISION_MARKERS: &[&str] = &[
    "decided",
    "chose",
    "will use",
    "switched to",
    "selected",
    "picked",
    "agreed on",
    "confirmed",
];

/// Maximum number of decisions extracted from a summary.
const MAX_DECISIONS: usize = 10;

impl SessionSnapshot {
    /// Extract key decision sentences from a summary string.
    ///
    /// Splits on `. ` (dot-space), filters for sentences containing decision marker
    /// keywords, and caps the result at [`MAX_DECISIONS`].
    pub fn extract_decisions(summary: &str) -> Vec<String> {
        summary
            .split(". ")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .filter(|sentence| {
                let lower = sentence.to_lowercase();
                DECISION_MARKERS.iter().any(|m| lower.contains(m))
            })
            .take(MAX_DECISIONS)
            .map(|s| s.to_string())
            .collect()
    }

    /// Render this snapshot as prompt text for LLM consumption.
    ///
    /// Empty sections are omitted to avoid noise.
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
    fn extract_decisions_finds_markers() {
        let summary = "We decided to use Rust. The team chose LanceDB for storage. \
                        Nothing special here. They confirmed the API design.";
        let decisions = SessionSnapshot::extract_decisions(summary);
        assert_eq!(decisions.len(), 3);
        assert!(decisions[0].contains("decided"));
        assert!(decisions[1].contains("chose"));
        assert!(decisions[2].contains("confirmed"));
    }

    #[test]
    fn extract_decisions_empty_for_no_markers() {
        let summary = "The weather is nice today. We had lunch at noon.";
        let decisions = SessionSnapshot::extract_decisions(summary);
        assert!(decisions.is_empty());
    }

    #[test]
    fn extract_decisions_caps_at_max() {
        // Build a summary with 12 decision sentences
        let sentences: Vec<String> = (0..12)
            .map(|i| format!("We decided on option {i}"))
            .collect();
        let summary = sentences.join(". ") + ".";
        let decisions = SessionSnapshot::extract_decisions(&summary);
        assert_eq!(decisions.len(), MAX_DECISIONS);
    }

    #[test]
    fn to_prompt_text_renders_all_sections() {
        let snapshot = SessionSnapshot {
            session_id: "s1".into(),
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
        assert_eq!(restored.summary, snapshot.summary);
        assert_eq!(restored.key_decisions, snapshot.key_decisions);
        assert_eq!(restored.active_files, snapshot.active_files);
        assert_eq!(restored.tool_state, snapshot.tool_state);
        assert_eq!(restored.pending_tasks, snapshot.pending_tasks);
    }
}
