//! `CompoundIngestor` trait + `DefaultCompoundIngestor` impl.
//!
//! Trait-only in this file so far; the production impl `DefaultCompoundIngestor`
//! is added in Spec 6 T7+T8.

use crate::error::AlephError;
use crate::memory::notes::ingest::plan::ApplyReport;
use crate::memory::store::raw_memory::RawMemory;
use async_trait::async_trait;

use crate::memory::embedding_provider::EmbeddingProvider;
use crate::memory::notes::governance::gate::NoteWriteGate;
use crate::memory::notes::indexer::NoteIndexer;
use crate::memory::notes::ingest::plan::IngestPlan;
use crate::memory::notes::ingest::prompts::build_compound_system_prompt;
use crate::memory::notes::ingest::ref_table::RefTable;
use crate::memory::notes::ingest::retrieve::{RelatedBudget, RelatedPage};
use crate::memory::notes::orientation::NoteOrientation;
use crate::memory::notes::store::NoteStore;
use crate::memory::store::raw_memory::RawMemorySource;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use crate::utils::json_extract::extract_json_robust;
use std::path::PathBuf;
use tracing::warn;

mod batch;
mod helpers;
mod plan_parse;
#[cfg(test)]
mod tests;

use helpers::{build_user_prompt, valid_op};
use plan_parse::{parse_plan_lenient, repair_kind_tags};

pub struct DefaultCompoundIngestor<S: NoteStore + Send + Sync + 'static> {
    pub store: Arc<S>,
    pub indexer: Arc<NoteIndexer<S>>,
    pub provider: Arc<dyn AiProvider>,
    pub embedder: Arc<dyn EmbeddingProvider>,
    pub orientation: Option<Arc<dyn NoteOrientation>>,
    pub memory_dir: PathBuf,
    pub budget: RelatedBudget,
    /// Age ceiling, in seconds, for abandoned `.tx/{id}` apply-staging trees.
    /// `try_apply` sweeps siblings past this age before staging its own, which
    /// is what keeps residue from accumulating inside a resident process (boot
    /// only catches what a dead process left behind).
    ///
    /// Carried as a field rather than read at the call site for the same reason
    /// `full_rebuild_all` takes it as an argument: a sweep whose policy is
    /// implicit is a sweep nobody can turn down. Production passes
    /// `memory.compound_ingest.tx_residue_gc_seconds`.
    pub tx_residue_gc_seconds: u64,
    /// Optional embedding manager. When set, `ingest_batch` pushes touched
    /// notes into the pending queue and flushes once at the tail so vectors
    /// are written without waiting for the next `reembed_all` migration.
    /// `None` keeps the legacy fallback behaviour where vectors are filled
    /// in lazily by the periodic re-embed migration.
    pub embedding_manager: Option<Arc<crate::memory::embedding_manager::EmbeddingManager>>,
    /// Optional admission gate. When set, `ingest_batch` evaluates each
    /// `PageOp::Create` through the gate before staging; `Defer`/`Reject`
    /// outcomes drop the action from the batch (the gate already enqueued
    /// the candidate into `notes_review_queue` for async review). `None`
    /// preserves the pre-governance bypass mode used by tests and any
    /// production wiring that has not yet installed a gate.
    pub gate: Option<Arc<dyn NoteWriteGate>>,
}

