//! In-memory vector store for semantic cache
//!
//! Provides storage, retrieval, and eviction of cache entries.

use std::collections::HashMap;
use std::time::SystemTime;
use tokio::sync::RwLock;

use super::types::{CacheEntry, CacheStats, EvictionPolicy, SemanticCacheConfig, SemanticCacheError};
use super::utils::cosine_similarity;

// =============================================================================
// In-Memory Vector Store
// =============================================================================

/// In-memory vector store for cache entries
pub struct InMemoryVectorStore {
    /// Entries indexed by ID
    entries: RwLock<HashMap<String, CacheEntry>>,

    /// Hash index for exact matching
    hash_index: RwLock<HashMap<String, String>>, // prompt_hash -> entry_id

    /// Statistics
    pub(crate) stats: RwLock<CacheStats>,

    /// Configuration
    config: SemanticCacheConfig,
}

impl InMemoryVectorStore {
    /// Create a new in-memory vector store
    pub fn new(config: SemanticCacheConfig) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            hash_index: RwLock::new(HashMap::new()),
            stats: RwLock::new(CacheStats::default()),
            config,
        }
    }

    /// Insert a cache entry
    pub async fn insert(&self, entry: CacheEntry) -> Result<(), SemanticCacheError> {
        let mut entries = self.entries.write().await;
        let mut hash_index = self.hash_index.write().await;

        // Check capacity
        if entries.len() >= self.config.max_entries {
            // Need to evict
            drop(entries);
            drop(hash_index);
            self.evict_by_policy(self.config.max_entries / 10).await?;
            entries = self.entries.write().await;
            hash_index = self.hash_index.write().await;
        }

        // Add to hash index
        hash_index.insert(entry.prompt_hash.clone(), entry.id.clone());

        // Add entry
        entries.insert(entry.id.clone(), entry);

        Ok(())
    }

    /// Lookup by exact hash match
    pub async fn lookup_exact(&self, prompt_hash: &str) -> Option<CacheEntry> {
        let hash_index = self.hash_index.read().await;
        let entry_id = hash_index.get(prompt_hash)?;

        let entries = self.entries.read().await;
        let entry = entries.get(entry_id)?;

        if entry.is_expired() {
            return None;
        }

        Some(entry.clone())
    }

    /// Search by semantic similarity
    pub async fn search_similar(
        &self,
        embedding: &[f32],
        threshold: f64,
    ) -> Option<(CacheEntry, f64)> {
        let entries = self.entries.read().await;

        let mut best_match: Option<(CacheEntry, f64)> = None;
        let mut best_similarity = threshold;

        for entry in entries.values() {
            if entry.is_expired() {
                continue;
            }

            let similarity = cosine_similarity(embedding, &entry.embedding);
            if similarity > best_similarity {
                best_similarity = similarity;
                best_match = Some((entry.clone(), similarity));
            }
        }

        best_match
    }

    /// Update hit count and last accessed time
    pub async fn record_hit(&self, entry_id: &str) {
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.get_mut(entry_id) {
            entry.hit_count += 1;
            entry.last_accessed = SystemTime::now();
        }
    }

    /// Delete an entry by ID
    pub async fn delete(&self, entry_id: &str) -> Result<(), SemanticCacheError> {
        let mut entries = self.entries.write().await;
        let mut hash_index = self.hash_index.write().await;

        if let Some(entry) = entries.remove(entry_id) {
            hash_index.remove(&entry.prompt_hash);
        }

        Ok(())
    }

    /// Clear all entries
    pub async fn clear(&self) -> Result<(), SemanticCacheError> {
        let mut entries = self.entries.write().await;
        let mut hash_index = self.hash_index.write().await;

        entries.clear();
        hash_index.clear();

        Ok(())
    }

    /// Get entry count
    pub async fn count(&self) -> usize {
        self.entries.read().await.len()
    }

    /// Evict expired entries
    pub async fn evict_expired(&self) -> Result<usize, SemanticCacheError> {
        let mut entries = self.entries.write().await;
        let mut hash_index = self.hash_index.write().await;
        let mut stats = self.stats.write().await;

        let expired_ids: Vec<String> = entries
            .iter()
            .filter(|(_, e)| e.is_expired())
            .map(|(id, _)| id.clone())
            .collect();

        let count = expired_ids.len();

        for id in &expired_ids {
            if let Some(entry) = entries.remove(id) {
                hash_index.remove(&entry.prompt_hash);
            }
        }

        stats.evictions += count as u64;

        Ok(count)
    }

    /// Evict entries by policy
    pub async fn evict_by_policy(&self, target_count: usize) -> Result<usize, SemanticCacheError> {
        // First evict expired
        let expired_count = self.evict_expired().await?;
        if expired_count >= target_count {
            return Ok(expired_count);
        }

        let remaining = target_count - expired_count;

        let mut entries = self.entries.write().await;
        let mut hash_index = self.hash_index.write().await;
        let mut stats = self.stats.write().await;

        // Score all entries based on policy
        let mut scored: Vec<(String, f64)> = match &self.config.eviction_policy {
            EvictionPolicy::Lru => entries
                .iter()
                .map(|(id, e)| {
                    let time_since_access = SystemTime::now()
                        .duration_since(e.last_accessed)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    (id.clone(), time_since_access as f64)
                })
                .collect(),
            EvictionPolicy::Lfu => entries
                .iter()
                .map(|(id, e)| (id.clone(), -(e.hit_count as f64)))
                .collect(),
            EvictionPolicy::Hybrid {
                age_weight,
                hits_weight,
            } => entries
                .iter()
                .map(|(id, e)| (id.clone(), -e.eviction_score(*age_weight, *hits_weight)))
                .collect(),
        };

        // Sort by score (lower = evict first for LRU, higher for LFU)
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Evict bottom entries
        let to_evict: Vec<String> = scored
            .into_iter()
            .take(remaining)
            .map(|(id, _)| id)
            .collect();

        for id in &to_evict {
            if let Some(entry) = entries.remove(id) {
                hash_index.remove(&entry.prompt_hash);
            }
        }

        stats.evictions += to_evict.len() as u64;

        Ok(expired_count + to_evict.len())
    }

    /// Get cache statistics
    pub async fn get_stats(&self) -> CacheStats {
        let entries = self.entries.read().await;
        let mut stats = self.stats.write().await;

        stats.total_entries = entries.len();
        stats.expired_entries = entries.values().filter(|e| e.is_expired()).count();

        // Calculate oldest entry age
        stats.oldest_entry_age_secs = entries.values().map(|e| e.age_secs()).max().unwrap_or(0);

        // Approximate size (rough estimate)
        stats.total_size_bytes = entries
            .values()
            .map(|e| {
                e.embedding.len() * 4 // f32 = 4 bytes
                    + e.response.content.len()
                    + e.prompt_preview.len()
                    + e.prompt_hash.len()
                    + e.model_used.len()
                    + 200 // overhead estimate
            })
            .sum();

        stats.update_hit_rate();

        stats.clone()
    }
}
