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
use serde::{Deserialize, Serialize};

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
/// list the LLM was shown — otherwise a hallucinated path could archive an
/// unrelated note out of the corpus (Supersede) or merge into the wrong note
/// (Strengthen). Stages use this helper to drop actions whose target is not
/// in the candidate set before invoking `apply_distill_action`.
#[must_use]
pub fn referenced_path(action: &DistillAction) -> Option<&str> {
    match action {
        DistillAction::Strengthen {
            existing_note_path, ..
        } => Some(existing_note_path),
        DistillAction::Supersede { old_note_path, .. } => Some(old_note_path),
        DistillAction::New { .. } | DistillAction::Skip { .. } => None,
    }
}

// ---------------------------------------------------------------------------
// DistillActionRecord — per-action audit row written into DreamReport so that
// the existing dream_events.jsonl carries enough provenance to replay or
// debug a specific skill mutation back to its triggering candidate set.
//
// Inspired by evolver's per-EvolutionEvent log (one row per genetic action,
// not per cycle). Aleph's outer `DreamEvent` already records per-cycle
// metadata; this struct fills in the inner per-action layer that was
// previously collapsed into a single counter (`skill_distill_count` /
// `feedback_distill_count`).
// ---------------------------------------------------------------------------

/// Outcome of attempting to apply a single [`DistillAction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistillOutcome {
    /// Action was applied (note created / strengthened / superseded, or skip
    /// recorded). Includes `skip` because the LLM still made a decision.
    Applied,
    /// Dropped before apply because the referenced path was not in the
    /// candidate set the LLM was shown (anti-hallucination guard).
    FilteredNonCandidate,
    /// Dropped before apply because the [`skill_gate`] rejected it on a
    /// format / semantic / safety check (Spec 5). The `error` field on the
    /// record carries the reason.
    FilteredInvalid,
    /// Dropped before apply because the recall-evidence gate rejected a
    /// destructive edit: the target note's recall hits outweigh the LLM's
    /// confidence in the replacement, or the same edit was already rejected
    /// on a prior cycle. The `error` field carries the reason.
    FilteredEvidence,
    /// `apply_distill_action` returned an error.
    Error,
}

/// Per-action provenance row persisted via `DreamReport.distill_actions`.
///
/// All fields are flat strings/scalars so the JSONL stays human-greppable.
/// The struct is intentionally self-contained (no embedded `Severity` enum)
/// to keep the on-disk log forward-compatible if `Severity` ever evolves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistillActionRecord {
    /// Stage that emitted the action ("`skill_distill`" or "`feedback_distill`").
    pub stage: String,
    /// Action variant ("new" | "strengthen" | "supersede" | "skip").
    pub action_kind: String,
    /// Existing note path for strengthen/supersede; None otherwise.
    pub target_path: Option<String>,
    /// kebab-case title for new/supersede; None for strengthen/skip.
    pub title: Option<String>,
    /// LLM-emitted confidence for new/supersede.
    pub confidence: Option<f32>,
    /// Severity (serialized as lowercase string) for new/supersede.
    pub severity: Option<String>,
    /// Apply outcome.
    pub outcome: DistillOutcome,
    /// Error message when `outcome == Error`; LLM-emitted reason when
    /// `action_kind == "skip"`.
    pub error: Option<String>,
}

impl DistillActionRecord {
    /// Build a record from a `DistillAction` and an apply outcome.
    ///
    /// `stage` should be either `"skill_distill"` or `"feedback_distill"`.
    #[must_use]
    pub fn from_action(
        stage: &str,
        action: &DistillAction,
        outcome: DistillOutcome,
        error: Option<String>,
    ) -> Self {
        let severity_str = |s: Severity| -> String {
            // Use serde's lowercase serialization to stay aligned with the
            // on-disk note frontmatter spelling.
            serde_json::to_value(s)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "low".to_string())
        };

