//! `CompoundIngestor` trait + `DefaultCompoundIngestor` impl.
//!
//! Trait-only in this file so far; the production impl `DefaultCompoundIngestor`
//! is added in Spec 6 T7+T8.

use crate::error::AlephError;
use crate::memory::notes::ingest::apply::{ApplyError, CompoundApplyTx};
use crate::memory::notes::ingest::plan::ApplyReport;
use crate::memory::notes::ingest::retrieve::gather_related;
use crate::memory::store::raw_memory::RawMemory;
use async_trait::async_trait;

use crate::memory::embedding_provider::EmbeddingProvider;
use crate::memory::notes::governance::gate::{
    CandidateNote, GateOutcome, NoteWriteAction, NoteWriteGate,
};
use crate::memory::notes::indexer::NoteIndexer;
use crate::memory::notes::ingest::plan::{IngestPlan, PageOp, SchemaProposal};
use crate::memory::notes::ingest::prompts::build_compound_system_prompt;
use crate::memory::notes::ingest::ref_table::RefTable;
use crate::memory::notes::ingest::retrieve::{RelatedBudget, RelatedPage};
use crate::memory::notes::orientation::NoteOrientation;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::KnowledgeNote;
use crate::memory::store::raw_memory::RawMemorySource;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use crate::utils::json_extract::extract_json_robust;
use std::path::PathBuf;
use tracing::{info, warn};

pub struct DefaultCompoundIngestor<S: NoteStore + Send + Sync + 'static> {
    pub store: Arc<S>,
    pub indexer: Arc<NoteIndexer<S>>,
    pub provider: Arc<dyn AiProvider>,
    pub embedder: Arc<dyn EmbeddingProvider>,
    pub orientation: Option<Arc<dyn NoteOrientation>>,
    pub memory_dir: PathBuf,
    pub budget: RelatedBudget,
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
    pub async fn plan(
        &self,
        _agent_id: &str,
        raws: &[crate::memory::store::raw_memory::RawMemory],
        related: &[RelatedPage],
        source: &RawMemorySource,
        extra_context: Option<&str>,
    ) -> Result<IngestPlan, AlephError> {
        if raws.is_empty() {
            return Ok(IngestPlan {
                reasoning: String::new(),
                ops: vec![],
                schema_proposals: vec![],
            });
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
                return Ok(IngestPlan {
                    reasoning: String::new(),
                    ops: vec![],
                    schema_proposals: vec![],
                });
            }
        };

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
        Ok(plan)
    }
}

/// Build an `IngestBatchSummary` from an `ApplyReport` by aggregating
/// `touched_paths` (each formatted as `"{category}/{filename}"`) into per-
/// category counts.
///
/// `ApplyReport` does not split per-path created vs updated, so `added` here
/// is conservatively the *total* touched-path count for the category and
/// `updated` is left at 0. This is sufficient for cadence-only consumers
/// (e.g. `refresh_index_after_ingest`); finer-grained breakdowns belong with
/// the planner once `ApplyReport` learns to track them per op.
fn summary_from_report(
    agent_id: &str,
    report: &ApplyReport,
) -> crate::memory::notes::orientation::types::IngestBatchSummary {
    use crate::memory::notes::orientation::types::{IngestBatchSummary, TouchedCategory};
    use std::collections::BTreeMap;

    let mut by_cat: BTreeMap<String, u32> = BTreeMap::new();
    for path in &report.touched_paths {
        if let Some((cat, _name)) = path.split_once('/') {
            *by_cat.entry(cat.to_string()).or_insert(0) += 1;
        }
    }

    IngestBatchSummary {
        agent_id: agent_id.to_string(),
        touched: by_cat
            .into_iter()
            .map(|(category, count)| TouchedCategory {
                category,
                added: count,
                updated: 0,
            })
            .collect(),
    }
}

/// Infer the missing `kind` discriminator from a plan op's field shape.
///
/// The planner LLM frequently emits an operation whose fields already pin down
/// exactly one `PageOp` variant but forgets the `kind` tag. Dropping such an op
/// (the old behaviour) discarded knowledge the model had already extracted, so
/// instead we recover the label it forgot. Checked most-specific field first so
/// the mapping stays unambiguous. Returns `None` when no variant fits.
fn infer_op_kind(op: &serde_json::Map<String, serde_json::Value>) -> Option<&'static str> {
    if op.contains_key("from") && op.contains_key("to") {
        return Some("link");
    }
    if op.contains_key("old_path") && op.contains_key("new_path") {
        return Some("supersede");
    }
    if op.contains_key("new_claim") {
        return Some("contradict");
    }
    if op.contains_key("expected_content_hash") {
        return Some("update");
    }
    if op.contains_key("title") || op.contains_key("summary") {
        return Some("create");
    }
    if op.contains_key("new_facts") || op.contains_key("new_links") {
        return Some("append");
    }
    None
}

/// Repair the `kind` discriminator on both tagged arrays of a raw plan JSON
/// before strict deserialization.
///
/// `ops` entries get `kind` inferred from their shape ([`infer_op_kind`]); any
/// op still unidentifiable is dropped rather than failing the whole batch.
/// `schema_proposals` have no reliable shape to infer from, so kindless ones
/// are simply dropped — leaving them in would make `serde` reject the entire
/// plan with `missing field kind`, starving the L1 note layer.
fn repair_kind_tags(mut value: serde_json::Value) -> serde_json::Value {
    let Some(obj) = value.as_object_mut() else {
        return value;
    };
    if let Some(arr) = obj.get_mut("ops").and_then(|v| v.as_array_mut()) {
        let before = arr.len();
        arr.retain_mut(|op| {
            let Some(o) = op.as_object_mut() else {
                return false;
            };
            if o.get("kind").and_then(|k| k.as_str()).is_some() {
                return true;
            }
            match infer_op_kind(o) {
                Some(k) => {
                    o.insert("kind".to_string(), serde_json::Value::String(k.to_string()));
                    true
                }
                None => false,
            }
        });
        let dropped = before - arr.len();
        if dropped > 0 {
            warn!("compound plan: dropped {dropped} ops with no identifiable kind");
        }
    }
    if let Some(arr) = obj
        .get_mut("schema_proposals")
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|p| {
            p.as_object()
                .and_then(|o| o.get("kind"))
                .and_then(|k| k.as_str())
                .is_some()
        });
    }
    value
}

/// Deserialize an `IngestPlan` element-wise so one malformed item can't sink
/// the whole batch.
///
/// `serde_json::from_value::<IngestPlan>` is all-or-nothing: a single op or
/// schema-proposal that the planner emitted with a missing required field
/// (`kind` on an op, `rationale` on a proposal, etc.) fails the entire parse
/// and starves the L1 note layer. Instead we parse each `ops` / `schema_proposals`
/// entry independently and drop only the entries that fail, keeping every
/// well-formed operation. `reasoning` defaults to empty when absent.
fn parse_plan_lenient(json: serde_json::Value) -> IngestPlan {
    let reasoning = json
        .get("reasoning")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let ops = json
        .get("ops")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|op| match serde_json::from_value::<PageOp>(op.clone()) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        warn!("compound plan: dropping malformed op ({e})");
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let schema_proposals = json
        .get("schema_proposals")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| serde_json::from_value::<SchemaProposal>(p.clone()).ok())
                .collect()
        })
        .unwrap_or_default();

    IngestPlan {
        reasoning,
        ops,
        schema_proposals,
    }
}

