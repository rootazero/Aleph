//! Transcript Indexer Module
//!
//! Provides near-realtime indexing of conversation transcripts for vector search.

pub mod config;
pub mod indexer;

pub use config::TranscriptIndexerConfig;
pub use indexer::TranscriptIndexer;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
    use crate::memory::store::{MemoryBackend, SqliteMemoryBackend};
    use crate::memory::EmbeddingProvider;
    use crate::sync_primitives::Arc;
    use tempfile::tempdir;

    fn create_test_db(temp_dir: &std::path::Path) -> MemoryBackend {
        let db_path = temp_dir.join("sqlite_db");
        Arc::new(SqliteMemoryBackend::new(&db_path).unwrap())
    }

    // NOTE: test_index_turn_basic removed - requires StateDatabase-specific
    // insert_memory and search_memories. Will be restored in Phase 5.

    #[test]
    fn test_indexer_chunk_text() {
        // Test TranscriptIndexer's chunk_text method
        let config = TranscriptIndexerConfig {
            max_tokens_per_chunk: 50,
            overlap_tokens: 10,
            enable_chunking: true,
        };

        let temp_dir = tempdir().unwrap();
        let db = create_test_db(temp_dir.path());
        let embedder = {
            let mock: Arc<dyn EmbeddingProvider> =
                Arc::new(MockEmbeddingProvider::new(1024, "mock-model"));
            mock
        };

        let indexer = TranscriptIndexer::with_config(db, embedder, config);

        // Test short text
        let short_text = "This is short.";
        let chunks = indexer.chunk_text(short_text);
        assert_eq!(chunks.len(), 1);

        // Test long text
        let long_text = "This is a sentence. ".repeat(40);
        let chunks = indexer.chunk_text(&long_text);
        assert!(chunks.len() > 1);
    }

    #[test]
    fn test_indexer_estimate_tokens() {
        let temp_dir = tempdir().unwrap();
        let db = create_test_db(temp_dir.path());
        let embedder = {
            let mock: Arc<dyn EmbeddingProvider> =
                Arc::new(MockEmbeddingProvider::new(1024, "mock-model"));
            mock
        };

        let indexer = TranscriptIndexer::new(db, embedder);

        // Test token estimation
        let text = "1234"; // 4 chars = 1 token
        assert_eq!(indexer.estimate_tokens(text), 1);

        let text = "12345678"; // 8 chars = 2 tokens
        assert_eq!(indexer.estimate_tokens(text), 2);

        let text = "123456789"; // 9 chars = 3 tokens (rounded up)
        assert_eq!(indexer.estimate_tokens(text), 3);
    }

    #[test]
    fn test_chunk_short_text() {
        // Test that short text is not chunked
        let config = TranscriptIndexerConfig {
            max_tokens_per_chunk: 100,
            overlap_tokens: 20,
            enable_chunking: true,
        };

        let short_text = "This is a short text.";
        let chunks = chunk_text_helper(short_text, &config);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], short_text);
    }

    #[test]
    fn test_chunk_long_text() {
        // Test that long text is chunked
        let config = TranscriptIndexerConfig {
            max_tokens_per_chunk: 50, // Small for testing
            overlap_tokens: 10,
            enable_chunking: true,
        };

        // Create text with ~200 tokens (800 chars)
        let long_text = "This is a sentence. ".repeat(40);
        let chunks = chunk_text_helper(&long_text, &config);

        // Should have multiple chunks
        assert!(
            chunks.len() > 1,
            "Expected multiple chunks, got {}",
            chunks.len()
        );

        // Each chunk should be within token limit (with some margin)
        for chunk in &chunks {
            let tokens = estimate_tokens_helper(chunk);
            assert!(
                tokens <= config.max_tokens_per_chunk + 20,
                "Chunk too large: {} tokens",
                tokens
            );
        }
    }

    #[test]
    fn test_chunk_with_overlap() {
        let config = TranscriptIndexerConfig {
            max_tokens_per_chunk: 50,
            overlap_tokens: 10,
            enable_chunking: true,
        };

        let text =
            "First sentence. Second sentence. Third sentence. Fourth sentence. Fifth sentence.";
        let chunks = chunk_text_helper(text, &config);

        if chunks.len() > 1 {
            // Check that consecutive chunks have overlap
            for i in 0..chunks.len() - 1 {
                // Use char-based slicing to avoid panics on multi-byte chars.
                // (`.rev().take(40).rev()` does not compile — `Take<Rev<Chars>>`
                // is not a DoubleEndedIterator; collect and index the tail.)
                let current_end: String = {
                    let chars: Vec<char> = chunks[i].chars().collect();
                    chars[chars.len().saturating_sub(40)..].iter().collect()
                };
                let next_start: String = chunks[i + 1].chars().take(40).collect();

                // There should be some common text
                let has_overlap = current_end.chars().any(|c| next_start.contains(c));
                assert!(has_overlap, "No overlap between chunks {} and {}", i, i + 1);
            }
        }
    }

    #[test]
    fn test_chunking_disabled() {
        let config = TranscriptIndexerConfig {
            max_tokens_per_chunk: 50,
            overlap_tokens: 10,
            enable_chunking: false, // Disabled
        };

        let long_text = "word ".repeat(200);
        let chunks = chunk_text_helper(&long_text, &config);

        // Should return single chunk even if text is long
        assert_eq!(chunks.len(), 1);
    }

    // Helper functions for testing
    fn chunk_text_helper(text: &str, config: &TranscriptIndexerConfig) -> Vec<String> {
        if !config.enable_chunking {
            return vec![text.to_string()];
        }

        let tokens = estimate_tokens_helper(text);
        if tokens <= config.max_tokens_per_chunk {
            return vec![text.to_string()];
        }

        // Split by sentences
        let sentences: Vec<&str> = text.split('.').filter(|s| !s.trim().is_empty()).collect();
        let mut chunks = Vec::new();
        let mut current_chunk = String::new();
        let mut current_tokens = 0;

        for sentence in sentences {
            let sentence_tokens = estimate_tokens_helper(sentence);

            if current_tokens + sentence_tokens > config.max_tokens_per_chunk
                && !current_chunk.is_empty()
            {
                chunks.push(current_chunk.clone());

                // Add overlap from previous chunk (UTF-8 safe)
                let overlap_char_count = config.overlap_tokens * 4;
                let total_chars = current_chunk.chars().count();
                if total_chars > overlap_char_count {
                    let skip = total_chars - overlap_char_count;
                    current_chunk = current_chunk
                        .char_indices()
                        .nth(skip)
                        .map(|(pos, _)| current_chunk[pos..].to_string())
                        .unwrap_or_default();
                    current_tokens = estimate_tokens_helper(&current_chunk);
                } else {
                    current_chunk.clear();
                    current_tokens = 0;
                }
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

    fn estimate_tokens_helper(text: &str) -> usize {
        text.len().div_ceil(4) // 4 chars per token, round up
    }
}
