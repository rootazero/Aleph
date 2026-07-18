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
                if let Err(e) = ctx.database.record_distill_reject(&ctx.agent_id, fp) {
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
