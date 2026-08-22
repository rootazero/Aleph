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
use crate::memory::notes::search_result::NoteSearchResult;
use crate::memory::notes::store::NoteStore;
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

    /// Bucket `ranked` by the note's true owning namespace and record each
    /// bucket under that owner.
    ///
    /// `recall_signals` rows are per-agent, and every consumer reads them under
    /// a *specific* agent id: `NoteDecay`'s `access_weight` and the evolution
    /// recall-evidence gate both run with the dream context's **scoped** id.
    /// Filing a project note's hit under the base namespace therefore makes the
    /// note look never-recalled to decay (early archival) while crediting a
    /// base-namespace note that was never surfaced.
    ///
    /// `to_scored_fact(agent_id)` already stamped the true owner onto
    /// `fact.agent` when the results were collected, so no new plumbing is
    /// needed — just group by it.
    async fn record_recall_by_owner(&self, query: &str, ranked: &[ScoredFact]) {
        let mut by_owner: HashMap<&str, Vec<ScoredFact>> = HashMap::new();
        for f in ranked {
            by_owner
                .entry(f.fact.agent.as_str())
                .or_default()
                .push(f.clone());
        }
        for (owner, bucket) in &by_owner {
            self.record_recall(query, owner, bucket).await;
        }
    }

    /// Reinforcement counts for a multi-owner candidate set, fetched per owning
    /// namespace and merged. The single-owner `fetch_reinforcement_counts` reads
    /// every path under one agent id, which returns zero for notes owned by any
    /// other namespace in the scope union — i.e. exactly the project notes the
    /// scoped read exists to surface.
    ///
    /// Keyed by `(owner, path)` because two namespaces can hold notes at the
    /// same relative path; a bare-path map would collapse them.
    async fn fetch_reinforcement_counts_by_owner(
        &self,
        facts: &[ScoredFact],
    ) -> HashMap<(String, String), i64> {
        let mut by_owner: HashMap<&str, Vec<ScoredFact>> = HashMap::new();
        for f in facts {
            by_owner
                .entry(f.fact.agent.as_str())
                .or_default()
                .push(f.clone());
        }
        let mut merged: HashMap<(String, String), i64> = HashMap::new();
        for (owner, bucket) in &by_owner {
            let counts = self.fetch_reinforcement_counts(owner, bucket).await;
            for (path, n) in counts {
                merged.insert(((*owner).to_string(), path), n);
            }
        }
        merged
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
        let counts = self.fetch_reinforcement_counts_by_owner(&results).await;
        let ranked = self.apply_scoring(results, now_unix(), &counts, sink);
        self.record_recall_by_owner(query, &ranked).await;
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

    async fn retrieve_multi_agent_inner(
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
    async fn multi_agent_text_fallback(
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

/// Current Unix time in seconds (for retrieval-time recency scoring).
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{MemoryFact, NoteType};
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

    /// Embedder that succeeds, but at a dimension the vector index has no
    /// table for — so the failure lands in the store, after the embed call.
    struct UnsupportedDimEmbeddingProvider;

    #[async_trait::async_trait]
    impl EmbeddingProvider for UnsupportedDimEmbeddingProvider {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
            Ok(vec![0.1; 999])
        }

        async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(texts.iter().map(|_| vec![0.1; 999]).collect())
        }

        fn dimensions(&self) -> usize {
            999
        }

        fn model_name(&self) -> &str {
            "unsupported-dim"
        }

        fn provider_id(&self) -> &str {
            "test"
        }
    }

    #[tokio::test]
    async fn retrieve_falls_back_to_fts_when_the_vector_leg_fails_in_the_store() {
        // The degradation guard covered only `embed()`. The very next call
        // used `?`, so a store-side vector failure emptied <memory-context>
        // for every turn — on the auto-recall path, silently.
        use crate::memory::notes::KnowledgeNote;

        let dir = tempdir().unwrap();
        let backend: Arc<SqliteMemoryBackend> =
            Arc::new(SqliteMemoryBackend::new(dir.path()).unwrap());
        let note = KnowledgeNote {
            title: "dreame brand incident".to_string(),
            category: "general".to_string(),
            facts: vec!["dreame shipped a broken firmware".to_string()],
            ..Default::default()
        };
        backend
            .index_note(&note, "default", "general")
            .await
            .unwrap();

        // rust-doctor-disable-next-line excessive-clone
        let indexer = Arc::new(NoteIndexer::new(dir.path().to_path_buf(), backend.clone()));
        let retrieval = NoteFactRetrieval::new(indexer, Arc::new(UnsupportedDimEmbeddingProvider));

        let results = retrieval
            .retrieve("dreame", "default", 10)
            .await
            .expect("a broken vector leg must degrade to FTS, not fail recall");
        assert!(
            !results.is_empty(),
            "FTS fallback should surface the indexed note"
        );

        let multi = retrieval
            .retrieve_multi_agent("dreame", &["default".to_string()], 10)
            .await
            .expect("multi-agent recall must degrade too");
        assert!(!multi.is_empty(), "multi-agent FTS fallback found nothing");
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

    /// A project-scoped read unions `[base, scoped]` (`read_scope_ids`), and the
    /// multi-agent path used to label every hit with `agent_ids.first()` — the
    /// base id. Decay's `access_weight` and the evolution recall-evidence gate
    /// read signals under the *scoped* id, so project notes looked
    /// never-recalled (early archival) while the base namespace collected
    /// phantom hits for notes it does not own.
    #[tokio::test]
    async fn recall_signals_are_filed_under_each_notes_owning_namespace() {
        let (retrieval, _dir) = create_retrieval().await;

        // Two notes at the SAME relative path in different namespaces — the
        // case a bare-path signal map cannot distinguish.
        let mut base_hit = scored("preference/editor", "base note", 0.9);
        base_hit.fact.agent = "main".to_string();
        let mut scoped_hit = scored("preference/editor", "project note", 0.8);
        scoped_hit.fact.agent = "main__proj-x".to_string();

        retrieval
            .record_recall_by_owner("which editor", &[base_hit, scoped_hit])
            .await;

        let store = retrieval.indexer.store();
        let path = vec!["preference/editor".to_string()];

        let scoped = store
            .recall_hit_counts("main__proj-x", &path)
            .await
            .unwrap();
        assert_eq!(
            scoped.get("preference/editor"),
            Some(&1),
            "the project-owned note must earn its signal under its own namespace"
        );

        let base = store.recall_hit_counts("main", &path).await.unwrap();
        assert_eq!(
            base.get("preference/editor"),
            Some(&1),
            "the base-owned note earns exactly its own signal, not the project's too"
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

    // `retrieve_all_agents_empty_dir_returns_empty` was here. Deleted with the
    // function; the property it checked (an empty corpus list is not an error)
    // is already `retrieve_multi_agent_empty_agents_returns_empty` above, and
    // keeping a second copy of it under a dead name is how a suite grows tests
    // nobody can attribute to a behaviour.

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
        counts.insert(("main".to_string(), "p/hot".to_string()), 40_i64);
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
        counts.insert(("main".to_string(), "p/b".to_string()), 999_i64);
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
        let counts: std::collections::HashMap<(String, String), i64> =
            std::collections::HashMap::new();

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

    /// The x-ray's entry point on an empty corpus still describes a pipeline
    /// rather than returning nothing at all — "no stages" and "stages that
    /// found nothing" are different answers, and only the second one tells the
    /// reader the retriever ran.
    ///
    /// Aimed at the MULTI entry point because that is the one the x-ray calls;
    /// its single-agent twin was cut when the last consumer moved (see the note
    /// where `retrieve_traced` used to be).
    #[tokio::test]
    async fn the_traced_entry_point_on_an_empty_store_still_returns_stages() {
        let (retrieval, _dir) = create_retrieval().await;
        let (results, stages) = retrieval
            .retrieve_multi_agent_traced("anything", &["main".to_string()], 5)
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

    /// Tracing must not change what comes back — the x-ray is an observer, and
    /// a debug view that perturbs the thing it is explaining is worse than no
    /// debug view. Asserted across the partition UNION, since that is the shape
    /// the x-ray actually asks for.
    #[tokio::test]
    async fn tracing_the_multi_path_does_not_change_its_results() {
        let (retrieval, _dir) = create_retrieval().await;
        let ids = vec!["main".to_string(), "main__u-owner".to_string()];

        let untraced = retrieval
            .retrieve_multi_agent("anything", &ids, 5)
            .await
            .unwrap();
        let (traced, stages) = retrieval
            .retrieve_multi_agent_traced("anything", &ids, 5)
            .await
            .unwrap();

        let key = |v: &[ScoredFact]| -> Vec<(String, f32)> {
            v.iter().map(|f| (f.fact.id.clone(), f.score)).collect()
        };
        assert_eq!(
            key(&untraced),
            key(&traced),
            "tracing must not change results"
        );
        assert!(!stages.is_empty(), "the traced call must record something");
    }
}
