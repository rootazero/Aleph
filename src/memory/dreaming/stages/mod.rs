//! Dream pipeline stages: trait definition and stage implementations.

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
pub mod types;
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

use async_trait::async_trait;

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

// Re-export types still needed by other modules
pub use types::{MemoryCluster, MetadataGroupKey};

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
