//! `NoteReviewStage` — consumes `notes_review_queue` and routes via LLM verdict.
//!
//! Each dream cycle, pending review rows older than `dwell_seconds` are read and
//! adjudicated by the LLM (R7: the model makes the semantic verdict; this stage
//! only routes it). The model returns a JSON verdict — `approve` (write the
//! candidate to disk), `reject` (archive and discard), or `rewrite` (write with
//! a corrected `facts` list). A transient provider error or an unparseable
//! verdict leaves the row in the queue for a later cycle rather than
//! rubber-stamping it, so the governance gate's Defer actually holds — but each
//! such failure bumps `retry_count`, and once it reaches `max_retries` the row
//! is evicted (`archive_review` with `max_retries_exceeded`) so a permanently
//! un-adjudicable candidate cannot grow the queue without bound. A
//! provider-wide fault (rate limit / auth) instead aborts the whole stage
//! without charging any row's retry counter — the fault is the provider's, not
//! the row's, and eviction bookkeeping over it would erode the queue on every
//! provider outage.

use async_trait::async_trait;

use super::DreamStage;
use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::governance::gate::CandidateNote;
use crate::memory::notes::ingest::apply::CompoundApplyTx;
use crate::memory::notes::ingest::plan::PageOp;
use crate::memory::notes::store::{NoteStore, ReviewQueueRow};
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;

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
    Reject(String),
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
                    // A parse failure is deterministic — re-reading the same
                    // bytes next cycle fails identically, and nothing on this
                    // path increments `retry_count`, so a retry-gated archive
                    // would reprocess the row forever. Archive immediately.
                    tracing::warn!(error = %e, queue_id = %row.id, "candidate json parse failed; archiving unparseable row");
                    let _ = store.archive_review(&row.id, "unparseable_candidate").await;
                    continue;
                }
            };

            let prompt = build_review_prompt(&row, &candidate);
            let msgs = vec![UnifiedMessage::user(&prompt)];
            let response = match ctx
                .provider
                .process(RequestPayload::new(&msgs).with_system(Some(REVIEW_SYSTEM)))
                .await
            {
                Ok(r) => r,
                Err(e) if super::is_provider_exhausted(&e) => {
                    tracing::warn!(error = %e, "note_review: provider exhausted — aborting dream cycle");
                    return Err(e);
                }
                Err(e) => {
                    // Transient provider failure: leave the row queued so a later
                    // cycle can adjudicate it. Rubber-stamping here would void the
                    // gate's Defer; discarding would lose the candidate. But bump
                    // the retry counter and evict once it has failed `max_retries`
                    // times, so a permanently un-adjudicable row cannot grow the
                    // queue unbounded (the previously-dead `max_retries` field).
                    let retries = store
                        .increment_review_retry(&row.id)
                        .await
                        .unwrap_or(row.retry_count + 1);
                    if retries >= self.max_retries {
                        tracing::warn!(error = %e, queue_id = %row.id, retries, "note_review: LLM call failed max_retries times; evicting row");
                        let _ = store.archive_review(&row.id, "max_retries_exceeded").await;
                    } else {
                        tracing::warn!(error = %e, queue_id = %row.id, retries, "note_review LLM call failed; leaving row queued");
                    }
                    continue;
                }
            };

            let resp_text = response.text_content();
            let verdict = match parse_review_verdict(&resp_text) {
                Some(v) => v,
                None => {
                    // Unparseable verdict: same retry-ceiling eviction as a
                    // transient provider failure — bounded so a row the model
                    // never answers cleanly cannot loop forever.
                    let retries = store
                        .increment_review_retry(&row.id)
                        .await
                        .unwrap_or(row.retry_count + 1);
                    if retries >= self.max_retries {
                        tracing::warn!(queue_id = %row.id, retries, "note_review: unparseable verdict max_retries times; evicting row");
                        let _ = store.archive_review(&row.id, "max_retries_exceeded").await;
                    } else {
                        tracing::warn!(queue_id = %row.id, retries, "note_review: unparseable verdict; leaving row queued");
                    }
                    continue;
                }
            };

            match verdict {
                ReviewVerdict::Approve => {
                    let applied = match candidate.replay_op.clone() {
                        // Deferred non-Create op (e.g. Contradict): replay the
                        // ORIGINAL op through the apply tx so its semantics hold
                        // (append the contradiction marker, merge, rename…). A
                        // plain write_note would overwrite the note with the delta.
                        Some(op_val) => replay_op(&ctx, op_val).await,
                        // Create / materialized note → write directly.
                        None => ctx
                            .indexer
                            .write_note(&candidate.agent_id, &candidate.category, &candidate.note)
                            .await
                            .map(|_| ()),
                    };
                    if let Err(e) = applied {
                        tracing::warn!(error = %e, queue_id = %row.id, "note_review apply failed; leaving row queued");
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
                    // Rewrite supplies corrected facts for a materialized-note
                    // candidate. It has no meaning for a replayed structural op
                    // (Contradict etc.) — leave such a row queued rather than
                    // fabricate a note from a delta.
                    if candidate.replay_op.is_some() {
                        tracing::warn!(queue_id = %row.id, "note_review: rewrite verdict on a replay op is unsupported; leaving row queued");
                        continue;
                    }
                    let mut admitted = candidate.clone();
                    admitted.note.facts = new_facts;
                    // Full facts replacement — a stale verbatim body would
                    // otherwise win over the rewrite on serialization.
                    admitted.note.body = None;
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

        // Retention. `archive_review` is an append-only writer and, until the
        // archive got a reader, nothing had any reason to notice the table only
        // ever grew — one row per gated candidate, each carrying a full note
        // payload, for the life of the install. Pruning belongs here rather
        // than at boot because this is the stage that *writes* the rows: the
        // producer owning its own retention is what keeps the ceiling from
        // becoming a second fact somebody has to remember to maintain.
        // Best-effort — a prune failure must not fail the review pass.
        match store
            .prune_review_archive(&ctx.agent_id, ARCHIVE_RETENTION_SECS)
            .await
        {
            Ok(n) if n > 0 => {
                tracing::info!(pruned = n, "note_review: aged out governance verdicts")
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "note_review: archive prune failed"),
        }

        Ok(ctx)
    }
}

/// How long a decided governance verdict stays readable: 90 days.
///
/// Long enough that "why was this rejected?" is answerable across the months a
/// user actually asks it in, short enough that the table cannot become the
/// largest thing in the database. `insights` shows the most recent handful; the
/// retention window is what stands behind it.
const ARCHIVE_RETENTION_SECS: i64 = 90 * 24 * 3600;

const REVIEW_SYSTEM: &str = "You are a memory governance reviewer. A candidate note was deferred \
    to review because it was low-confidence, high-severity, or contradicts existing knowledge. \
    Decide whether to admit it to long-term memory. Respond with ONLY a JSON object: \
    {\"verdict\":\"approve|reject|rewrite\",\"facts\":[\"...\"],\"reason\":\"...\"}. Use \"approve\" \
    to admit the candidate as-is, \"reject\" to discard it, or \"rewrite\" to admit it with a \
    corrected non-empty \"facts\" list. Include \"facts\" only for \"rewrite\".";

/// Build the adjudication prompt from the queue row (defer reason / confidence /
/// severity) and its candidate note (category / title / facts). Facts are capped
/// so a large note cannot blow the prompt budget.
fn build_review_prompt(row: &ReviewQueueRow, candidate: &CandidateNote) -> String {
    let facts: String = candidate
        .note
        .facts
        .iter()
        .map(|f| format!("- {f}\n"))
        .collect::<String>()
        .chars()
        .take(1500)
        .collect();
    format!(
        "Defer reason: {reason}\n\
         Confidence: {conf:.2}   Severity: {sev}\n\
         Category: {cat}\n\
         Title: {title}\n\
         Facts:\n{facts}\n\
         Should this candidate be admitted to long-term memory?",
        reason = row.reason,
        conf = row.confidence,
        sev = row.severity,
        cat = candidate.category,
        title = candidate.note.title,
    )
}

#[derive(serde::Deserialize)]
struct RawVerdict {
    verdict: String,
    #[serde(default)]
    facts: Vec<String>,
    #[serde(default)]
    reason: String,
}

/// Parse an LLM response into a `ReviewVerdict`. Tolerates prose / code-fence
/// wrapping by extracting the first `{ .. }` span. Returns `None` for anything
/// unrecognised (a rewrite with no replacement facts included) so the caller
/// leaves the row queued rather than admitting an empty body or guessing.
fn parse_review_verdict(text: &str) -> Option<ReviewVerdict> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    let raw: RawVerdict = serde_json::from_str(&text[start..=end]).ok()?;
    match raw.verdict.trim().to_lowercase().as_str() {
        "approve" => Some(ReviewVerdict::Approve),
        "reject" => Some(ReviewVerdict::Reject(raw.reason)),
        "rewrite" if !raw.facts.is_empty() => Some(ReviewVerdict::Rewrite(raw.facts)),
        _ => None,
    }
}

