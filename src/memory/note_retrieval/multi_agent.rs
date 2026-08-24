//! Retrieval across several corpora at once, and the FTS-only fallback it
//! degrades to when the embedding leg is unavailable.
//!
//! Split out of `note_retrieval/mod.rs` verbatim; logic unchanged. These are
//! inherent methods, so they need no delegation layer — the type simply has
//! more than one `impl` block.

use super::*;

impl<S: NoteStore + Send + Sync + 'static> NoteFactRetrieval<S> {
    /// Hybrid search across multiple agents. Results from each agent are
    /// collected, merged, sorted by score, and truncated to `limit`.
    ///
    /// Used for "smart recall" — queries that should span multiple workspaces.
    pub async fn retrieve_multi_agent(
        &self,
        query: &str,
        agent_ids: &[String],
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let mut sink = TraceSink::Off;
        self.retrieve_multi_agent_inner(query, agent_ids, limit, &mut sink)
            .await
    }

    /// [`Self::retrieve_multi_agent`] with per-stage telemetry, for the Panel's
    /// retrieval x-ray. Results and ordering are identical to the untraced call
    /// — the sink only observes.
    ///
    /// The x-ray needs the MULTI-partition entry point for the same reason the
    /// note list does: what it is asked to explain is why a real recall did or
    /// did not surface a note, and real recall reads
    /// `project_scope::session_read_ids`. Tracing the bare persona would draw a
    /// faithful funnel through a partition nothing writes to — an honest
    /// picture of the wrong population, which is worse than no picture because
    /// it looks like an answer.
    pub async fn retrieve_multi_agent_traced(
        &self,
        query: &str,
        agent_ids: &[String],
        limit: usize,
    ) -> Result<(Vec<ScoredFact>, Vec<StageTrace>), AlephError> {
        let mut sink = TraceSink::On(Vec::new());
        let results = self
            .retrieve_multi_agent_inner(query, agent_ids, limit, &mut sink)
            .await?;
        Ok((results, sink.into_stages()))
    }

    pub(super) async fn retrieve_multi_agent_inner(
        &self,
        query: &str,
        agent_ids: &[String],
        limit: usize,
        sink: &mut TraceSink,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        if agent_ids.is_empty() {
            return Ok(Vec::new());
        }

        // No embedder configured (FTS-only deployment): degrade to per-agent
        // keyword search, mirroring the single-agent path.
        let Some(embedder) = self.embedder.as_ref() else {
            return self
                .multi_agent_text_fallback(query, agent_ids, limit, sink)
                .await;
        };

        // Embed once, reuse across agents. When the embedding endpoint is
        // unreachable (network outage, provider down) the notes are still
        // local — degrade to FTS-only rather than failing all "smart recall"
        // (P7), mirroring the single-agent `retrieve_inner` path.
        let embedding = match embedder.embed(query).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "multi-agent recall: embedding unavailable, falling back to FTS-only search"
                );
                return self
                    .multi_agent_text_fallback(query, agent_ids, limit, sink)
                    .await;
            }
        };
        let dim = embedding.len() as u32;

        let mut all_results: Vec<ScoredFact> = Vec::new();
        // Over-fetch per agent so merged top-k is well-populated
        let per_agent_limit = limit.max(10);
        let t_search = Instant::now();

        for agent_id in agent_ids {
            // Same reasoning as `retrieve_inner`: a store-side vector failure
            // (typically an unsupported embedding dimension) is not agent- or
            // query-specific, so retry-per-agent would just repeat it. Degrade
            // the whole call to the keyword path instead of failing recall.
            let hybrid = self
                .indexer
                .store()
                .hybrid_search_notes(&embedding, query, agent_id, dim, per_agent_limit)
                .await;
            let mut results = match hybrid {
                Ok(r) => r.results,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        dim,
                        "multi-agent recall: vector leg unavailable, falling back to FTS-only search"
                    );
                    return self
                        .multi_agent_text_fallback(query, agent_ids, limit, sink)
                        .await;
                }
            };
            if self.expansion.is_active() {
                let peers = expansion::graph_expand(
                    self.indexer.store().as_ref(),
                    agent_id,
                    &results,
                    &self.expansion,
                )
                .await;
                results.extend(peers);
            }
            for r in results {
                all_results.push(r.to_scored_fact(agent_id));
            }
        }

        // One stage for the whole fan-out, not one per partition: the funnel is
        // a pipeline and each partition is a shard of the SAME stage, so N
        // sibling `hybrid_search` rows would read as N sequential narrowings.
        // The input count is 0 for the same reason `retrieve_inner` records 0 —
        // a search has no input population, only an output.
        sink.record(
            "hybrid_search",
            t_search.elapsed().as_millis() as u64,
            0,
            all_results.len(),
        );

        // Sort by score DESC, then bound the pool before the (optional) rerank.
        all_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let before_cap = all_results.len();
        all_results.truncate(self.fetch_limit(limit));
        sink.record("fetch_cap", 0, before_cap, all_results.len());

        // Close the hot-floating loop across the multi-agent path too. Signals
        // are filed per owning namespace (see `record_recall_by_owner`) — this
        // is not only the opt-in "smart recall" path any more: `gather.rs`
        // routes *all* project-scoped auto-recall through here, and
        // `read_scope_ids` returns `[base, scoped]`, so labelling every hit with
        // `agent_ids.first()` filed every project note's hit under the base
        // namespace.
        let ranked = self.apply_rerank(query, all_results, sink).await;
        let counts = self.fetch_reinforcement_counts_by_owner(&ranked).await;
        let mut ranked = self.apply_scoring(ranked, now_unix(), &counts, sink);
        let before_limit = ranked.len();
        ranked.truncate(limit);
        sink.record("limit", 0, before_limit, ranked.len());
        self.record_recall_by_owner(query, &ranked).await;
        Ok(ranked)
    }

    /// FTS-only multi-agent recall: per-agent keyword search merged, sorted,
    /// truncated, and recorded. Shared by the no-embedder deployment and the
    /// embed-failure degrade path so both honor the same graceful-degradation
    /// contract (P7) — a transient embed outage must not brick smart recall.
    pub(super) async fn multi_agent_text_fallback(
        &self,
        query: &str,
        agent_ids: &[String],
        limit: usize,
        sink: &mut TraceSink,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let per_agent_limit = limit.max(10);
        let mut all_results: Vec<ScoredFact> = Vec::new();
        let t0 = Instant::now();
        for agent_id in agent_ids {
            all_results.extend(self.text_retrieve(query, agent_id, per_agent_limit).await?);
        }
        // Named `fts_search`, matching `retrieve_inner`'s keyword stage: the
        // x-ray legend explains stages by name, and calling the same operation
        // something else on the degraded path would read as a different
        // pipeline rather than the same one without its vector leg.
        sink.record(
            "fts_search",
            t0.elapsed().as_millis() as u64,
            0,
            all_results.len(),
        );
        all_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let before_limit = all_results.len();
        all_results.truncate(limit);
        sink.record("limit", 0, before_limit, all_results.len());
        self.record_recall_by_owner(query, &all_results).await;
        Ok(all_results)
    }

    // `retrieve_all_agents` used to live here: enumerate every corpus on disk
    // and retrieve across all of them. It was removed on 2026-08-13 with zero
    // production consumers left.
    //
    // Its shape was the problem, not its body. "Every corpus on disk" is a
    // decision about who may be read, and taking no actor meant every caller
    // made that decision by accident — `memory_search`'s Smart Recall phase 2
    // called it on a sparse primary result and returned other principals'
    // notes to the model, past a narrowing its own comment said was the single
    // decision point. A helper that cannot be called without answering "who is
    // asking" would be fine here; a helper that cannot be called WITH that
    // answer is a hole with a convenient name.
    //
    // Callers now enumerate with `project_scope::list_note_corpora`, filter
    // with `visibility::partition_visible_to`, and hand the result to
    // [`Self::retrieve_multi_agent`] — which is exactly what this function did
    // minus the filter.
}
