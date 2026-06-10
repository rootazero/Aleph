//! Note-based retrieval engine.
//!
//! Drop-in replacement for `FactRetrieval` that queries notes (markdown + SQLite index)
//! instead of the legacy facts table. Returns `Vec<ScoredFact>` so downstream
//! consumers don't require changes.

pub mod hybrid;
pub mod scoring;

use std::collections::HashMap;

use crate::config::types::memory::RetrievalScoringConfig;
use crate::error::AlephError;
use crate::memory::context::{MemoryFact, NoteType};
use crate::memory::notes::store::{NoteIndexEntry, NoteStore};
use crate::memory::notes::NoteIndexer;
use crate::memory::rerank::{blend_scores, build_provider, RerankConfig, RerankProvider};
use crate::memory::store::types::ScoredFact;
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;

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

/// Notes-based retrieval engine. Drop-in replacement for FactRetrieval.
pub struct NoteFactRetrieval<S: NoteStore + Send + Sync + 'static> {
    indexer: Arc<NoteIndexer<S>>,
    embedder: Arc<dyn EmbeddingProvider>,
    /// Optional cross-encoder reranker applied as a final retrieval stage.
    /// `None` (the default) reproduces the legacy behaviour byte-for-byte.
    reranker: Option<Arc<dyn RerankProvider>>,
    /// Blend weight for the reranker score in `[0.0, 1.0]` (only used when
    /// `reranker` is `Some`).
    rerank_weight: f32,
    /// Retrieval-time recency decay + MMR diversity. Default-inactive, so the
    /// base `new()` reproduces legacy ranking byte-for-byte.
    scoring: RetrievalScoringConfig,
}

