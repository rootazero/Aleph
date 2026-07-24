//! Note-based retrieval engine.
//!
//! Drop-in replacement for `FactRetrieval` that queries notes (markdown + `SQLite` index)
//! instead of the legacy facts table. Returns `Vec<ScoredFact>` so downstream
//! consumers don't require changes.

pub mod expansion;
mod relation_surface;
pub mod scoring;
pub mod trace;

use std::collections::HashMap;

use self::trace::{StageTrace, TraceSink};
use crate::config::types::memory::{ExpansionConfig, RetrievalScoringConfig};
use crate::error::AlephError;
use crate::memory::context::{MemoryFact, NoteType};
use crate::memory::notes::store::{NoteIndexEntry, NoteStore};
use crate::memory::notes::NoteIndexer;
use crate::memory::rerank::{blend_scores, build_provider, RerankConfig, RerankProvider};
use crate::memory::store::types::ScoredFact;
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;
use std::time::Instant;

/// When a cross-encoder reranker is active, over-fetch candidates by this factor
/// so the reranker has a meaningful pool to reorder before truncation.
const RERANK_CANDIDATE_MULTIPLIER: usize = 3;

/// Hard ceiling on the candidate pool sent to the cross-encoder, to bound the
/// remote rerank request cost regardless of the caller's `limit`.
const RERANK_MAX_CANDIDATES: usize = 50;

/// Channel label stamped on recall signals emitted automatically by the primary
/// retrieval path. Kept distinct from explicit `memory_reflect` synthesis signals
/// so the two dedup independently in `recall_signals`
/// (`UNIQUE(note_path, query_hash, day_bucket, channel)`).
const AUTO_RECALL_CHANNEL: &str = "auto-recall";

/// Notes-based retrieval engine. Drop-in replacement for `FactRetrieval`.
pub struct NoteFactRetrieval<S: NoteStore + Send + Sync + 'static> {
    indexer: Arc<NoteIndexer<S>>,
    /// `None` = FTS-only deployment (no embedding provider configured):
    /// hybrid/vector legs are skipped and every retrieval degrades to
    /// keyword (FTS) search instead of failing.
    embedder: Option<Arc<dyn EmbeddingProvider>>,
    /// Optional cross-encoder reranker applied as a final retrieval stage.
    /// `None` (the default) reproduces the legacy behaviour byte-for-byte.
    reranker: Option<Arc<dyn RerankProvider>>,
    /// Blend weight for the reranker score in `[0.0, 1.0]` (only used when
    /// `reranker` is `Some`).
    rerank_weight: f32,
    /// Retrieval-time recency decay + MMR diversity. Default-inactive, so the
    /// base `new()` reproduces legacy ranking byte-for-byte.
    scoring: RetrievalScoringConfig,
    /// Associative graph expansion of the candidate pool before rerank.
    /// Default-on; a cold graph cache makes it a no-op.
    expansion: ExpansionConfig,
}

