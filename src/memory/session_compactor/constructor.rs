use std::collections::HashMap;

use super::{SessionCompactor, SessionCompactorConfig};
use crate::memory::store::MemoryBackend;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

impl SessionCompactor {
    /// Create a new `SessionCompactor` with the given storage backend and config.
    pub fn new(database: MemoryBackend, config: SessionCompactorConfig) -> Self {
        Self {
            database,
            provider: None,
            config,
            indexer: None,
            raw_memory_writer: None,
            capture_registry: None,
            project_scoped: false,
            compress_locks: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Enable per-project namespacing of post-turn session memory. When on,
    /// captures are written under the active project's composed agent id;
    /// off (default) keeps the base id — byte-identical to before.
    #[must_use]
    pub const fn with_project_scoping(mut self, enabled: bool) -> Self {
        self.project_scoped = enabled;
        self
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
    pub fn with_raw_memory_writer(
        mut self,
        writer: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
    ) -> Self {
        self.raw_memory_writer = Some(writer);
        self
    }

    /// Attach a capture-filter registry.
    pub fn with_capture_registry(
        mut self,
        registry: Arc<crate::memory::extensions::MemoryExtensionRegistry>,
    ) -> Self {
        if let Some(indexer) = self.indexer.take() {
            self.indexer = Some(indexer.with_capture_registry(registry.clone()));
        }
        self.capture_registry = Some(registry);
        self
    }

    /// Return a reference to the compactor configuration.
    #[must_use]
    pub const fn config(&self) -> &SessionCompactorConfig {
        &self.config
    }
}