impl<S: NoteStore + Send + Sync + 'static> NoteFactRetrieval<S> {
    pub fn new(indexer: Arc<NoteIndexer<S>>, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            indexer,
            embedder,
            reranker: None,
            rerank_weight: 0.6,
            scoring: RetrievalScoringConfig::default(),
        }
    }

    /// Attach retrieval-time scoring (recency decay + MMR diversity). An
    /// inactive config (the default) is a no-op, so callers may wire it
    /// unconditionally without changing legacy behaviour.
    pub fn with_scoring_config(mut self, cfg: &RetrievalScoringConfig) -> Self {
        self.scoring = cfg.clone();
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
    ) -> Vec<ScoredFact> {
        if !self.scoring.is_active() || facts.len() < 2 {
            return facts;
        }
        let mut facts = facts;

        // 1) Recency + reinforcement reweight, then re-sort by adjusted score.
        if self.scoring.recency_enabled || self.scoring.reinforcement_enabled {
            for f in facts.iter_mut() {
                if self.scoring.recency_enabled {
                    let mult = scoring::recency_multiplier(
                        f.fact.updated_at,
                        now,
                        self.scoring.recency_half_life_days,
                    );
                    f.score = scoring::apply_recency(f.score, mult, self.scoring.recency_weight);
                }
                if self.scoring.reinforcement_enabled {
                    let hits = reinforcement_counts.get(&f.fact.id).copied().unwrap_or(0);
                    f.score = scoring::apply_reinforcement(
                        f.score,
                        hits,
                        self.scoring.reinforcement_weight,
                    );
                }
            }
            facts.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        // 2) MMR diversity reorder over the (relevance-sorted) pool.
        if self.scoring.mmr_enabled {
            let contents: Vec<String> = facts.iter().map(|f| f.fact.content.clone()).collect();
            let relevance: Vec<f32> = facts.iter().map(|f| f.score).collect();
            let order = scoring::mmr_reorder(&contents, &relevance, self.scoring.mmr_lambda);
            let mut slots: Vec<Option<ScoredFact>> = facts.into_iter().map(Some).collect();
            facts = order
                .into_iter()
                .filter_map(|i| slots.get_mut(i).and_then(Option::take))
                .collect();
        }

        facts
    }

    /// Fetch recall-frequency counts for the candidate notes when reinforcement
    /// salience is enabled. Returns an empty map when disabled or on any store
    /// error, degrading gracefully to neutral (legacy) scoring.
    async fn fetch_reinforcement_counts(&self, facts: &[ScoredFact]) -> HashMap<String, i64> {
        if !self.scoring.reinforcement_enabled || facts.is_empty() {
            return HashMap::new();
        }
        let paths: Vec<String> = facts.iter().map(|f| f.fact.id.clone()).collect();
        self.indexer
            .store()
            .recall_hit_counts(&paths)
            .await
            .unwrap_or_default()
    }

    /// Producer half of the hot-floating loop: record recall signals for the
    /// notes a retrieval actually surfaced, so frequently-recalled notes accrue
    /// reinforcement salience ("热门记忆浮顶") that `fetch_reinforcement_counts`
    /// later consumes. Best-effort — a write failure never breaks recall — and
    /// gated on `reinforcement_enabled` so disabling hot-floating also stops
    /// recording. The store dedups per `(note_path, query, day, channel)`, so a
    /// note must surface across *distinct* queries/days to genuinely heat up.
    async fn record_recall(&self, query: &str, namespace: &str, ranked: &[ScoredFact]) {
        if !self.scoring.reinforcement_enabled || ranked.is_empty() {
            return;
        }
        let hits: Vec<(String, f32)> = ranked
            .iter()
            .map(|f| (f.fact.id.clone(), f.score))
            .collect();
        if let Err(e) = self
            .indexer
            .store()
            .record_recall_hits(query, AUTO_RECALL_CHANNEL, &hits, namespace)
            .await
        {
            tracing::debug!(error = %e, "failed to record recall signals (non-fatal)");
        }
    }

    /// Apply the cross-encoder reranker to a candidate set, blending its scores
    /// with the original retrieval scores via `blend_scores`. Falls back to the
    /// original ordering on any reranker error (graceful degradation). The result
    /// is keyed by note path (unique), so blending never confuses two facts.
    async fn apply_rerank(&self, query: &str, facts: Vec<ScoredFact>) -> Vec<ScoredFact> {
        let Some(reranker) = self.reranker.as_ref() else {
            return facts;
        };
        // Nothing to reorder for trivial sets.
        if facts.len() < 2 {
            return facts;
        }

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

        let originals: Vec<(String, f32)> =
            facts.iter().map(|f| (f.fact.id.clone(), f.score)).collect();
        let blended = blend_scores(&originals, &reranked, self.rerank_weight);

        // Rebuild ScoredFacts in blended order, carrying the new scores.
        let mut by_path: HashMap<String, ScoredFact> =
            facts.into_iter().map(|f| (f.fact.id.clone(), f)).collect();
        let mut out = Vec::with_capacity(by_path.len());
        for (path, score) in blended {
            if let Some(mut fact) = by_path.remove(&path) {
                fact.score = score;
                out.push(fact);
            }
        }
        out
    }

    /// Hybrid vector + FTS search with RRF fusion.
    /// Returns ScoredFact for downstream compatibility.
    pub async fn retrieve(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        // Embedding requires a remote API call; when that endpoint is
        // unreachable (network outage, provider down) the notes themselves
        // are still local — degrade to FTS-only search instead of failing
        // the whole retrieval.
        let embedding = match self.embedder.embed(query).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "note retrieval: embedding unavailable, falling back to FTS-only search"
                );
                return self.text_retrieve(query, agent_id, limit).await;
            }
        };
        let dim = embedding.len() as u32;

        let results = self
            .indexer
            .store()
            .hybrid_search_notes(&embedding, query, agent_id, dim, self.fetch_limit(limit))
            .await?;

        let facts: Vec<ScoredFact> = results.iter().map(|r| r.to_scored_fact(agent_id)).collect();
        let ranked = self.apply_rerank(query, facts).await;
        let counts = self.fetch_reinforcement_counts(&ranked).await;
        let mut ranked = self.apply_scoring(ranked, now_unix(), &counts);
        ranked.truncate(limit);
        // Close the hot-floating loop: record the surfaced notes as recall hits.
        self.record_recall(query, agent_id, &ranked).await;
        Ok(ranked)
    }

    /// Pure vector similarity search.
    pub async fn vector_retrieve(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let embedding = self.embedder.embed(query).await?;
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

        // Embed once, reuse across agents
        let embedding = self.embedder.embed(query).await?;
        let dim = embedding.len() as u32;

        let mut all_results: Vec<ScoredFact> = Vec::new();
        // Over-fetch per agent so merged top-k is well-populated
        let per_agent_limit = limit.max(10);

        for agent_id in agent_ids {
            let results = self
                .indexer
                .store()
                .hybrid_search_notes(&embedding, query, agent_id, dim, per_agent_limit)
                .await?;
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

        let ranked = self.apply_rerank(query, all_results).await;
        let counts = self.fetch_reinforcement_counts(&ranked).await;
        let mut ranked = self.apply_scoring(ranked, now_unix(), &counts);
        ranked.truncate(limit);
        // Close the hot-floating loop across the multi-agent ("smart recall")
        // path too. `namespace` is signal metadata only (not part of the dedup
        // key), so the first workspace id is a fine representative label.
        let namespace = agent_ids.first().map(String::as_str).unwrap_or("owner");
        self.record_recall(query, namespace, &ranked).await;
        Ok(ranked)
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

/// Discover agent IDs by reading directory names under memory_dir.
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
        entry.tags.clone(),
    );
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
        backend.index_note(&note, "default", "general").await.unwrap();

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
    async fn retrieve_empty_returns_empty() {
        let (retrieval, _dir) = create_retrieval().await;
        let results = retrieval
            .retrieve("test query", "default", 10)
            .await
            .unwrap();
        assert!(results.is_empty());
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
            .recall_hit_counts(&["notes/a.md".to_string(), "notes/b.md".to_string()])
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
            .recall_hit_counts(&["notes/a.md".to_string()])
            .await
            .unwrap();
        assert_eq!(counts2.get("notes/a.md"), Some(&2));
    }

    #[tokio::test]
    async fn record_recall_empty_or_disabled_writes_nothing() {
        let (retrieval, _dir) = create_retrieval().await;
        // Empty result set → no write, no panic (reinforcement default-on).
        retrieval.record_recall("q", "default", &[]).await;

        // Reinforcement disabled → recording is skipped even with surfaced notes.
        let off = NoteFactRetrieval::new(retrieval.indexer.clone(), retrieval.embedder.clone())
            .with_scoring_config(&inactive_scoring());
        off.record_recall("q", "default", &[scored("notes/x.md", "x", 0.9)])
            .await;

        let counts = retrieval
            .indexer
            .store()
            .recall_hit_counts(&["notes/x.md".to_string()])
            .await
            .unwrap();
        assert!(
            counts.is_empty(),
            "disabled reinforcement must not record signals"
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
        let out = retrieval.apply_rerank("q", facts).await;
        let order: Vec<&str> = out.iter().map(|f| f.fact.id.as_str()).collect();
        assert_eq!(order, vec!["p/c", "p/a", "p/b"]);
    }

    #[tokio::test]
    async fn apply_rerank_falls_back_on_error() {
        let (retrieval, _dir) = create_retrieval().await;
        let facts = vec![scored("p/a", "alpha", 0.9), scored("p/b", "beta", 0.5)];
        let retrieval = with_mock(retrieval, vec![], true, 1.0);
        let out = retrieval.apply_rerank("q", facts).await;
        // Error → original order preserved, no facts dropped.
        let order: Vec<&str> = out.iter().map(|f| f.fact.id.as_str()).collect();
        assert_eq!(order, vec!["p/a", "p/b"]);
    }

    #[tokio::test]
    async fn apply_rerank_noop_without_reranker() {
        let (retrieval, _dir) = create_retrieval().await;
        let facts = vec![scored("p/a", "alpha", 0.9), scored("p/b", "beta", 0.5)];
        let out = retrieval.apply_rerank("q", facts).await;
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
    /// reinforcement ("自动冒泡"), so focused unit tests that isolate one knob
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
        let out = retrieval.apply_scoring(facts, 1_000_000, &HashMap::new());
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
        let out = retrieval.apply_scoring(facts, now, &HashMap::new());
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
        let out = retrieval.apply_scoring(facts, 1_000_000, &HashMap::new());
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
        let out = retrieval.apply_scoring(facts, 1_000_000, &counts);
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
        let out = retrieval.apply_scoring(facts, 1_000_000, &counts);
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
}
