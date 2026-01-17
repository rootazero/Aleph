//! Intent Cache Implementation
//!
//! Provides fast-path routing for repeated intent patterns:
//!
//! - LRU-based cache with configurable capacity
//! - Time-based confidence decay (exponential)
//! - Success/failure tracking for adaptive learning
//! - Thread-safe operations with tokio RwLock
//!
//! # Example
//!
//! ```ignore
//! use aethecore::intent::cache::{IntentCache, CacheConfig, CachedIntent};
//!
//! let cache = IntentCache::new(CacheConfig::default());
//!
//! // Cache an intent
//! let intent = CachedIntent::new("search test", "search", "BuiltinSearch", 0.9);
//! cache.put("search test", intent).await;
//!
//! // Retrieve it later
//! if let Some(cached) = cache.get("search test").await {
//!     println!("Hit! Tool: {}, Confidence: {}", cached.tool_name, cached.confidence);
//! }
//!
//! // Record execution outcomes
//! cache.record_success("search test").await;
//! cache.record_failure("other query").await;
//! ```

use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

// =============================================================================
// CachedIntent
// =============================================================================

/// A cached intent entry with time decay and success tracking.
#[derive(Debug, Clone)]
pub struct CachedIntent {
    /// Normalized input pattern (for logging/debugging)
    pub pattern: String,

    /// Matched tool name
    pub tool_name: String,

    /// The intent type (e.g., "FileOrganize", "BuiltinSearch")
    pub intent_type: String,

    /// Current confidence (may be decayed from original)
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
}

impl CachedIntent {
    /// Create a new cached intent entry.
    ///
    /// # Arguments
    ///
    /// * `pattern` - Normalized input pattern (for logging)
    /// * `tool_name` - The matched tool name
    /// * `intent_type` - The intent type (e.g., "FileOrganize")
    /// * `confidence` - Initial confidence score (0.0-1.0)
    pub fn new(
        pattern: impl Into<String>,
        tool_name: impl Into<String>,
        intent_type: impl Into<String>,
        confidence: f32,
    ) -> Self {
        Self {
            pattern: pattern.into(),
            tool_name: tool_name.into(),
            intent_type: intent_type.into(),
            confidence,
            original_confidence: confidence,
            cached_at: Instant::now(),
            hit_count: 0,
            success_count: 0,
            failure_count: 0,
        }
    }

    /// Calculate current confidence with exponential time decay.
    ///
    /// Uses the formula: `original_confidence * e^(-t / half_life)`
    ///
    /// # Arguments
    ///
    /// * `half_life_secs` - Time in seconds for confidence to decay to half
    ///
    /// # Returns
    ///
    /// Decayed confidence value (0.0-1.0)
    pub fn decayed_confidence(&self, half_life_secs: f32) -> f32 {
        let age_secs = self.cached_at.elapsed().as_secs_f32();
        // Use ln(2) / half_life for proper half-life decay
        let decay_constant = std::f32::consts::LN_2 / half_life_secs;
        let decay = (-age_secs * decay_constant).exp();
        self.original_confidence * decay
    }

    /// Calculate the success rate of this cached intent.
    ///
    /// Returns 1.0 if no executions have been recorded yet (optimistic default).
    pub fn success_rate(&self) -> f32 {
        let total = self.success_count + self.failure_count;
        if total == 0 {
            1.0 // Assume success if no data
        } else {
            self.success_count as f32 / total as f32
        }
    }

    /// Calculate adjusted confidence combining decay and success rate.
    ///
    /// Formula: `decayed_confidence * success_rate`
    ///
    /// This provides a more realistic confidence that accounts for both
    /// time-based staleness and historical execution outcomes.
    pub fn adjusted_confidence(&self, half_life_secs: f32) -> f32 {
        let decayed = self.decayed_confidence(half_life_secs);
        decayed * self.success_rate()
    }

    /// Check if this entry should be evicted due to poor performance.
    ///
    /// Returns true if:
    /// - failure_count > 3 AND success_count == 0
    ///
    /// This prevents keeping entries that consistently fail.
    pub fn should_evict(&self) -> bool {
        self.failure_count > 3 && self.success_count == 0
    }
}

// =============================================================================
// CacheMetrics
// =============================================================================

/// Metrics for cache operations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheMetrics {
    /// Total cache hits
    pub hits: u64,

    /// Total cache misses
    pub misses: u64,

    /// Total evictions (due to capacity or failures)
    pub evictions: u64,
}

