/// Memory retrieval module
///
/// This module previously handled retrieval of raw memories (Layer 1)
/// via SessionStore. Raw memory retrieval has been removed — facts
/// (Layer 2) are now the primary retrieval target via FactRetrieval.
///
/// The MemoryRetrieval struct is retained as a stub returning empty
/// results so that existing callers continue to compile.
use crate::config::MemoryConfig;
use crate::error::AlephError;
use crate::memory::context::{ContextAnchor, MemoryEntry};
use crate::memory::dreaming::record_activity;
use crate::memory::store::MemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;
use tracing::debug;

/// Memory retrieval service (stub — raw memory search removed)
#[derive(Clone)]
pub struct MemoryRetrieval {
    _database: MemoryBackend,
    _embedder: Arc<dyn EmbeddingProvider>,
    config: Arc<MemoryConfig>,
}

impl MemoryRetrieval {
    /// Create new retrieval service
    pub fn new(
        database: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        config: Arc<MemoryConfig>,
    ) -> Self {
        Self {
            _database: database,
            _embedder: embedder,
            config,
        }
    }

    /// Retrieve memories for current context (returns empty — use FactRetrieval instead)
    pub async fn retrieve_memories(
        &self,
        _context: &ContextAnchor,
        _query: &str,
    ) -> Result<Vec<MemoryEntry>, AlephError> {
        record_activity();

        if !self.config.enabled {
            debug!("Memory retrieval skipped: memory disabled");
        }

        // Raw memory search removed — facts are the primary retrieval target.
        Ok(Vec::new())
    }

    /// Retrieve memories with custom limit (returns empty — use FactRetrieval instead)
    pub async fn retrieve_memories_with_limit(
        &self,
        _context: &ContextAnchor,
        _query: &str,
        _limit: usize,
    ) -> Result<Vec<MemoryEntry>, AlephError> {
        record_activity();

        if !self.config.enabled {
            debug!("Memory retrieval skipped: memory disabled");
        }

        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::SqliteMemoryBackend;
    use uuid::Uuid;

    fn create_test_db() -> MemoryBackend {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_retrieval_{}", Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&db_path).unwrap())
    }

    fn create_test_model() -> Arc<dyn EmbeddingProvider> {
        use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
        Arc::new(MockEmbeddingProvider::new(1024, "mock-model"))
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_retrieve_when_disabled() {
        let db = create_test_db();
        let model = create_test_model();
        let config = MemoryConfig {
            enabled: false,
            ..MemoryConfig::default()
        };
        let config = Arc::new(config);

        let retrieval = MemoryRetrieval::new(db, model, config);

        let context = ContextAnchor::now("Test.txt".to_string());
        let memories = retrieval
            .retrieve_memories(&context, "any query")
            .await
            .unwrap();

        assert!(memories.is_empty());
    }
}
