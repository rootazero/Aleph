//! Semantic Cache Module
//!
//! Provides semantic similarity-based caching for AI responses.
//! Uses text embeddings to match similar prompts and return cached responses.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     SemanticCacheManager                        │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
//! │  │ Embedder     │  │ VectorStore  │  │   SimilarityMatcher    │ │
//! │  │ (provider)   │  │ (in-memory)  │  │   (cosine sim)         │ │
//! │  └──────────────┘  └──────────────┘  └────────────────────────┘ │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::dispatcher::model_router::{SemanticCacheManager, SemanticCacheConfig};
//!
//! let config = SemanticCacheConfig::default();
//! let cache = SemanticCacheManager::new(config)?;
//!
//! // Lookup
//! if let Some(hit) = cache.lookup("What is Rust?").await? {
//!     println!("Cache hit! Similarity: {:.2}", hit.similarity);
//!     return Ok(hit.response);
//! }
//!
//! // Store after API call
//! cache.store("What is Rust?", &response, "claude-sonnet", None).await?;
//! ```

mod embedder;
mod manager;
mod store;
mod types;
mod utils;

// Re-export all public types for backward compatibility
pub use embedder::{BridgeEmbedder, EmbeddingError, TextEmbedder};
pub use manager::SemanticCacheManager;
pub use store::InMemoryVectorStore;
pub use types::{
    CacheEntry, CacheHit, CacheHitType, CacheMetadata, CacheStats, CachedResponse, EvictionPolicy,
    SemanticCacheConfig, SemanticCacheError,
};
pub use utils::{cosine_similarity, hash_prompt, normalize_prompt, prompt_preview};

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Arc;
    use std::time::Duration;

    // Mock embedder for testing
    struct MockEmbedder {
        dimensions: usize,
    }

    impl MockEmbedder {
        fn new(dimensions: usize) -> Self {
            Self { dimensions }
        }
    }

    #[async_trait::async_trait]
    impl TextEmbedder for MockEmbedder {
        async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
            // Generate a deterministic embedding based on text hash
            let hash = hash_prompt(text);
            let seed: u64 = hash
                .bytes()
                .take(8)
                .fold(0u64, |acc, b| acc * 256 + b as u64);

            let mut embedding = Vec::with_capacity(self.dimensions);
            let mut x = seed;
            for _ in 0..self.dimensions {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                embedding.push((x as f32 / u64::MAX as f32) * 2.0 - 1.0);
            }

            // Normalize
            let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
            for v in &mut embedding {
                *v /= norm;
            }

            Ok(embedding)
        }

        async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
            let mut results = Vec::with_capacity(texts.len());
            for text in texts {
                results.push(self.embed(text).await?);
            }
            Ok(results)
        }

        fn dimensions(&self) -> usize {
            self.dimensions
        }

        fn model_name(&self) -> &str {
            "mock-embedder"
        }
    }

    fn create_test_cache() -> SemanticCacheManager {
        let config = SemanticCacheConfig {
            enabled: true,
            embedding_dimensions: 128,
            similarity_threshold: 0.8,
            max_entries: 100,
            default_ttl: Duration::from_secs(3600),
            min_response_length: 10, // Lower for testing
            ..Default::default()
        };

        let embedder = Arc::new(MockEmbedder::new(128));
        SemanticCacheManager::with_embedder(embedder, config)
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        let c = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &c).abs() < 0.001);

        let d = vec![-1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &d) + 1.0).abs() < 0.001);
    }

    #[test]
    fn test_hash_prompt() {
        let hash1 = hash_prompt("Hello World");
        let hash2 = hash_prompt("  hello   world  ");
        assert_eq!(hash1, hash2); // Normalized hashes should match

        let hash3 = hash_prompt("Different text");
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_normalize_prompt() {
        assert_eq!(normalize_prompt("  Hello   World  "), "hello world");
        assert_eq!(normalize_prompt("TEST"), "test");
    }

    #[test]
    fn test_prompt_preview() {
        let short = "Short text";
        assert_eq!(prompt_preview(short, 20), "Short text");

        let long = "This is a very long text that should be truncated";
        assert_eq!(prompt_preview(long, 20), "This is a very long ...");
    }

    #[tokio::test]
    async fn test_cache_store_and_lookup() {
        let cache = create_test_cache();

        let prompt = "What is Rust?";
        let response =
            CachedResponse::new("Rust is a programming language".to_string(), 50, 100, 0.001);

        // Store
        cache
            .store(prompt, &response, "test-model", None, None)
            .await
            .unwrap();

        // Lookup (exact match)
        let hit = cache.lookup(prompt).await.unwrap();
        assert!(hit.is_some());

        let hit = hit.unwrap();
        assert_eq!(hit.hit_type, CacheHitType::Exact);
        assert_eq!(hit.similarity, 1.0);
        assert_eq!(hit.response().content, "Rust is a programming language");
    }

    #[tokio::test]
    async fn test_cache_miss() {
        let cache = create_test_cache();

        let hit = cache.lookup("Non-existent prompt").await.unwrap();
        assert!(hit.is_none());
    }

    #[tokio::test]
    async fn test_cache_invalidate() {
        let cache = create_test_cache();

        let prompt = "Test prompt";
        let response = CachedResponse::new("Test response".to_string(), 10, 50, 0.001);

        cache
            .store(prompt, &response, "test-model", None, None)
            .await
            .unwrap();

        // Verify it exists
        let hit = cache.lookup(prompt).await.unwrap();
        assert!(hit.is_some());

        // Invalidate
        cache.invalidate(prompt).await.unwrap();

        // Verify it's gone
        let hit = cache.lookup(prompt).await.unwrap();
        assert!(hit.is_none());
    }

    #[tokio::test]
    async fn test_cache_clear() {
        let cache = create_test_cache();

        // Store multiple entries
        for i in 0..5 {
            let prompt = format!("Prompt {}", i);
            let response = CachedResponse::new(format!("Response {}", i), 10, 50, 0.001);
            cache
                .store(&prompt, &response, "test-model", None, None)
                .await
                .unwrap();
        }

        assert_eq!(cache.entry_count().await, 5);

        // Clear
        cache.clear().await.unwrap();

        assert_eq!(cache.entry_count().await, 0);
    }

    #[tokio::test]
    async fn test_cache_stats() {
        let cache = create_test_cache();

        let response = CachedResponse::new(
            "Response text that is long enough".to_string(),
            10,
            50,
            0.001,
        );
        cache
            .store("prompt1", &response, "model", None, None)
            .await
            .unwrap();
        cache
            .store("prompt2", &response, "model", None, None)
            .await
            .unwrap();

        // Lookup to generate hits/misses
        cache.lookup("prompt1").await.unwrap();
        cache.lookup("prompt1").await.unwrap();
        cache.lookup("nonexistent").await.unwrap();

        let stats = cache.stats().await;
        assert_eq!(stats.total_entries, 2);
        assert_eq!(stats.hit_count, 2);
        assert_eq!(stats.miss_count, 1);
    }

    #[tokio::test]
    async fn test_cache_disabled() {
        let config = SemanticCacheConfig {
            enabled: false,
            ..Default::default()
        };

        let embedder = Arc::new(MockEmbedder::new(128));
        let cache = SemanticCacheManager::with_embedder(embedder, config);

        let response = CachedResponse::new("Response".to_string(), 10, 50, 0.001);

        // Store should be a no-op when disabled
        cache
            .store("prompt", &response, "model", None, None)
            .await
            .unwrap();

        // Lookup should return None when disabled
        let hit = cache.lookup("prompt").await.unwrap();
        assert!(hit.is_none());
    }

    #[tokio::test]
    async fn test_cache_min_response_length() {
        let cache = create_test_cache();

        // Short response should not be cached
        let short_response = CachedResponse::new("Hi".to_string(), 1, 10, 0.001);
        cache
            .store("prompt", &short_response, "model", None, None)
            .await
            .unwrap();

        let hit = cache.lookup("prompt").await.unwrap();
        assert!(hit.is_none()); // Not cached due to min length

        // Longer response should be cached
        let long_response = CachedResponse::new(
            "This is a sufficiently long response that exceeds the minimum length requirement"
                .to_string(),
            20,
            50,
            0.001,
        );
        cache
            .store("prompt2", &long_response, "model", None, None)
            .await
            .unwrap();

        let hit = cache.lookup("prompt2").await.unwrap();
        assert!(hit.is_some());
    }

    #[test]
    fn test_cache_entry_expired() {
        use std::time::SystemTime;

        let entry = CacheEntry {
            id: "test".to_string(),
            prompt_hash: "hash".to_string(),
            prompt_preview: "preview".to_string(),
            embedding: vec![0.0; 10],
            response: CachedResponse::new("response".to_string(), 10, 50, 0.001),
            model_used: "model".to_string(),
            created_at: SystemTime::now() - Duration::from_secs(3600),
            expires_at: Some(SystemTime::now() - Duration::from_secs(1)), // Expired
            hit_count: 0,
            last_accessed: SystemTime::now(),
            metadata: CacheMetadata::default(),
        };

        assert!(entry.is_expired());

        let entry_not_expired = CacheEntry {
            expires_at: Some(SystemTime::now() + Duration::from_secs(3600)),
            ..entry.clone()
        };

        assert!(!entry_not_expired.is_expired());

        let entry_no_expiry = CacheEntry {
            expires_at: None,
            ..entry
        };

        assert!(!entry_no_expiry.is_expired());
    }

    #[test]
    fn test_eviction_score() {
        use std::time::SystemTime;

        let entry = CacheEntry {
            id: "test".to_string(),
            prompt_hash: "hash".to_string(),
            prompt_preview: "preview".to_string(),
            embedding: vec![0.0; 10],
            response: CachedResponse::new("response".to_string(), 10, 50, 0.001),
            model_used: "model".to_string(),
            created_at: SystemTime::now(),
            expires_at: None,
            hit_count: 10,
            last_accessed: SystemTime::now(),
            metadata: CacheMetadata::default(),
        };

        let score = entry.eviction_score(0.4, 0.6);
        assert!(score > 0.0);

        // Higher hit count should give higher score
        let entry_high_hits = CacheEntry {
            hit_count: 100,
            ..entry
        };
        assert!(entry_high_hits.eviction_score(0.4, 0.6) > score);
    }
}