/// Replay a deferred `PageOp` (carried on the candidate as `replay_op`) through
/// the same transactional apply path the ingestor uses, so append / update /
/// contradict / supersede semantics are preserved on approval. The tx borrows
/// `ctx.indexer` (a concrete `NoteIndexer`) and its store; on stage failure the
/// tx's `Drop` cleans up the staged files.
async fn replay_op(ctx: &DreamContext, op_val: serde_json::Value) -> Result<(), AlephError> {
    let op: PageOp = serde_json::from_value(op_val)
        .map_err(|e| AlephError::other(format!("note_review replay_op deserialize: {e}")))?;
    let mut tx = CompoundApplyTx::new(
        &ctx.indexer,
        ctx.indexer.store(),
        ctx.indexer.memory_dir().to_path_buf(),
        &ctx.agent_id,
    );
    tx.stage(&op)
        .await
        .map_err(|e| AlephError::other(format!("note_review replay stage: {e}")))?;
    tx.commit()
        .await
        .map(|_| ())
        .map_err(|e| AlephError::other(format!("note_review replay commit: {e}")))
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

    #[test]
    fn parses_approve() {
        assert!(matches!(
            parse_review_verdict(r#"{"verdict":"approve"}"#),
            Some(ReviewVerdict::Approve)
        ));
    }

    #[test]
    fn parses_reject_with_reason() {
        match parse_review_verdict(r#"{"verdict":"reject","reason":"speculative"}"#) {
            Some(ReviewVerdict::Reject(r)) => assert_eq!(r, "speculative"),
            _ => panic!("expected Reject"),
        }
    }

    #[test]
    fn parses_rewrite_with_facts() {
        match parse_review_verdict(r#"{"verdict":"rewrite","facts":["a","b"]}"#) {
            Some(ReviewVerdict::Rewrite(f)) => assert_eq!(f, vec!["a", "b"]),
            _ => panic!("expected Rewrite"),
        }
    }

    #[test]
    fn rewrite_without_facts_is_unparseable() {
        // An empty-facts rewrite would blank the note body; treat as unparseable
        // so the row stays queued instead of being admitted with no content.
        assert!(parse_review_verdict(r#"{"verdict":"rewrite","facts":[]}"#).is_none());
    }

    #[test]
    fn verdict_is_case_insensitive() {
        assert!(matches!(
            parse_review_verdict(r#"{"verdict":"APPROVE"}"#),
            Some(ReviewVerdict::Approve)
        ));
    }

    #[test]
    fn tolerates_prose_and_code_fence_wrapping() {
        let wrapped = "Sure, here is my verdict:\n```json\n{\"verdict\":\"approve\"}\n```\nDone.";
        assert!(matches!(
            parse_review_verdict(wrapped),
            Some(ReviewVerdict::Approve)
        ));
    }

    #[test]
    fn garbage_and_unknown_verdict_return_none() {
        assert!(parse_review_verdict("not json at all").is_none());
        assert!(parse_review_verdict(r#"{"verdict":"maybe"}"#).is_none());
        assert!(parse_review_verdict("{}").is_none());
    }

    // --- stage-level eviction test (the revived `max_retries` field) ---

    struct ReviewStubEmbedder;
    #[async_trait]
    impl crate::memory::embedding_provider::EmbeddingProvider for ReviewStubEmbedder {
        async fn embed(&self, _t: &str) -> Result<Vec<f32>, AlephError> {
            Ok(Vec::new())
        }
        async fn embed_batch(&self, _t: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(Vec::new())
        }
        fn dimensions(&self) -> usize {
            0
        }
        fn model_name(&self) -> &str {
            "stub"
        }
        fn provider_id(&self) -> &str {
            "stub"
        }
    }

    /// A well-formed candidate the stage can parse, so tests reach the LLM
    /// call rather than the json-parse immediate-archive path.
    fn parseable_candidate_json() -> String {
        use crate::memory::notes::governance::gate::{CandidateNote, NoteWriteAction};
        use crate::memory::notes::KnowledgeNote;

        let candidate = CandidateNote {
            agent_id: "default".into(),
            category: "learning".into(),
            note: KnowledgeNote {
                title: "t".into(),
                category: "learning".into(),
                ..Default::default()
            },
            action: NoteWriteAction::Create,
            bypass_review: false,
            contradicts_existing: false,
            replay_op: None,
        };
        serde_json::to_string(&candidate).unwrap()
    }

    /// Backend + context wired to `provider`, with `candidate_json` already
    /// queued for review. Returns the queue row id so tests can track it.
    async fn queued_review_ctx(
        provider: std::sync::Arc<dyn crate::providers::AiProvider>,
        candidate_json: &str,
    ) -> (
        tempfile::TempDir,
        crate::sync_primitives::Arc<crate::memory::store::SqliteMemoryBackend>,
        String,
        DreamContext,
    ) {
        use crate::memory::dreaming::{DreamReport, DreamStrategy};
        use crate::memory::notes::NoteIndexer;
        use crate::memory::store::SqliteMemoryBackend;
        use crate::sync_primitives::Arc;

        let (temp_guard, temp) = crate::memory::dreaming::scratch_root();
        tokio::fs::create_dir_all(&temp).await.unwrap();
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let indexer = NoteIndexer::new(temp.clone(), store.clone());

        let id = store
            .enqueue_review("default", candidate_json, "med", 0.4, "low confidence")
            .await
            .unwrap();

        let ctx = DreamContext {
            notes: Vec::new(),
            note_contents: std::collections::HashMap::new(),
            agent_id: "default".into(),
            database: store.clone(),
            indexer,
            provider,
            embedder: std::sync::Arc::new(ReviewStubEmbedder),
            report: DreamReport::default(),
            pipeline_type: "consolidate".into(),
            activity_checker: std::sync::Arc::new(|| false),
            strategy: DreamStrategy::Consolidate,
            orientation: None,
            evolution_budget: crate::memory::dreaming::EditBudget::default(),
        };
        (temp_guard, store, id, ctx)
    }

    #[tokio::test]
    async fn unparseable_verdict_evicts_row_at_max_retries() {
        use crate::providers::mock::MockProvider;

        // The MockProvider returns non-JSON, so the verdict is unparseable →
        // the retry/eviction branch runs.
        let provider: std::sync::Arc<dyn crate::providers::AiProvider> =
            std::sync::Arc::new(MockProvider::new("not-a-json-verdict"));
        let (_temp_guard, store, id, ctx) =
            queued_review_ctx(provider, &parseable_candidate_json()).await;

        // dwell_seconds negative → the just-enqueued row is "old enough";
        // max_retries=1 → the first unparseable verdict hits the ceiling.
        let stage = NoteReviewStage {
            dwell_seconds: -100,
            max_retries: 1,
        };
        let _ = stage.execute(ctx).await.unwrap();

        // Row evicted from the pending queue rather than looping forever.
        let pending = store
            .list_pending_review("default", i64::MAX)
            .await
            .unwrap();
        assert!(
            !pending.iter().any(|r| r.id == id),
            "row must be evicted after max_retries unparseable verdicts"
        );
    }

    #[tokio::test]
    async fn provider_rate_limit_aborts_stage_without_charging_retry_counts() {
        use crate::providers::mock::{MockError, MockProvider};

        let provider: std::sync::Arc<dyn crate::providers::AiProvider> = std::sync::Arc::new(
            MockProvider::new("ignored").with_error(MockError::RateLimit("quota".into())),
        );
        let (_temp_guard, store, id, ctx) =
            queued_review_ctx(provider, &parseable_candidate_json()).await;

        let stage = NoteReviewStage {
            dwell_seconds: -100,
            max_retries: 3,
        };
        let result = stage.execute(ctx).await;
        assert!(
            result.is_err(),
            "an exhausted provider must abort the stage instead of looping"
        );

        let pending = store
            .list_pending_review("default", i64::MAX)
            .await
            .unwrap();
        let row = pending
            .iter()
            .find(|r| r.id == id)
            .expect("row must stay queued for a later cycle");
        assert_eq!(
            row.retry_count, 0,
            "a provider-wide fault must not count against the row's eviction budget"
        );
    }

    #[tokio::test]
    async fn unparseable_candidate_is_archived_under_its_own_final_status() {
        use crate::providers::mock::MockProvider;

        let provider: std::sync::Arc<dyn crate::providers::AiProvider> =
            std::sync::Arc::new(MockProvider::new(r#"{"verdict":"approve"}"#));
        let (_temp_guard, store, id, ctx) =
            queued_review_ctx(provider, "definitely-not-json").await;

        let stage = NoteReviewStage {
            dwell_seconds: -100,
            max_retries: 3,
        };
        let _ = stage.execute(ctx).await.unwrap();

        let pending = store
            .list_pending_review("default", i64::MAX)
            .await
            .unwrap();
        assert!(
            !pending.iter().any(|r| r.id == id),
            "an unparseable candidate must not stay queued (retries cannot fix it)"
        );

        // The archived verdict is rendered verbatim to the model by the
        // `insights` action, so the status must name the actual defect —
        // not masquerade as a timeout or any other adjudication outcome.
        let archived = store.list_review_archive("default", 10).await.unwrap();
        let row = archived
            .iter()
            .find(|r| r.id == id)
            .expect("archived row must be reachable through the archive reader");
        assert_eq!(row.final_status, "unparseable_candidate");
    }
}
