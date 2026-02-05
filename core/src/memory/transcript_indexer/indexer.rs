use crate::error::Result;
use crate::memory::database::VectorDatabase;
use crate::memory::smart_embedder::SmartEmbedder;
use std::sync::Arc;

use super::config::TranscriptIndexerConfig;

/// Near-realtime transcript indexer
pub struct TranscriptIndexer {
    database: Arc<VectorDatabase>,
    embedder: Arc<SmartEmbedder>,
    config: TranscriptIndexerConfig,
}

impl TranscriptIndexer {
    /// Create new indexer with default config
    pub fn new(
        database: Arc<VectorDatabase>,
        embedder: Arc<SmartEmbedder>,
    ) -> Self {
        Self {
            database,
            embedder,
            config: TranscriptIndexerConfig::default(),
        }
    }

    /// Create with custom config
    pub fn with_config(
        database: Arc<VectorDatabase>,
        embedder: Arc<SmartEmbedder>,
        config: TranscriptIndexerConfig,
    ) -> Self {
        Self {
            database,
            embedder,
            config,
        }
    }

    /// Index a single conversation turn
    ///
    /// This is called after a conversation turn completes.
    /// The memory entry should already be in the database.
    pub async fn index_turn(&self, _memory_id: &str) -> Result<()> {
        // Memory is already inserted by MemoryIngestion
        // This is a no-op for MVP since memories table already has embeddings
        // In future, this will handle chunking and additional indexing
        Ok(())
    }

    /// Index with chunking support (future enhancement)
    pub async fn index_with_chunking(&self, memory_id: &str) -> Result<Vec<String>> {
        // TODO: Implement sliding window chunking
        // For now, return single chunk ID
        Ok(vec![memory_id.to_string()])
    }
}
