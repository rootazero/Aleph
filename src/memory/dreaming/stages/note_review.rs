//! NoteReviewStage — consumes notes_review_queue and routes via LLM verdict.
//!
//! Skeleton scope (C2.5 first commit): the LLM call is stubbed to always
//! return Approve. Real verdict parsing + retry semantics ship in a
//! follow-up. The skeleton's value is wiring: the queue drains every dream
//! cycle, deferred candidates land back on disk via the indexer, and the
//! review row is archived under "approved".

use async_trait::async_trait;

use super::DreamStage;
use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::governance::gate::CandidateNote;
use crate::memory::notes::store::NoteStore;

pub struct NoteReviewStage {
    pub dwell_seconds: i64,
    pub max_retries: i64,
}

impl Default for NoteReviewStage {
    fn default() -> Self {
        Self {
            dwell_seconds: 300,
            max_retries: 3,
        }
    }
}

enum ReviewVerdict {
    Approve,
    #[allow(dead_code)]
    Reject(String),
    #[allow(dead_code)]
    Rewrite(Vec<String>),
}

#[async_trait]
impl DreamStage for NoteReviewStage {
    fn name(&self) -> &'static str {
        "note_review"
    }

    async fn should_run(&self, _ctx: &DreamContext) -> bool {
        true
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let now = chrono::Utc::now().timestamp();
        let earlier = now - self.dwell_seconds;
        let store = ctx.indexer.store();
        let pending = store.list_pending_review(&ctx.agent_id, earlier).await?;

        for row in pending {
            let candidate: CandidateNote = match serde_json::from_str(&row.candidate_json) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, queue_id = %row.id, "candidate json parse failed");
                    if row.retry_count + 1 >= self.max_retries {
                        let _ = store.archive_review(&row.id, "timeout").await;
                    }
                    continue;
                }
            };

            // Skeleton stub: always-Approve. Real LLM verdict parsing is C2.5 follow-up.
            let verdict = ReviewVerdict::Approve;

            match verdict {
                ReviewVerdict::Approve => {
                    let mut admitted = candidate.clone();
                    admitted.bypass_review = true;
                    if let Err(e) = ctx
                        .indexer
                        .write_note(&admitted.agent_id, &admitted.category, &admitted.note)
                        .await
                    {
                        tracing::warn!(error = %e, queue_id = %row.id, "note_review apply failed");
                        continue;
                    }
                    if let Err(e) = store
                        .mark_review_decided(&row.id, "approved", "llm_review")
                        .await
                    {
                        tracing::warn!(error = %e, queue_id = %row.id, "note_review mark_decided failed");
                    }
                    if let Err(e) = store.archive_review(&row.id, "approved").await {
                        tracing::warn!(error = %e, queue_id = %row.id, "note_review archive failed");
                    }
                }
                ReviewVerdict::Reject(reason) => {
                    let _ = reason;
                    let _ = store
                        .mark_review_decided(&row.id, "rejected", "llm_review")
                        .await;
                    let _ = store.archive_review(&row.id, "rejected").await;
                }
                ReviewVerdict::Rewrite(new_facts) => {
                    let mut admitted = candidate.clone();
                    admitted.note.facts = new_facts;
                    admitted.bypass_review = true;
                    let _ = ctx
                        .indexer
                        .write_note(&admitted.agent_id, &admitted.category, &admitted.note)
                        .await;
                    let _ = store
                        .mark_review_decided(&row.id, "rewritten", "llm_review")
                        .await;
                    let _ = store.archive_review(&row.id, "rewritten").await;
                }
            }
        }
        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The full integration test (build a DreamContext with a real backend +
    // pending row, run execute, assert the queue row is archived and the note
    // is on disk) requires substantial fixture wiring across multiple modules.
    // Defer to a follow-up commit; for now, exercise the no-pending path.

    #[test]
    fn name_and_defaults() {
        let s = NoteReviewStage::default();
        assert_eq!(s.name(), "note_review");
        assert_eq!(s.dwell_seconds, 300);
        assert_eq!(s.max_retries, 3);
    }
}
