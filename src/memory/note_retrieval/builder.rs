//! Construction and configuration of [`NoteFactRetrieval`].
//!
//! Split out of `note_retrieval/mod.rs` verbatim; logic unchanged. These are
//! inherent methods, so they need no delegation layer — the type simply has
//! more than one `impl` block.

use super::*;

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
    pub(super) fn with_reranker(mut self, reranker: Arc<dyn RerankProvider>, weight: f32) -> Self {
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
}