#[async_trait]
impl<S: NoteStore + Send + Sync + 'static> CompoundIngestor for DefaultCompoundIngestor<S> {
    async fn ingest_batch(
        &self,
        agent_id: &str,
        raws: Vec<crate::memory::store::raw_memory::RawMemory>,
        extra_context: Option<&str>,
    ) -> Result<ApplyReport, AlephError> {
        if raws.is_empty() {
            return Ok(ApplyReport::default());
        }
        // G2 fix: ensure the agent's orientation files (SCHEMA.md, index.md,
        // log.md) exist before we touch any notes. Dynamically-created agents
        // never get the startup-time bootstrap that the default agent gets.
        // The bootstrap is idempotent and cheap (file existence check + a
        // single write of minimal markdown) so we can call it every batch.
        if let Some(orient) = &self.orientation {
            if let Err(e) = orient.bootstrap(agent_id).await {
                warn!("orientation bootstrap for {agent_id} failed (continuing): {e}");
            }
        }
        let source = raws[0].source.clone();
        // Related-page gathering is best-effort context enrichment for the
        // planning LLM: it needs an embedding round-trip to hybrid-search for
        // related notes. When the embedding endpoint is unavailable
        // (network/quota outage), this MUST NOT abort the whole batch —
        // propagating the error here starves the entire L1 note layer because
        // `compress_to_notes` then returns without marking the raws processed,
        // so they pile up unprocessed forever and no note is ever written.
        // Degrade to an empty related set instead: notes are still planned and
        // written from the raw batch, and vector freshness is backfilled later
        // by the embedding queue / `reembed_all` safety net (P7 graceful
        // degradation).
        let related = match gather_related(
            self.store.clone(),
            self.embedder.clone(),
            &raws,
            agent_id,
            &self.budget,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!(
                    error = %e,
                    "compound ingest: related-page gathering failed (embedding/store); \
                     proceeding without related context"
                );
                Vec::new()
            }
        };

        let mut plan = self
            .plan(agent_id, &raws, &related, &source, extra_context)
            .await?;
        if plan.ops.is_empty() {
            return Ok(ApplyReport::default());
        }

        // Write-time semantic dedup (mem0-style): redirect near-duplicate
        // `Create` ops into `Append`s onto the matching existing note. No-op
        // unless `budget.dedup_enabled`. Runs before the gate so a redirected
        // Append (which targets an already-admitted note) bypasses re-gating.
        plan.ops = self
            .dedup_redirect_creates(agent_id, plan.ops, &related)
            .await;

        // Governance gate: scoped to PageOp::Create in this commit. Other
        // PageOp variants (Append/Update/Contradict/Link/Supersede) pass
        // through unchanged and will be gated in a follow-up commit. When
        // `self.gate` is `None` the entire pre-filter is skipped, preserving
        // backward compatibility for tests and any production wiring that
        // has not yet installed a gate.
        if self.gate.is_some() {
            plan.ops = self.filter_ops_through_gate(agent_id, plan.ops).await?;
            if plan.ops.is_empty() {
                return Ok(ApplyReport::default());
            }
        }

        let report = match self.try_apply(agent_id, &plan).await {
            Ok(r) => r,
            Err(ApplyError::HashConflict { path, actual, .. }) => {
                warn!("compound ingest: hash conflict on {path}; re-planning");
                let mut augmented = raws.clone();
                if let Some(last) = augmented.last_mut() {
                    last.content.push_str(&format!(
                        "\n\n[system] previous plan referenced {path} with a stale hash; actual hash is {actual}. Re-plan using fresh data."
                    ));
                }
                let mut plan2 = self
                    .plan(agent_id, &augmented, &related, &source, extra_context)
                    .await?;
                if plan2.ops.is_empty() {
                    return Ok(ApplyReport::default());
                }
                plan2.ops = self
                    .dedup_redirect_creates(agent_id, plan2.ops, &related)
                    .await;
                if self.gate.is_some() {
                    plan2.ops = self.filter_ops_through_gate(agent_id, plan2.ops).await?;
                    if plan2.ops.is_empty() {
                        return Ok(ApplyReport::default());
                    }
                }
                self.try_apply(agent_id, &plan2)
                    .await
                    .map_err(|e| match e {
                        ApplyError::Other(inner) => inner,
                        other => AlephError::other(format!("apply after re-plan: {other}")),
                    })?
            }
            Err(ApplyError::Other(e)) => return Err(e),
        };

        if let Some(orient) = &self.orientation {
            let reasoning_preview: String = plan.reasoning.chars().take(80).collect();
            let detail: Vec<String> = report
                .touched_paths
                .iter()
                .take(15)
                .map(|p| format!("touched {p}"))
                .collect();
            let entry = crate::memory::notes::orientation::types::LogEntry {
                timestamp_utc: chrono::Utc::now().timestamp(),
                action: crate::memory::notes::orientation::types::LogAction::Ingest,
                summary: format!(
                    "{} pages touched | tx={} | {}",
                    report.touched_paths.len(),
                    report.tx_id,
                    reasoning_preview
                ),
                detail_lines: detail,
            };
            if let Err(e) = orient.record_ingest(agent_id, entry).await {
                warn!("compound ingest: log record failed: {e}");
            }
        }

        // Forward-compatible: tell orientation which categories were touched in
        // this batch so it can refresh `index.md` immediately. Best-effort —
        // failures are logged and ignored so the ingest still returns success;
        // the next dream cycle's full `rebuild_index` will reconcile.
        if let Some(orient) = &self.orientation {
            let summary = summary_from_report(agent_id, &report);
            if !summary.touched.is_empty() {
                if let Err(e) = orient.refresh_index_after_ingest(agent_id, &summary).await {
                    warn!(
                        "ingest_batch: refresh_index_after_ingest failed (non-fatal); \
                         next dream cycle will reconcile: {e}"
                    );
                }
            }
        }

        // Embedding queue — best-effort push for each touched note, then a
        // single flush. Failures are logged at warn; the next `reembed_all`
        // migration is the safety net. Mirrors `reembed_agent_notes`'s
        // body-extraction pattern: read the freshly written file from disk
        // and pass its full content (frontmatter + body) to the embedder.
        if let Some(em) = &self.embedding_manager {
            for path in &report.touched_paths {
                let safe_path = path.replace("..", "").replace('\\', "/");
                let file = self
                    .memory_dir
                    .join(agent_id)
                    .join(format!("{safe_path}.md"));
                match tokio::fs::read_to_string(&file).await {
                    Ok(content) => {
                        em.push_pending(agent_id, path, &content).await;
                    }
                    Err(e) => {
                        warn!(
                            path = %file.display(),
                            error = %e,
                            "ingest_batch: failed to read note for embedding push"
                        );
                    }
                }
            }
            if let Err(e) = em.flush_pending(&*self.store, 64).await {
                warn!(error = %e, "ingest_batch: flush_pending failed");
            }
        }

        Ok(report)
    }
}