impl CacheMetrics {
    /// Calculate the cache hit rate.
    ///
    /// Returns 0.0 if no requests have been made.
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

// =============================================================================
// CacheConfig
// =============================================================================

/// Configuration for the intent cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Maximum number of entries in the cache
    pub capacity: usize,

    /// Half-life for confidence decay in seconds (default: 3600 = 1 hour)
    pub half_life_secs: f32,

    /// Minimum confidence threshold for cache hits
    pub min_confidence: f32,

    /// Whether caching is enabled
    pub enabled: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            capacity: 1000,
            half_life_secs: 3600.0,
            min_confidence: 0.5,
            enabled: true,
        }
    }
}

// =============================================================================
// IntentCache
// =============================================================================

/// Thread-safe LRU cache for intent lookups.
///
/// The cache uses a hash of the normalized input (lowercase, trimmed, first 100 chars)
/// as the key. This allows similar inputs to match the same cached intent.
pub struct IntentCache {
    /// LRU cache backing store (u64 is hash of normalized input)
    cache: Arc<RwLock<LruCache<u64, CachedIntent>>>,

    /// Cache configuration
    config: CacheConfig,

    /// Metrics counters
    metrics: Arc<RwLock<CacheMetrics>>,
}

impl IntentCache {
    /// Create a new intent cache with the given configuration.
    pub fn new(config: CacheConfig) -> Self {
        let capacity = NonZeroUsize::new(config.capacity.max(1))
            .expect("Cache capacity must be > 0");

        Self {
            cache: Arc::new(RwLock::new(LruCache::new(capacity))),
            config,
            metrics: Arc::new(RwLock::new(CacheMetrics::default())),
        }
    }

    /// Get a cached intent for the given input.
    ///
    /// Returns `None` if:
    /// - Cache is disabled
    /// - No entry exists for this input
    /// - Entry's adjusted confidence is below min_confidence
    /// - Entry should be evicted due to failures
    ///
    /// On hit, the entry's hit_count is incremented and confidence is updated.
    pub async fn get(&self, input: &str) -> Option<CachedIntent> {
        if !self.config.enabled {
            return None;
        }

        let hash = self.hash_input(input);
        let mut cache = self.cache.write().await;

        if let Some(entry) = cache.get_mut(&hash) {
            // Check if should evict due to failures
            if entry.should_evict() {
                cache.pop(&hash);
                // Merge all metrics updates into single lock scope
                let mut metrics = self.metrics.write().await;
                metrics.evictions += 1;
                metrics.misses += 1;
                return None;
            }

            // Calculate adjusted confidence
            let adjusted = entry.adjusted_confidence(self.config.half_life_secs);

            // Check minimum confidence threshold
            if adjusted < self.config.min_confidence {
                // Merge all metrics updates into single lock scope
                let mut metrics = self.metrics.write().await;
                metrics.misses += 1;
                return None;
            }

            // Update entry
            entry.confidence = adjusted;
            entry.hit_count += 1;

            // Merge all metrics updates into single lock scope
            let mut metrics = self.metrics.write().await;
            metrics.hits += 1;

            Some(entry.clone())
        } else {
            // Merge all metrics updates into single lock scope
            let mut metrics = self.metrics.write().await;
            metrics.misses += 1;
            None
        }
    }

    /// Add or update an entry in the cache.
    ///
    /// If caching is disabled, this is a no-op.
    pub async fn put(&self, input: &str, intent: CachedIntent) {
        if !self.config.enabled {
            return;
        }

        let hash = self.hash_input(input);
        let mut cache = self.cache.write().await;

        // Check if we'll cause an eviction due to capacity
        if cache.len() >= self.config.capacity && !cache.contains(&hash) {
            let mut metrics = self.metrics.write().await;
            metrics.evictions += 1;
        }

        cache.put(hash, intent);
    }

    /// Record a successful tool execution for the given input.
    ///
    /// Increments the success_count of the cached entry if it exists.
    /// Uses saturating arithmetic to prevent integer overflow at u32::MAX.
    pub async fn record_success(&self, input: &str) {
        if !self.config.enabled {
            return;
        }

        let hash = self.hash_input(input);
        let mut cache = self.cache.write().await;

        if let Some(entry) = cache.get_mut(&hash) {
            entry.success_count = entry.success_count.saturating_add(1);
        }
    }