impl<S: NoteStore + Send + Sync + 'static> NoteFactRetrieval<S> {
    pub fn new(indexer: Arc<NoteIndexer<S>>, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            indexer,
            embedder: Some(embedder),
            reranker: None,
            rerank_weight: 0.6,
            scoring: RetrievalScoringConfig::default(),
            expansion: ExpansionConfig::default(),
        }
    }

    /// Build a retrieval engine with no embedding provider. All searches run
    /// FTS-only; `vector_retrieve` returns a config error. Used when the
    /// deployment has no embedder so prompt-injected memory keeps working.
    pub fn new_fts_only(indexer: Arc<NoteIndexer<S>>) -> Self {
        Self {
            indexer,
            embedder: None,
            reranker: None,
            rerank_weight: 0.6,
            scoring: RetrievalScoringConfig::default(),
            expansion: ExpansionConfig::default(),
        }
    }

    /// Attach retrieval-time scoring (recency decay + MMR diversity). An
    /// inactive config (the default) is a no-op, so callers may wire it
    /// unconditionally without changing legacy behaviour.
    #[must_use]
    pub fn with_scoring_config(mut self, cfg: &RetrievalScoringConfig) -> Self {
        // rust-doctor-disable-next-line excessive-clone
        self.scoring = cfg.clone();
        self
    }

    /// Attach associative graph-expansion config. `new()` is already on; this
    /// lets callers tune or disable it. `weight` is clamped to `[0,1]`.
    #[must_use]
    pub fn with_expansion_config(mut self, cfg: &ExpansionConfig) -> Self {
        // rust-doctor-disable-next-line excessive-clone
        self.expansion = cfg.clone();
        self.expansion.weight = self.expansion.weight.clamp(0.0, 1.0);
        self
    }

    /// Attach a cross-encoder reranker as a final retrieval stage (non-breaking
    /// builder; the base `new()` keeps reranking disabled).
    pub fn with_reranker(mut self, reranker: Arc<dyn RerankProvider>, weight: f32) -> Self {
        self.reranker = Some(reranker);
        self.rerank_weight = weight.clamp(0.0, 1.0);
        self
    }

    /// Build and attach a reranker from configuration. A disabled config is a
    /// no-op (returns `self` unchanged), so callers can wire unconditionally.
    ///
    /// Activates the otherwise-dormant `memory::rerank` provider backends.
    #[must_use]
    pub fn with_rerank_config(self, cfg: &RerankConfig) -> Self {
        if !cfg.enabled {
            return self;
        }
        let provider: Arc<dyn RerankProvider> = Arc::from(build_provider(cfg));
        self.with_reranker(provider, cfg.rerank_weight)
    }

    /// Candidate pool size to fetch before reranking/scoring. Without a reranker
    /// or active scoring this is exactly `limit` (preserving legacy fetch counts);
    /// otherwise it over-fetches up to a bounded ceiling so the reranker / MMR
    /// pass has a real pool to reorder before truncation.
    fn fetch_limit(&self, limit: usize) -> usize {
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
    fn apply_scoring(
        &self,
        facts: Vec<ScoredFact>,
        now: i64,
        reinforcement_counts: &HashMap<String, i64>,
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
                let hits = reinforcement_counts.get(&f.fact.id).copied().unwrap_or(0);
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

    /// Fetch recall-frequency counts for the candidate notes when reinforcement
    /// salience is enabled. Returns an empty map when disabled or on any store
    /// error, degrading gracefully to neutral (legacy) scoring.
    async fn fetch_reinforcement_counts(
        &self,
        agent_id: &str,
        facts: &[ScoredFact],
    ) -> HashMap<String, i64> {
        if !self.scoring.reinforcement_enabled || facts.is_empty() {
            return HashMap::new();
        }
        // rust-doctor-disable-next-line excessive-clone
        let paths: Vec<String> = facts.iter().map(|f| f.fact.id.clone()).collect();
        self.indexer
            .store()
            .recall_hit_counts(agent_id, &paths)
            .await
            .unwrap_or_default()
    }

    /// Producer of the `recall_signals` table — the raw access signal three
    /// independent consumers depend on: hot-floating reinforcement ranking
    /// (`fetch_reinforcement_counts`), `NoteDecay`'s `access_weight`, and the
    /// evolution recall-evidence gate (`recall_hit_counts`). Recording is
    /// therefore **unconditional** (only skipped on an empty result set): it is
    /// NOT gated on `reinforcement_enabled`, because disabling hot-floating
    /// ranking must not silently blind decay and the evolution gate to which
    /// notes are actually being used (which made every note look never-recalled
    /// and biased archival). The reinforcement *ranking boost* stays gated in
    /// `apply_scoring`. Best-effort — a write failure never breaks recall. The
    /// store dedups per `(note_path, query, day, channel)`, so a note must
    /// surface across *distinct* queries/days to genuinely heat up.
    async fn record_recall(&self, query: &str, agent_id: &str, ranked: &[ScoredFact]) {
        if ranked.is_empty() {
            return;
        }
        let hits: Vec<(String, f32)> = ranked
            .iter()
            // rust-doctor-disable-next-line excessive-clone
            .map(|f| (f.fact.id.clone(), f.score))
            .collect();
        if let Err(e) = self
            .indexer
            .store()
            .record_recall_hits(query, AUTO_RECALL_CHANNEL, &hits, agent_id)
            .await
        {
            tracing::debug!(error = %e, "failed to record recall signals (non-fatal)");
        }
    }

    /// Annotate surfaced notes with backlink counts + structural-strong
    /// relations, and force-inject the targets of structural-strong relations
    /// that the score-based ranking dropped. Scoped to already-surfaced notes.
    /// Non-fatal: store errors are logged and skipped.
    async fn surface_relations(&self, agent_id: &str, ranked: &mut Vec<ScoredFact>) {
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
    async fn apply_rerank(
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

    /// Same as [`retrieve`], but also returns per-stage telemetry for the
    /// retrieval debug panel. Results and ordering are identical to `retrieve`.
    pub async fn retrieve_traced(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<(Vec<ScoredFact>, Vec<StageTrace>), AlephError> {
        let mut sink = TraceSink::On(Vec::new());
        let results = self
            .retrieve_inner(query, agent_id, limit, &mut sink)
            .await?;
        Ok((results, sink.into_stages()))
    }

    /// Shared orchestration for `retrieve` / `retrieve_traced`. The `sink`
    /// records stage telemetry only when `On`; `Off` is a no-op hot path with
    /// byte-identical results.
    async fn retrieve_inner(
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
        let mut results = self
            .indexer
            .store()
            .hybrid_search_notes(&embedding, query, agent_id, dim, self.fetch_limit(limit))
            .await?;
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
        let counts = self.fetch_reinforcement_counts(agent_id, &ranked).await;
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
        self.record_recall(query, agent_id, &ranked).await;
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
    /// Note: FTS results don't carry scores natively — rank-based scores are assigned.
    pub async fn text_retrieve(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let entries = self
            .indexer
            .store()
            .search_notes_fts(query, agent_id, limit)
            .await?;

        let total = entries.len() as f32;
        Ok(entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                scored_fact_from_index_entry(entry, agent_id, 1.0 - (i as f32 / total.max(1.0)))
            })
            .collect())
    }

    /// FTS-only recall tail shared by both `retrieve_inner` degradation branches
    /// (no embedder configured, and transient embed-endpoint outage). Runs the
    /// same reinforcement + recency scoring and recall recording as the hybrid
    /// path, so hot-surfacing accrues signal and ranks even when the
    /// vector leg is unavailable — mirrors `multi_agent_text_fallback`'s
    /// graceful-degradation contract (P7). Without this, an FTS-only deployment
    /// never writes `recall_signals`, leaving reinforcement a permanent no-op.
    async fn text_retrieve_scored(
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
        let counts = self.fetch_reinforcement_counts(agent_id, &results).await;
        let ranked = self.apply_scoring(results, now_unix(), &counts, sink);
        self.record_recall(query, agent_id, &ranked).await;
        Ok(ranked)
    }

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
        if agent_ids.is_empty() {
            return Ok(Vec::new());
        }

        // No embedder configured (FTS-only deployment): degrade to per-agent
        // keyword search, mirroring the single-agent path.
        let Some(embedder) = self.embedder.as_ref() else {
            return self
                .multi_agent_text_fallback(query, agent_ids, limit)
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
                    .multi_agent_text_fallback(query, agent_ids, limit)
                    .await;
            }
        };
        let dim = embedding.len() as u32;

        let mut all_results: Vec<ScoredFact> = Vec::new();
        // Over-fetch per agent so merged top-k is well-populated
        let per_agent_limit = limit.max(10);

        for agent_id in agent_ids {
            let mut results = self
                .indexer
                .store()
                .hybrid_search_notes(&embedding, query, agent_id, dim, per_agent_limit)
                .await?;
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

        // Sort by score DESC, then bound the pool before the (optional) rerank.
        all_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all_results.truncate(self.fetch_limit(limit));

        // Close the hot-floating loop across the multi-agent ("smart recall")
        // path too. Recall signals are per-agent, so this path records and reads
        // reinforcement under the first workspace id as a representative label
        // (notes owned by the other agents contribute no cross-agent
        // reinforcement here — acceptable for the fuzzy smart-recall path).
        let recall_agent = agent_ids.first().map_or("owner", String::as_str);
        let ranked = self
            .apply_rerank(query, all_results, &mut TraceSink::Off)
            .await;
        let counts = self.fetch_reinforcement_counts(recall_agent, &ranked).await;
        let mut ranked = self.apply_scoring(ranked, now_unix(), &counts, &mut TraceSink::Off);
        ranked.truncate(limit);
        self.record_recall(query, recall_agent, &ranked).await;
        Ok(ranked)
    }

    /// FTS-only multi-agent recall: per-agent keyword search merged, sorted,
    /// truncated, and recorded. Shared by the no-embedder deployment and the
    /// embed-failure degrade path so both honor the same graceful-degradation
    /// contract (P7) — a transient embed outage must not brick smart recall.
    async fn multi_agent_text_fallback(
        &self,
        query: &str,
        agent_ids: &[String],
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let per_agent_limit = limit.max(10);
        let mut all_results: Vec<ScoredFact> = Vec::new();
        for agent_id in agent_ids {
            all_results.extend(self.text_retrieve(query, agent_id, per_agent_limit).await?);
        }
        all_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all_results.truncate(limit);
        let namespace = agent_ids.first().map_or("owner", String::as_str);
        self.record_recall(query, namespace, &all_results).await;
        Ok(all_results)
    }

    /// Discover all agent IDs by listing subdirectories of the memory dir,
    /// then retrieve across all of them.
    ///
    /// Returns empty if no agents or memory dir is unreadable.
    pub async fn retrieve_all_agents(
        &self,
        query: &str,
        memory_dir: &std::path::Path,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let agent_ids = discover_agent_ids(memory_dir).await;
        if agent_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.retrieve_multi_agent(query, &agent_ids, limit).await
    }
}

/// Current Unix time in seconds (for retrieval-time recency scoring).
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Discover agent IDs by reading directory names under `memory_dir`.
async fn discover_agent_ids(memory_dir: &std::path::Path) -> Vec<String> {
    let mut agents = Vec::new();
    let mut dir = match tokio::fs::read_dir(memory_dir).await {
        Ok(d) => d,
        Err(_) => return agents,
    };
    while let Ok(Some(entry)) = dir.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if let Ok(ft) = entry.file_type().await {
            if ft.is_dir() {
                agents.push(name);
            }
        }
    }
    agents
}

/// Build a `ScoredFact` from a lightweight `NoteIndexEntry`.
///
/// Content is not available in index entries — the `content` field is left empty.
/// Callers that need full content should use the vector or hybrid search paths.
fn scored_fact_from_index_entry(entry: &NoteIndexEntry, agent_id: &str, score: f32) -> ScoredFact {
    let note_type = NoteType::from_str_or_other(&entry.category);
    let mut fact = MemoryFact::new(
        String::new(), // content not stored in index entries
        note_type,
        // rust-doctor-disable-next-line excessive-clone
        entry.tags.clone(),
    );
    // rust-doctor-disable-next-line excessive-clone
    fact.id = entry.path.clone();
    fact.path = format!("note://{}", entry.path);
    fact.agent = agent_id.to_string();
    fact.created_at = entry.created_at;
    fact.updated_at = entry.updated_at;
    fact.is_valid = true;
    ScoredFact { fact, score }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::SqliteMemoryBackend;
    use tempfile::tempdir;

    // MockEmbeddingProvider lives in a #[cfg(test)] mod inside embedding_provider.rs
    use crate::memory::embedding_provider::tests::MockEmbeddingProvider;

    async fn create_retrieval() -> (NoteFactRetrieval<SqliteMemoryBackend>, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let backend: Arc<SqliteMemoryBackend> =
            Arc::new(SqliteMemoryBackend::new(dir.path()).unwrap());
        let indexer = Arc::new(NoteIndexer::new(dir.path().to_path_buf(), backend.clone()));
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock"));
        (NoteFactRetrieval::new(indexer, embedder), dir)
    }

    /// Embedder that always fails — simulates the embedding API being
    /// unreachable (network outage / provider down).
    struct FailingEmbeddingProvider;

    #[async_trait::async_trait]
    impl EmbeddingProvider for FailingEmbeddingProvider {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
            Err(AlephError::network("embedding endpoint unreachable"))
        }

        async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Err(AlephError::network("embedding endpoint unreachable"))
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

    #[tokio::test]
    async fn retrieve_falls_back_to_fts_when_embedding_fails() {
        use crate::memory::notes::KnowledgeNote;

        let dir = tempdir().unwrap();
        let backend: Arc<SqliteMemoryBackend> =
            Arc::new(SqliteMemoryBackend::new(dir.path()).unwrap());
        let note = KnowledgeNote {
            title: "dreame brand incident".to_string(),
            category: "general".to_string(),
            tags: vec!["test".to_string()],
            facts: vec!["dreame brand incident fact".to_string()],
            created_at: 1000,
            updated_at: 1000,
            content_hash: "hash_dreame".to_string(),
            ..Default::default()
        };
        backend
            .index_note(&note, "default", "general")
            .await
            .unwrap();

        let indexer = Arc::new(NoteIndexer::new(dir.path().to_path_buf(), backend.clone()));
        let retrieval = NoteFactRetrieval::new(indexer, Arc::new(FailingEmbeddingProvider));

        let results = retrieval
            .retrieve("dreame", "default", 10)
            .await
            .expect("embedding outage must degrade to FTS, not fail the whole search");
        assert!(
            !results.is_empty(),
            "FTS fallback should surface the indexed note"
        );
    }

    #[tokio::test]
    async fn retrieve_multi_agent_falls_back_to_fts_when_embedding_fails() {
        // Regression (B1): with an embedder configured, a transient embed
        // outage used to propagate through `retrieve_multi_agent` and brick
        // ALL smart recall, while the single-agent path degraded to FTS. Both
        // must degrade (P7).
        use crate::memory::notes::KnowledgeNote;

        let dir = tempdir().unwrap();
        let backend: Arc<SqliteMemoryBackend> =
            Arc::new(SqliteMemoryBackend::new(dir.path()).unwrap());
        let note = KnowledgeNote {
            title: "dreame brand incident".to_string(),
            category: "general".to_string(),
            tags: vec!["test".to_string()],
            facts: vec!["dreame brand incident fact".to_string()],
            created_at: 1000,
            updated_at: 1000,
            content_hash: "hash_dreame".to_string(),
            ..Default::default()
        };
        backend
            .index_note(&note, "default", "general")
            .await
            .unwrap();

        let indexer = Arc::new(NoteIndexer::new(dir.path().to_path_buf(), backend.clone()));
        let retrieval = NoteFactRetrieval::new(indexer, Arc::new(FailingEmbeddingProvider));

        let results = retrieval
            .retrieve_multi_agent("dreame", &["default".to_string()], 10)
            .await
            .expect("embed outage must degrade multi-agent recall to FTS, not fail it");
        assert!(
            !results.is_empty(),
            "multi-agent FTS fallback should surface the indexed note"
        );
    }

    #[tokio::test]
    async fn retrieve_works_fts_only_without_embedder() {
        use crate::memory::notes::KnowledgeNote;

        let dir = tempdir().unwrap();
        let backend: Arc<SqliteMemoryBackend> =
            Arc::new(SqliteMemoryBackend::new(dir.path()).unwrap());
        let note = KnowledgeNote {
            title: "dreame brand incident".to_string(),
            category: "general".to_string(),
            tags: vec!["test".to_string()],
            facts: vec!["dreame brand incident fact".to_string()],
            created_at: 1000,
            updated_at: 1000,
            content_hash: "hash_dreame".to_string(),
            ..Default::default()
        };
        backend
            .index_note(&note, "default", "general")
            .await
            .unwrap();

        let indexer = Arc::new(NoteIndexer::new(dir.path().to_path_buf(), backend.clone()));
        let retrieval = NoteFactRetrieval::new_fts_only(indexer);

        let results = retrieval
            .retrieve("dreame", "default", 10)
            .await
            .expect("FTS-only deployment must retrieve without an embedder");
        assert!(
            !results.is_empty(),
            "FTS-only retrieval should surface the indexed note"
        );

        // Multi-agent smart recall degrades the same way.
        let multi = retrieval
            .retrieve_multi_agent("dreame", &["default".to_string()], 10)
            .await
            .unwrap();
        assert!(
            !multi.is_empty(),
            "multi-agent FTS fallback should surface the note"
        );

        // Vector search is honestly unavailable.
        assert!(retrieval
            .vector_retrieve("dreame", "default", 10)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn retrieve_empty_returns_empty() {
        let (retrieval, _dir) = create_retrieval().await;
        let results = retrieval
            .retrieve("test query", "default", 10)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn retrieve_surfaces_graph_peer_only_when_materialized() {
        use crate::memory::notes::KnowledgeNote;

        let dir = tempdir().unwrap();
        let backend: Arc<SqliteMemoryBackend> =
            Arc::new(SqliteMemoryBackend::new(dir.path()).unwrap());

        // A matches the query token "dreame"; B is unrelated lexically.
        let a = KnowledgeNote {
            title: "alpha".to_string(),
            category: "general".to_string(),
            facts: vec!["dreame brand incident".to_string()],
            content_hash: "h_a".to_string(),
            ..Default::default()
        };
        let b = KnowledgeNote {
            title: "beta".to_string(),
            category: "general".to_string(),
            facts: vec!["unrelated vacuum robotics note".to_string()],
            content_hash: "h_b".to_string(),
            ..Default::default()
        };
        backend.index_note(&a, "default", "general").await.unwrap();
        backend.index_note(&b, "default", "general").await.unwrap();

        let indexer = Arc::new(NoteIndexer::new(dir.path().to_path_buf(), backend.clone()));
        // MockEmbeddingProvider (not Failing): retrieve() must reach
        // hybrid_search_notes + the expansion stage. FailingEmbeddingProvider
        // would divert to text_retrieve, which does NOT run expansion. Notes
        // have no stored vectors, so the vector leg is empty and FTS surfaces A.
        let retrieval =
            NoteFactRetrieval::new(indexer, Arc::new(MockEmbeddingProvider::new(1024, "mock")));

        // Cold cache: B must NOT surface for a query that only matches A.
        let cold = retrieval.retrieve("dreame", "default", 10).await.unwrap();
        assert!(
            cold.iter().all(|f| f.fact.id != "general/beta"),
            "without a materialized edge, the unrelated note must not surface"
        );

        // Materialize A -> B; now B surfaces via associative expansion.
        backend
            .replace_graph_related(
                "default",
                &[("general/alpha".to_string(), "general/beta".to_string(), 4.0)],
            )
            .await
            .unwrap();
        let warm = retrieval.retrieve("dreame", "default", 10).await.unwrap();
        assert!(
            warm.iter().any(|f| f.fact.id == "general/beta"),
            "with a materialized edge, the graph peer must surface"
        );
    }

    // --- Hot-floating recall-signal producer wiring ------------------------

    #[tokio::test]
    async fn record_recall_hits_roundtrips_to_hit_counts() {
        // Producer (record_recall_hits) and consumer (recall_hit_counts) close
        // the hot-floating loop: a recorded recall becomes a non-zero hit count.
        let (retrieval, _dir) = create_retrieval().await;
        let store = retrieval.indexer.store();
        let hits = vec![
            ("notes/a.md".to_string(), 0.9_f32),
            ("notes/b.md".to_string(), 0.7),
        ];

        let inserted = store
            .record_recall_hits("hello world", AUTO_RECALL_CHANNEL, &hits, "default")
            .await
            .unwrap();
        assert_eq!(inserted, 2);

        // Same query + day + channel dedups to zero new rows.
        let dup = store
            .record_recall_hits("hello world", AUTO_RECALL_CHANNEL, &hits, "default")
            .await
            .unwrap();
        assert_eq!(dup, 0);

        let counts = store
            .recall_hit_counts(
                "default",
                &["notes/a.md".to_string(), "notes/b.md".to_string()],
            )
            .await
            .unwrap();
        assert_eq!(counts.get("notes/a.md"), Some(&1));
        assert_eq!(counts.get("notes/b.md"), Some(&1));

        // A distinct query accrues an additional, independent hit.
        store
            .record_recall_hits("another query", AUTO_RECALL_CHANNEL, &hits, "default")
            .await
            .unwrap();
        let counts2 = store
            .recall_hit_counts("default", &["notes/a.md".to_string()])
            .await
            .unwrap();
        assert_eq!(counts2.get("notes/a.md"), Some(&2));
    }

    #[tokio::test]
    async fn record_recall_empty_writes_nothing_but_disabled_still_records() {
        let (retrieval, _dir) = create_retrieval().await;
        // Empty result set → no write, no panic (reinforcement default-on).
        retrieval.record_recall("q", "default", &[]).await;

        // Reinforcement RANKING disabled must NOT blind the recall signal:
        // NoteDecay's access_weight and the evolution recall-evidence gate both
        // consume `recall_signals` independently of hot-floating. Recording is
        // therefore decoupled from `reinforcement_enabled`.
        let off = NoteFactRetrieval::new(
            retrieval.indexer.clone(),
            retrieval.embedder.clone().unwrap(),
        )
        .with_scoring_config(&inactive_scoring());
        off.record_recall("q", "default", &[scored("notes/x.md", "x", 0.9)])
            .await;

        let counts = retrieval
            .indexer
            .store()
            .recall_hit_counts("default", &["notes/x.md".to_string()])
            .await
            .unwrap();
        assert_eq!(
            counts.get("notes/x.md"),
            Some(&1),
            "recall must be recorded even when reinforcement ranking is disabled"
        );
    }

    #[tokio::test]
    async fn vector_retrieve_empty_returns_empty() {
        let (retrieval, _dir) = create_retrieval().await;
        let results = retrieval
            .vector_retrieve("test", "default", 10)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn text_retrieve_empty_returns_empty() {
        let (retrieval, _dir) = create_retrieval().await;
        let results = retrieval
            .text_retrieve("query", "default", 10)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn retrieve_multi_agent_empty_agents_returns_empty() {
        let (retrieval, _dir) = create_retrieval().await;
        let results = retrieval
            .retrieve_multi_agent("query", &[], 10)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn retrieve_multi_agent_unknown_agents_returns_empty() {
        let (retrieval, _dir) = create_retrieval().await;
        let agents = vec!["agent-a".to_string(), "agent-b".to_string()];
        let results = retrieval
            .retrieve_multi_agent("query", &agents, 10)
            .await
            .unwrap();
        assert!(results.is_empty(), "No notes indexed yet → no results");
    }

    #[tokio::test]
    async fn retrieve_all_agents_empty_dir_returns_empty() {
        let (retrieval, dir) = create_retrieval().await;
        let results = retrieval
            .retrieve_all_agents("query", dir.path(), 10)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    // --- Cross-encoder rerank wiring ---------------------------------------

    use crate::memory::rerank::{RerankProvider, RerankResult};
    use async_trait::async_trait;

    /// Deterministic mock reranker: returns the configured per-index scores, or
    /// an error when `fail` is set (to exercise graceful degradation).
    struct MockReranker {
        scores: Vec<(usize, f32)>,
        fail: bool,
    }

    #[async_trait]
    impl RerankProvider for MockReranker {
        async fn rerank(
            &self,
            _query: &str,
            _documents: &[String],
            _top_n: usize,
        ) -> Result<Vec<RerankResult>, AlephError> {
            if self.fail {
                return Err(AlephError::provider("mock rerank failure"));
            }
            Ok(self
                .scores
                .iter()
                .map(|(index, relevance_score)| RerankResult {
                    index: *index,
                    relevance_score: *relevance_score,
                })
                .collect())
        }
        fn provider_id(&self) -> &str {
            "mock"
        }
    }

    /// Build a `ScoredFact` whose id (path) is unique, carrying content + score.
    fn scored(path: &str, content: &str, score: f32) -> ScoredFact {
        let mut fact = MemoryFact::new(content.to_string(), NoteType::Other, vec![]);
        fact.id = path.to_string();
        fact.path = format!("note://{path}");
        fact.is_valid = true;
        ScoredFact { fact, score }
    }

    fn with_mock(
        retrieval: NoteFactRetrieval<SqliteMemoryBackend>,
        scores: Vec<(usize, f32)>,
        fail: bool,
        weight: f32,
    ) -> NoteFactRetrieval<SqliteMemoryBackend> {
        retrieval.with_reranker(Arc::new(MockReranker { scores, fail }), weight)
    }

    #[tokio::test]
    async fn apply_rerank_reorders_by_blended_score() {
        let (retrieval, _dir) = create_retrieval().await;
        // Original order a > b > c; full rerank weight flips c to the top.
        let facts = vec![
            scored("p/a", "alpha", 0.9),
            scored("p/b", "beta", 0.8),
            scored("p/c", "gamma", 0.7),
        ];
        let retrieval = with_mock(retrieval, vec![(2, 0.99), (0, 0.5), (1, 0.1)], false, 1.0);
        let out = retrieval
            .apply_rerank("q", facts, &mut TraceSink::Off)
            .await;
        let order: Vec<&str> = out.iter().map(|f| f.fact.id.as_str()).collect();
        assert_eq!(order, vec!["p/c", "p/a", "p/b"]);
    }

    #[tokio::test]
    async fn apply_rerank_keeps_same_path_notes_across_agents() {
        // Regression: in the multi-agent path two agents can each hold a note at
        // the same relative path (fact.id, e.g. "general/index"). Keying the
        // rebuild map by fact.id collapsed them into one HashMap slot, silently
        // dropping a note. Positional-index keying must keep both.
        let (retrieval, _dir) = create_retrieval().await;
        let mut a = scored("general/index", "alpha notes", 0.9);
        a.fact.agent = "agent-a".to_string();
        let mut b = scored("general/index", "beta notes", 0.8);
        b.fact.agent = "agent-b".to_string();
        // Full rerank weight; boost the second candidate so both must survive
        // AND reorder (proving neither the drop nor a score swap happens).
        let retrieval = with_mock(retrieval, vec![(1, 0.99), (0, 0.1)], false, 1.0);
        let out = retrieval
            .apply_rerank("q", vec![a, b], &mut TraceSink::Off)
            .await;
        assert_eq!(out.len(), 2, "both same-path notes must survive rerank");
        let agents: Vec<&str> = out.iter().map(|f| f.fact.agent.as_str()).collect();
        assert_eq!(agents, vec!["agent-b", "agent-a"]);
    }

    #[tokio::test]
    async fn apply_rerank_falls_back_on_error() {
        let (retrieval, _dir) = create_retrieval().await;
        let facts = vec![scored("p/a", "alpha", 0.9), scored("p/b", "beta", 0.5)];
        let retrieval = with_mock(retrieval, vec![], true, 1.0);
        let out = retrieval
            .apply_rerank("q", facts, &mut TraceSink::Off)
            .await;
        // Error → original order preserved, no facts dropped.
        let order: Vec<&str> = out.iter().map(|f| f.fact.id.as_str()).collect();
        assert_eq!(order, vec!["p/a", "p/b"]);
    }

    #[tokio::test]
    async fn apply_rerank_noop_without_reranker() {
        let (retrieval, _dir) = create_retrieval().await;
        let facts = vec![scored("p/a", "alpha", 0.9), scored("p/b", "beta", 0.5)];
        let out = retrieval
            .apply_rerank("q", facts, &mut TraceSink::Off)
            .await;
        let order: Vec<&str> = out.iter().map(|f| f.fact.id.as_str()).collect();
        assert_eq!(order, vec!["p/a", "p/b"]);
    }

    #[test]
    fn with_rerank_config_disabled_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let backend: Arc<SqliteMemoryBackend> =
            Arc::new(SqliteMemoryBackend::new(dir.path()).unwrap());
        let indexer = Arc::new(NoteIndexer::new(dir.path().to_path_buf(), backend));
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock"));
        // Hold scoring inactive so fetch_limit isolates the reranker's effect
        // (scoring now defaults active, which would over-fetch on its own).
        let retrieval =
            NoteFactRetrieval::new(indexer, embedder).with_scoring_config(&inactive_scoring());
        let cfg = crate::memory::rerank::RerankConfig::default(); // enabled = false
        let retrieval = retrieval.with_rerank_config(&cfg);
        assert!(retrieval.reranker.is_none());
        // No reranker and scoring inactive → fetch_limit stays exactly `limit`.
        assert_eq!(retrieval.fetch_limit(5), 5);
    }

    #[test]
    fn fetch_limit_overfetches_only_with_reranker() {
        let dir = tempfile::tempdir().unwrap();
        let backend: Arc<SqliteMemoryBackend> =
            Arc::new(SqliteMemoryBackend::new(dir.path()).unwrap());
        let indexer = Arc::new(NoteIndexer::new(dir.path().to_path_buf(), backend));
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock"));
        let retrieval = NoteFactRetrieval::new(indexer, embedder).with_reranker(
            Arc::new(MockReranker {
                scores: vec![],
                fail: false,
            }),
            0.6,
        );
        assert_eq!(retrieval.fetch_limit(5), 15); // 5 * 3
        assert_eq!(retrieval.fetch_limit(20), RERANK_MAX_CANDIDATES); // capped at 50
    }

    // --- Retrieval-time scoring wiring -------------------------------------

    /// Like `scored` but stamps an `updated_at` for recency tests.
    fn scored_at(path: &str, content: &str, score: f32, updated_at: i64) -> ScoredFact {
        let mut f = scored(path, content, score);
        f.fact.updated_at = updated_at;
        f
    }

    /// All-off scoring config. The production default now enables recency +
    /// reinforcement ("auto-surfacing"), so focused unit tests that isolate one knob
    /// (or assert the legacy no-op path) start from this explicit baseline.
    fn inactive_scoring() -> RetrievalScoringConfig {
        RetrievalScoringConfig {
            recency_enabled: false,
            reinforcement_enabled: false,
            mmr_enabled: false,
            ..RetrievalScoringConfig::default()
        }
    }

    #[tokio::test]
    async fn apply_scoring_inactive_is_noop() {
        let (retrieval, _dir) = create_retrieval().await;
        // Explicitly-disabled scoring → order preserved, scores untouched.
        let retrieval = retrieval.with_scoring_config(&inactive_scoring());
        let facts = vec![scored("p/a", "alpha", 0.9), scored("p/b", "beta", 0.5)];
        let out = retrieval.apply_scoring(facts, 1_000_000, &HashMap::new(), &mut TraceSink::Off);
        let order: Vec<&str> = out.iter().map(|f| f.fact.id.as_str()).collect();
        assert_eq!(order, vec!["p/a", "p/b"]);
        assert!((out[0].score - 0.9).abs() < 1e-6);
    }

    #[tokio::test]
    async fn apply_scoring_recency_promotes_fresh_note() {
        let (retrieval, _dir) = create_retrieval().await;
        let cfg = RetrievalScoringConfig {
            recency_enabled: true,
            recency_half_life_days: 90.0,
            recency_weight: 1.0,
            ..inactive_scoring()
        };
        let retrieval = retrieval.with_scoring_config(&cfg);

        let day = 86_400_i64;
        let now = 300 * day;
        // Stale but higher raw relevance vs fresh but lower relevance.
        let facts = vec![
            scored_at("p/stale", "old knowledge", 0.9, now - 200 * day),
            scored_at("p/fresh", "new knowledge", 0.8, now),
        ];
        let out = retrieval.apply_scoring(facts, now, &HashMap::new(), &mut TraceSink::Off);
        let order: Vec<&str> = out.iter().map(|f| f.fact.id.as_str()).collect();
        assert_eq!(
            order,
            vec!["p/fresh", "p/stale"],
            "recency decay should promote the fresh note above the stale one"
        );
    }

    #[tokio::test]
    async fn apply_scoring_mmr_demotes_duplicate() {
        let (retrieval, _dir) = create_retrieval().await;
        let cfg = RetrievalScoringConfig {
            mmr_enabled: true,
            mmr_lambda: 0.5,
            ..inactive_scoring()
        };
        let retrieval = retrieval.with_scoring_config(&cfg);

        let facts = vec![
            scored("p/a", "rust async tokio runtime scheduler", 0.95),
            scored("p/b", "rust async tokio runtime scheduler details", 0.90),
            scored("p/c", "python pandas dataframe analysis", 0.60),
        ];
        let out = retrieval.apply_scoring(facts, 1_000_000, &HashMap::new(), &mut TraceSink::Off);
        let order: Vec<&str> = out.iter().map(|f| f.fact.id.as_str()).collect();
        assert_eq!(order, vec!["p/a", "p/c", "p/b"]);
    }

    #[tokio::test]
    async fn apply_scoring_reinforcement_promotes_hot_note() {
        let (retrieval, _dir) = create_retrieval().await;
        let cfg = RetrievalScoringConfig {
            reinforcement_enabled: true,
            reinforcement_weight: 0.5,
            ..inactive_scoring()
        };
        let retrieval = retrieval.with_scoring_config(&cfg);

        // Lower raw relevance but recalled many times vs higher relevance never recalled.
        let facts = vec![
            scored("p/cold", "rarely used knowledge", 0.80),
            scored("p/hot", "frequently used knowledge", 0.70),
        ];
        let mut counts = HashMap::new();
        counts.insert("p/hot".to_string(), 40_i64);
        // 0.70 * (1 + 0.5 * ln(41)) = 0.70 * (1 + 0.5 * 3.714) = 0.70 * 2.857 = 2.0
        let out = retrieval.apply_scoring(facts, 1_000_000, &counts, &mut TraceSink::Off);
        let order: Vec<&str> = out.iter().map(|f| f.fact.id.as_str()).collect();
        assert_eq!(
            order,
            vec!["p/hot", "p/cold"],
            "a frequently-recalled note should be promoted above a higher-relevance cold one"
        );
    }

    #[tokio::test]
    async fn apply_scoring_reinforcement_disabled_ignores_counts() {
        let (retrieval, _dir) = create_retrieval().await;
        // Reinforcement explicitly disabled → counts are ignored, order untouched.
        let retrieval = retrieval.with_scoring_config(&inactive_scoring());
        let facts = vec![scored("p/a", "alpha", 0.9), scored("p/b", "beta", 0.5)];
        let mut counts = HashMap::new();
        counts.insert("p/b".to_string(), 999_i64);
        let out = retrieval.apply_scoring(facts, 1_000_000, &counts, &mut TraceSink::Off);
        let order: Vec<&str> = out.iter().map(|f| f.fact.id.as_str()).collect();
        assert_eq!(order, vec!["p/a", "p/b"]);
    }

    #[test]
    fn fetch_limit_overfetches_when_mmr_active() {
        let dir = tempfile::tempdir().unwrap();
        let backend: Arc<SqliteMemoryBackend> =
            Arc::new(SqliteMemoryBackend::new(dir.path()).unwrap());
        let indexer = Arc::new(NoteIndexer::new(dir.path().to_path_buf(), backend));
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock"));
        let cfg = RetrievalScoringConfig {
            mmr_enabled: true,
            ..RetrievalScoringConfig::default()
        };
        let retrieval = NoteFactRetrieval::new(indexer, embedder).with_scoring_config(&cfg);
        // No reranker, but MMR active → over-fetch a real pool.
        assert_eq!(retrieval.fetch_limit(5), 15);
    }

    /// Active scoring config so apply_scoring exercises all three sub-stages.
    fn active_scoring() -> RetrievalScoringConfig {
        RetrievalScoringConfig {
            recency_enabled: true,
            reinforcement_enabled: true,
            mmr_enabled: true,
            ..RetrievalScoringConfig::default()
        }
    }

    #[tokio::test]
    async fn apply_scoring_trace_matches_untraced_and_records_stages() {
        let (retrieval, _dir) = create_retrieval().await;
        let retrieval = retrieval.with_scoring_config(&active_scoring());

        let facts = vec![
            scored("a", "alpha content one", 0.9),
            scored("b", "beta content two", 0.5),
            scored("c", "gamma content three", 0.3),
        ];
        let counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();

        // Untraced (Off) reference result.
        let mut off = TraceSink::Off;
        let ref_out = retrieval.apply_scoring(facts.clone(), 1_700_000_000, &counts, &mut off);

        // Traced (On) result must be identical in scores + order.
        let mut on = TraceSink::On(Vec::new());
        let traced_out = retrieval.apply_scoring(facts, 1_700_000_000, &counts, &mut on);

        let ref_ids: Vec<(&str, f32)> = ref_out
            .iter()
            .map(|f| (f.fact.id.as_str(), f.score))
            .collect();
        let traced_ids: Vec<(&str, f32)> = traced_out
            .iter()
            .map(|f| (f.fact.id.as_str(), f.score))
            .collect();
        assert_eq!(ref_ids, traced_ids, "tracing must not change results");

        let stages = on.into_stages();
        let names: Vec<&str> = stages.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"recency_decay"));
        assert!(names.contains(&"reinforcement"));
        assert!(names.contains(&"mmr_diversity"));
        // Recency/reinforcement preserve cardinality.
        for s in &stages {
            if s.name == "recency_decay" || s.name == "reinforcement" {
                assert_eq!(s.input_count, s.output_count);
            }
        }
    }

    #[tokio::test]
    async fn retrieve_traced_on_empty_store_returns_stages() {
        let (retrieval, _dir) = create_retrieval().await;
        let (results, stages) = retrieval
            .retrieve_traced("anything", "main", 5)
            .await
            .unwrap();
        assert!(results.is_empty(), "empty store yields no results");
        // The search stage always runs; with a mock embedder it is hybrid_search.
        assert!(
            stages
                .iter()
                .any(|s| s.name == "hybrid_search" || s.name == "fts_search"),
            "a search stage must be recorded, got {stages:?}"
        );
    }
}