impl<S: NoteStore + Send + Sync + 'static> DefaultCompoundIngestor<S> {
    async fn try_apply(
        &self,
        agent_id: &str,
        plan: &IngestPlan,
    ) -> Result<ApplyReport, ApplyError> {
        let mut tx = CompoundApplyTx::new(
            &self.indexer,
            &self.store,
            self.memory_dir.clone(),
            agent_id,
        );
        for op in &plan.ops {
            tx.stage(op).await?;
        }
        tx.commit().await
    }

    /// mem0-style write-time semantic dedup. For each planned `PageOp::Create`,
    /// embed the candidate note's text and compare it (exact cosine, so the
    /// decision is independent of the store's vec0 distance metric) against the
    /// stored embeddings of the already-gathered related pages. When the
    /// nearest related page meets `dedup_similarity_threshold`, the `Create` is
    /// rewritten into an `Append` onto that page — the genuinely-new facts
    /// merge into the existing note (Append dedups facts intra-note) instead of
    /// spawning a near-duplicate page.
    ///
    /// This stops obvious duplicates at the door cheaply and synchronously
    /// (no extra LLM call → R7/R10-safe), while Aleph keeps its richer offline
    /// dream-consolidation on top — surpassing mem0, which only drops the dup.
    ///
    /// Returns `ops` unchanged when dedup is disabled, the related set is
    /// empty, or embeddings are unavailable (graceful degradation, P7).
    async fn dedup_redirect_creates(
        &self,
        agent_id: &str,
        ops: Vec<PageOp>,
        related: &[RelatedPage],
    ) -> Vec<PageOp> {
        if !self.budget.dedup_enabled || related.is_empty() {
            return ops;
        }
        let dedup_threshold = self.budget.dedup_similarity_threshold.clamp(0.0, 1.0);
        let noop_threshold = self
            .budget
            .dedup_noop_threshold
            .clamp(0.0, 1.0)
            .max(dedup_threshold);
        let dim = self.embedder.dimensions() as u32;

        // Stored vectors for the related pages. Pages without a vector at this
        // dim (e.g. link-expanded pages not yet embedded) are simply skipped.
        let mut related_vecs: Vec<(&str, Vec<f32>)> = Vec::with_capacity(related.len());
        for rp in related {
            if let Ok(Some(v)) = self.store.get_embedding(&rp.path, agent_id, dim).await {
                if !v.is_empty() {
                    related_vecs.push((rp.path.as_str(), v));
                }
            }
        }
        if related_vecs.is_empty() {
            return ops;
        }

        // Batch-embed every Create candidate in a single round-trip.
        let mut create_idx: Vec<usize> = Vec::new();
        let mut create_texts: Vec<String> = Vec::new();
        for (i, op) in ops.iter().enumerate() {
            if let PageOp::Create {
                title,
                summary,
                facts,
                ..
            } = op
            {
                create_idx.push(i);
                create_texts.push(candidate_dedup_text(title, summary, facts));
            }
        }
        if create_idx.is_empty() {
            return ops;
        }
        let text_refs: Vec<&str> = create_texts.iter().map(String::as_str).collect();
        let cand_vecs = match self.embedder.embed_batch(&text_refs).await {
            Ok(v) if v.len() == create_idx.len() => v,
            // Degrade: embedding endpoint down or size mismatch → keep Creates.
            _ => return ops,
        };

        // For each Create, classify its best related match into three tiers:
        //   sim >= noop_threshold         → NOOP  (drop the Create)
        //   dedup_threshold <= sim < noop → MERGE (Create → Append)
        //   sim < dedup_threshold         → ADD   (keep the Create)
        // Never self-redirecting onto the Create's own path.
        use std::collections::{HashMap, HashSet};
        let mut redirect: HashMap<usize, String> = HashMap::new();
        let mut drop_noop: HashSet<usize> = HashSet::new();
        for (slot, &op_i) in create_idx.iter().enumerate() {
            let PageOp::Create { note_path, .. } = &ops[op_i] else {
                continue;
            };
            let cand = &cand_vecs[slot];
            let mut best: Option<(&str, f32)> = None;
            for (path, vec) in &related_vecs {
                if *path == note_path.as_str() {
                    continue;
                }
                let sim = cosine_similarity(cand, vec);
                if best.is_none_or(|(_, b)| sim > b) {
                    best = Some((*path, sim));
                }
            }
            if let Some((path, sim)) = best {
                if sim >= noop_threshold {
                    drop_noop.insert(op_i);
                } else if sim >= dedup_threshold {
                    redirect.insert(op_i, path.to_string());
                }
            }
        }
        if redirect.is_empty() && drop_noop.is_empty() {
            return ops;
        }

        // Rewrite: NOOP Creates are dropped; MERGE Creates become Append onto the
        // matched existing note (the existing page owns its title/summary, so
        // only the candidate's facts and links carry over); everything else
        // passes through.
        ops.into_iter()
            .enumerate()
            .filter_map(|(i, op)| {
                if drop_noop.contains(&i) {
                    if let PageOp::Create { note_path, .. } = &op {
                        info!(
                            note = %note_path,
                            "ingest dedup: dropping near-identical Create as NOOP"
                        );
                    }
                    return None;
                }
                match (redirect.remove(&i), op) {
                    (
                        Some(target),
                        PageOp::Create {
                            note_path,
                            facts,
                            links,
                            ..
                        },
                    ) => {
                        info!(
                            from = %note_path,
                            into = %target,
                            "ingest dedup: redirecting near-duplicate Create into Append"
                        );
                        Some(PageOp::Append {
                            note_path: target,
                            new_facts: facts,
                            new_links: links,
                            new_relations: vec![],
                        })
                    }
                    (_, op) => Some(op),
                }
            })
            .collect()
    }

    /// Run each `PageOp::Create` through `self.gate` and drop ops whose
    /// outcome is `Defer` (already enqueued by the gate) or `Reject`. Other
    /// op kinds pass through unchanged in this scoped commit; their gating
    /// ships in a follow-up. Returns the filtered op vector.
    ///
    /// Caller must ensure `self.gate.is_some()` before invoking; if it is
    /// `None` the input vector is returned unchanged.
    async fn filter_ops_through_gate(
        &self,
        agent_id: &str,
        ops: Vec<PageOp>,
    ) -> Result<Vec<PageOp>, AlephError> {
        let Some(gate) = self.gate.as_ref() else {
            return Ok(ops);
        };
        let mut out: Vec<PageOp> = Vec::with_capacity(ops.len());
        for op in ops {
            match candidate_from_pageop(agent_id, &op) {
                Some(candidate) => match gate.evaluate(&candidate).await? {
                    GateOutcome::Accept(_) => out.push(op),
                    GateOutcome::Defer { queue_id, reason } => {
                        info!(
                            queue_id = %queue_id,
                            reason = %reason,
                            note_path = %op.primary_path(),
                            "ingest deferred to review queue"
                        );
                    }
                    GateOutcome::Reject { archive_id, reason } => {
                        warn!(
                            archive_id = %archive_id,
                            reason = %reason,
                            note_path = %op.primary_path(),
                            "ingest rejected at gate"
                        );
                    }
                },
                // Op kinds the gate does not understand in this scoped
                // commit (Append/Update/Contradict/Link/Supersede) pass
                // through unchanged.
                None => out.push(op),
            }
        }
        Ok(out)
    }
}

