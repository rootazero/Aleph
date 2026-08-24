//! The ranking pipeline applied to a candidate pool before it is returned:
//! how many candidates to fetch, retrieval-time scoring, graph-peer surfacing
//! and the cross-encoder rerank.
//!
//! Split out of `note_retrieval/mod.rs` verbatim; logic unchanged. These are
//! inherent methods, so they need no delegation layer — the type simply has
//! more than one `impl` block.

use super::*;

impl<S: NoteStore + Send + Sync + 'static> NoteFactRetrieval<S> {
    /// Candidate pool size to fetch before reranking/scoring. Without a reranker
    /// or active scoring this is exactly `limit` (preserving legacy fetch counts);
    /// otherwise it over-fetches up to a bounded ceiling so the reranker / MMR
    /// pass has a real pool to reorder before truncation.
    pub(super) fn fetch_limit(&self, limit: usize) -> usize {
        if self.reranker.is_none() && !self.scoring.is_active() {
            return limit;
        }
        limit
            .saturating_mul(RERANK_CANDIDATE_MULTIPLIER)
            .min(RERANK_MAX_CANDIDATES)
            .max(limit)
    }

    /// Apply retrieval-time recency decay, reinforcement salience, and MMR
    /// diversity (after rerank, before truncation). A no-op when scoring is
    /// inactive, so the default path is unchanged. `now` is the current Unix time
    /// in seconds. `reinforcement_counts` maps note path → recall-frequency count
    /// (empty when reinforcement is disabled).
    pub(super) fn apply_scoring(
        &self,
        facts: Vec<ScoredFact>,
        now: i64,
        reinforcement_counts: &HashMap<(String, String), i64>,
        sink: &mut TraceSink,
    ) -> Vec<ScoredFact> {
        if !self.scoring.is_active() || facts.len() < 2 {
            return facts;
        }
        let mut facts = facts;

        // 1a) Recency reweight.
        if self.scoring.recency_enabled {
            let t0 = Instant::now();
            let n = facts.len();
            for f in facts.iter_mut() {
                let mult = scoring::recency_multiplier(
                    f.fact.updated_at,
                    now,
                    self.scoring.recency_half_life_days,
                );
                f.score = scoring::apply_recency(f.score, mult, self.scoring.recency_weight);
            }
            sink.record("recency_decay", t0.elapsed().as_millis() as u64, n, n);
        }

        // 1b) Reinforcement reweight.
        if self.scoring.reinforcement_enabled {
            let t0 = Instant::now();
            let n = facts.len();
            for f in facts.iter_mut() {
                // Keyed by (owner, path): in the project-scoped read union two
                // namespaces can hold notes at the same relative path, and a
                // bare-path map would let one namespace's heat leak into the
                // other's ranking.
                let hits = reinforcement_counts
                    .get(&(f.fact.agent.clone(), f.fact.id.clone()))
                    .copied()
                    .unwrap_or(0);
                f.score =
                    scoring::apply_reinforcement(f.score, hits, self.scoring.reinforcement_weight);
            }
            sink.record("reinforcement", t0.elapsed().as_millis() as u64, n, n);
        }

        // 1c) Re-sort by adjusted score (once, after both reweights).
        if self.scoring.recency_enabled || self.scoring.reinforcement_enabled {
            facts.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        // 2) MMR diversity reorder over the (relevance-sorted) pool.
        if self.scoring.mmr_enabled {
            let t0 = Instant::now();
            let n = facts.len();
            // rust-doctor-disable-next-line excessive-clone
            let contents: Vec<String> = facts.iter().map(|f| f.fact.content.clone()).collect();
            let relevance: Vec<f32> = facts.iter().map(|f| f.score).collect();
            let order = scoring::mmr_reorder(&contents, &relevance, self.scoring.mmr_lambda);
            let mut slots: Vec<Option<ScoredFact>> = facts.into_iter().map(Some).collect();
            facts = order
                .into_iter()
                .filter_map(|i| slots.get_mut(i).and_then(Option::take))
                .collect();
            sink.record(
                "mmr_diversity",
                t0.elapsed().as_millis() as u64,
                n,
                facts.len(),
            );
        }

        facts
    }

    /// Annotate surfaced notes with backlink counts + structural-strong
    /// relations, and force-inject the targets of structural-strong relations
    /// that the score-based ranking dropped. Scoped to already-surfaced notes.
    /// Non-fatal: store errors are logged and skipped.
    pub(super) async fn surface_relations(&self, agent_id: &str, ranked: &mut Vec<ScoredFact>) {
        use std::collections::HashSet;
        let store = self.indexer.store();
        // path form in ScoredFact is "note://category/filename"; strip the scheme.
        let strip = |p: &str| p.strip_prefix("note://").unwrap_or(p).to_string();
        let mut present: HashSet<String> = ranked.iter().map(|f| strip(&f.fact.path)).collect();

        let mut inject: Vec<(String, String, String)> = Vec::new(); // (target_path, rel, source_path)
        for f in ranked.iter_mut() {
            let path = strip(&f.fact.path);
            let filename = path.rsplit('/').next().unwrap_or(&path).to_string();
            let relations = store
                .get_typed_relations(&path, agent_id)
                .await
                .unwrap_or_default();
            let backlinks = store
                .get_incoming_links_any(&path, &filename, agent_id)
                .await
                .unwrap_or_default();
            let strong_outs: Vec<(String, String)> = relations
                .iter()
                .filter(|(_, rel)| crate::memory::notes::is_structural_strong(rel))
                .cloned()
                .collect();
            if let Some(footer) = relation_surface::backlink_footer(&strong_outs, backlinks.len()) {
                f.fact.content.push('\n');
                f.fact.content.push_str(&footer);
            }
            for (to, rel) in relation_surface::structural_targets(&relations, &present) {
                // rust-doctor-disable-next-line excessive-clone
                if present.insert(to.clone()) {
                    // rust-doctor-disable-next-line excessive-clone
                    inject.push((to, rel, path.clone()));
                }
            }
        }

        if inject.is_empty() {
            return;
        }
        // rust-doctor-disable-next-line excessive-clone
        let paths: Vec<String> = inject.iter().map(|(t, _, _)| t.clone()).collect();
        let hydrated = match store.get_notes_with_content(agent_id, &paths).await {
            Ok(h) => h,
            Err(e) => {
                tracing::debug!(error = %e, "surface_relations: hydrate failed (non-fatal)");
                return;
            }
        };
        for r in hydrated {
            let meta = inject.iter().find(|(t, _, _)| *t == r.path);
            let mut fact = r.to_scored_fact(agent_id);
            if let Some((_, rel, src)) = meta {
                fact.fact.content.push('\n');
                fact.fact
                    .content
                    .push_str(&format!("[relations] ⚠ {rel} ← {src} (force-surfaced)"));
            }
            // Sentinel score below any real hit; presence is the point.
            fact.score = 0.0;
            ranked.push(fact);
        }
    }

    /// Apply the cross-encoder reranker to a candidate set, blending its scores
    /// with the original retrieval scores via `blend_scores`. Falls back to the
    /// original ordering on any reranker error (graceful degradation). Candidates
    /// are carried through by positional index (not note path): in the multi-agent
    /// path two agents can hold notes at the same relative path, so keying by path
    /// would collapse them and silently drop one.
    pub(super) async fn apply_rerank(
        &self,
        query: &str,
        facts: Vec<ScoredFact>,
        sink: &mut TraceSink,
    ) -> Vec<ScoredFact> {
        let Some(reranker) = self.reranker.as_ref() else {
            return facts;
        };
        // Nothing to reorder for trivial sets.
        if facts.len() < 2 {
            return facts;
        }
        let t0 = Instant::now();
        let input = facts.len();

        // rust-doctor-disable-next-line excessive-clone
        let docs: Vec<String> = facts.iter().map(|f| f.fact.content.clone()).collect();
        let reranked = match reranker.rerank(query, &docs, docs.len()).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    provider = reranker.provider_id(),
                    "cross-encoder rerank failed; keeping original order"
                );
                return facts;
            }
        };

        // Carry-through key is the positional index, NOT fact.id: across agents
        // two notes can share the same relative path (fact.id), and keying the
        // rebuild map by it would collapse them, silently dropping one. The index
        // is unique per candidate and positionally aligned with `reranked`.
        let originals: Vec<(String, f32)> = facts
            .iter()
            .enumerate()
            .map(|(i, f)| (i.to_string(), f.score))
            .collect();
        let blended = blend_scores(&originals, &reranked, self.rerank_weight);

        // Rebuild ScoredFacts in blended order, carrying the new scores.
        let mut by_key: HashMap<String, ScoredFact> = facts
            .into_iter()
            .enumerate()
            .map(|(i, f)| (i.to_string(), f))
            .collect();
        let mut out = Vec::with_capacity(by_key.len());
        for (key, score) in blended {
            if let Some(mut fact) = by_key.remove(&key) {
                fact.score = score;
                out.push(fact);
            }
        }
        sink.record("rerank", t0.elapsed().as_millis() as u64, input, out.len());
        out
    }
}
