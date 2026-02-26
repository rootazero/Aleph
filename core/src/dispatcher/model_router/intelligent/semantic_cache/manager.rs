//! Semantic cache manager
//!
//! Main entry point for semantic caching operations.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::embedder::{BridgeEmbedder, TextEmbedder};
use super::store::InMemoryVectorStore;
use super::types::{
    CacheEntry, CacheHit, CacheHitType, CacheMetadata, CacheStats, CachedResponse,
    SemanticCacheConfig, SemanticCacheError,
};
use super::utils::{hash_prompt, prompt_preview};
use crate::memory::EmbeddingProvider;

// =============================================================================
// Semantic Cache Manager
// =============================================================================

/// Main semantic cache manager
pub struct SemanticCacheManager {
    /// Text embedder for generating embeddings
    embedder: Arc<dyn TextEmbedder>,

    /// Vector store for cache entries
    store: Arc<InMemoryVectorStore>,

    /// Configuration
    config: SemanticCacheConfig,
}

impl SemanticCacheManager {
    /// Create a new semantic cache manager with an embedding provider
    pub fn new(
        embedding_provider: Arc<dyn EmbeddingProvider>,
        config: SemanticCacheConfig,
    ) -> Self {
        let embedder = BridgeEmbedder::new(embedding_provider);
        let store = InMemoryVectorStore::new(config.clone());

        Self {
            embedder: Arc::new(embedder),
            store: Arc::new(store),
            config,
        }
    }

    /// Create with a custom embedder (for testing)
    pub fn with_embedder(embedder: Arc<dyn TextEmbedder>, config: SemanticCacheConfig) -> Self {
        let store = InMemoryVectorStore::new(config.clone());

        Self {
            embedder,
            store: Arc::new(store),
            config,
        }
    }

    /// Check if caching is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Lookup a prompt in the cache
    pub async fn lookup(&self, prompt: &str) -> Result<Option<CacheHit>, SemanticCacheError> {
        if !self.config.enabled {
            return Ok(None);
        }

        // Try exact match first if enabled
        if self.config.exact_match_priority {
            let prompt_hash = hash_prompt(prompt);
            if let Some(mut entry) = self.store.lookup_exact(&prompt_hash).await {
                // Record hit
                self.store.record_hit(&entry.id).await;
                entry.hit_count += 1;
                entry.last_accessed = SystemTime::now();

                // Update stats
                {
                    let mut stats = self.store.stats.write().await;
                    stats.hit_count += 1;
                    stats.exact_hits += 1;
                }

                return Ok(Some(CacheHit {
                    entry,
                    similarity: 1.0,
                    hit_type: CacheHitType::Exact,
                }));
            }
        }

        // Generate embedding for semantic search
        let embedding = self
            .embedder
            .embed(prompt)
            .await
            .map_err(|e| SemanticCacheError::EmbeddingFailed(e.to_string()))?;

        // Search for similar entries
        if let Some((mut entry, similarity)) = self
            .store
            .search_similar(&embedding, self.config.similarity_threshold)
            .await
        {
            // Record hit
            self.store.record_hit(&entry.id).await;
            entry.hit_count += 1;
            entry.last_accessed = SystemTime::now();

            // Update stats
            {
                let mut stats = self.store.stats.write().await;
                stats.hit_count += 1;
                stats.semantic_hits += 1;
                // Update average similarity (running average)
                let total_semantic = stats.semantic_hits as f64;
                stats.avg_similarity =
                    (stats.avg_similarity * (total_semantic - 1.0) + similarity) / total_semantic;
            }

            return Ok(Some(CacheHit {
                entry,
                similarity,
                hit_type: CacheHitType::Semantic,
            }));
        }

        // Cache miss
        {
            let mut stats = self.store.stats.write().await;
            stats.miss_count += 1;
        }

        Ok(None)
    }

    /// Store a response in the cache
    pub async fn store(
        &self,
        prompt: &str,
        response: &CachedResponse,
        model: &str,
        ttl: Option<Duration>,
        metadata: Option<CacheMetadata>,
    ) -> Result<(), SemanticCacheError> {
        if !self.config.enabled {
            return Ok(());
        }

        // Check exclusions
        if self.config.exclude_models.contains(&model.to_string()) {
            return Ok(());
        }

        if let Some(ref meta) = metadata {
            if let Some(ref intent) = meta.task_intent {
                if self.config.exclude_intents.contains(intent) {
                    return Ok(());
                }
            }
        }

        // Check minimum response length
        if response.content.len() < self.config.min_response_length {
            return Ok(());
        }

        // Generate embedding
        let embedding = self
            .embedder
            .embed(prompt)
            .await
            .map_err(|e| SemanticCacheError::EmbeddingFailed(e.to_string()))?;

        // Calculate TTL
        let effective_ttl = ttl
            .map(|t| t.min(self.config.max_ttl))
            .unwrap_or(self.config.default_ttl);

        let now = SystemTime::now();

        // Create entry
        let entry = CacheEntry {
            id: uuid::Uuid::new_v4().to_string(),
            prompt_hash: hash_prompt(prompt),
            prompt_preview: prompt_preview(prompt, 100),
            embedding,
            response: response.clone(),
            model_used: model.to_string(),
            created_at: now,
            expires_at: Some(now + effective_ttl),
            hit_count: 0,
            last_accessed: now,
            metadata: metadata.unwrap_or_default(),
        };

        // Store
        self.store.insert(entry).await
    }

    /// Invalidate a specific prompt from cache
    pub async fn invalidate(&self, prompt: &str) -> Result<(), SemanticCacheError> {
        let prompt_hash = hash_prompt(prompt);

        if let Some(entry) = self.store.lookup_exact(&prompt_hash).await {
            self.store.delete(&entry.id).await?;
        }

        Ok(())
    }

    /// Clear all cache entries
    pub async fn clear(&self) -> Result<(), SemanticCacheError> {
        self.store.clear().await
    }

    /// Get cache statistics
    pub async fn stats(&self) -> CacheStats {
        self.store.get_stats().await
    }

    /// Get the number of cached entries
    pub async fn entry_count(&self) -> usize {
        self.store.count().await
    }

    /// Manually trigger eviction
    pub async fn evict(&self, count: usize) -> Result<usize, SemanticCacheError> {
        self.store.evict_by_policy(count).await
    }
}