/// Build a `CandidateNote` for the gate from a `PageOp`. Returns `None` for
/// op kinds that this scoped commit does not gate (Append/Update/Contradict/
/// Link/Supersede). The candidate's `confidence` and `severity` come from
/// the LLM-generated note shape; in this scoped commit `PageOp::Create`
/// does not carry those fields, so we use `KnowledgeNote::default()` (which
/// sets `confidence = 1.0` and `severity = Low`) so the gate's default
/// thresholds (`min_confidence = 0.5`, `high_severity_min_confidence = 0.8`)
/// admit them. Confidence/severity wiring on `PageOp::Create` is a
/// follow-up task once the planner prompt is updated to emit them.
fn candidate_from_pageop(agent_id: &str, op: &PageOp) -> Option<CandidateNote> {
    match op {
        PageOp::Create {
            note_path,
            title,
            facts,
            links,
            tags,
            ..
        } => {
            let category = note_path
                .split_once('/')
                .map(|(c, _)| c.to_string())
                .unwrap_or_default();
            let note = KnowledgeNote {
                title: title.clone(),
                category: category.clone(),
                tags: tags.clone(),
                facts: facts.clone(),
                links: links.clone(),
                ..KnowledgeNote::default()
            };
            Some(CandidateNote {
                agent_id: agent_id.to_string(),
                category,
                note,
                source_path: None,
                fact_provenance: Vec::new(),
                action: NoteWriteAction::Create,
                bypass_review: false,
                contradicts_existing: false,
            })
        }
        // Scoped out in this commit; pass-through.
        PageOp::Append { .. }
        | PageOp::Update { .. }
        | PageOp::Contradict { .. }
        | PageOp::Link { .. }
        | PageOp::Supersede { .. } => None,
    }
}

fn build_user_prompt(
    raws: &[crate::memory::store::raw_memory::RawMemory],
    related: &[RelatedPage],
    observation_date: &str,
) -> String {
    // Temporal grounding: give the model an absolute anchor so it can resolve
    // relative time references ("today", "next week") into permanent dates
    // before they are written into a fact. Without this, an Ephemeral/Contextual
    // fact like "user wants to focus on docs today" becomes uninterpretable on
    // later recall. The model does the resolution (R9: intelligence in prompt).
    let mut out = format!(
        "## Observation date\n\n\
         The current date is {observation_date}. Resolve every relative time \
         reference (\"today\", \"yesterday\", \"next week\", \"in 3 days\", \"last \
         month\") to an absolute date before writing it into a fact, so the memory \
         stays interpretable when recalled later.\n\n"
    );
    out.push_str("## New raw memories\n\n");
    for (i, r) in raws.iter().enumerate() {
        out.push_str(&format!(
            "### raw-{} (id={}, source={:?})\n",
            i + 1,
            r.id,
            r.source
        ));
        out.push_str(&r.content);
        out.push_str("\n\n");
        if let Some(att) = &r.attachment_text {
            out.push_str("[Attachment]\n");
            out.push_str(att);
            out.push_str("\n\n");
        }
    }
    if !related.is_empty() {
        out.push_str(
            "## Related existing pages\n\n\
             Each page below carries a `[P<n>]` reference token. To act on an \
             EXISTING page (append / update / contradict / link / supersede, or \
             a create's `links`), put its token in the path field instead of \
             retyping the path — the system resolves tokens to exact paths.\n\n",
        );
        for (i, p) in related.iter().enumerate() {
            out.push_str(&format!(
                "### {token} path={path} (hash={hash})\n",
                token = RefTable::token(i),
                path = p.path,
                hash = p.content_hash
            ));
            out.push_str(&format!("title: {}\n", p.title));
            if !p.tags.is_empty() {
                out.push_str(&format!("tags: {}\n", p.tags.join(", ")));
            }
            out.push_str("preview:\n");
            out.push_str(&p.content_preview);
            out.push_str("\n\n");
        }
    } else {
        out.push_str("## Related existing pages\n\n(none — empty wiki or no matches)\n");
    }
    out.push_str(
        "Reminder: every object in `ops` and `schema_proposals` MUST begin with \
         its `kind` field. Omitting `kind` makes the operation invalid.\n\n",
    );
    out.push_str("Produce the IngestPlan JSON now.");
    out
}

fn valid_op(op: &PageOp) -> bool {
    match op {
        // A `Create` only needs a well-formed `category/filename` path. It is
        // deliberately NOT required to carry links: on a sparse/just-bootstrapped
        // wiki (or when `gather_related` degraded to an empty set because the
        // embedding endpoint was down) there are no existing pages to link to,
        // and the planner's `[P<n>]` link tokens get stripped as out-of-range by
        // `RefTable::resolve_plan`. Dropping the resulting linkless Create here
        // silently discarded knowledge the model had already extracted — the
        // dominant reason the L1 note layer could not grow past its seed. A seed
        // note is recoverable (dream consolidation links it later); discarded
        // knowledge is gone forever. This mirrors the anti-starvation degradation
        // in `ingest_batch` (related-gathering failure must not starve the layer).
        PageOp::Create { note_path, .. } => note_path.contains('/'),
        PageOp::Append { note_path, .. }
        | PageOp::Update { note_path, .. }
        | PageOp::Contradict { note_path, .. } => note_path.contains('/'),
        PageOp::Link { from, to } => from.contains('/') && to.contains('/') && from != to,
        PageOp::Supersede { old_path, new_path } => {
            old_path.contains('/') && new_path.contains('/') && old_path != new_path
        }
    }
}

/// Build the probe text for a `Create` candidate in the write-time dedup gate.
/// Mirrors the salient content of a note (human title, summary, facts) so the
/// candidate's embedding is comparable to a stored note's vector.
fn candidate_dedup_text(title: &str, summary: &str, facts: &[String]) -> String {
    let mut s = String::new();
    if !title.is_empty() {
        s.push_str(title);
        s.push('\n');
    }
    if !summary.is_empty() {
        s.push_str(summary);
        s.push('\n');
    }
    for f in facts {
        s.push_str(f);
        s.push('\n');
    }
    s
}

/// Cosine similarity in `[-1, 1]`. Returns `0.0` for mismatched-length,
/// empty, or zero-norm vectors so such pairs are treated as "not similar"
/// and never trigger a dedup redirect.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
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

#[cfg(test)]
mod trait_tests {
    use super::*;

    struct StubIngestor;

    #[async_trait]
    impl CompoundIngestor for StubIngestor {
        async fn ingest_batch(
            &self,
            _agent_id: &str,
            _raws: Vec<RawMemory>,
            _extra_context: Option<&str>,
        ) -> Result<ApplyReport, AlephError> {
            Ok(ApplyReport {
                tx_id: "stub".into(),
                ..Default::default()
            })
        }
    }

    #[tokio::test]
    async fn trait_object_dispatch() {
        let ing: Box<dyn CompoundIngestor> = Box::new(StubIngestor);
        let r = ing.ingest_batch("default", vec![], None).await.unwrap();
        assert_eq!(r.tx_id, "stub");
    }
}

