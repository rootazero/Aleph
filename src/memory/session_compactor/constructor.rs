use super::{SessionCompactor, CompactorMetrics, SessionCompactorConfig};
use crate::memory::store::MemoryBackend;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

impl SessionCompactor {
    /// Create a new SessionCompactor with the given storage backend and config.
    pub fn new(database: MemoryBackend, config: SessionCompactorConfig) -> Self {
        Self {
            database,
            provider: None,
            config,
            metrics: Arc::new(CompactorMetrics::default()),
            indexer: None,
            raw_memory_writer: None,
            capture_registry: None,
        }
    }

    /// Return a reference to the compactor metrics.
    pub fn metrics(&self) -> &Arc<CompactorMetrics> {
        &self.metrics
    }

    /// Attach an AI provider for LLM-based summary generation.
    pub fn with_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Attach an embedding provider for transcript indexing.
    pub fn with_embedder(mut self, embedder: Arc<dyn crate::memory::EmbeddingProvider>) -> Self {
        self.indexer = Some(crate::memory::transcript_indexer::TranscriptIndexer::new(
            self.database.clone(),
            embedder,
        ));
        self
    }

    /// Attach an optional raw-memory writer for pre-compress hooks.
    pub fn with_raw_memory_writer(mut self, writer: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>) -> Self {
        self.raw_memory_writer = Some(writer);
        self
    }

    /// Attach a capture-filter registry.
    pub fn with_capture_registry(mut self, registry: Arc<crate::memory::extensions::MemoryExtensionRegistry>) -> Self {
        if let Some(indexer) = self.indexer.take() {
            self.indexer = Some(indexer.with_capture_registry(registry.clone()));
        }
        self.capture_registry = Some(registry);
        self
    }

    /// Return a reference to the compactor configuration.
    pub fn config(&self) -> &SessionCompactorConfig {
        &self.config
    }
}
