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

    /// Chunk text into overlapping segments
    pub fn chunk_text(&self, text: &str) -> Vec<String> {
        if !self.config.enable_chunking {
            return vec![text.to_string()];
        }

        let tokens = self.estimate_tokens(text);
        if tokens <= self.config.max_tokens_per_chunk {
            return vec![text.to_string()];
        }

        // Split by sentences
        let sentences: Vec<&str> = text.split('.').filter(|s| !s.trim().is_empty()).collect();
        let mut chunks = Vec::new();
        let mut current_chunk = String::new();
        let mut current_tokens = 0;

        for sentence in sentences {
            let sentence_tokens = self.estimate_tokens(sentence);

            if current_tokens + sentence_tokens > self.config.max_tokens_per_chunk && !current_chunk.is_empty() {
                chunks.push(current_chunk.clone());

                // Add overlap from previous chunk
                let overlap_text = self.get_overlap_text(&current_chunk);
                current_chunk = overlap_text;
                current_tokens = self.estimate_tokens(&current_chunk);
            }

            if !current_chunk.is_empty() && !current_chunk.ends_with(' ') {
                current_chunk.push(' ');
            }
            current_chunk.push_str(sentence.trim());
            current_chunk.push('.');
            current_tokens += sentence_tokens;
        }

        if !current_chunk.is_empty() {
            chunks.push(current_chunk);
        }

        if chunks.is_empty() {
            chunks.push(text.to_string());
        }

        chunks
    }

    /// Estimate token count for text
    pub fn estimate_tokens(&self, text: &str) -> usize {
        (text.len() + 3) / 4  // 4 chars per token, round up
    }

    /// Get overlap text from end of chunk
    fn get_overlap_text(&self, text: &str) -> String {
        let overlap_chars = self.config.overlap_tokens * 4;
        if text.len() <= overlap_chars {
            return text.to_string();
        }
        text[text.len() - overlap_chars..].to_string()
    }
}