    /// Record a failed/cancelled tool execution for the given input.
    ///
    /// Increments the failure_count. If the entry should be evicted
    /// (too many failures with no successes), it will be removed.
    /// Uses saturating arithmetic to prevent integer overflow at u32::MAX.
    pub async fn record_failure(&self, input: &str) {
        if !self.config.enabled {
            return;
        }

        let hash = self.hash_input(input);
        let mut cache = self.cache.write().await;

        if let Some(entry) = cache.get_mut(&hash) {
            entry.failure_count = entry.failure_count.saturating_add(1);

            // Evict if too many failures
            if entry.should_evict() {
                cache.pop(&hash);
                let mut metrics = self.metrics.write().await;
                metrics.evictions += 1;
            }
        }
    }

    /// Get a snapshot of the current cache metrics.
    pub async fn metrics(&self) -> CacheMetrics {
        self.metrics.read().await.clone()
    }

    /// Clear all entries from the cache.
    ///
    /// This also increments the eviction count for each removed entry.
    pub async fn clear(&self) {
        let mut cache = self.cache.write().await;
        let count = cache.len() as u64;
        cache.clear();

        let mut metrics = self.metrics.write().await;
        metrics.evictions += count;
    }

    /// Get the current number of entries in the cache.
    pub async fn size(&self) -> usize {
        self.cache.read().await.len()
    }

    /// Hash the input for use as a cache key.
    ///
    /// The input is normalized by:
    /// 1. Trimming whitespace
    /// 2. Converting to lowercase
    /// 3. Taking only the first 100 characters
    fn hash_input(&self, input: &str) -> u64 {
        let normalized: String = input
            .trim()
            .to_lowercase()
            .chars()
            .take(100)
            .collect();

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

    // -------------------------------------------------------------------------
    // CachedIntent Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cached_intent_new() {
        let intent = CachedIntent::new("test pattern", "search", "BuiltinSearch", 0.9);

        assert_eq!(intent.pattern, "test pattern");
        assert_eq!(intent.tool_name, "search");
        assert_eq!(intent.intent_type, "BuiltinSearch");
        assert!((intent.confidence - 0.9).abs() < 0.001);
        assert!((intent.original_confidence - 0.9).abs() < 0.001);
        assert_eq!(intent.hit_count, 0);
        assert_eq!(intent.success_count, 0);
        assert_eq!(intent.failure_count, 0);
    }

    #[test]
    fn test_cached_intent_decayed_confidence_at_zero() {
        let intent = CachedIntent::new("test", "tool", "Type", 0.9);

        // At t=0, confidence should be approximately original
        let decayed = intent.decayed_confidence(3600.0);
        assert!((decayed - 0.9).abs() < 0.01);
    }

