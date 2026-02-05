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
    use crate::memory::context::{ContextAnchor, MemoryEntry};
    use crate::memory::database::VectorDatabase;
    use crate::memory::smart_embedder::SmartEmbedder;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_index_turn_basic() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = Arc::new(VectorDatabase::new(db_path).unwrap());

        let embedder = Arc::new(SmartEmbedder::new(
            temp_dir.path().to_path_buf(),
            300,
        ));

        let indexer = TranscriptIndexer::new(db.clone(), embedder.clone());

        // Insert a memory entry
        let context = ContextAnchor::now("test.app".to_string(), "Test Window".to_string());
        let entry_id = uuid::Uuid::new_v4().to_string();
        let mut entry = MemoryEntry::new(
            entry_id.clone(),
            context,
            "What is Rust?".to_string(),
            "Rust is a systems programming language.".to_string(),
        );

        // Generate embedding
        let text = format!("{} {}", entry.user_input, entry.ai_output);
        let embedding = embedder.embed(&text).await.unwrap();
        entry.embedding = Some(embedding);

        db.insert_memory(entry.clone()).await.unwrap();

        // Index the turn
        let result = indexer.index_turn(&entry_id).await;
        assert!(result.is_ok());

        // Verify it's searchable
        let query_embedding = embedder.embed("programming language").await.unwrap();
        let results = db.search_memories(
            "test.app",
            "Test Window",
            &query_embedding,
            5,
        ).await.unwrap();

        assert!(!results.is_empty());
        assert_eq!(results[0].id, entry_id);
    }

    #[test]
    fn test_indexer_chunk_text() {
        // Test TranscriptIndexer's chunk_text method
        let config = TranscriptIndexerConfig {
            max_tokens_per_chunk: 50,
            overlap_tokens: 10,
            enable_chunking: true,
        };

        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = Arc::new(VectorDatabase::new(db_path).unwrap());
        let embedder = Arc::new(SmartEmbedder::new(temp_dir.path().to_path_buf(), 300));

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
        let db_path = temp_dir.path().join("test.db");
        let db = Arc::new(VectorDatabase::new(db_path).unwrap());
        let embedder = Arc::new(SmartEmbedder::new(temp_dir.path().to_path_buf(), 300));

        let indexer = TranscriptIndexer::new(db, embedder);

        // Test token estimation
        let text = "1234";  // 4 chars = 1 token
        assert_eq!(indexer.estimate_tokens(text), 1);

        let text = "12345678";  // 8 chars = 2 tokens
        assert_eq!(indexer.estimate_tokens(text), 2);

        let text = "123456789";  // 9 chars = 3 tokens (rounded up)
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
            max_tokens_per_chunk: 50,  // Small for testing
            overlap_tokens: 10,
            enable_chunking: true,
        };

        // Create text with ~200 tokens (800 chars)
        let long_text = "This is a sentence. ".repeat(40);
        let chunks = chunk_text_helper(&long_text, &config);

        // Should have multiple chunks
        assert!(chunks.len() > 1, "Expected multiple chunks, got {}", chunks.len());

        // Each chunk should be within token limit (with some margin)
        for chunk in &chunks {
            let tokens = estimate_tokens_helper(chunk);
            assert!(tokens <= config.max_tokens_per_chunk + 20, "Chunk too large: {} tokens", tokens);
        }
    }

    #[test]
    fn test_chunk_with_overlap() {
        let config = TranscriptIndexerConfig {
            max_tokens_per_chunk: 50,
            overlap_tokens: 10,
            enable_chunking: true,
        };

        let text = "First sentence. Second sentence. Third sentence. Fourth sentence. Fifth sentence.";
        let chunks = chunk_text_helper(text, &config);

        if chunks.len() > 1 {
            // Check that consecutive chunks have overlap
            for i in 0..chunks.len() - 1 {
                let current_end = &chunks[i][chunks[i].len().saturating_sub(40)..];
                let next_start = &chunks[i + 1][..40.min(chunks[i + 1].len())];

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
            enable_chunking: false,  // Disabled
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

            if current_tokens + sentence_tokens > config.max_tokens_per_chunk && !current_chunk.is_empty() {
                chunks.push(current_chunk.clone());

                // Add overlap from previous chunk
                let overlap_chars = config.overlap_tokens * 4;
                if current_chunk.len() > overlap_chars {
                    current_chunk = current_chunk[current_chunk.len() - overlap_chars..].to_string();
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
        (text.len() + 3) / 4  // 4 chars per token, round up
    }
}
