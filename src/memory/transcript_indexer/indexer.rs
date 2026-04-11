use crate::error::Result;
use crate::memory::context::{FactSource, NoteType, MemoryFact, MemoryTier};
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;

use super::config::TranscriptIndexerConfig;

/// Near-realtime transcript indexer
pub struct TranscriptIndexer {
    database: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
    config: TranscriptIndexerConfig,
}

impl TranscriptIndexer {
    /// Create new indexer with default config
    pub fn new(database: MemoryBackend, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            database,
            embedder,
            config: TranscriptIndexerConfig::default(),
        }
    }

    /// Create with custom config
    pub fn with_config(
        database: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        config: TranscriptIndexerConfig,
    ) -> Self {
        Self {
            database,
            embedder,
            config,
        }
    }

    /// Index a single conversation turn's text into memory facts
    ///
    /// Combines user input and AI output, chunks if necessary, embeds each
    /// chunk, and stores as `NoteType::Transcript` facts. Returns the IDs of
    /// all successfully created facts. Never fails — partial failures are
    /// logged and skipped.
    pub async fn index_turn_text(
        &self,
        session_key: &str,
        seq: u32,
        user_input: &str,
        ai_output: &str,
        namespace: &str,
        agent: &str,
    ) -> Vec<String> {
        // Build combined text
        let combined = if ai_output.trim().is_empty() {
            user_input.to_string()
        } else {
            format!("[user]: {}\n\n[assistant]: {}", user_input, ai_output)
        };

        if combined.trim().is_empty() {
            return Vec::new();
        }

        // Chunk if exceeding ~800 estimated tokens
        let chunks = if self.estimate_tokens(&combined) > 800 {
            self.chunk_text(&combined)
        } else {
            vec![combined]
        };

        let multi_chunk = chunks.len() > 1;
        let mut created_ids = Vec::with_capacity(chunks.len());

        for (i, chunk) in chunks.iter().enumerate() {
            // Embed
            let embedding = match self.embedder.embed(chunk).await {
                Ok(emb) => emb,
                Err(e) => {
                    tracing::warn!(
                        session_key,
                        seq,
                        chunk_idx = i,
                        error = %e,
                        "transcript indexer: embed failed, skipping chunk"
                    );
                    continue;
                }
            };

            // Build fact
            let path = if multi_chunk {
                format!("aleph://transcript/{session_key}/{seq}_chunk{i}")
            } else {
                format!("aleph://transcript/{session_key}/{seq}")
            };
            let parent_path = format!("aleph://transcript/{session_key}");

            let mut fact =
                MemoryFact::new(chunk.clone(), NoteType::Transcript, vec![]);
            fact.path = path;
            fact.parent_path = parent_path;
            fact.embedding = Some(embedding);
            fact.embedding_model = self.embedder.model_name().to_string();
            fact.fact_source = FactSource::Extracted;
            fact.tier = MemoryTier::ShortTerm;
            fact.namespace = namespace.to_string();
            fact.agent = agent.to_string();

            let fact_id = fact.id.clone();

            // Insert
            if let Err(e) = self.database.insert_fact(&fact).await {
                tracing::warn!(
                    session_key,
                    seq,
                    chunk_idx = i,
                    error = %e,
                    "transcript indexer: insert_fact failed, skipping chunk"
                );
                continue;
            }

            created_ids.push(fact_id);
        }

        created_ids
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

            if current_tokens + sentence_tokens > self.config.max_tokens_per_chunk
                && !current_chunk.is_empty()
            {
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
        text.len().div_ceil(4) // 4 chars per token, round up
    }

    /// Get overlap text from end of chunk (UTF-8 safe)
    fn get_overlap_text(&self, text: &str) -> String {
        let overlap_chars = self.config.overlap_tokens * 4;
        let char_count = text.chars().count();
        if char_count <= overlap_chars {
            return text.to_string();
        }
        let skip = char_count - overlap_chars;
        text.char_indices()
            .nth(skip)
            .map(|(byte_pos, _)| text[byte_pos..].to_string())
            .unwrap_or_else(|| text.to_string())
    }
}