#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource};
    use crate::memory::store::SqliteMemoryBackend;
    use crate::providers::recording_mock::RecordingMockProvider;

    async fn mk() -> (
        tempfile::TempDir,
        Arc<SqliteMemoryBackend>,
        Arc<NoteIndexer<SqliteMemoryBackend>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = Arc::new(NoteIndexer::new(dir.path().join("note"), backend.clone()));
        (dir, backend, indexer)
    }

    #[tokio::test]
    async fn plan_parses_valid_json() {
        let (dir, backend, indexer) = mk().await;
        let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
            r#"{
              "reasoning": "new page + link",
              "ops": [
                {"kind": "create", "note_path": "learning/tokio", "title": "Tokio",
                 "summary": "async runtime", "facts": ["event loop"],
                 "links": ["learning/rust-async"], "tags": ["rust"]}
              ],
              "schema_proposals": []
            }"#
            .into(),
        ));
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
            embedding_manager: None,
            gate: None,
        };
        let raw = RawMemory::new("some content".to_string(), RawMemorySource::Transcript);
        let plan = ing
            .plan("default", &[raw], &[], &RawMemorySource::Transcript, None)
            .await
            .unwrap();
        assert_eq!(plan.ops.len(), 1);
        match &plan.ops[0] {
            PageOp::Create { note_path, .. } => assert_eq!(note_path, "learning/tokio"),
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn plan_returns_empty_on_invalid_json() {
        let (dir, backend, indexer) = mk().await;
        let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new("not json".into()));
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
            embedding_manager: None,
            gate: None,
        };
        let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
        let plan = ing
            .plan("default", &[raw], &[], &RawMemorySource::Transcript, None)
            .await
            .unwrap();
        assert!(plan.ops.is_empty());
    }

    #[tokio::test]
    async fn plan_filters_invalid_ops() {
        let (dir, backend, indexer) = mk().await;
        let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
            r#"{"ops":[
                {"kind":"create","note_path":"learning/x","title":"X","summary":"","facts":[],"links":[],"tags":[]},
                {"kind":"create","note_path":"bad-no-slash","title":"Y","summary":"","facts":[],"links":["learning/x"],"tags":[]},
                {"kind":"append","note_path":"learning/y","new_facts":["f"],"new_links":[]}
            ]}"#.into()));
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
            embedding_manager: None,
            gate: None,
        };
        let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
        let plan = ing
            .plan("default", &[raw], &[], &RawMemorySource::Transcript, None)
            .await
            .unwrap();
        // The no-slash create is dropped (malformed path); the linkless create
        // with a valid path is KEPT as a seed note (see `valid_op`), and the
        // append is kept. Linkless creates are no longer discarded — that silent
        // drop was starving the L1 note layer on a sparse wiki.
        assert_eq!(plan.ops.len(), 2);
        assert!(plan.ops.iter().any(|op| matches!(
            op,
            PageOp::Create { note_path, links, .. } if note_path == "learning/x" && links.is_empty()
        )));
        assert!(
            !plan.ops.iter().any(
                |op| matches!(op, PageOp::Create { note_path, .. } if note_path == "bad-no-slash")
            ),
            "no-slash create must still be dropped"
        );
    }

    /// Bootstrap regression: on a sparse wiki the planner emits a `create`
    /// whose only links are out-of-range `[P<n>]` tokens (e.g. the few-shot
    /// examples use `[P3]` but no related pages are shown). `RefTable` strips
    /// the hallucinated link, leaving the create linkless — it must STILL
    /// survive as a seed note instead of being silently dropped, otherwise the
    /// note layer can never grow past its seed. This is the exact live failure
    /// observed on agent `main` (notes_index stuck at 1).
    #[tokio::test]
    async fn plan_keeps_seed_create_when_all_link_tokens_hallucinated() {
        let (dir, backend, indexer) = mk().await;
        let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
            r#"{"ops":[
                {"kind":"create","note_path":"system/video-config","title":"Video config",
                 "summary":"durable fact","facts":["The user configured a custom video provider."],
                 "links":["[P3]"],"tags":["system"]}
            ]}"#
            .into(),
        ));
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
            embedding_manager: None,
            gate: None,
        };
        // No related pages (sparse wiki) → `[P3]` is out of range and stripped.
        let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
        let plan = ing
            .plan("default", &[raw], &[], &RawMemorySource::Transcript, None)
            .await
            .unwrap();
        assert_eq!(plan.ops.len(), 1, "seed create must survive stripped links");
        match &plan.ops[0] {
            PageOp::Create {
                note_path, links, ..
            } => {
                assert_eq!(note_path, "system/video-config");
                assert!(links.is_empty(), "hallucinated link token must be stripped");
            }
            other => panic!("expected surviving Create, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn plan_resolves_reference_token_to_canonical_path() {
        let (dir, backend, indexer) = mk().await;
        // LLM emits an append targeting the related page by its [P0] token
        // rather than retyping the path. Resolution rewrites it exactly.
        let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
            r#"{"ops":[
                {"kind":"append","note_path":"[P0]","new_facts":["new fact"],"new_links":[]}
            ]}"#
            .into(),
        ));
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
            embedding_manager: None,
            gate: None,
        };
        let related = vec![RelatedPage {
            path: "preference/coding-style".into(),
            title: "coding-style".into(),
            summary: String::new(),
            content_preview: String::new(),
            tags: vec![],
            content_hash: "h0".into(),
            score: 1.0,
        }];
        let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
        let plan = ing
            .plan(
                "default",
                &[raw],
                &related,
                &RawMemorySource::Transcript,
                None,
            )
            .await
            .unwrap();
        assert_eq!(plan.ops.len(), 1);
        match &plan.ops[0] {
            PageOp::Append { note_path, .. } => assert_eq!(note_path, "preference/coding-style"),
            _ => panic!("expected append"),
        }
    }

    #[tokio::test]
    async fn plan_drops_op_with_hallucinated_token() {
        let (dir, backend, indexer) = mk().await;
        // [P9] is out of range (only P0 exists) → the op is dropped, not
        // applied against a forged orphan page.
        let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
            r#"{"ops":[
                {"kind":"append","note_path":"[P9]","new_facts":["x"],"new_links":[]}
            ]}"#
            .into(),
        ));
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
            embedding_manager: None,
            gate: None,
        };
        let related = vec![RelatedPage {
            path: "preference/coding-style".into(),
            title: "coding-style".into(),
            summary: String::new(),
            content_preview: String::new(),
            tags: vec![],
            content_hash: "h0".into(),
            score: 1.0,
        }];
        let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
        let plan = ing
            .plan(
                "default",
                &[raw],
                &related,
                &RawMemorySource::Transcript,
                None,
            )
            .await
            .unwrap();
        assert!(plan.ops.is_empty(), "hallucinated-token op must be dropped");
    }

    #[test]
    fn build_user_prompt_renders_reference_tokens() {
        let raws = vec![RawMemory::new("hello".into(), RawMemorySource::Transcript)];
        let related = vec![
            RelatedPage {
                path: "preference/coding-style".into(),
                title: "coding-style".into(),
                summary: String::new(),
                content_preview: "prefers vim".into(),
                tags: vec!["tool".into()],
                content_hash: "h0".into(),
                score: 1.0,
            },
            RelatedPage {
                path: "personal/li-wei".into(),
                title: "li-wei".into(),
                summary: String::new(),
                content_preview: String::new(),
                tags: vec![],
                content_hash: "h1".into(),
                score: 0.5,
            },
        ];
        let prompt = build_user_prompt(&raws, &related, "2026-01-15 (Thursday)");
        assert!(prompt.contains("[P0] path=preference/coding-style"));
        assert!(prompt.contains("[P1] path=personal/li-wei"));
        assert!(prompt.contains("reference token"));
    }

    #[test]
    fn build_user_prompt_injects_observation_date() {
        let raws = vec![RawMemory::new("hello".into(), RawMemorySource::Transcript)];
        let prompt = build_user_prompt(&raws, &[], "2026-01-15 (Thursday)");
        assert!(prompt.contains("## Observation date"));
        assert!(prompt.contains("2026-01-15 (Thursday)"));
        assert!(
            prompt.contains("absolute date"),
            "must instruct the model to resolve relative time"
        );
    }

    #[test]
    fn build_user_prompt_reminds_about_kind_field() {
        let raws = vec![RawMemory::new("hello".into(), RawMemorySource::Transcript)];
        let prompt = build_user_prompt(&raws, &[], "2026-01-15 (Thursday)");
        assert!(
            prompt.contains("`kind`"),
            "prompt must remind the model to emit the kind discriminator"
        );
    }

    #[test]
    fn infer_op_kind_maps_each_variant_shape() {
        let cases = [
            (serde_json::json!({"from": "a/b", "to": "c/d"}), "link"),
            (
                serde_json::json!({"old_path": "a/b", "new_path": "c/d"}),
                "supersede",
            ),
            (
                serde_json::json!({"note_path": "a/b", "new_claim": "x"}),
                "contradict",
            ),
            (
                serde_json::json!({"note_path": "a/b", "expected_content_hash": "h"}),
                "update",
            ),
            (
                serde_json::json!({"note_path": "a/b", "title": "T", "summary": "S"}),
                "create",
            ),
            (
                serde_json::json!({"note_path": "a/b", "new_facts": ["f"]}),
                "append",
            ),
        ];
        for (op, expected) in cases {
            let obj = op.as_object().unwrap();
            assert_eq!(infer_op_kind(obj), Some(expected), "shape: {op}");
        }
        // Unidentifiable shape → None.
        assert_eq!(
            infer_op_kind(serde_json::json!({"note_path": "a/b"}).as_object().unwrap()),
            None
        );
    }

    #[test]
    fn repair_kind_tags_recovers_kindless_plan() {
        // The dominant failure: ops lack `kind`, and a schema_proposal also
        // lacks `kind` (which used to hard-fail the whole parse).
        let raw = serde_json::json!({
            "reasoning": "r",
            "ops": [
                {"note_path": "learning/tokio", "title": "Tokio", "summary": "rt",
                 "facts": ["x"], "links": ["learning/rust"], "tags": []},
                {"kind": "append", "note_path": "learning/rust", "new_facts": ["y"]},
                {"foo": "bar"}
            ],
            "schema_proposals": [
                {"tag": "rust", "rationale": "r"},
                {"kind": "new_tag", "tag": "async", "rationale": "r"}
            ]
        });
        let repaired = repair_kind_tags(raw);
        // The kindless create is recovered, the explicit append kept, the
        // unidentifiable op dropped → 2 ops.
        let ops = repaired["ops"].as_array().unwrap();
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0]["kind"], "create");
        assert_eq!(ops[1]["kind"], "append");
        // Kindless schema_proposal dropped, valid one kept.
        let props = repaired["schema_proposals"].as_array().unwrap();
        assert_eq!(props.len(), 1);
        assert_eq!(props[0]["kind"], "new_tag");
        // Crucially: the repaired JSON now deserializes into a real plan.
        let plan: IngestPlan = serde_json::from_value(repaired).unwrap();
        assert_eq!(plan.ops.len(), 2);
        assert_eq!(plan.schema_proposals.len(), 1);
    }

    #[test]
    fn parse_plan_lenient_keeps_good_op_despite_malformed_proposal() {
        // Regression for the live failure: the planner emitted a valid create
        // op AND a schema_proposal missing its required `rationale` field. The
        // old all-or-nothing parse failed the whole batch (`missing field
        // rationale`), discarding the good op. Element-wise parsing must keep
        // the op and drop only the malformed proposal.
        let json = serde_json::json!({
            "reasoning": "extract durable ops fact",
            "ops": [
                {"kind": "create", "note_path": "reference/aws-tokyo", "title": "AWS Tokyo",
                 "summary": "prod db region", "facts": ["ap-northeast-1"],
                 "links": ["reference/ops"], "tags": ["ops"]}
            ],
            "schema_proposals": [
                {"kind": "new_tag", "tag": "ops"}
            ]
        });
        let plan = parse_plan_lenient(json);
        assert_eq!(plan.ops.len(), 1, "valid create op must survive");
        assert_eq!(
            plan.schema_proposals.len(),
            0,
            "proposal missing required `rationale` is dropped, not fatal"
        );
        assert_eq!(plan.reasoning, "extract durable ops fact");
    }

    #[test]
    fn summary_from_report_aggregates_touched_paths_by_category() {
        let report = ApplyReport {
            tx_id: "tx".into(),
            touched_paths: vec![
                "learning/rust-async".into(),
                "learning/tokio".into(),
                "preference/editor".into(),
                "no-slash-bad-path".into(), // dropped by split_once('/')
            ],
            ..Default::default()
        };
        let summary = summary_from_report("default", &report);
        assert_eq!(summary.agent_id, "default");
        // BTreeMap ordering: "learning" < "preference" alphabetically.
        assert_eq!(summary.touched.len(), 2);
        assert_eq!(summary.touched[0].category, "learning");
        assert_eq!(summary.touched[0].added, 2);
        assert_eq!(summary.touched[0].updated, 0);
        assert_eq!(summary.touched[1].category, "preference");
        assert_eq!(summary.touched[1].added, 1);
    }

    #[test]
    fn summary_from_report_empty_when_no_touched_paths() {
        let report = ApplyReport::default();
        let summary = summary_from_report("default", &report);
        assert!(summary.touched.is_empty());
    }

    #[tokio::test]
    async fn ingest_batch_refreshes_index_md_at_tail() {
        use crate::memory::notes::orientation::FsNoteOrientation;

        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("note");
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), backend.clone()));

        let orient: Arc<dyn NoteOrientation> =
            Arc::new(FsNoteOrientation::new(memory_dir.clone(), backend.clone()));
        // bootstrap is also done by ingest_batch, but doing it here gives a
        // pre-ingest baseline for index.md so we can prove the refresh fires.
        orient.bootstrap("default").await.unwrap();

        let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
            r#"{"ops":[
                {"kind":"create","note_path":"preference/editor","title":"editor",
                 "summary":"prefers vim","facts":["uses vim"],
                 "links":["preference/keymap"],"tags":["tool"]}
            ]}"#
            .into(),
        ));
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: Some(orient.clone()),
            memory_dir: memory_dir.clone(),
            budget: RelatedBudget::default(),
            embedding_manager: None,
            gate: None,
        };

        let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
        let report = ing.ingest_batch("default", vec![raw], None).await.unwrap();
        assert_eq!(report.created, 1);

        let index_md_path = memory_dir.join("default").join("index.md");
        assert!(
            index_md_path.exists(),
            "index.md must exist after ingest_batch"
        );
        let body = std::fs::read_to_string(&index_md_path).unwrap();
        assert!(
            body.contains("preference"),
            "index.md must list the touched 'preference' category; got:\n{body}"
        );
    }

    /// An embedder that always fails — models a down/quota-exhausted embedding
    /// endpoint. Used to prove `ingest_batch` degrades gracefully instead of
    /// starving the note layer.
    struct FailingEmbeddingProvider;

    #[async_trait]
    impl crate::memory::embedding_provider::EmbeddingProvider for FailingEmbeddingProvider {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
            Err(AlephError::other("embedding endpoint unavailable"))
        }
        async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Err(AlephError::other("embedding endpoint unavailable"))
        }
        fn dimensions(&self) -> usize {
            1024
        }
        fn model_name(&self) -> &str {
            "failing"
        }
        fn provider_id(&self) -> &str {
            "failing"
        }
    }

    /// Regression: when the embedding endpoint is down, `gather_related` fails,
    /// but the batch must STILL produce a note. Previously the error
    /// propagated out of `ingest_batch`, `compress_to_notes` returned without
    /// marking the raws processed, and the L1 note layer starved indefinitely.
    #[tokio::test]
    async fn ingest_batch_degrades_when_embedding_fails() {
        let (dir, backend, indexer) = mk().await;
        let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
            r#"{"ops":[
                {"kind":"create","note_path":"learning/tokio","title":"Tokio",
                 "summary":"async runtime","facts":["event loop"],
                 "links":["learning/rust-async"],"tags":["rust"]}
            ]}"#
            .into(),
        ));
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(FailingEmbeddingProvider),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
            embedding_manager: None,
            gate: None,
        };

        let raw = RawMemory::new("some content".to_string(), RawMemorySource::Transcript);
        let report = ing
            .ingest_batch("default", vec![raw], None)
            .await
            .expect("ingest must succeed despite embedding failure");
        assert_eq!(
            report.created, 1,
            "note must be created even when related-page embedding fails"
        );
    }

    #[tokio::test]
    async fn ingest_batch_pushes_and_flushes_embedding() {
        use crate::config::types::memory::EmbeddingSettings;
        use crate::memory::embedding_manager::EmbeddingManager;
        use crate::memory::embedding_provider::EmbeddingProvider;
        use crate::memory::notes::store::NoteStore;

        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("note");
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), backend.clone()));

        // Seat a Mock provider on the manager so flush_pending writes vectors.
        let mgr = Arc::new(EmbeddingManager::new(EmbeddingSettings::default()));
        let mock: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock-1024"));
        mgr.install_provider_for_test(mock).await;

        let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
            r#"{"ops":[
                {"kind":"create","note_path":"preference/editor","title":"editor",
                 "summary":"prefers vim","facts":["uses vim"],
                 "links":["preference/keymap"],"tags":["tool"]}
            ]}"#
            .into(),
        ));
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: memory_dir.clone(),
            budget: RelatedBudget::default(),
            embedding_manager: Some(mgr.clone()),
            gate: None,
        };

        let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
        let report = ing.ingest_batch("default", vec![raw], None).await.unwrap();
        assert_eq!(report.created, 1);
        assert_eq!(report.touched_paths.len(), 1);

        // Queue must be drained — flush_pending ran at the tail.
        assert_eq!(
            mgr.pending_len().await,
            0,
            "pending queue must be drained after ingest tail flush"
        );

        // Vector must be persisted for the touched path.
        let touched = &report.touched_paths[0];
        let v = backend
            .get_embedding(touched, "default", 1024)
            .await
            .unwrap();
        assert!(
            v.is_some(),
            "embedding must be present after ingest tail flush; touched={touched}"
        );
    }

    #[tokio::test]
    async fn end_to_end_append_on_existing() {
        let (dir, backend, indexer) = mk().await;

        // Seed: create learning/rust-async first
        let provider_seed: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
            r#"{"ops":[
                {"kind":"create","note_path":"learning/rust-async","title":"rust-async",
                 "summary":"async primitives","facts":["Futures are lazy"],
                 "links":["learning/tokio"],"tags":["rust","async"]}
            ]}"#
            .into(),
        ));
        let ing_seed = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer: indexer.clone(),
            provider: provider_seed,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
            embedding_manager: None,
            gate: None,
        };
        let r1 = ing_seed
            .ingest_batch(
                "default",
                vec![RawMemory::new(
                    "seed".to_string(),
                    RawMemorySource::Transcript,
                )],
                None,
            )
            .await
            .unwrap();
        assert_eq!(r1.created, 1);

        // Second batch: append
        let provider2: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
            r#"{"ops":[
                {"kind":"append","note_path":"learning/rust-async",
                 "new_facts":["tokio is the runtime"],"new_links":[]}
            ]}"#
            .into(),
        ));
        let ing2 = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer: indexer.clone(),
            provider: provider2,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
            embedding_manager: None,
            gate: None,
        };
        let r2 = ing2
            .ingest_batch(
                "default",
                vec![RawMemory::new(
                    "body2".to_string(),
                    RawMemorySource::Transcript,
                )],
                None,
            )
            .await
            .unwrap();
        assert_eq!(r2.appended, 1);

        let body =
            tokio::fs::read_to_string(dir.path().join("note/default/learning/rust-async.md"))
                .await
                .unwrap();
        assert!(body.contains("Futures are lazy"));
        assert!(body.contains("tokio is the runtime"));
    }

    // ---- Write-time semantic dedup (mem0-style) ----

    #[test]
    fn cosine_similarity_identical_is_one() {
        let v = vec![0.1f32; 8];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn cosine_similarity_orthogonal_is_zero() {
        let a = vec![1.0f32, 0.0];
        let b = vec![0.0f32, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-5);
    }

    #[test]
    fn cosine_similarity_guards_mismatch_and_zero_norm() {
        assert_eq!(cosine_similarity(&[1.0, 2.0], &[1.0]), 0.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[0.0, 0.0]), 0.0);
    }

    #[test]
    fn candidate_dedup_text_skips_empty_fields() {
        let t = candidate_dedup_text("Tokio", "", &["event loop".into(), "tasks".into()]);
        assert!(t.contains("Tokio"));
        assert!(t.contains("event loop"));
        assert!(t.contains("tasks"));
        // empty summary contributes no blank line of its own
        assert!(!t.contains("\n\n"));
    }

    /// With dedup enabled and an existing note in the MERGE band, a planned
    /// `Create` is rewritten into an `Append` onto the existing page, carrying
    /// its facts and links. The mock embedder returns a constant `[0.1; 1024]`
    /// for the candidate, so cosine is fixed by the seeded stored vector:
    /// `0.03125 * sqrt(num 0.1-entries)`. 900 entries → cosine 0.9375, which is
    /// in `[dedup 0.92, noop 0.985)` → MERGE (not NOOP).
    #[tokio::test]
    async fn dedup_redirects_near_duplicate_create_to_append() {
        let (dir, backend, indexer) = mk().await;
        // Seed a MERGE-band embedding (cosine 0.9375 vs the candidate's
        // [0.1; 1024]) so the Create redirects to Append rather than dropping.
        let mut merge_vec = vec![0.1f32; 900];
        merge_vec.extend(std::iter::repeat_n(0.0f32, 124));
        backend
            .upsert_embedding("learning/tokio", "default", &merge_vec, 1024)
            .await
            .unwrap();

        let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new("{}".into()));
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget {
                dedup_enabled: true,
                ..RelatedBudget::default()
            },
            embedding_manager: None,
            gate: None,
        };

        let related = vec![RelatedPage {
            path: "learning/tokio".into(),
            title: "tokio".into(),
            summary: String::new(),
            content_preview: String::new(),
            tags: vec![],
            content_hash: "h".into(),
            score: 1.0,
        }];
        let ops = vec![PageOp::Create {
            note_path: "learning/tokio-runtime".into(),
            title: "Tokio runtime".into(),
            summary: "async".into(),
            facts: vec!["event loop".into()],
            links: vec!["learning/rust".into()],
            tags: vec![],
            relations: vec![],
        }];
        let out = ing.dedup_redirect_creates("default", ops, &related).await;
        assert_eq!(out.len(), 1);
        match &out[0] {
            PageOp::Append {
                note_path,
                new_facts,
                new_links,
                ..
            } => {
                assert_eq!(note_path, "learning/tokio");
                assert!(new_facts.iter().any(|f| f.contains("event loop")));
                assert_eq!(new_links, &vec!["learning/rust".to_string()]);
            }
            other => panic!("expected Append redirect, got {other:?}"),
        }
    }

    /// Dedup is off by default → the planned `Create` passes through unchanged
    /// even when an identical existing note is present (byte-identical ingest).
    #[tokio::test]
    async fn dedup_disabled_keeps_create_unchanged() {
        let (dir, backend, indexer) = mk().await;
        backend
            .upsert_embedding("learning/tokio", "default", &vec![0.1f32; 1024], 1024)
            .await
            .unwrap();

        let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new("{}".into()));
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(), // dedup_enabled = false
            embedding_manager: None,
            gate: None,
        };

        let related = vec![RelatedPage {
            path: "learning/tokio".into(),
            title: "tokio".into(),
            summary: String::new(),
            content_preview: String::new(),
            tags: vec![],
            content_hash: "h".into(),
            score: 1.0,
        }];
        let ops = vec![PageOp::Create {
            note_path: "learning/tokio-runtime".into(),
            title: "Tokio runtime".into(),
            summary: "async".into(),
            facts: vec!["event loop".into()],
            links: vec!["learning/rust".into()],
            tags: vec![],
            relations: vec![],
        }];
        let out = ing.dedup_redirect_creates("default", ops, &related).await;
        assert!(
            matches!(out[0], PageOp::Create { .. }),
            "dedup disabled must leave Create unchanged"
        );
    }

    /// A `Create` whose own path matches the only related page must NOT
    /// self-redirect (that would turn a legitimate overwrite into a no-op
    /// append against itself).
    #[tokio::test]
    async fn dedup_never_self_redirects() {
        let (dir, backend, indexer) = mk().await;
        backend
            .upsert_embedding("learning/tokio", "default", &vec![0.1f32; 1024], 1024)
            .await
            .unwrap();

        let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new("{}".into()));
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget {
                dedup_enabled: true,
                ..RelatedBudget::default()
            },
            embedding_manager: None,
            gate: None,
        };

        let related = vec![RelatedPage {
            path: "learning/tokio".into(),
            title: "tokio".into(),
            summary: String::new(),
            content_preview: String::new(),
            tags: vec![],
            content_hash: "h".into(),
            score: 1.0,
        }];
        let ops = vec![PageOp::Create {
            note_path: "learning/tokio".into(),
            title: "Tokio".into(),
            summary: "async".into(),
            facts: vec!["event loop".into()],
            links: vec!["learning/rust".into()],
            tags: vec![],
            relations: vec![],
        }];
        let out = ing.dedup_redirect_creates("default", ops, &related).await;
        assert!(
            matches!(out[0], PageOp::Create { .. }),
            "must not redirect a Create onto its own path"
        );
    }

    /// Stored vector with `m` entries of 0.1 and the rest 0.0. Its cosine vs the
    /// mock candidate ([0.1; 1024]) is `0.03125 * sqrt(m)`, so `m` selects the
    /// dedup tier: 100 → 0.3125 (ADD), 400 → 0.625, 900 → 0.9375 (MERGE),
    /// 1024 → 1.0 (NOOP).
    fn seed_with_cosine_entries(m: usize) -> Vec<f32> {
        let mut v = vec![0.1f32; m];
        v.extend(std::iter::repeat_n(0.0f32, 1024 - m));
        v
    }

    /// Run one Create through dedup against a single related page "learning/tokio"
    /// seeded with `seed_vec`, under `budget`. The candidate path differs from the
    /// related path (no self-skip), so the seed's cosine decides the tier.
    async fn run_dedup_tier(seed_vec: Vec<f32>, budget: RelatedBudget) -> Vec<PageOp> {
        let (dir, backend, indexer) = mk().await;
        backend
            .upsert_embedding("learning/tokio", "default", &seed_vec, 1024)
            .await
            .unwrap();
        let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new("{}".into()));
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget,
            embedding_manager: None,
            gate: None,
        };
        let related = vec![RelatedPage {
            path: "learning/tokio".into(),
            title: "tokio".into(),
            summary: String::new(),
            content_preview: String::new(),
            tags: vec![],
            content_hash: "h".into(),
            score: 1.0,
        }];
        let ops = vec![PageOp::Create {
            note_path: "learning/tokio-runtime".into(),
            title: "Tokio runtime".into(),
            summary: "async".into(),
            facts: vec!["event loop".into()],
            links: vec!["learning/rust".into()],
            tags: vec![],
            relations: vec![],
        }];
        ing.dedup_redirect_creates("default", ops, &related).await
    }

    fn budget_dedup_on() -> RelatedBudget {
        RelatedBudget {
            dedup_enabled: true,
            ..RelatedBudget::default()
        }
    }

    /// ADD tier: best match below `dedup_threshold` (cosine 0.3125 < 0.92) →
    /// the Create passes through unchanged.
    #[tokio::test]
    async fn dedup_tier_add_keeps_create() {
        let out = run_dedup_tier(seed_with_cosine_entries(100), budget_dedup_on()).await;
        assert!(
            matches!(out.as_slice(), [PageOp::Create { .. }]),
            "below dedup_threshold the Create must survive as ADD"
        );
    }

    /// NOOP tier: best match at/above `noop_threshold` (cosine 1.0 >= 0.985) →
    /// the Create is dropped entirely.
    #[tokio::test]
    async fn dedup_tier_noop_drops_create() {
        let out = run_dedup_tier(vec![0.1f32; 1024], budget_dedup_on()).await;
        assert!(
            out.is_empty(),
            "near-identical Create must be dropped as NOOP"
        );
    }

    /// FLOOR: `dedup_noop_threshold` misconfigured BELOW `dedup_similarity_threshold`.
    /// `noop_threshold` is floored to `max(0.50, 0.92) = 0.92`, so a 0.625-cosine
    /// match — which the raw 0.50 would have NOOP-dropped — stays an ADD Create.
    /// The floor guarantees NOOP never fires below the MERGE threshold.
    #[tokio::test]
    async fn dedup_noop_floor_never_fires_below_merge() {
        let budget = RelatedBudget {
            dedup_enabled: true,
            dedup_similarity_threshold: 0.92,
            dedup_noop_threshold: 0.50,
            ..RelatedBudget::default()
        };
        let out = run_dedup_tier(seed_with_cosine_entries(400), budget).await;
        assert!(
            matches!(out.as_slice(), [PageOp::Create { .. }]),
            "noop floored to >= dedup_threshold, so a 0.625 match stays ADD, not NOOP"
        );
    }
}
