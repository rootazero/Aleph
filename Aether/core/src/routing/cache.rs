//! Intent Cache Implementation
//!
//! Provides fast-path routing for repeated intent patterns:
//!
//! - LRU-based cache with configurable capacity
//! - Time-based confidence decay
//! - Success/failure tracking for learning
//! - Thread-safe operations with RwLock

use crate::routing::{CacheConfig, IntentAction};
use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

// =============================================================================
// Cached Intent
// =============================================================================

/// A cached intent entry
#[derive(Debug, Clone)]
pub struct CachedIntent {
    /// Normalized input pattern (for logging/debugging)
    pub pattern: String,

    /// Matched tool name
    pub tool_name: String,

    /// Extracted parameters (if any)
    pub parameters: serde_json::Value,

    /// Current confidence (may be decayed)
    pub confidence: f32,

    /// Original confidence when cached
    pub original_confidence: f32,

    /// When this entry was created
    pub cached_at: Instant,

    /// Number of times this entry was hit
    pub hit_count: u32,

    /// Number of successful executions
    pub success_count: u32,

    /// Number of failed/cancelled executions
    pub failure_count: u32,

    /// Recommended action at time of caching
    pub action: IntentAction,
}

impl CachedIntent {
    /// Create a new cached intent
    pub fn new(
        pattern: impl Into<String>,
        tool_name: impl Into<String>,
        parameters: serde_json::Value,
        confidence: f32,
        action: IntentAction,
    ) -> Self {
        Self {
            pattern: pattern.into(),
            tool_name: tool_name.into(),
            parameters,
            confidence,
            original_confidence: confidence,
            cached_at: Instant::now(),
            hit_count: 0,
            success_count: 0,
            failure_count: 0,
            action,
        }
    }

    /// Calculate current confidence with decay
    pub fn decayed_confidence(&self, half_life_secs: f32) -> f32 {
        let age_secs = self.cached_at.elapsed().as_secs_f32();
        let decay = (-age_secs / half_life_secs).exp();
        self.original_confidence * decay
    }

    /// Calculate success rate
    pub fn success_rate(&self) -> f32 {
        let total = self.success_count + self.failure_count;
        if total == 0 {
            1.0 // Assume success if no data
        } else {
            self.success_count as f32 / total as f32
        }
    }

    /// Calculate adjusted confidence (decay + success rate)
    pub fn adjusted_confidence(&self, half_life_secs: f32) -> f32 {
        let decayed = self.decayed_confidence(half_life_secs);
        decayed * self.success_rate()
    }

    /// Check if entry should be evicted (too many failures)
    pub fn should_evict(&self) -> bool {
        self.failure_count > 3 && self.success_count == 0
    }
}

// =============================================================================
// Cache Metrics
// =============================================================================

/// Metrics for cache operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMetrics {
    /// Total cache hits
    pub hits: u64,

    /// Total cache misses
    pub misses: u64,

    /// Total evictions
    pub evictions: u64,

    /// Current cache size
    pub size: usize,

    /// Maximum cache size
    pub max_size: usize,

    /// Average confidence of cached entries
    pub avg_confidence: f32,

    /// Average success rate of cached entries
    pub avg_success_rate: f32,
}

impl CacheMetrics {
    /// Calculate hit rate
    pub fn hit_rate(&self) -> f32 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f32 / total as f32
        }
    }
}

/// Thread-safe atomic metrics counters
struct AtomicMetrics {
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
}

impl AtomicMetrics {
    fn new() -> Self {
        Self {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    fn record_eviction(&self) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
    }

    fn get_hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    fn get_misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    fn get_evictions(&self) -> u64 {
        self.evictions.load(Ordering::Relaxed)
    }

    fn reset(&self) {
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.evictions.store(0, Ordering::Relaxed);
    }
}

// =============================================================================
// Intent Cache
// =============================================================================

/// Thread-safe intent cache with LRU eviction
pub struct IntentCache {
    /// LRU cache backing store
    cache: Arc<RwLock<LruCache<u64, CachedIntent>>>,

    /// Cache configuration
    config: CacheConfig,

    /// Atomic metrics counters
    metrics: AtomicMetrics,
}

impl IntentCache {
    /// Create a new intent cache with the given configuration
    pub fn new(config: CacheConfig) -> Self {
        let capacity = NonZeroUsize::new(config.max_size.max(1))
            .expect("Cache size must be > 0");

        Self {
            cache: Arc::new(RwLock::new(LruCache::new(capacity))),
            config,
            metrics: AtomicMetrics::new(),
        }
    }

    /// Create a cache with default configuration
    pub fn default_cache() -> Self {
        Self::new(CacheConfig::default())
    }

