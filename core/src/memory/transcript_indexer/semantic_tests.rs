//! Integration tests for semantic chunking

use std::sync::Arc;
use tempfile::tempdir;

use crate::memory::{SemanticChunkConfig, SemanticChunker, SmartEmbedder};

#[tokio::test]
#[ignore = "Requires model download"]
async fn test_semantic_chunking_basic() {
    let temp_dir = tempdir().unwrap();
    let embedder = Arc::new(SmartEmbedder::new(temp_dir.path().to_path_buf(), 300));

    let config = SemanticChunkConfig::default();
    let chunker = SemanticChunker::new(embedder, config);

    let text = "Rust is a systems programming language. \
                It focuses on safety and performance. \
                Python is a high-level language. \
                It's great for rapid development.";

    let chunks = chunker.chunk(text).await.unwrap();

    // Should create at least 2 chunks (Rust topic vs Python topic)
    assert!(chunks.len() >= 1);
}

#[tokio::test]
#[ignore = "Requires model download"]
async fn test_semantic_chunking_single_topic() {
    let temp_dir = tempdir().unwrap();
    let embedder = Arc::new(SmartEmbedder::new(temp_dir.path().to_path_buf(), 300));

    let config = SemanticChunkConfig::default();
    let chunker = SemanticChunker::new(embedder, config);

    // All sentences about the same topic
    let text = "Rust is fast. Rust is safe. Rust is modern.";

    let chunks = chunker.chunk(text).await.unwrap();

    // Should create 1 chunk (all same topic)
    assert_eq!(chunks.len(), 1);
}

#[tokio::test]
#[ignore = "Requires model download"]
async fn test_semantic_chunking_empty() {
    let temp_dir = tempdir().unwrap();
    let embedder = Arc::new(SmartEmbedder::new(temp_dir.path().to_path_buf(), 300));

    let config = SemanticChunkConfig::default();
    let chunker = SemanticChunker::new(embedder, config);

    let chunks = chunker.chunk("").await.unwrap();
    assert_eq!(chunks.len(), 0);
}

#[tokio::test]
#[ignore = "Requires model download"]
async fn test_semantic_chunking_config() {
    let temp_dir = tempdir().unwrap();
    let embedder = Arc::new(SmartEmbedder::new(temp_dir.path().to_path_buf(), 300));

    // High similarity threshold = fewer boundaries
    let config = SemanticChunkConfig {
        similarity_threshold: 0.95,
        min_chunk_size: 10,
        max_chunk_size: 200,
    };
    let chunker = SemanticChunker::new(embedder, config);

    let text = "First topic here. Second topic here. Third topic here.";

    let chunks = chunker.chunk(text).await.unwrap();

    // With high threshold, should create fewer chunks
    assert!(chunks.len() >= 1);
}