impl<S: NoteStore + Send + Sync + 'static> DefaultCompoundIngestor<S> {
    /// Plan a batch and report whether the PLANNER degraded.
    ///
    /// The second element is `true` only when the planning call failed to yield
    /// a usable plan: the response carried no extractable JSON, or the planner
    /// emitted operations and every one of them was dropped (unidentifiable
    /// `kind`, malformed shape, hallucinated `[P<n>]` reference, invalid path).
    /// A planner that read the batch and deliberately returned `ops: []` — the
    /// behaviour every source prompt asks for when nothing clears the bar — is
    /// `false`: there is nothing here to extract and re-running the same call
    /// will keep finding nothing.
    pub(crate) async fn plan_with_health(
        &self,
        _agent_id: &str,
        raws: &[crate::memory::store::raw_memory::RawMemory],
        related: &[RelatedPage],
        source: &RawMemorySource,
        extra_context: Option<&str>,
    ) -> Result<(IngestPlan, bool), AlephError> {
        if raws.is_empty() {
            return Ok((
                IngestPlan {
                    reasoning: String::new(),
                    ops: vec![],
                    schema_proposals: vec![],
                },
                false,
            ));
        }

        let system = build_compound_system_prompt(source);
        let observation_date = chrono::Utc::now().format("%Y-%m-%d (%A)").to_string();
        let user = build_user_prompt(raws, related, &observation_date);
        // X1 C3: fold extension-contributed pre-compress context into the
        // planning prompt so extracted insights survive compression.
        let user = match extra_context {
            Some(extra) if !extra.trim().is_empty() => {
                format!("Extension context (preserve relevant facts):\n{extra}\n\n{user}")
            }
            _ => user,
        };
        let msgs = [UnifiedMessage::user(&user)];
        let resp = self
            .provider
            .process(RequestPayload::new(&msgs).with_system(Some(&system)))
            .await
            .map_err(|e| AlephError::other(format!("compound plan LLM: {e}")))?;
        let text = resp.text_content();

        let json = match extract_json_robust(&text) {
            Some(v) => v,
            None => {
                warn!("compound plan: no JSON in LLM response; returning empty plan");
                return Ok((
                    IngestPlan {
                        reasoning: String::new(),
                        ops: vec![],
                        schema_proposals: vec![],
                    },
                    // Degraded: the planner never got to decide anything. The
                    // raw rows must survive for a retry.
                    true,
                ));
            }
        };

        // How many operations the planner actually emitted, counted BEFORE any
        // repair/parse/resolve pass can drop them. It is the only way to tell
        // "the planner deliberately proposed nothing" (0 emitted) apart from
        // "everything the planner proposed was unusable" (>0 emitted, 0 left).
        let emitted_ops = json
            .get("ops")
            .and_then(|v| v.as_array())
            .map_or(0, Vec::len);

        // Defensive: repair the `kind` discriminator before strict parsing.
        // The LLM frequently omits it despite the prompt; rather than failing
        // the whole batch (which starves the L1 note layer), we infer the op's
        // kind from its field shape and only drop ops that fit no variant.
        let json = repair_kind_tags(json);

        let mut plan = parse_plan_lenient(json);

        // Anti-hallucination: rewrite `[P<n>]` reference tokens back to the
        // exact canonical paths of the related pages, and drop ops whose token
        // is out of range. Raw-path fields pass through unchanged, so planners
        // that still emit full paths keep working. See `ref_table`.
        //
        // Run this even when the related set is EMPTY: on a sparse/empty wiki
        // (or when `gather_related` degraded because the embedding endpoint was
        // down) every `[P<n>]` token is by definition out of range, so this
        // strips the stray tokens the planner emits by imitating the prompt's
        // examples. Without it those tokens leak into notes as literal "[P3]"
        // links (and a token-targeted append/link would forge a `[P3]` orphan
        // page — the exact silent data loss `RefTable` exists to prevent).
        let refs = RefTable::from_related(related);
        let stats = refs.resolve_plan(&mut plan);
        if stats.dropped_ops > 0 || stats.dropped_links > 0 {
            warn!(
                resolved = stats.resolved,
                dropped_ops = stats.dropped_ops,
                dropped_links = stats.dropped_links,
                "compound plan: dropped hallucinated page references"
            );
        }

        plan.ops.retain(valid_op);
        let degraded = emitted_ops > 0 && plan.ops.is_empty();
        Ok((plan, degraded))
    }
}

#[async_trait]
pub trait CompoundIngestor: Send + Sync {
    async fn ingest_batch(
        &self,
        agent_id: &str,
        raws: Vec<RawMemory>,
        extra_context: Option<&str>,
    ) -> Result<ApplyReport, AlephError>;
}