    #[test]
    fn test_cached_intent_success_rate_no_data() {
        let intent = CachedIntent::new("test", "tool", "Type", 0.9);

        // No data should assume 100% success
        assert!((intent.success_rate() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cached_intent_success_rate_with_data() {
        let mut intent = CachedIntent::new("test", "tool", "Type", 0.9);
        intent.success_count = 3;
        intent.failure_count = 1;

        assert!((intent.success_rate() - 0.75).abs() < 0.001);
    }

    #[test]
    fn test_cached_intent_adjusted_confidence() {
        let mut intent = CachedIntent::new("test", "tool", "Type", 0.9);
        intent.success_count = 4;
        intent.failure_count = 1;

        // At t=0: adjusted = 0.9 * (4/5) = 0.72
        let adjusted = intent.adjusted_confidence(3600.0);
        assert!((adjusted - 0.72).abs() < 0.02);
    }

    #[test]
    fn test_cached_intent_should_evict_false() {
        let mut intent = CachedIntent::new("test", "tool", "Type", 0.9);
        intent.failure_count = 3; // Not > 3
        intent.success_count = 0;

        assert!(!intent.should_evict());
    }

    #[test]
    fn test_cached_intent_should_evict_true() {
        let mut intent = CachedIntent::new("test", "tool", "Type", 0.9);
        intent.failure_count = 4; // > 3
        intent.success_count = 0;

        assert!(intent.should_evict());
    }

    #[test]
    fn test_cached_intent_should_evict_with_success() {
        let mut intent = CachedIntent::new("test", "tool", "Type", 0.9);
        intent.failure_count = 10;
        intent.success_count = 1; // Has at least one success

        assert!(!intent.should_evict());
    }

    // -------------------------------------------------------------------------
    // CacheMetrics Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cache_metrics_default() {
        let metrics = CacheMetrics::default();

        assert_eq!(metrics.hits, 0);
        assert_eq!(metrics.misses, 0);
        assert_eq!(metrics.evictions, 0);
    }

    #[test]
    fn test_cache_metrics_hit_rate_zero() {
        let metrics = CacheMetrics::default();

        assert!((metrics.hit_rate() - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_cache_metrics_hit_rate() {
        let metrics = CacheMetrics {
            hits: 75,
            misses: 25,
            evictions: 5,
        };

        assert!((metrics.hit_rate() - 0.75).abs() < 0.001);
    }

    // -------------------------------------------------------------------------
    // CacheConfig Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cache_config_default() {
        let config = CacheConfig::default();

        assert_eq!(config.capacity, 1000);
        assert!((config.half_life_secs - 3600.0).abs() < 0.001);
        assert!((config.min_confidence - 0.5).abs() < 0.001);
        assert!(config.enabled);
    }

    // -------------------------------------------------------------------------
    // IntentCache Tests
    // -------------------------------------------------------------------------

    fn test_config() -> CacheConfig {
        CacheConfig {
            capacity: 100,
            half_life_secs: 3600.0,
            min_confidence: 0.3,
            enabled: true,
        }
    }

    #[tokio::test]
    async fn test_cache_put_and_get() {
        let cache = IntentCache::new(test_config());

        let intent = CachedIntent::new("search test", "search", "BuiltinSearch", 0.9);
        cache.put("search test", intent).await;

        let retrieved = cache.get("search test").await;
        assert!(retrieved.is_some());

        let entry = retrieved.unwrap();
        assert_eq!(entry.tool_name, "search");
        assert_eq!(entry.intent_type, "BuiltinSearch");
        assert!(entry.confidence > 0.8); // May have slight decay
        assert_eq!(entry.hit_count, 1);
    }

    #[tokio::test]
    async fn test_cache_miss() {
        let cache = IntentCache::new(test_config());

        let entry = cache.get("nonexistent").await;
        assert!(entry.is_none());

        let metrics = cache.metrics().await;
        assert_eq!(metrics.misses, 1);
        assert_eq!(metrics.hits, 0);
    }

    #[tokio::test]
    async fn test_cache_normalization() {
        let cache = IntentCache::new(test_config());

        // Put with different casing/whitespace
        let intent = CachedIntent::new("SEARCH Test", "search", "BuiltinSearch", 0.9);
        cache.put("  SEARCH Test  ", intent).await;

        // Should match normalized version
        let entry = cache.get("search test").await;
        assert!(entry.is_some());
    }

    #[tokio::test]
    async fn test_cache_normalization_truncation() {
        let cache = IntentCache::new(test_config());

        // Create a long input (> 100 chars)
        let long_input = "a".repeat(150);
        let truncated_input = "a".repeat(100);

        let intent = CachedIntent::new(&long_input, "tool", "Type", 0.9);
        cache.put(&long_input, intent).await;

        // Should match truncated version
        let entry = cache.get(&truncated_input).await;
        assert!(entry.is_some());
    }

    #[tokio::test]
    async fn test_cache_record_success() {
        let cache = IntentCache::new(test_config());

        let intent = CachedIntent::new("test", "tool", "Type", 0.9);
        cache.put("test", intent).await;

        cache.record_success("test").await;

        let entry = cache.get("test").await.unwrap();
        assert_eq!(entry.success_count, 1);
        assert_eq!(entry.failure_count, 0);
    }

    #[tokio::test]
    async fn test_cache_record_failure() {
        // Use min_confidence of 0.0 so we can verify failure_count
        // (otherwise adjusted_confidence = confidence * success_rate = 0.9 * 0 = 0)
        let mut config = test_config();
        config.min_confidence = 0.0;
        let cache = IntentCache::new(config);

        let intent = CachedIntent::new("test", "tool", "Type", 0.9);
        cache.put("test", intent).await;

        cache.record_failure("test").await;

        let entry = cache.get("test").await.unwrap();
        assert_eq!(entry.failure_count, 1);
    }

    #[tokio::test]
    async fn test_cache_eviction_on_failures() {
        let cache = IntentCache::new(test_config());

        let intent = CachedIntent::new("fail test", "tool", "Type", 0.9);
        cache.put("fail test", intent).await;

        // Record 4 failures (> 3)
        for _ in 0..4 {
            cache.record_failure("fail test").await;
        }

        // Entry should be evicted
        let entry = cache.get("fail test").await;
        assert!(entry.is_none());

        let metrics = cache.metrics().await;
        assert!(metrics.evictions > 0);
    }

    #[tokio::test]
    async fn test_cache_disabled() {
        let mut config = test_config();
        config.enabled = false;
        let cache = IntentCache::new(config);

        let intent = CachedIntent::new("test", "tool", "Type", 0.9);
        cache.put("test", intent).await;

        let entry = cache.get("test").await;
        assert!(entry.is_none());
    }

    #[tokio::test]
    async fn test_cache_min_confidence_threshold() {
        let mut config = test_config();
        config.min_confidence = 0.8;
        let cache = IntentCache::new(config);

        // Put an entry with low confidence
        let intent = CachedIntent::new("test", "tool", "Type", 0.5);
        cache.put("test", intent).await;

        // Should not match due to min_confidence threshold
        let entry = cache.get("test").await;
        assert!(entry.is_none());

        let metrics = cache.metrics().await;
        assert_eq!(metrics.misses, 1);
    }

    #[tokio::test]
    async fn test_cache_clear() {
        let cache = IntentCache::new(test_config());

        for i in 0..10 {
            let intent = CachedIntent::new(format!("test {}", i), "tool", "Type", 0.9);
            cache.put(&format!("test {}", i), intent).await;
        }

        assert_eq!(cache.size().await, 10);

        cache.clear().await;
        assert_eq!(cache.size().await, 0);

        let metrics = cache.metrics().await;
        assert_eq!(metrics.evictions, 10);
    }

    #[tokio::test]
    async fn test_cache_metrics() {
        let cache = IntentCache::new(test_config());

        let intent = CachedIntent::new("test", "tool", "Type", 0.9);
        cache.put("test", intent).await;

        // Generate hits and misses
        let _ = cache.get("test").await;
        let _ = cache.get("test").await;
        let _ = cache.get("nonexistent").await;

        let metrics = cache.metrics().await;
        assert_eq!(metrics.hits, 2);
        assert_eq!(metrics.misses, 1);
        assert!((metrics.hit_rate() - 0.666).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_cache_capacity_eviction() {
        let mut config = test_config();
        config.capacity = 3;
        let cache = IntentCache::new(config);

        // Fill the cache
        for i in 0..3 {
            let intent = CachedIntent::new(format!("test {}", i), "tool", "Type", 0.9);
            cache.put(&format!("test {}", i), intent).await;
        }

        // Add one more - should evict LRU
        let intent = CachedIntent::new("test 3", "tool", "Type", 0.9);
        cache.put("test 3", intent).await;

        assert_eq!(cache.size().await, 3);

        let metrics = cache.metrics().await;
        assert_eq!(metrics.evictions, 1);
    }

    #[tokio::test]
    async fn test_cache_hit_increments_count() {
        let cache = IntentCache::new(test_config());

        let intent = CachedIntent::new("test", "tool", "Type", 0.9);
        cache.put("test", intent).await;

        let _ = cache.get("test").await;
        let _ = cache.get("test").await;
        let entry = cache.get("test").await.unwrap();

        assert_eq!(entry.hit_count, 3);
    }

    #[tokio::test]
    async fn test_cache_success_rate_affects_retrieval() {
        let mut config = test_config();
        config.min_confidence = 0.5;
        let cache = IntentCache::new(config);

        // Start with confidence 0.6
        let intent = CachedIntent::new("test", "tool", "Type", 0.6);
        cache.put("test", intent).await;

        // Record failures - success_rate drops to 0
        cache.record_failure("test").await;

        // Get entry to manually inspect adjusted confidence
        // With 0 success and 1 failure, success_rate = 0 (since we have data now)
        // adjusted = 0.6 * 0 = 0, which is below min_confidence
        // But wait - success_rate with 0 success and 1 failure = 0/1 = 0
        // So adjusted confidence = ~0.6 * 0 = 0
        // This should fail min_confidence check

        // First let's record more failures to be sure
        cache.record_failure("test").await;
        cache.record_failure("test").await;

        let entry = cache.get("test").await;
        assert!(entry.is_none());
    }
}
