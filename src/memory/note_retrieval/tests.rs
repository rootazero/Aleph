#[cfg(test)]
mod tests {
    use super::super::*;
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
