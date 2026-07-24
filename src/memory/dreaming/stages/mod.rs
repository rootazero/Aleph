//! Dream pipeline stages: trait definition and stage implementations.

use async_trait::async_trait;

pub mod co_recall_edges;
pub mod corpus_narrative;
pub mod daily_digest;
pub mod feedback_distill;
pub mod goal_lessons_promote;
pub mod graph_recompute;
pub mod index_refresher;
pub mod mention_weave;
pub mod note_consolidate;
pub mod note_decay;
pub mod note_drift;
pub mod note_lint;
pub mod note_review;
pub mod note_synthesis;
pub mod note_weave;
pub mod skill_distill;
pub mod skill_lifecycle;
pub mod workflow_proposal;

pub use co_recall_edges::CoRecallEdgesStage;
pub use corpus_narrative::CorpusNarrativeStage;
pub use daily_digest::DailyDigestStage;
pub use feedback_distill::FeedbackDistillStage;
pub use goal_lessons_promote::GoalLessonsPromoteStage;
pub use graph_recompute::GraphRecomputeStage;
pub use index_refresher::IndexRefresherStage;
pub use mention_weave::MentionWeaveStage;
pub use note_consolidate::NoteConsolidateStage;
pub use note_decay::NoteDecayStage;
pub use note_drift::NoteDriftStage;
pub use note_lint::NoteLintStage;
pub use note_review::NoteReviewStage;
pub use note_synthesis::NoteSynthesisStage;
pub use note_weave::NoteWeaveStage;
pub use skill_distill::SkillDistillStage;
pub use skill_lifecycle::SkillLifecycleStage;
pub use workflow_proposal::WorkflowProposalStage;

/// Whether a provider error makes every further LLM call in this cycle futile.
///
/// An exhausted quota or a rejected key does not heal between two calls issued
/// milliseconds apart. Stages that loop over items used to treat such an error
/// exactly like a transient per-item hiccup — `warn!` then `continue` — so once
/// the provider started returning 403 they kept hammering it for every remaining
/// item (over 13,000 doomed calls in one observed night). Abort the cycle
/// instead: the daemon records the failure and, per the once-per-day guard, does
/// not retry until tomorrow.
pub(crate) fn is_provider_exhausted(err: &crate::error::AlephError) -> bool {
    matches!(
        err,
        crate::error::AlephError::AuthenticationError { .. }
            | crate::error::AlephError::RateLimitError { .. }
    )
}

use super::{distill_action, DreamContext};
use crate::error::AlephError;

/// A single stage in the dream pipeline.
///
/// Each stage receives a `DreamContext`, performs its work, and returns
/// the (potentially modified) context for the next stage.
#[async_trait]
pub trait DreamStage: Send + Sync {
    /// Human-readable name of this stage (used for logging and reports).
    fn name(&self) -> &'static str;

    /// Whether this stage should run given the current context.
    /// Returning `false` skips this stage without error.
    async fn should_run(&self, _ctx: &DreamContext) -> bool {
        true
    }

    /// Execute the stage, consuming and returning the context.
    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError>;
}

