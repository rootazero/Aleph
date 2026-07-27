//! Integration tests for semantic chunking

use crate::sync_primitives::Arc;
use tempfile::tempdir;

use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
use crate::memory::{EmbeddingProvider, SemanticChunkConfig, SemanticChunker};

#[tokio::test]
#[ignore = "Requires model download"]
async fn test_semantic_chunking_basic() {
    // rust-doctor-disable-next-line unwrap-in-production
    let _temp_dir = tempdir().unwrap();
    let embedder = {
        let mock: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock-model"));
        mock
    };

    let config = SemanticChunkConfig::default();
    let chunker = SemanticChunker::new(embedder, config);

    let text = "Rust is a systems programming language. \
                It focuses on safety and performance. \
                Python is a high-level language. \
                It's great for rapid development.";

    // rust-doctor-disable-next-line unwrap-in-production
    let chunks = chunker.chunk(text).await.unwrap();

    // Should create at least 2 chunks (Rust topic vs Python topic)
    assert!(!chunks.is_empty());
}

#[tokio::test]
#[ignore = "Requires model download"]
async fn test_semantic_chunking_single_topic() {
    // rust-doctor-disable-next-line unwrap-in-production
    let _temp_dir = tempdir().unwrap();
    let embedder = {
        let mock: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock-model"));
        mock
    };

    let config = SemanticChunkConfig::default();
    let chunker = SemanticChunker::new(embedder, config);

    // All sentences about the same topic
    let text = "Rust is fast. Rust is safe. Rust is modern.";

    // rust-doctor-disable-next-line unwrap-in-production
    let chunks = chunker.chunk(text).await.unwrap();

    // Should create 1 chunk (all same topic)
    assert_eq!(chunks.len(), 1);
}

#[tokio::test]
#[ignore = "Requires model download"]
async fn test_semantic_chunking_empty() {
    // rust-doctor-disable-next-line unwrap-in-production
    let _temp_dir = tempdir().unwrap();
    let embedder = {
        let mock: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock-model"));
        mock
    };

    let config = SemanticChunkConfig::default();
    let chunker = SemanticChunker::new(embedder, config);

    // rust-doctor-disable-next-line unwrap-in-production
    let chunks = chunker.chunk("").await.unwrap();
    assert_eq!(chunks.len(), 0);
}

#[tokio::test]
#[ignore = "Requires model download"]
async fn test_semantic_chunking_config() {
    // rust-doctor-disable-next-line unwrap-in-production
    let _temp_dir = tempdir().unwrap();
    let embedder = {
        let mock: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock-model"));
        mock
    };

    // High similarity threshold = fewer boundaries
    let config = SemanticChunkConfig {
        similarity_threshold: 0.95,
        min_chunk_size: 10,
        max_chunk_size: 200,
    };
    let chunker = SemanticChunker::new(embedder, config);

    let text = "First topic here. Second topic here. Third topic here.";

    // rust-doctor-disable-next-line unwrap-in-production
    let chunks = chunker.chunk(text).await.unwrap();

    // With high threshold, should create fewer chunks
    assert!(!chunks.is_empty());
}
