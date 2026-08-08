//! `CompoundIngestor` trait impl for `DefaultCompoundIngestor` plus the
//! inherent batch-pipeline methods (apply, dedup/redirect, link-contract
//! enforcement, keyword linking, gate filtering).

use crate::error::AlephError;
use crate::memory::notes::governance::gate::{CandidateNote, GateOutcome, NoteWriteAction};
use crate::memory::notes::ingest::apply::{ApplyError, CompoundApplyTx};
use crate::memory::notes::ingest::plan::{ApplyReport, IngestPlan, PageOp};
use crate::memory::notes::ingest::prompts::PROMPT_LINK_REPAIR;
use crate::memory::notes::ingest::ref_table::{RefTable, ResolveStats};
use crate::memory::notes::ingest::retrieve::{gather_related, RelatedPage};
use crate::memory::notes::keyword_linker::{extract_keywords, pair_by_overlap, NoteForExtraction};
use crate::memory::notes::note::sanitize_title;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::KnowledgeNote;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::utils::json_extract::extract_json_robust;
use async_trait::async_trait;
use tracing::{info, warn};

use super::helpers::{
    candidate_dedup_text, candidate_from_pageop, cosine_similarity, keyword_query_terms,
};
use super::plan_parse::summary_from_report;
use super::{CompoundIngestor, DefaultCompoundIngestor};