/// Shared recall-evidence gate for destructive distill actions (Supersede).
///
/// Returns `Some(record)` when the action must be dropped — either its
/// fingerprint was rejected on a prior cycle, or the target note's recall
/// support outweighs the LLM's confidence (`gate_supersede_evidence`). The
/// record carries [`distill_action::DistillOutcome::FilteredEvidence`] and the
/// reason; `None` means the action may proceed to apply. Non-destructive
/// variants always pass. Store read failures fail open (hits = 0) so a broken
/// signals table never blocks distillation.
pub(crate) async fn gate_action_evidence(
    ctx: &DreamContext,
    action: &distill_action::DistillAction,
    rejected_fingerprints: &[String],
    stage: &str,
) -> Option<distill_action::DistillActionRecord> {
    use crate::memory::dreaming::distill_action::{
        DistillAction, DistillActionRecord, DistillOutcome,
    };
    use crate::memory::dreaming::evolution::{
        action_fingerprint, gate_supersede_evidence, EvidenceDecision,
    };
    use crate::memory::notes::store::NoteStore;

    let DistillAction::Supersede {
        old_note_path,
        title,
        confidence,
        ..
    } = action
    else {
        return None;
    };

    let fingerprint = action_fingerprint(action);
    if let Some(fp) = &fingerprint {
        if rejected_fingerprints.iter().any(|f| f == fp) {
            tracing::info!(
                stage,
                target = %old_note_path,
                "evidence gate: dropping previously rejected supersede"
            );
            return Some(DistillActionRecord::from_action(
                stage,
                action,
                DistillOutcome::FilteredEvidence,
                Some("previously rejected by recall-evidence gate".into()),
            ));
        }
    }

    let hits = match ctx
        .indexer
        .store()
        .recall_hit_counts(&ctx.agent_id, std::slice::from_ref(old_note_path))
        .await
    {
        Ok(counts) => counts.get(old_note_path).copied().unwrap_or(0),
        Err(e) => {
            tracing::warn!(error = %e, "evidence gate: recall_hit_counts failed; treating as 0");
            0
        }
    };

    match gate_supersede_evidence(*confidence, hits) {
        EvidenceDecision::Allow => None,
        EvidenceDecision::Reject(reason) => {
            if let Some(fp) = &fingerprint {
                // Persist the full context, not just the fingerprint, so the
                // next distill prompt can replay it as negative feedback
                // ("you already tried X → rejected because Y").
                let record = crate::memory::store::sqlite::dream_kv::DistillRejectRecord {
                    fingerprint: fp.clone(),
                    target: old_note_path.clone(),
                    summary: title.clone(),
                    reason: reason.clone(),
                };
                if let Err(e) = ctx.database.record_distill_reject(&ctx.agent_id, &record) {
                    tracing::warn!(error = %e, "evidence gate: failed to persist reject record");
                }
            }
            tracing::warn!(
                stage,
                target = %old_note_path,
                reason = %reason,
                "evidence gate: rejecting destructive supersede"
            );
            Some(DistillActionRecord::from_action(
                stage,
                action,
                DistillOutcome::FilteredEvidence,
                Some(reason),
            ))
        }
    }
}

/// Render the "previously rejected — do not re-propose" block for a distill
/// prompt from `(target, summary, reason)` triples (SkillOpt's rejected-edit
/// buffer fed back into reflection). `summary` is the proposed title so the
/// model sees *what* it tried, not just *where*. Empty input → empty string
/// (no block, byte-identical to the pre-feedback prompt). Bounded upstream by
/// the buffer's FIFO cap.
#[must_use]
pub(crate) fn render_rejected_block(rejected: &[(String, String, String)]) -> String {
    if rejected.is_empty() {
        return String::new();
    }
    let entries: Vec<String> = rejected
        .iter()
        .filter(|(target, _, _)| !target.is_empty())
        .map(|(target, summary, reason)| {
            if summary.is_empty() {
                format!("  - supersede of '{target}' — rejected: {reason}")
            } else {
                format!("  - supersede of '{target}' (proposed '{summary}') — rejected: {reason}")
            }
        })
        .collect();
    if entries.is_empty() {
        // Legacy fingerprint-only rows carry no context; nothing to show.
        return String::new();
    }
    format!(
        "Previously REJECTED edits — do NOT re-propose these (the recall-evidence \
         gate already turned them down; propose something better or SKIP):\n{}\n\n",
        entries.join("\n")
    )
}

/// Charge the cycle's shared `EditBudget` ("textual learning rate") for a
/// destructive distill action.
///
/// Only `Supersede` replaces existing knowledge and spends budget; additive
/// `New` / `Strengthen` / `Skip` are free, so the growth path (synthesis →
/// distill of new skills) is never starved by earlier destructive stages.
/// Returns `Some(record)` when the budget is exhausted and the action must be
/// dropped this cycle (recorded as `FilteredEvidence` for the provenance
/// trail); `None` when the action may proceed. Composes with — never replaces
/// — [`gate_action_evidence`]; call it only after the evidence gate passes, so
/// a would-be-rejected supersede does not consume budget.
///
/// Takes `&mut EditBudget` (not the whole `DreamContext`) since the budget is
/// all it touches — which also makes the invariant unit-testable without a
/// full pipeline fixture.
pub(crate) fn charge_distill_budget(
    budget: &mut crate::memory::dreaming::EditBudget,
    action: &distill_action::DistillAction,
    stage: &str,
) -> Option<distill_action::DistillActionRecord> {
    use distill_action::{DistillAction, DistillActionRecord, DistillOutcome};

    let DistillAction::Supersede { title, rule, .. } = action else {
        return None;
    };
    let bytes = (title.len() + rule.len()) as u64;
    if budget.try_spend(bytes) {
        None
    } else {
        tracing::info!(stage, "edit budget exhausted; deferring destructive supersede");
        Some(DistillActionRecord::from_action(
            stage,
            action,
            DistillOutcome::FilteredEvidence,
            Some("edit budget exhausted this cycle".into()),
        ))
    }
}

