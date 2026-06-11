//! Skill Validation Gate — pre-apply checks on LLM-emitted `DistillActions`.
//!
//! Spec 5 wiring: sits between `parse_distill_response` (LLM output) and
//! `apply_distill_action` (filesystem mutation). Catches the malformed and
//! the dangerous before they touch the notes layer.
//!
//! Three layers:
//! * **L0 format** — title/rule length, non-empty, severity validity (the
//!   enum itself enforces severity range, but a stricter check could be
//!   added if upstream loosens it).
//! * **L1 semantic** — low-confidence + high-severity → severity downgrade
//!   to `Med` so an unconvinced LLM cannot raise a critical-priority skill.
//!   Distinct from rejection: the action still applies, just less loudly.
//! * **L2 safety** — defensive path-traversal check on `target_path` for
//!   strengthen/supersede. `sanitize_title` already protects `title`, but
//!   `existing_note_path` / `old_note_path` come straight from the LLM
//!   prompt and need their own guard.
//!
//! Rejected actions are recorded with [`DistillOutcome::FilteredInvalid`]
//! and never reach the indexer.

use crate::memory::dreaming::distill_action::DistillAction;
use crate::memory::notes::Severity;

// Format-layer bounds. Chosen well above realistic LLM emissions so they
// only catch pathological outputs (token-runaway, prompt injection echo).
const MAX_TITLE_LEN: usize = 80;
const MAX_RULE_LEN: usize = 4_000;
const MIN_CONFIDENCE_FOR_HIGH_SEVERITY: f32 = 0.5;

/// Decision returned by [`validate_skill_action`].
#[derive(Debug, Clone)]
pub enum SkillGateDecision {
    /// Action is allowed. The wrapped value may have been modified from
    /// the input (currently: severity downgrade on low-confidence high-sev).
    Allow(DistillAction),
    /// Action is rejected. The reason is short and human-readable so it
    /// surfaces cleanly in `DistillActionRecord.error`.
    Reject(String),
}

/// Validate one LLM-emitted action against L0/L1/L2 checks.
///
/// Pure function: no I/O, no state. The caller decides what to do with
/// `Reject` — typically push a `DistillActionRecord` with outcome
/// `FilteredInvalid` and skip the apply.
#[must_use]
pub fn validate_skill_action(action: DistillAction) -> SkillGateDecision {
    match action {
        DistillAction::New {
            title,
            rule,
            confidence,
            severity,
            source_facts,
        } => {
            if let Err(r) = check_title(&title) {
                return SkillGateDecision::Reject(r);
            }
            if let Err(r) = check_rule(&rule) {
                return SkillGateDecision::Reject(r);
            }
            let severity = downgrade_if_low_confidence(severity, confidence, &title);
            SkillGateDecision::Allow(DistillAction::New {
                title,
                rule,
                confidence,
                severity,
                source_facts,
            })
        }
        DistillAction::Supersede {
            old_note_path,
            title,
            rule,
            confidence,
            severity,
            source_facts,
        } => {
            if let Err(r) = check_title(&title) {
                return SkillGateDecision::Reject(r);
            }
            if let Err(r) = check_rule(&rule) {
                return SkillGateDecision::Reject(r);
            }
            if let Err(r) = check_target_path(&old_note_path, "old_note_path") {
                return SkillGateDecision::Reject(r);
            }
            let severity = downgrade_if_low_confidence(severity, confidence, &title);
            SkillGateDecision::Allow(DistillAction::Supersede {
                old_note_path,
                title,
                rule,
                confidence,
                severity,
                source_facts,
            })
        }
        DistillAction::Strengthen {
            existing_note_path,
            source_facts,
        } => {
            if let Err(r) = check_target_path(&existing_note_path, "existing_note_path") {
                return SkillGateDecision::Reject(r);
            }
            SkillGateDecision::Allow(DistillAction::Strengthen {
                existing_note_path,
                source_facts,
            })
        }
        DistillAction::Skip {
            source_fact,
            reason,
        } => {
            // Skip carries no filesystem effect and no claim about a skill;
            // any non-pathological string is fine. Length-cap reason so it
            // doesn't bloat the JSONL audit log.
            let reason = if reason.len() > MAX_RULE_LEN {
                let truncated: String = reason.chars().take(MAX_RULE_LEN).collect();
                truncated
            } else {
                reason
            };
            SkillGateDecision::Allow(DistillAction::Skip {
                source_fact,
                reason,
            })
        }
    }
}