#[async_trait]
impl<S: NoteStore + Send + Sync + 'static> CompoundIngestor for DefaultCompoundIngestor<S> {
    // rust-doctor-disable-next-line high-cyclomatic-complexity
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
        // rust-doctor-disable-next-line excessive-clone
        let source = raws[0].source.clone();
        // rust-doctor-disable-next-line excessive-clone
        let batch_ids: Vec<String> = raws.iter().map(|r| r.id.clone()).collect();
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
            // rust-doctor-disable-next-line excessive-clone
            self.store.clone(),
            // rust-doctor-disable-next-line excessive-clone
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

        let (mut plan, planner_degraded) = self
            .plan_with_health(agent_id, &raws, &related, &source, extra_context)
            .await?;
        if plan.ops.is_empty() {
            // Carry WHY the plan was empty. `CompressionService` defers the raw
            // rows for a retry only when the planner degraded; a deliberate
            // empty plan (what every source prompt asks for when nothing clears
            // the bar) consumes its rows now instead of being re-planned on
            // every tick for six hours.
            return Ok(ApplyReport {
                planner_degraded,
                ..Default::default()
            });
        }

        // Write-time semantic dedup (mem0-style): redirect near-duplicate
        // `Create` ops into `Append`s onto the matching existing note. No-op
        // unless `budget.dedup_enabled`. Runs before the gate so a redirected
        // Append (which targets an already-admitted note) bypasses re-gating.
        plan.ops = self
            .dedup_redirect_creates(agent_id, plan.ops, &related)
            .await;

        // Link-contract harmony gate: repair linkless Creates (or accept an
        // explicit isolation) before governance gating. Runs after dedup so a
        // Create already redirected into an Append is not re-examined.
        plan.ops = self
            .enforce_link_contract(agent_id, plan.ops, &related)
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
                // NOT a planner failure: the gate deferred every op and already
                // enqueued the candidates into `notes_review_queue`, so the
                // knowledge is preserved. Re-planning would only re-defer.
                return Ok(ApplyReport::default());
            }
        }

        let report = match self.try_apply(agent_id, &plan, &batch_ids).await {
            Ok(r) => r,
            Err(ApplyError::HashConflict { path, actual, .. }) => {
                warn!("compound ingest: hash conflict on {path}; re-planning");
                // rust-doctor-disable-next-line excessive-clone
                let mut augmented = raws.clone();
                if let Some(last) = augmented.last_mut() {
                    last.content.push_str(&format!(
                        "\n\n[system] previous plan referenced {path} with a stale hash; actual hash is {actual}. Re-plan using fresh data."
                    ));
                }
                let (mut plan2, planner_degraded) = self
                    .plan_with_health(agent_id, &augmented, &related, &source, extra_context)
                    .await?;
                if plan2.ops.is_empty() {
                    return Ok(ApplyReport {
                        planner_degraded,
                        ..Default::default()
                    });
                }
                plan2.ops = self
                    .dedup_redirect_creates(agent_id, plan2.ops, &related)
                    .await;
                plan2.ops = self
                    .enforce_link_contract(agent_id, plan2.ops, &related)
                    .await;
                if self.gate.is_some() {
                    plan2.ops = self.filter_ops_through_gate(agent_id, plan2.ops).await?;
                    if plan2.ops.is_empty() {
                        // Gate-deferred, not planner-degraded — see above.
                        return Ok(ApplyReport::default());
                    }
                }
                self.try_apply(agent_id, &plan2, &batch_ids)
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
        batch_ids: &[String],
    ) -> Result<ApplyReport, ApplyError> {
        let mut tx = CompoundApplyTx::new(
            &self.indexer,
            &self.store,
            // rust-doctor-disable-next-line excessive-clone
            self.memory_dir.clone(),
            agent_id,
        )
        .with_batch_sources(batch_ids.to_vec());
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
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    pub(crate) async fn dedup_redirect_creates(
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
                            source_ids,
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
                            source_ids,
                        })
                    }
                    (_, op) => Some(op),
                }
            })
            .collect()
    }

    /// Link-contract harmony gate. When the related set is non-empty, a
    /// `Create` with neither `links` nor `relations` violates the mandatory
    /// linking contract in `PROMPT_COMPOUND_PLAN` rule 6. One lightweight
    /// repair LLM call asks for `[P<n>]` links (anti-hallucination via
    /// `RefTable`) or an explicit `isolated` declaration. Repaired links
    /// merge back into the op; every failure degrades to pass-through —
    /// linking is an enhancement and must never block memory persistence.
    pub(crate) async fn enforce_link_contract(
        &self,
        agent_id: &str,
        ops: Vec<PageOp>,
        related: &[RelatedPage],
    ) -> Vec<PageOp> {
        if related.is_empty() {
            // Embedding-derived related set is empty (sparse wiki or embedding
            // down): fall back to keyword-overlap linking via FTS candidates
            // instead of leaving every create an orphan.
            return self.keyword_link_creates(agent_id, ops).await;
        }
        let violating: Vec<usize> = ops
            .iter()
            .enumerate()
            .filter_map(|(i, op)| match op {
                PageOp::Create {
                    links, relations, ..
                } if links.is_empty() && relations.is_empty() => Some(i),
                _ => None,
            })
            .collect();
        if violating.is_empty() {
            return ops;
        }

        // Repair prompt input: the violating notes plus the same [P<n>]
        // table the planner saw.
        let mut user = String::from("## New notes with no links\n\n");
        for (slot, &i) in violating.iter().enumerate() {
            if let PageOp::Create {
                note_path,
                title,
                summary,
                facts,
                ..
            } = &ops[i]
            {
                user.push_str(&format!(
                    "[note {slot}] path={note_path} title={title}\nsummary: {summary}\nfacts:\n"
                ));
                for f in facts.iter().take(6) {
                    user.push_str(&format!("- {f}\n"));
                }
                user.push('\n');
            }
        }
        user.push_str("## Related existing pages\n\n");
        for (i, rp) in related.iter().enumerate() {
            user.push_str(&format!(
                "{} {} — {}\n",
                RefTable::token(i),
                rp.path,
                rp.summary
            ));
        }

        let msgs = [UnifiedMessage::user(&user)];
        let resp = match self
            .provider
            .process(RequestPayload::new(&msgs).with_system(Some(PROMPT_LINK_REPAIR)))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!("link contract repair LLM failed (pass-through): {e}");
                return ops;
            }
        };
        let Some(json) = extract_json_robust(&resp.text_content()) else {
            warn!("link contract repair: no JSON in response (pass-through)");
            return ops;
        };

        let refs = RefTable::from_related(related);
        let mut ops = ops;
        let repairs = json
            .get("repairs")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut repaired = 0usize;
        for rep in &repairs {
            let Some(slot) = rep.get("note_index").and_then(|v| v.as_u64()) else {
                continue;
            };
            let Some(&op_i) = violating.get(slot as usize) else {
                continue;
            };
            if rep
                .get("isolated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                continue; // explicit isolation accepted
            }
            let mut new_links: Vec<String> = rep
                .get("links")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|l| l.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let mut stats = ResolveStats::default();
            refs.resolve_links(&mut new_links, &mut stats);
            if stats.dropped_links > 0 {
                warn!(
                    dropped = stats.dropped_links,
                    "link contract repair: dropped hallucinated tokens"
                );
            }
            if new_links.is_empty() {
                continue;
            }
            if let PageOp::Create { links, .. } = &mut ops[op_i] {
                links.extend(new_links);
                links.dedup();
                repaired += 1;
            }
        }
        if repaired > 0 {
            info!(repaired, "link contract: repaired linkless creates");
        }
        ops
    }

    /// Keyword-overlap linking for `Create` ops left unlinked by the embedding
    /// `related` path. Pulls FTS candidates, extracts keyword sets, pairs, and
    /// merges links into the creates. Degrades to pass-through on any failure
    /// (linking is an enhancement, never block memory persistence — P7).
    ///
    /// Invoked only when the embedding-derived `related` set is empty (sparse
    /// wiki or embedding endpoint down), so without it every fresh note becomes
    /// an orphan island. FTS needs no embedding, so it links notes the vector
    /// path could not reach.
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    async fn keyword_link_creates(&self, agent_id: &str, mut ops: Vec<PageOp>) -> Vec<PageOp> {
        use std::collections::{HashMap, HashSet};

        // 1. Collect linkless + relationless Create ops as extraction inputs,
        //    remembering each one's op index so links can be merged back.
        let mut targets: Vec<(usize, String)> = Vec::new(); // (op_index, path)
        let mut union: Vec<NoteForExtraction> = Vec::new();
        let mut batch_paths: HashSet<String> = HashSet::new();
        for (i, op) in ops.iter().enumerate() {
            if let PageOp::Create {
                note_path,
                title,
                summary,
                facts,
                links,
                relations,
                ..
            } = op
            {
                // rust-doctor-disable-next-line excessive-clone
                batch_paths.insert(note_path.clone());
                if !links.is_empty() || !relations.is_empty() {
                    continue;
                }
                // rust-doctor-disable-next-line excessive-clone
                targets.push((i, note_path.clone()));
                union.push(NoteForExtraction {
                    // rust-doctor-disable-next-line excessive-clone
                    path: note_path.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    title: title.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    summary: summary.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    facts: facts.clone(),
                });
            }
        }
        if targets.is_empty() {
            return ops;
        }

        // 2. Gather FTS candidates for each target. `search_notes_fts` treats
        //    its whole query as ONE FTS5 phrase, so search per significant
        //    keyword (mirroring the note_manage create path) and merge by path.
        //    Skip paths already in this batch and paths already collected.
        // rust-doctor-disable-next-line excessive-clone
        let mut seen_candidates: HashSet<String> = batch_paths.clone();
        for (_, target_path) in &targets {
            // Build the keyword query from the target's own extraction text.
            let Some(nfe) = union.iter().find(|n| &n.path == target_path) else {
                continue;
            };
            let mut query_text = format!("{} {}", nfe.title, nfe.summary);
            for f in &nfe.facts {
                query_text.push(' ');
                query_text.push_str(f);
            }
            for kw in keyword_query_terms(&query_text) {
                // FTS candidates are scoped to the ingesting agent so each
                // agent's notes link only within its own wiki.
                match self.store.search_notes_fts(&kw, agent_id, 3).await {
                    Ok(hits) => {
                        for hit in hits {
                            if seen_candidates.contains(&hit.path) {
                                continue;
                            }
                            // rust-doctor-disable-next-line excessive-clone
                            seen_candidates.insert(hit.path.clone());
                            // FTS hits carry no title/summary/facts; the path
                            // (and filename) is enough for keyword extraction.
                            union.push(NoteForExtraction {
                                // rust-doctor-disable-next-line excessive-clone
                                path: hit.path.clone(),
                                title: hit.filename,
                                // rust-doctor-disable-next-line unnecessary-allocation
                                summary: String::new(),
                                // rust-doctor-disable-next-line unnecessary-allocation
                                facts: Vec::new(),
                            });
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, keyword = %kw, "keyword link: FTS search failed");
                    }
                }
            }
        }
        // No FTS candidates beyond the batch itself → nothing to link to.
        if union.len() == targets.len() {
            return ops;
        }

        // 3. Extract keyword sets for the union (one batched LLM call).
        let kw = match extract_keywords(&*self.provider, &union).await {
            Ok(k) if !k.is_empty() => k,
            Ok(_) => return ops,
            Err(e) => {
                warn!(error = %e, "keyword link: extraction failed (pass-through)");
                return ops;
            }
        };

        // 4. Pair by overlap; for each target Create, merge the OTHER side of
        //    every link triple it participates in into its `links` (dedup).
        let triples = pair_by_overlap(&kw);
        if triples.is_empty() {
            return ops;
        }
        let target_index: HashMap<&str, usize> = targets
            .iter()
            .map(|(op_i, path)| (path.as_str(), *op_i))
            .collect();
        let mut linked = 0usize;
        for t in &triples {
            // A triple links `from`↔`to`. Merge into whichever side is one of
            // our linkless creates (both sides may be, on a multi-create batch).
            for (this, other) in [(&t.from, &t.to), (&t.to, &t.from)] {
                if let Some(&op_i) = target_index.get(this.as_str()) {
                    if let PageOp::Create { links, .. } = &mut ops[op_i] {
                        if !links.iter().any(|l| l == other) {
                            // rust-doctor-disable-next-line excessive-clone
                            links.push(other.clone());
                            linked += 1;
                        }
                    }
                }
            }
        }
        if linked > 0 {
            info!(
                linked,
                "keyword link: merged FTS-overlap links into creates"
            );
        }
        ops
    }

    /// Run each `PageOp::Create` through `self.gate` and drop ops whose outcome
    /// is `Defer` (already enqueued by the gate into `notes_review_queue`).
    /// Other op kinds pass through unchanged in this scoped commit; their gating
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
            // `Supersede` is gated on the *superseded* note's existing severity,
            // which lives on disk rather than in the index — an async lookup the
            // sync `candidate_from_pageop` can't perform. Build its candidate
            // here so the gate can defer supersession of High/Critical knowledge.
            let candidate = if let PageOp::Supersede { old_path, .. } = &op {
                self.supersede_candidate(agent_id, &op, old_path).await
            } else {
                candidate_from_pageop(agent_id, &op)
            };
            match candidate {
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
                },
                // Op kinds the gate does not understand in this scoped commit
                // (Append/Update/Link, plus a Supersede whose target can't be
                // read) pass through unchanged — applied immediately.
                None => out.push(op),
            }
        }
        Ok(out)
    }

    /// Build a gate candidate for a `Supersede` op from the *superseded* note's
    /// existing severity. Severity is not an index column, so the target note is
    /// read from disk and parsed. Returns `None` when the target can't be read or
    /// the op can't be serialized (fail-open → the supersession applies
    /// immediately, matching the pre-governance bypass). The serialized op rides
    /// along as `replay_op` so an approved deferral replays the supersession
    /// verbatim rather than losing it.
    async fn supersede_candidate(
        &self,
        agent_id: &str,
        op: &PageOp,
        old_path: &str,
    ) -> Option<CandidateNote> {
        let (category, filename) = old_path.split_once('/')?;
        let safe = sanitize_title(filename).ok()?;
        let disk = self
            .memory_dir
            .join(agent_id)
            .join(category)
            .join(format!("{safe}.md"));
        let content = tokio::fs::read_to_string(&disk).await.ok()?;
        let target = KnowledgeNote::from_markdown(&safe, &content).ok()?;
        let replay = serde_json::to_value(op).ok()?;
        Some(CandidateNote {
            agent_id: agent_id.to_string(),
            category: category.to_string(),
            // A minimal note carrying only the risk signal the gate reads.
            note: KnowledgeNote {
                severity: target.severity,
                ..KnowledgeNote::default()
            },
            action: NoteWriteAction::Supersede,
            bypass_review: false,
            contradicts_existing: false,
            replay_op: Some(replay),
        })
    }
}