    /// Check if caching is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get a cached intent, applying time decay
    ///
    /// Returns `None` if:
    /// - Cache is disabled
    /// - No entry exists
    /// - Entry has expired (TTL exceeded)
    pub async fn get(&self, input: &str) -> Option<CachedIntent> {
        if !self.config.enabled {
            return None;
        }

        let hash = self.hash_input(input);
        let mut cache = self.cache.write().await;

        if let Some(entry) = cache.get_mut(&hash) {
            // Check TTL
            if entry.cached_at.elapsed().as_secs() > self.config.ttl_seconds {
                // Entry expired
                cache.pop(&hash);
                self.metrics.record_eviction();
                self.metrics.record_miss();
                return None;
            }

            // Check if should evict due to failures
            if entry.should_evict() {
                cache.pop(&hash);
                self.metrics.record_eviction();
                self.metrics.record_miss();
                return None;
            }

            // Update confidence with decay
            let adjusted = entry.adjusted_confidence(self.config.decay_half_life_seconds);
            entry.confidence = adjusted;
            entry.hit_count += 1;

            self.metrics.record_hit();
            Some(entry.clone())
        } else {
            self.metrics.record_miss();
            None
        }
    }

    /// Add a new entry to the cache
    pub async fn put(
        &self,
        input: &str,
        tool_name: &str,
        parameters: serde_json::Value,
        confidence: f32,
        action: IntentAction,
    ) {
        if !self.config.enabled {
            return;
        }

        let hash = self.hash_input(input);
        let entry = CachedIntent::new(input, tool_name, parameters, confidence, action);

        let mut cache = self.cache.write().await;

        // Check if we'll cause an eviction
        if cache.len() >= self.config.max_size && !cache.contains(&hash) {
            self.metrics.record_eviction();
        }

        cache.put(hash, entry);
    }

    /// Record a successful tool execution
    pub async fn record_success(&self, input: &str) {
        if !self.config.enabled {
            return;
        }

        let hash = self.hash_input(input);
        let mut cache = self.cache.write().await;

        if let Some(entry) = cache.get_mut(&hash) {
            entry.success_count += 1;
        }
    }

    /// Record a failed/cancelled tool execution
    pub async fn record_failure(&self, input: &str) {
        if !self.config.enabled {
            return;
        }

        let hash = self.hash_input(input);
        let mut cache = self.cache.write().await;

        if let Some(entry) = cache.get_mut(&hash) {
            entry.failure_count += 1;

            // Evict if too many failures
            if entry.should_evict() {
                cache.pop(&hash);
                self.metrics.record_eviction();
            }
        }
    }

    /// Clear all entries from the cache
    pub async fn clear(&self) {
        let mut cache = self.cache.write().await;
        let count = cache.len();
        cache.clear();

        // Record evictions
        for _ in 0..count {
            self.metrics.record_eviction();
        }
    }

    /// Get the current cache size
    pub async fn size(&self) -> usize {
        self.cache.read().await.len()
    }

    /// Get current metrics snapshot
    pub async fn metrics(&self) -> CacheMetrics {
        let cache = self.cache.read().await;

        // Calculate average confidence and success rate
        let (total_conf, total_rate, count) = cache
            .iter()
            .fold((0.0, 0.0, 0usize), |(tc, tr, c), (_, entry)| {
                (
                    tc + entry.confidence,
                    tr + entry.success_rate(),
                    c + 1,
                )
            });

        let avg_confidence = if count > 0 { total_conf / count as f32 } else { 0.0 };
        let avg_success_rate = if count > 0 { total_rate / count as f32 } else { 0.0 };

        CacheMetrics {
            hits: self.metrics.get_hits(),
            misses: self.metrics.get_misses(),
            evictions: self.metrics.get_evictions(),
            size: cache.len(),
            max_size: self.config.max_size,
            avg_confidence,
            avg_success_rate,
        }
    }

    /// Reset metrics counters
    pub fn reset_metrics(&self) {
        self.metrics.reset();
    }

    /// Update cache configuration
    ///
    /// Note: Changing max_size may require clearing the cache
    pub async fn update_config(&mut self, config: CacheConfig) {
        let old_max_size = self.config.max_size;
        self.config = config;

        // If max_size decreased, clear the cache to avoid issues
        if self.config.max_size < old_max_size {
            self.clear().await;
        }
    }

