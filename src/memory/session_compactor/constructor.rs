use std::collections::HashMap;

use super::{SessionCompactor, SessionCompactorConfig};
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

impl SessionCompactor {
    /// Create a new `SessionCompactor` with the given storage backend and config.
    pub fn new(database: Arc<dyn RawMemoryStore>, config: SessionCompactorConfig) -> Self {
        Self {
            raw_memory_writer: Some(database.clone()),
            database,
            provider: None,
            config,
            indexer: None,
            capture_registry: None,
            memory_runtime: None,
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

    /// Enable transcript indexing: post-turn raw text is chunked and stored
    /// as `RawMemorySource::Transcript` rows for substring recall and later
    /// compression. Needs no embedding provider — the indexer produces no
    /// vectors.
    #[must_use]
    pub fn with_transcript_indexing(mut self) -> Self {
        self.indexer = Some(self.transcript_indexer(self.database.clone()));
        self
    }

    /// Attach an optional raw-memory writer for pre-compress hooks.
    pub fn with_raw_memory_writer(
        mut self,
        writer: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
    ) -> Self {
        self.database = writer.clone();
        if self.indexer.is_some() {
            self.indexer = Some(self.transcript_indexer(writer.clone()));
        }
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

    /// Keep raw compaction available while admitting derived callbacks separately.
    pub fn with_memory_runtime(
        mut self,
        runtime: std::sync::Weak<crate::memory::runtime::MemoryRuntime>,
    ) -> Self {
        if let Some(indexer) = self.indexer.take() {
            self.indexer = Some(indexer.with_memory_runtime(runtime.clone()));
        }
        self.memory_runtime = Some(runtime);
        self
    }

    fn transcript_indexer(
        &self,
        writer: Arc<dyn RawMemoryStore>,
    ) -> crate::memory::transcript_indexer::TranscriptIndexer {
        let indexer = crate::memory::transcript_indexer::TranscriptIndexer::new(writer);
        let indexer = match &self.capture_registry {
            Some(registry) => indexer.with_capture_registry(registry.clone()),
            None => indexer,
        };
        match &self.memory_runtime {
            Some(runtime) => indexer.with_memory_runtime(runtime.clone()),
            None => indexer,
        }
    }

    /// Return a reference to the compactor configuration.
    #[must_use]
    pub const fn config(&self) -> &SessionCompactorConfig {
        &self.config
    }
}
