//! Single-corpus retrieval entry points.
//!
//! Split out of `note_retrieval/mod.rs` verbatim; logic unchanged. These are
//! inherent methods, so they need no delegation layer — the type simply has
//! more than one `impl` block.

use super::*;

impl<S: NoteStore + Send + Sync + 'static> NoteFactRetrieval<S> {
    /// Hybrid vector + FTS search with RRF fusion.
    /// Returns `ScoredFact` for downstream compatibility.
    pub async fn retrieve(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let mut sink = TraceSink::Off;
        self.retrieve_inner(query, agent_id, limit, &mut sink).await
    }

    // `retrieve_traced` used to live here: the single-agent twin of
    // [`Self::retrieve_multi_agent_traced`]. It was removed on 2026-08-21 with
    // zero production consumers left.
    //
    // Its one caller was the Panel's retrieval x-ray, which had to move to the
    // multi-partition entry point because that is what real recall reads
    // (`project_scope::session_read_ids`) — tracing one bare persona drew a
    // faithful funnel through a partition nothing writes to. Nothing else ever
    // wanted a trace, so keeping the wrapper would have been a hole with a
    // convenient name (R10's YAGNI withdrawal).
    //
    // Consequence worth knowing before adding a caller: `retrieve_inner` is now
    // reached only through [`Self::retrieve`], which passes `TraceSink::Off`, so
    // its own `sink.record(...)` calls are inert in production. The stage
    // vocabulary the x-ray legend depends on comes from `apply_scoring` /
    // `apply_rerank`, which the MULTI path drives with a live sink — that is
    // where the coverage for it lives now.

    /// Shared orchestration for `retrieve` / `retrieve_traced`. The `sink`
    /// records stage telemetry only when `On`; `Off` is a no-op hot path with
    /// byte-identical results.
    pub(super) async fn retrieve_inner(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
        sink: &mut TraceSink,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        // No embedder configured (FTS-only deployment): skip the vector leg
        // silently — this is a known steady state, not an outage.
        let Some(embedder) = self.embedder.as_ref() else {
            return self
                .text_retrieve_scored(query, agent_id, limit, sink)
                .await;
        };
        // Embedding requires a remote API call; when that endpoint is
        // unreachable (network outage, provider down) the notes themselves
        // are still local — degrade to FTS-only search instead of failing.
        let embedding = match embedder.embed(query).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "note retrieval: embedding unavailable, falling back to FTS-only search"
                );
                return self
                    .text_retrieve_scored(query, agent_id, limit, sink)
                    .await;
            }
        };
        let dim = embedding.len() as u32;

        let t0 = Instant::now();
        // The vector leg can also fail *after* a successful embed — most often
        // because the provider's dimension has no vec0 table. The reason to
        // degrade is unchanged (FTS and the notes on disk are both intact), so
        // the fallback has to cover this arm too. It did not, and this is the
        // auto-recall path: a failure here silently emptied <memory-context>
        // for every turn.
        let hybrid = self
            .indexer
            .store()
            .hybrid_search_notes(&embedding, query, agent_id, dim, self.fetch_limit(limit))
            .await;
        let mut results = match hybrid {
            Ok(r) => r.results,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    dim,
                    "note retrieval: vector leg unavailable, falling back to FTS-only search"
                );
                return self
                    .text_retrieve_scored(query, agent_id, limit, sink)
                    .await;
            }
        };
        sink.record(
            "hybrid_search",
            t0.elapsed().as_millis() as u64,
            0,
            results.len(),
        );

        if self.expansion.is_active() {
            let t0 = Instant::now();
            let before = results.len();
            let peers = expansion::graph_expand(
                self.indexer.store().as_ref(),
                agent_id,
                &results,
                &self.expansion,
            )
            .await;
            results.extend(peers);
            // Bound the merged pool so rerank cost stays capped despite expansion.
            if results.len() > RERANK_MAX_CANDIDATES {
                results.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                results.truncate(RERANK_MAX_CANDIDATES);
            }
            sink.record(
                "graph_expand",
                t0.elapsed().as_millis() as u64,
                before,
                results.len(),
            );
        }

        let facts: Vec<ScoredFact> = results.iter().map(|r| r.to_scored_fact(agent_id)).collect();
        let ranked = self.apply_rerank(query, facts, sink).await;
        let counts = self.fetch_reinforcement_counts_by_owner(&ranked).await;
        let mut ranked = self.apply_scoring(ranked, now_unix(), &counts, sink);
        let before = ranked.len();
        let t0 = Instant::now();
        ranked.truncate(limit);
        sink.record(
            "truncate",
            t0.elapsed().as_millis() as u64,
            before,
            ranked.len(),
        );
        // Close the hot-floating loop: record the surfaced notes as recall hits.
        self.record_recall_by_owner(query, &ranked).await;
        self.surface_relations(agent_id, &mut ranked).await;
        Ok(ranked)
    }

    /// Pure vector similarity search.
    pub async fn vector_retrieve(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let embedder = self.embedder.as_ref().ok_or_else(|| {
            AlephError::config("no embedding provider configured; vector search unavailable")
        })?;
        let embedding = embedder.embed(query).await?;
        let dim = embedding.len() as u32;

        let results = self
            .indexer
            .store()
            .vector_search_notes_with_content(&embedding, agent_id, dim, limit)
            .await?;

        Ok(results.iter().map(|r| r.to_scored_fact(agent_id)).collect())
    }

    /// FTS-only search (no embedding required).
    ///
    /// Note: FTS results don't carry scores natively — rank-based scores are
    /// assigned.
    ///
    /// Hits are hydrated with their note bodies via `get_notes_with_content`,
    /// the same trait method the hybrid path uses. The FTS index rows carry only
    /// metadata, so emitting them directly produced facts with `content == ""`:
    /// the model received a recall block of titles with no substance, while
    /// `text_retrieve_scored` still wrote a `recall_signals` row for each one —
    /// durably teaching reinforcement that empty notes are hot. This is the
    /// degraded path (no embedder configured, or a transient embed outage), so
    /// it is exactly when recall fidelity matters most.
    ///
    /// A hit whose body cannot be loaded is skipped rather than emitted empty:
    /// it contributes nothing to the prompt, so it must not earn a recall
    /// signal either.
    pub async fn text_retrieve(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let store = self.indexer.store();
        let entries = store.search_notes_fts(query, agent_id, limit).await?;
        if entries.is_empty() {
            return Ok(Vec::new());
        }

        // rust-doctor-disable-next-line excessive-clone
        let paths: Vec<String> = entries.iter().map(|e| e.path.clone()).collect();
        let hydrated = store.get_notes_with_content(agent_id, &paths).await?;
        let by_path: std::collections::HashMap<&str, &NoteSearchResult> =
            hydrated.iter().map(|r| (r.path.as_str(), r)).collect();

        let total = entries.len() as f32;
        let mut out = Vec::with_capacity(entries.len());
        for (i, entry) in entries.iter().enumerate() {
            let Some(result) = by_path.get(entry.path.as_str()) else {
                tracing::debug!(
                    path = %entry.path,
                    "text_retrieve: FTS hit has no loadable body; skipping"
                );
                continue;
            };
            let mut fact = result.to_scored_fact(agent_id);
            // FTS carries no native score; derive it from rank as before.
            fact.score = 1.0 - (i as f32 / total.max(1.0));
            out.push(fact);
        }
        Ok(out)
    }

    /// FTS-only recall tail shared by both `retrieve_inner` degradation branches
    /// (no embedder configured, and transient embed-endpoint outage). Runs the
    /// same reinforcement + recency scoring and recall recording as the hybrid
    /// path, so hot-surfacing accrues signal and ranks even when the
    /// vector leg is unavailable — mirrors `multi_agent_text_fallback`'s
    /// graceful-degradation contract (P7). Without this, an FTS-only deployment
    /// never writes `recall_signals`, leaving reinforcement a permanent no-op.
    pub(super) async fn text_retrieve_scored(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
        sink: &mut TraceSink,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let t0 = Instant::now();
        let results = self.text_retrieve(query, agent_id, limit).await?;
        sink.record(
            "fts_search",
            t0.elapsed().as_millis() as u64,
            0,
            results.len(),
        );
        let counts = self.fetch_reinforcement_counts_by_owner(&results).await;
        let ranked = self.apply_scoring(results, now_unix(), &counts, sink);
        self.record_recall_by_owner(query, &ranked).await;
        Ok(ranked)
    }
}