    /// Hash the input for cache key
    fn hash_input(&self, input: &str) -> u64 {
        let normalized = input.trim().to_lowercase();
        let mut hasher = DefaultHasher::new();
        normalized.hash(&mut hasher);
        hasher.finish()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn create_test_config() -> CacheConfig {
        CacheConfig {
            enabled: true,
            max_size: 100,
            ttl_seconds: 3600,
            decay_half_life_seconds: 1800.0,
            cache_auto_execute_threshold: 0.95,
        }
    }

    #[tokio::test]
    async fn test_cache_put_get() {
        let cache = IntentCache::new(create_test_config());

        // Put an entry
        cache
            .put(
                "search test",
                "search",
                serde_json::json!({"query": "test"}),
                0.9,
                IntentAction::Execute,
            )
            .await;

        // Get it back
        let entry = cache.get("search test").await;
        assert!(entry.is_some());

        let entry = entry.unwrap();
        assert_eq!(entry.tool_name, "search");
        assert!(entry.confidence > 0.8); // May have slight decay
        assert_eq!(entry.hit_count, 1);
    }

    #[tokio::test]
    async fn test_cache_miss() {
        let cache = IntentCache::new(create_test_config());

        let entry = cache.get("nonexistent").await;
        assert!(entry.is_none());

        let metrics = cache.metrics().await;
        assert_eq!(metrics.misses, 1);
        assert_eq!(metrics.hits, 0);
    }

    #[tokio::test]
    async fn test_cache_normalization() {
        let cache = IntentCache::new(create_test_config());

        // Put with different casing/whitespace
        cache
            .put(
                "  SEARCH Test  ",
                "search",
                serde_json::json!({}),
                0.9,
                IntentAction::Execute,
            )
            .await;

        // Should match normalized version
        let entry = cache.get("search test").await;
        assert!(entry.is_some());
    }

    #[tokio::test]
    async fn test_cache_success_failure() {
        let cache = IntentCache::new(create_test_config());

        cache
            .put(
                "test input",
                "tool",
                serde_json::json!({}),
                0.9,
                IntentAction::Execute,
            )
            .await;

        // Record success
        cache.record_success("test input").await;

        let entry = cache.get("test input").await.unwrap();
        assert_eq!(entry.success_count, 1);
        assert_eq!(entry.failure_count, 0);
        assert!((entry.success_rate() - 1.0).abs() < 0.01);

        // Record failure
        cache.record_failure("test input").await;

        let entry = cache.get("test input").await.unwrap();
        assert_eq!(entry.failure_count, 1);
        assert!((entry.success_rate() - 0.5).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_cache_eviction_on_failures() {
        let mut config = create_test_config();
        config.max_size = 10;
        let cache = IntentCache::new(config);

        cache
            .put(
                "fail test",
                "tool",
                serde_json::json!({}),
                0.9,
                IntentAction::Execute,
            )
            .await;

        // Record multiple failures
        for _ in 0..4 {
            cache.record_failure("fail test").await;
        }

        // Entry should be evicted
        let entry = cache.get("fail test").await;
        assert!(entry.is_none());
    }

    #[tokio::test]
    async fn test_cache_disabled() {
        let mut config = create_test_config();
        config.enabled = false;
        let cache = IntentCache::new(config);

        cache
            .put(
                "test",
                "tool",
                serde_json::json!({}),
                0.9,
                IntentAction::Execute,
            )
            .await;

        let entry = cache.get("test").await;
        assert!(entry.is_none());
    }

    #[tokio::test]
    async fn test_cache_clear() {
        let cache = IntentCache::new(create_test_config());

        for i in 0..10 {
            cache
                .put(
                    &format!("test {}", i),
                    "tool",
                    serde_json::json!({}),
                    0.9,
                    IntentAction::Execute,
                )
                .await;
        }

        assert_eq!(cache.size().await, 10);

        cache.clear().await;
        assert_eq!(cache.size().await, 0);
    }

    #[tokio::test]
    async fn test_cache_metrics() {
        let cache = IntentCache::new(create_test_config());

        cache
            .put(
                "test",
                "tool",
                serde_json::json!({}),
                0.9,
                IntentAction::Execute,
            )
            .await;

        // Generate some hits and misses
        let _ = cache.get("test").await;
        let _ = cache.get("test").await;
        let _ = cache.get("nonexistent").await;

        let metrics = cache.metrics().await;
        assert_eq!(metrics.hits, 2);
        assert_eq!(metrics.misses, 1);
        assert!((metrics.hit_rate() - 0.666).abs() < 0.01);
    }

    #[test]
    fn test_cached_intent_decay() {
        let mut entry = CachedIntent::new(
            "test",
            "tool",
            serde_json::json!({}),
            0.9,
            IntentAction::Execute,
        );

        // At t=0, confidence should be original
        let confidence = entry.decayed_confidence(1800.0);
        assert!((confidence - 0.9).abs() < 0.01);

        // Simulate time passage by changing cached_at
        // (in real use, time naturally passes)
    }

    #[test]
    fn test_cached_intent_success_rate() {
        let mut entry = CachedIntent::new(
            "test",
            "tool",
            serde_json::json!({}),
            0.9,
            IntentAction::Execute,
        );

        // No data - assume 100%
        assert_eq!(entry.success_rate(), 1.0);

        entry.success_count = 3;
        entry.failure_count = 1;
        assert!((entry.success_rate() - 0.75).abs() < 0.01);
    }

    #[test]
    fn test_cache_metrics_hit_rate() {
        let metrics = CacheMetrics {
            hits: 75,
            misses: 25,
            evictions: 0,
            size: 100,
            max_size: 1000,
            avg_confidence: 0.85,
            avg_success_rate: 0.9,
        };

        assert!((metrics.hit_rate() - 0.75).abs() < 0.01);
    }
}
