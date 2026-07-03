//! Recall-evidence gate for destructive `DistillAction`s (SkillOpt F-gate analog).
//!
//! SkillOpt validates every skill edit against a held-out split before
//! accepting it. Aleph's cheap analog of that split is `recall_signals`: a
//! note that keeps getting recalled is *proven in production*, so replacing
//! it (`Supersede`) demands proportionally stronger LLM confidence. The gate
//! projects both sides onto the existing [`evaluate_gate`] scalar comparison:
//!
//! * `current`  = recall support of the target note (saturating hit count)
//! * `candidate` = the LLM's confidence in the replacement rule
//!
//! and accepts only strict improvement. Rejected edits are fingerprinted so
//! the same bad proposal is dropped immediately on later cycles instead of
//! re-litigating (and re-failing) the same gate every night.
//!
//! Pure functions only — persistence of the rejected-edit set lives on
//! `SqliteMemoryBackend` (`distill_rejects` / `record_distill_reject`), and
//! this gate *composes with* (never replaces) `MutationGate`'s churn
//! detection and `skill_gate`'s static validation.

use sha2::{Digest, Sha256};

use super::gate::evaluate_gate;
use super::HEALTH_GATE_EPSILON;
use crate::memory::dreaming::distill_action::DistillAction;

/// Saturation constant for recall support: `hits / (hits + K)`. With `K = 3`,
/// 3 recall hits → 0.5 support, 9 hits → 0.75 — a well-recalled note quickly
/// becomes hard to displace without high confidence.
const RECALL_SUPPORT_SATURATION: f64 = 3.0;

/// Decision returned by [`gate_supersede_evidence`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceDecision {
    /// The edit's confidence strictly beats the target's recall support.
    Allow,
    /// The edit is rejected; the reason is human-readable for the audit log.
    Reject(String),
}

/// Saturating recall-support score in `[0, 1)` for a note's deduped recall
/// hit count. Never-recalled notes score 0.0 (freely replaceable).
#[must_use]
pub fn recall_support(hit_count: i64) -> f64 {
    let h = hit_count.max(0) as f64;
    h / (h + RECALL_SUPPORT_SATURATION)
}

/// Gate a destructive supersede against the target note's recall evidence.
///
/// Accepts only when the LLM's `candidate_confidence` strictly improves on
/// the target's recall support (epsilon-guarded via [`evaluate_gate`]).
#[must_use]
pub fn gate_supersede_evidence(
    candidate_confidence: f32,
    target_hit_count: i64,
) -> EvidenceDecision {
    let current = recall_support(target_hit_count);
    let candidate = f64::from(candidate_confidence.clamp(0.0, 1.0));
    if evaluate_gate(candidate, current, current, HEALTH_GATE_EPSILON).is_accepted() {
        EvidenceDecision::Allow
    } else {
        EvidenceDecision::Reject(format!(
            "recall-evidence gate: confidence {candidate:.2} does not strictly improve on \
             target recall support {current:.2} ({target_hit_count} hits)"
        ))
    }
}

/// Stable fingerprint of a destructive action, for the rejected-edit record.
///
/// Only `Supersede` is fingerprinted (the one variant that destroys an
/// existing note); additive/no-op variants return `None`. The hash covers
/// target + replacement content so a *reworded* proposal is a fresh edit,
/// not a replay of the rejected one.
#[must_use]
pub fn action_fingerprint(action: &DistillAction) -> Option<String> {
    match action {
        DistillAction::Supersede {
            old_note_path,
            title,
            rule,
            ..
        } => {
            let digest = Sha256::digest(format!("supersede|{old_note_path}|{title}|{rule}"));
            Some(hex::encode(&digest[..8]))
        }
        DistillAction::New { .. }
        | DistillAction::Strengthen { .. }
        | DistillAction::Skip { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::Severity;

    fn supersede(path: &str, title: &str, rule: &str) -> DistillAction {
        DistillAction::Supersede {
            old_note_path: path.into(),
            title: title.into(),
            rule: rule.into(),
            confidence: 0.8,
            severity: Severity::Med,
            source_facts: vec![],
        }
    }

    #[test]
    fn recall_support_saturates_toward_one() {
        assert!(recall_support(0) < f64::EPSILON);
        assert!((recall_support(3) - 0.5).abs() < 1e-9);
        assert!(recall_support(100) > 0.9);
        assert!(recall_support(100) < 1.0);
        // Defensive: negative counts clamp to zero support.
        assert!(recall_support(-5) < f64::EPSILON);
    }

    #[test]
    fn never_recalled_target_is_freely_replaceable() {
        assert_eq!(gate_supersede_evidence(0.3, 0), EvidenceDecision::Allow);
    }

    #[test]
    fn hot_target_rejects_low_confidence_supersede() {
        // 10 hits → support ≈ 0.77; confidence 0.5 must not displace it.
        match gate_supersede_evidence(0.5, 10) {
            EvidenceDecision::Reject(reason) => {
                assert!(reason.contains("recall-evidence gate"), "{reason}");
                assert!(reason.contains("10 hits"), "{reason}");
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn hot_target_allows_high_confidence_supersede() {
        assert_eq!(gate_supersede_evidence(0.9, 10), EvidenceDecision::Allow);
    }

    #[test]
    fn marginal_gain_within_epsilon_is_rejected() {
        // 3 hits → support 0.5; candidate 0.505 is inside the epsilon band.
        assert!(matches!(
            gate_supersede_evidence(0.505, 3),
            EvidenceDecision::Reject(_)
        ));
    }

    #[test]
    fn out_of_range_confidence_is_clamped() {
        // >1.0 clamps to 1.0 (still allowed); <0 clamps to 0 (rejected).
        assert_eq!(gate_supersede_evidence(5.0, 10), EvidenceDecision::Allow);
        assert!(matches!(
            gate_supersede_evidence(-1.0, 0),
            EvidenceDecision::Reject(_)
        ));
    }

    #[test]
    fn fingerprint_only_for_supersede() {
        assert!(action_fingerprint(&supersede("skill/a", "t", "r")).is_some());
        assert!(action_fingerprint(&DistillAction::Strengthen {
            existing_note_path: "skill/a".into(),
            source_facts: vec![],
        })
        .is_none());
        assert!(action_fingerprint(&DistillAction::Skip {
            source_fact: "F".into(),
            reason: "noise".into(),
        })
        .is_none());
    }

    #[test]
    fn fingerprint_is_stable_and_content_sensitive() {
        let a = action_fingerprint(&supersede("skill/a", "t", "r")).unwrap();
        let same = action_fingerprint(&supersede("skill/a", "t", "r")).unwrap();
        let reworded = action_fingerprint(&supersede("skill/a", "t", "different rule")).unwrap();
        assert_eq!(a, same);
        assert_ne!(a, reworded, "a reworded proposal is a fresh edit");
        assert_eq!(a.len(), 16, "16-hex fingerprint");
    }
}