fn check_title(title: &str) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("title is empty".into());
    }
    if title.len() > MAX_TITLE_LEN {
        return Err(format!(
            "title too long ({} chars, max {MAX_TITLE_LEN})",
            title.len()
        ));
    }
    Ok(())
}

fn check_rule(rule: &str) -> Result<(), String> {
    if rule.trim().is_empty() {
        return Err("rule is empty".into());
    }
    if rule.len() > MAX_RULE_LEN {
        return Err(format!(
            "rule too long ({} chars, max {MAX_RULE_LEN})",
            rule.len()
        ));
    }
    Ok(())
}

/// Defensive path-traversal check for LLM-emitted candidate paths.
/// Mirrors the patterns `sanitize_title` strips from titles. Strengthen
/// and Supersede both take a path string straight from the prompt, so this
/// is the last line of defense before the indexer reads it.
fn check_target_path(path: &str, field: &str) -> Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} is empty"));
    }
    if trimmed.contains("..") {
        return Err(format!("{field} contains path traversal: {trimmed:?}"));
    }
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return Err(format!("{field} is absolute path: {trimmed:?}"));
    }
    if trimmed.contains('\0') {
        return Err(format!("{field} contains null byte"));
    }
    Ok(())
}

/// L1 — if the LLM emitted High/Critical severity with low confidence,
/// downgrade severity to Med so an unconvinced model can't force a
/// loud skill into the notes layer. Logs the downgrade for the audit
/// trail. Returns the (possibly downgraded) severity.
fn downgrade_if_low_confidence(severity: Severity, confidence: f32, title: &str) -> Severity {
    let high_severity = matches!(severity, Severity::High | Severity::Critical);
    if high_severity && confidence < MIN_CONFIDENCE_FOR_HIGH_SEVERITY {
        tracing::info!(
            %title,
            %confidence,
            ?severity,
            "skill_gate: downgrading severity due to low confidence"
        );
        Severity::Med
    } else {
        severity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good_new() -> DistillAction {
        DistillAction::New {
            title: "async-error".into(),
            rule: "Use ? operator for Result propagation".into(),
            confidence: 0.85,
            severity: Severity::High,
            source_facts: vec!["F1".into()],
        }
    }

    #[test]
    fn allows_well_formed_new_action() {
        let action = good_new();
        match validate_skill_action(action) {
            SkillGateDecision::Allow(DistillAction::New {
                title, severity, ..
            }) => {
                assert_eq!(title, "async-error");
                // Confidence 0.85 above 0.5 threshold → severity preserved.
                assert!(matches!(severity, Severity::High));
            }
            other => panic!("expected Allow(New), got {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_title() {
        let mut a = good_new();
        if let DistillAction::New { ref mut title, .. } = a {
            *title = "".into();
        }
        assert!(matches!(
            validate_skill_action(a),
            SkillGateDecision::Reject(_)
        ));
    }

    #[test]
    fn rejects_overlong_title() {
        let mut a = good_new();
        if let DistillAction::New { ref mut title, .. } = a {
            *title = "x".repeat(MAX_TITLE_LEN + 1);
        }
        match validate_skill_action(a) {
            SkillGateDecision::Reject(r) => assert!(r.contains("too long")),
            _ => panic!("expected Reject"),
        }
    }

    #[test]
    fn rejects_overlong_rule() {
        let mut a = good_new();
        if let DistillAction::New { ref mut rule, .. } = a {
            *rule = "y".repeat(MAX_RULE_LEN + 1);
        }
        match validate_skill_action(a) {
            SkillGateDecision::Reject(r) => assert!(r.contains("rule too long")),
            _ => panic!("expected Reject"),
        }
    }

    #[test]
    fn downgrades_high_severity_on_low_confidence() {
        let a = DistillAction::New {
            title: "x".into(),
            rule: "y".into(),
            confidence: 0.2,
            severity: Severity::Critical,
            source_facts: vec![],
        };
        match validate_skill_action(a) {
            SkillGateDecision::Allow(DistillAction::New { severity, .. }) => {
                assert!(matches!(severity, Severity::Med));
            }
            other => panic!("expected Allow(New) with downgrade, got {other:?}"),
        }
    }

    #[test]
    fn keeps_low_severity_on_any_confidence() {
        let a = DistillAction::New {
            title: "x".into(),
            rule: "y".into(),
            confidence: 0.05,
            severity: Severity::Low,
            source_facts: vec![],
        };
        match validate_skill_action(a) {
            SkillGateDecision::Allow(DistillAction::New { severity, .. }) => {
                assert!(matches!(severity, Severity::Low));
            }
            _ => panic!("expected Allow(New)"),
        }
    }

    #[test]
    fn rejects_path_traversal_in_strengthen() {
        let a = DistillAction::Strengthen {
            existing_note_path: "../../etc/passwd".into(),
            source_facts: vec![],
        };
        match validate_skill_action(a) {
            SkillGateDecision::Reject(r) => assert!(r.contains("traversal")),
            _ => panic!("expected Reject"),
        }
    }

    #[test]
    fn rejects_absolute_path_in_supersede() {
        let a = DistillAction::Supersede {
            old_note_path: "/etc/shadow".into(),
            title: "x".into(),
            rule: "y".into(),
            confidence: 0.8,
            severity: Severity::Med,
            source_facts: vec![],
        };
        match validate_skill_action(a) {
            SkillGateDecision::Reject(r) => assert!(r.contains("absolute")),
            _ => panic!("expected Reject"),
        }
    }

    #[test]
    fn rejects_null_byte_in_path() {
        let a = DistillAction::Strengthen {
            existing_note_path: "skill/async\0sneaky".into(),
            source_facts: vec![],
        };
        match validate_skill_action(a) {
            SkillGateDecision::Reject(r) => assert!(r.contains("null byte")),
            _ => panic!("expected Reject"),
        }
    }

    #[test]
    fn allows_legitimate_strengthen() {
        let a = DistillAction::Strengthen {
            existing_note_path: "skill/async-error-handling".into(),
            source_facts: vec!["F1".into()],
        };
        assert!(matches!(
            validate_skill_action(a),
            SkillGateDecision::Allow(_)
        ));
    }

    #[test]
    fn skip_action_passes_unchanged() {
        let a = DistillAction::Skip {
            source_fact: "F".into(),
            reason: "transient noise".into(),
        };
        match validate_skill_action(a) {
            SkillGateDecision::Allow(DistillAction::Skip { reason, .. }) => {
                assert_eq!(reason, "transient noise");
            }
            _ => panic!("expected Allow(Skip)"),
        }
    }

    #[test]
    fn skip_action_truncates_overlong_reason() {
        let big = "z".repeat(MAX_RULE_LEN + 500);
        let a = DistillAction::Skip {
            source_fact: "F".into(),
            reason: big,
        };
        match validate_skill_action(a) {
            SkillGateDecision::Allow(DistillAction::Skip { reason, .. }) => {
                assert_eq!(reason.chars().count(), MAX_RULE_LEN);
            }
            _ => panic!("expected Allow(Skip)"),
        }
    }
}
