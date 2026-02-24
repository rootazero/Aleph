//! LRU embedding cache with per-entry TTL
//!
//! Caches embedding vectors keyed by (task_prefix, text) to avoid
//! redundant model invocations. Each entry expires after a configurable
//! TTL, and the cache size is bounded via LRU eviction.

use lru::LruCache;
use sha2::{Digest, Sha256};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// An LRU embedding cache with per-entry TTL.
///
/// Entries are keyed by a SHA-256 hash of `"prefix:text"`. When an entry
/// is older than `ttl`, it is treated as a miss and removed on access.
pub struct EmbeddingCache {
    entries: Mutex<LruCache<String, CacheEntry>>,
    ttl: Duration,
    hits: AtomicU64,
    misses: AtomicU64,
}

struct CacheEntry {
    vector: Vec<f32>,
    created_at: Instant,
}

impl EmbeddingCache {
    /// Create a new cache with the given maximum size and entry TTL.
    pub fn new(max_size: usize, ttl: Duration) -> Self {
        let cap = NonZeroUsize::new(max_size).expect("max_size must be > 0");
        Self {
            entries: Mutex::new(LruCache::new(cap)),
            ttl,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Compute a deterministic cache key from task prefix and text.
    ///
    /// Returns the first 24 hex characters of `SHA-256("prefix:text")`.
    pub fn cache_key(task_prefix: &str, text: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(format!("{}:{}", task_prefix, text).as_bytes());
        let hash = hasher.finalize();
        hex::encode(hash)[..24].to_string()
    }

    /// Look up a cached embedding vector.
    ///
    /// Returns `None` on cache miss or if the entry has expired. Updates
    /// hit/miss counters accordingly.
    pub async fn get(&self, task_prefix: &str, text: &str) -> Option<Vec<f32>> {
        let key = Self::cache_key(task_prefix, text);
        let mut cache = self.entries.lock().await;

        if let Some(entry) = cache.get(&key) {
            if entry.created_at.elapsed() < self.ttl {
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Some(entry.vector.clone());
            }
            // Entry expired — remove it and count as miss
            cache.pop(&key);
        }

        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Insert (or update) a cached embedding vector.
    pub async fn put(&self, task_prefix: &str, text: &str, vector: Vec<f32>) {
        let key = Self::cache_key(task_prefix, text);
        let mut cache = self.entries.lock().await;
        cache.put(
            key,
            CacheEntry {
                vector,
                created_at: Instant::now(),
            },
        );
    }

    /// Return `(hits, misses)` counters.
    pub fn stats(&self) -> (u64, u64) {
        (
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
        )
    }
}

impl Default for EmbeddingCache {
    /// Default: 256 entries, 1800 s TTL.
    fn default() -> Self {
        Self::new(256, Duration::from_secs(1800))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_hit_and_miss_with_stats() {
        let cache = EmbeddingCache::new(16, Duration::from_secs(60));

        // Miss on empty cache
        assert!(cache.get("query", "hello").await.is_none());
        assert_eq!(cache.stats(), (0, 1));

        // Put and hit
        cache.put("query", "hello", vec![1.0, 2.0, 3.0]).await;
        let v = cache.get("query", "hello").await;
        assert_eq!(v, Some(vec![1.0, 2.0, 3.0]));
        assert_eq!(cache.stats(), (1, 1));

        // Second hit
        let _ = cache.get("query", "hello").await;
        assert_eq!(cache.stats(), (2, 1));
    }

    #[tokio::test]
    async fn test_task_prefix_isolation() {
        let cache = EmbeddingCache::new(16, Duration::from_secs(60));

        cache.put("query", "hello", vec![1.0]).await;
        cache.put("passage", "hello", vec![2.0]).await;

        let q = cache.get("query", "hello").await.unwrap();
        let p = cache.get("passage", "hello").await.unwrap();

        assert_eq!(q, vec![1.0]);
        assert_eq!(p, vec![2.0]);

        // Keys should differ
        let k1 = EmbeddingCache::cache_key("query", "hello");
        let k2 = EmbeddingCache::cache_key("passage", "hello");
        assert_ne!(k1, k2);
    }

    #[tokio::test]
    async fn test_ttl_expiry() {
        let cache = EmbeddingCache::new(16, Duration::from_millis(50));

        cache.put("query", "ephemeral", vec![9.0]).await;
        assert!(cache.get("query", "ephemeral").await.is_some());

        tokio::time::sleep(Duration::from_millis(100)).await;

        // Should be expired now
        assert!(cache.get("query", "ephemeral").await.is_none());
        // 1 hit + 1 miss (from expiry check) + 1 miss (the expired get)
        // First get: hit. Second get after sleep: miss.
        assert_eq!(cache.stats(), (1, 1));
    }

    #[tokio::test]
    async fn test_lru_eviction() {
        let cache = EmbeddingCache::new(2, Duration::from_secs(60));

        cache.put("x", "a", vec![1.0]).await;
        cache.put("x", "b", vec![2.0]).await;
        cache.put("x", "c", vec![3.0]).await; // evicts "a"

        assert!(cache.get("x", "a").await.is_none()); // miss
        assert_eq!(cache.get("x", "b").await, Some(vec![2.0]));
        assert_eq!(cache.get("x", "c").await, Some(vec![3.0]));
    }

    #[test]
    fn test_cache_key_deterministic() {
        let k1 = EmbeddingCache::cache_key("query", "test");
        let k2 = EmbeddingCache::cache_key("query", "test");
        assert_eq!(k1, k2);
        assert_eq!(k1.len(), 24);
    }

    #[test]
    fn test_default_impl() {
        let cache = EmbeddingCache::default();
        assert_eq!(cache.stats(), (0, 0));
    }
}