#[cfg(test)]
mod exhaustion_tests {
    use super::is_provider_exhausted;
    use crate::error::AlephError;

    #[test]
    fn quota_exhausted_403_surfaces_as_authentication_error_and_aborts() {
        // The shape actually observed in production: Kimi/Moonshot returns 403
        // "You've reached your usage limit for this billing cycle".
        let err = AlephError::authentication(
            "kimi",
            "Anthropic authentication failed (403): You've reached your usage limit",
        );
        assert!(is_provider_exhausted(&err));
    }

    #[test]
    fn rate_limit_aborts() {
        assert!(is_provider_exhausted(&AlephError::rate_limit(
            "429 too many requests"
        )));
    }

    #[test]
    fn transient_errors_do_not_abort_the_cycle() {
        assert!(!is_provider_exhausted(&AlephError::NetworkError {
            message: "connection reset".to_string(),
            suggestion: None,
        }));
        assert!(!is_provider_exhausted(&AlephError::other("bad json")));
    }
}

#[cfg(test)]
mod budget_gate_tests {
    use super::{charge_distill_budget, render_rejected_block};
    use crate::memory::dreaming::distill_action::{DistillAction, DistillOutcome};
    use crate::memory::dreaming::EditBudget;
    use crate::memory::notes::Severity;

    fn supersede() -> DistillAction {
        DistillAction::Supersede {
            old_note_path: "skill/foo".into(),
            title: "bar".into(),
            rule: "always baz".into(),
            confidence: 0.9,
            severity: Severity::Med,
            source_facts: vec![],
        }
    }

    #[test]
    fn supersede_spends_budget_and_passes_when_available() {
        let mut budget = EditBudget::new(4, 100_000);
        let before = budget.edits_remaining;
        let out = charge_distill_budget(&mut budget, &supersede(), "skill_distill");
        assert!(out.is_none(), "an affordable supersede must proceed");
        assert_eq!(budget.edits_remaining, before - 1, "one edit must be spent");
    }

    #[test]
    fn supersede_dropped_when_budget_exhausted() {
        let mut budget = EditBudget::new(0, 0);
        let out = charge_distill_budget(&mut budget, &supersede(), "skill_distill");
        match out {
            Some(rec) => assert_eq!(rec.outcome, DistillOutcome::FilteredEvidence),
            None => panic!("exhausted budget must drop the supersede as FilteredEvidence"),
        }
    }

    #[test]
    fn additive_actions_are_free_and_never_spend() {
        // New / Strengthen / Skip must never consume the destructive budget, so
        // the growth path is never starved by earlier destructive stages.
        let mut budget = EditBudget::new(1, 10);
        let new = DistillAction::New {
            title: "t".into(),
            rule: "r".into(),
            confidence: 0.8,
            severity: Severity::Low,
            source_facts: vec![],
        };
        let strengthen = DistillAction::Strengthen {
            existing_note_path: "skill/x".into(),
            source_facts: vec![],
        };
        let skip = DistillAction::Skip {
            source_fact: "f".into(),
            reason: "noise".into(),
        };
        for action in [new, strengthen, skip] {
            assert!(charge_distill_budget(&mut budget, &action, "skill_distill").is_none());
        }
        assert_eq!(budget.edits_remaining, 1, "additive actions must not spend");
        assert_eq!(budget.bytes_remaining, 10);
    }

    #[test]
    fn rejected_block_empty_without_context_rows() {
        // Legacy fingerprint-only rows (empty target) render no block.
        assert!(render_rejected_block(&[]).is_empty());
        assert!(render_rejected_block(&[(String::new(), String::new(), "r".into())]).is_empty());
    }
}
