//! Shared `DistillAction` enum used by `SkillDistill` and `FeedbackDistill`.
//!
//! Code path (per Phase 2 Decision 2 in
//! `docs/superpowers/plans/2026-04-29-aleph-self-evolution.md`):
//!
//!   1. Code calls `find_similar_notes` → top-N existing candidates
//!   2. Code injects candidates into the LLM prompt (Task 15)
//!   3. LLM emits a `DistillAction` referencing a candidate ID verbatim
//!   4. `NoteIndexer::apply_distill_action` executes — pure plumbing,
//!      no judgment.

use crate::memory::notes::Severity;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DistillAction {
    /// Create a brand-new note. Used when no existing candidate matches.
    New {
        /// kebab-case filename (without `.md`)
        title: String,
        /// body content of the new note
        rule: String,
        confidence: f32,
        severity: Severity,
        source_facts: Vec<String>,
    },
    /// Reinforce an existing note: append `source_facts` and bump `updated_at`.
    /// Confidence is NOT re-judged here (LLM didn't re-evaluate the rule itself).
    Strengthen {
        /// e.g. `"skill/async-error-handling"` — must come from injected candidates
        existing_note_path: String,
        source_facts: Vec<String>,
    },
    /// Replace an old note with a new rule (LLM judged the new wording supersedes).
    Supersede {
        old_note_path: String,
        title: String,
        rule: String,
        confidence: f32,
        severity: Severity,
        source_facts: Vec<String>,
    },
    /// LLM rejected this candidate (transient noise, not actionable).
    Skip { source_fact: String, reason: String },
}

/// Path this action references in the existing note set, if any.
///
/// `New` and `Skip` do not reference an existing note. `Strengthen` and
/// `Supersede` both name an existing path that *must* come from the candidate
/// list the LLM was shown — otherwise a hallucinated path could trigger
/// cross-category file deletion (Supersede) or merge into the wrong note
/// (Strengthen). Stages use this helper to drop actions whose target is not
/// in the candidate set before invoking `apply_distill_action`.
pub fn referenced_path(action: &DistillAction) -> Option<&str> {
    match action {
        DistillAction::Strengthen {
            existing_note_path, ..
        } => Some(existing_note_path),
        DistillAction::Supersede { old_note_path, .. } => Some(old_note_path),
        DistillAction::New { .. } | DistillAction::Skip { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_new_action() {
        let j = r#"{"type":"new","title":"async-err","rule":"Use ?","confidence":0.9,"severity":"high","source_facts":["F1"]}"#;
        let a: DistillAction = serde_json::from_str(j).unwrap();
        match a {
            DistillAction::New {
                confidence,
                severity,
                ..
            } => {
                assert!((confidence - 0.9).abs() < 1e-6);
                assert_eq!(severity, Severity::High);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn deserialize_strengthen_action() {
        let j =
            r#"{"type":"strengthen","existing_note_path":"skill/async-err","source_facts":["F1"]}"#;
        let a: DistillAction = serde_json::from_str(j).unwrap();
        assert!(matches!(a, DistillAction::Strengthen { .. }));
    }

    #[test]
    fn deserialize_supersede_action() {
        let j = r#"{"type":"supersede","old_note_path":"skill/old","title":"new","rule":"X","confidence":0.8,"severity":"med","source_facts":[]}"#;
        let a: DistillAction = serde_json::from_str(j).unwrap();
        assert!(matches!(a, DistillAction::Supersede { .. }));
    }

    #[test]
    fn deserialize_skip_action() {
        let j = r#"{"type":"skip","source_fact":"F1","reason":"transient"}"#;
        let a: DistillAction = serde_json::from_str(j).unwrap();
        assert!(matches!(a, DistillAction::Skip { .. }));
    }

    #[test]
    fn referenced_path_extracts_strengthen_target() {
        let a = DistillAction::Strengthen {
            existing_note_path: "skill/async-error".into(),
            source_facts: vec![],
        };
        assert_eq!(referenced_path(&a), Some("skill/async-error"));
    }

    #[test]
    fn referenced_path_extracts_supersede_target() {
        let a = DistillAction::Supersede {
            old_note_path: "feedback/typo".into(),
            title: "fix".into(),
            rule: "x".into(),
            confidence: 0.8,
            severity: Severity::Med,
            source_facts: vec![],
        };
        assert_eq!(referenced_path(&a), Some("feedback/typo"));
    }

    #[test]
    fn referenced_path_is_none_for_new_and_skip() {
        let n = DistillAction::New {
            title: "x".into(),
            rule: "x".into(),
            confidence: 1.0,
            severity: Severity::Low,
            source_facts: vec![],
        };
        let s = DistillAction::Skip {
            source_fact: "x".into(),
            reason: "x".into(),
        };
        assert_eq!(referenced_path(&n), None);
        assert_eq!(referenced_path(&s), None);
    }
}