        match action {
            DistillAction::New {
                title,
                confidence,
                severity,
                ..
            } => Self {
                stage: stage.to_string(),
                action_kind: "new".to_string(),
                target_path: None,
                // rust-doctor-disable-next-line excessive-clone
                title: Some(title.clone()),
                confidence: Some(*confidence),
                severity: Some(severity_str(*severity)),
                outcome,
                error,
            },
            DistillAction::Strengthen {
                existing_note_path, ..
            } => Self {
                stage: stage.to_string(),
                action_kind: "strengthen".to_string(),
                // rust-doctor-disable-next-line excessive-clone
                target_path: Some(existing_note_path.clone()),
                title: None,
                confidence: None,
                severity: None,
                outcome,
                error,
            },
            DistillAction::Supersede {
                old_note_path,
                title,
                confidence,
                severity,
                ..
            } => Self {
                stage: stage.to_string(),
                action_kind: "supersede".to_string(),
                // rust-doctor-disable-next-line excessive-clone
                target_path: Some(old_note_path.clone()),
                // rust-doctor-disable-next-line excessive-clone
                title: Some(title.clone()),
                confidence: Some(*confidence),
                severity: Some(severity_str(*severity)),
                outcome,
                error,
            },
            DistillAction::Skip { reason, .. } => Self {
                stage: stage.to_string(),
                action_kind: "skip".to_string(),
                target_path: None,
                title: None,
                confidence: None,
                severity: None,
                outcome,
                // Carry the LLM-emitted skip reason in the error slot so the
                // on-disk row preserves *why* the LLM rejected the signal.
                // rust-doctor-disable-next-line excessive-clone
                error: error.or_else(|| Some(reason.clone())),
            },
        }
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

    #[test]
    fn record_from_new_action_captures_title_and_confidence() {
        let action = DistillAction::New {
            title: "async-error".into(),
            rule: "always use ?".into(),
            confidence: 0.92,
            severity: Severity::High,
            source_facts: vec!["F1".into()],
        };
        let r = DistillActionRecord::from_action(
            "skill_distill",
            &action,
            DistillOutcome::Applied,
            None,
        );
        assert_eq!(r.action_kind, "new");
        assert_eq!(r.title.as_deref(), Some("async-error"));
        assert_eq!(r.confidence, Some(0.92));
        assert_eq!(r.severity.as_deref(), Some("high"));
        assert!(r.target_path.is_none());
        assert!(matches!(r.outcome, DistillOutcome::Applied));
    }

    #[test]
    fn record_from_strengthen_action_captures_target_path() {
        let action = DistillAction::Strengthen {
            existing_note_path: "skill/async-error".into(),
            source_facts: vec!["F2".into()],
        };
        let r = DistillActionRecord::from_action(
            "skill_distill",
            &action,
            DistillOutcome::FilteredNonCandidate,
            None,
        );
        assert_eq!(r.action_kind, "strengthen");
        assert_eq!(r.target_path.as_deref(), Some("skill/async-error"));
        assert!(r.title.is_none());
        assert!(r.confidence.is_none());
        assert!(matches!(r.outcome, DistillOutcome::FilteredNonCandidate));
    }

    #[test]
    fn record_from_skip_action_preserves_reason_as_error() {
        let action = DistillAction::Skip {
            source_fact: "F3".into(),
            reason: "transient noise".into(),
        };
        let r = DistillActionRecord::from_action(
            "feedback_distill",
            &action,
            DistillOutcome::Applied,
            None,
        );
        assert_eq!(r.action_kind, "skip");
        assert_eq!(r.error.as_deref(), Some("transient noise"));
    }

    #[test]
    fn record_serializes_to_flat_json() {
        let action = DistillAction::New {
            title: "x".into(),
            rule: "y".into(),
            confidence: 0.5,
            severity: Severity::Med,
            source_facts: vec![],
        };
        let r = DistillActionRecord::from_action(
            "skill_distill",
            &action,
            DistillOutcome::Error,
            Some("backend offline".into()),
        );
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["stage"], "skill_distill");
        assert_eq!(v["action_kind"], "new");
        assert_eq!(v["severity"], "med");
        assert_eq!(v["outcome"], "error");
        assert_eq!(v["error"], "backend offline");
    }
}
