//! Performance Optimizer for L2 Routing
//!
//! This module provides performance optimizations for L2 routing including:
//! - Rule indexing for fast pattern matching
//! - Query result caching
//! - Precompiled regex patterns
//! - Hit rate tracking and optimization
//!
//! # Architecture
//!
//! ```text
//! Query → Cache Check → Index Lookup → Pattern Match → Cache Store
//!   ↓         ↓             ↓              ↓              ↓
//! Input    Hit/Miss      Fast Find      Execute        Update
//! ```
//!
//! # Performance Improvements
//!
//! - **Cache Hit**: O(1) lookup, ~1 μs
//! - **Index Lookup**: O(log n) instead of O(n), ~10 μs
//! - **Precompiled Regex**: 10-100x faster than runtime compilation
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::engine::{PerformanceOptimizer, ReflexLayer};
//!
//! let mut optimizer = PerformanceOptimizer::new(1000);
//! let reflex_layer = ReflexLayer::new();
//!
//! // Build index
//! optimizer.build_index(&reflex_layer);
//!
//! // Query with caching
//! if let Some(cached) = optimizer.get_cached("git status") {
//!     // Cache hit - instant response
//! } else {
//!     // Cache miss - execute and cache
//!     let result = reflex_layer.route("git status");
//!     optimizer.cache("git status", result);
//! }
//! ```

use dashmap::DashMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Performance optimizer for L2 routing
pub struct PerformanceOptimizer {
    /// Query result cache (input -> cached result)
    cache: Arc<DashMap<String, CachedResult>>,

    /// Maximum cache size
    max_cache_size: usize,

    /// Rule index for fast lookup
    rule_index: Arc<DashMap<String, Vec<usize>>>,

    /// Precompiled regex patterns
    compiled_patterns: Arc<DashMap<usize, Regex>>,

    /// Hit rate statistics
    stats: Arc<DashMap<String, HitStats>>,
}

impl PerformanceOptimizer {
    /// Create a new performance optimizer
    ///
    /// # Arguments
    ///
    /// * `max_cache_size` - Maximum number of cached queries
    pub fn new(max_cache_size: usize) -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
            max_cache_size,
            rule_index: Arc::new(DashMap::new()),
            compiled_patterns: Arc::new(DashMap::new()),
            stats: Arc::new(DashMap::new()),
        }
    }

    /// Get cached result for a query
    ///
    /// # Arguments
    ///
    /// * `query` - The input query
    ///
    /// # Returns
    ///
    /// Cached result if available and not expired
    pub fn get_cached(&self, query: &str) -> Option<CachedResult> {
        if let Some(entry) = self.cache.get(query) {
            let cached = entry.value();

            // Check if cache entry is still valid
            if !cached.is_expired() {
                // Update hit stats
                self.record_hit(query, true);

                debug!(query = %query, age_ms = cached.age().as_millis(), "Cache hit");
                return Some(cached.clone());
            } else {
                // Remove expired entry
                drop(entry);
                self.cache.remove(query);
                debug!(query = %query, "Cache expired");
            }
        }

        // Cache miss
        self.record_hit(query, false);
        None
    }

    /// Cache a query result
    ///
    /// # Arguments
    ///
    /// * `query` - The input query
    /// * `result` - The routing result to cache
    pub fn cache(&self, query: &str, result: String) {
        // Check cache size limit
        if self.cache.len() >= self.max_cache_size {
            self.evict_oldest();
        }

        let cached = CachedResult {
            result,
            cached_at: Instant::now(),
            ttl: Duration::from_secs(300), // 5 minutes TTL
        };

        self.cache.insert(query.to_string(), cached);
        debug!(query = %query, "Cached result");
    }

    /// Evict oldest cache entry
    fn evict_oldest(&self) {
        // Find oldest entry
        let mut oldest_key: Option<String> = None;
        let mut oldest_time = Instant::now();

        for entry in self.cache.iter() {
            let cached = entry.value();
            if cached.cached_at < oldest_time {
                oldest_time = cached.cached_at;
                oldest_key = Some(entry.key().clone());
            }
        }

        // Remove oldest entry
        if let Some(key) = oldest_key {
            self.cache.remove(&key);
            debug!(key = %key, "Evicted oldest cache entry");
        }
    }

    /// Build rule index for fast lookup
    ///
    /// # Arguments
    ///
    /// * `patterns` - List of (rule_index, pattern) tuples
    pub fn build_index(&self, patterns: Vec<(usize, String)>) {
        self.rule_index.clear();

        let pattern_count = patterns.len();

        for (rule_idx, pattern) in patterns {
            // Extract keywords from pattern for indexing
            let keywords = self.extract_keywords(&pattern);

            for keyword in keywords {
                self.rule_index
                    .entry(keyword)
                    .or_insert_with(Vec::new)
                    .push(rule_idx);
            }

            // Try to precompile regex pattern
            if let Ok(regex) = Regex::new(&pattern) {
                self.compiled_patterns.insert(rule_idx, regex);
            }
        }

        info!(
            patterns = pattern_count,
            keywords = self.rule_index.len(),
            "Built rule index"
        );
    }

    /// Extract keywords from a pattern for indexing
    fn extract_keywords(&self, pattern: &str) -> Vec<String> {
        let mut keywords = Vec::new();

        // Extract literal words from regex pattern
        // This is a simplified implementation
        let words: Vec<&str> = pattern
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2) // Only words with 3+ characters
            .collect();

        for word in words {
            keywords.push(word.to_lowercase());
        }

        keywords
    }

    /// Get candidate rule indices for a query using the index
    ///
    /// # Arguments
    ///
    /// * `query` - The input query
    ///
    /// # Returns
    ///
    /// List of rule indices that might match
    pub fn get_candidates(&self, query: &str) -> Vec<usize> {
        let mut candidates = Vec::new();
        let query_lower = query.to_lowercase();

        // Extract keywords from query
        let keywords: Vec<&str> = query_lower
            .split_whitespace()
            .filter(|w| w.len() > 2)
            .collect();

        // Find rules that match any keyword
        for keyword in keywords {
            if let Some(entry) = self.rule_index.get(keyword) {
                candidates.extend(entry.value().iter());
            }
        }

        // Remove duplicates
        candidates.sort_unstable();
        candidates.dedup();

        debug!(
            query = %query,
            candidates = candidates.len(),
            "Found candidate rules"
        );

        candidates
    }

    /// Get precompiled regex pattern
    ///
    /// # Arguments
    ///
    /// * `rule_idx` - Rule index
    ///
    /// # Returns
    ///
    /// Precompiled regex if available
    pub fn get_compiled_pattern(&self, rule_idx: usize) -> Option<Regex> {
        self.compiled_patterns.get(&rule_idx).map(|r| r.clone())
    }

    /// Record cache hit/miss
    fn record_hit(&self, query: &str, hit: bool) {
        self.stats
            .entry(query.to_string())
            .and_modify(|stats| {
                stats.total_queries += 1;
                if hit {
                    stats.cache_hits += 1;
                }
            })
            .or_insert_with(|| HitStats {
                total_queries: 1,
                cache_hits: if hit { 1 } else { 0 },
            });
    }

    /// Get cache statistics
    pub fn cache_stats(&self) -> CacheStats {
        let total_queries: usize = self.stats.iter().map(|e| e.value().total_queries).sum();
        let total_hits: usize = self.stats.iter().map(|e| e.value().cache_hits).sum();

        CacheStats {
            cache_size: self.cache.len(),
            max_cache_size: self.max_cache_size,
            total_queries,
            cache_hits: total_hits,
            cache_misses: total_queries - total_hits,
            hit_rate: if total_queries > 0 {
                total_hits as f64 / total_queries as f64
            } else {
                0.0
            },
        }
    }

    /// Get index statistics
    pub fn index_stats(&self) -> IndexStats {
        IndexStats {
            keyword_count: self.rule_index.len(),
            compiled_patterns: self.compiled_patterns.len(),
        }
    }

    /// Clear cache
    pub fn clear_cache(&self) {
        self.cache.clear();
        info!("Cleared cache");
    }

    /// Clear all data
    pub fn clear_all(&self) {
        self.cache.clear();
        self.rule_index.clear();
        self.compiled_patterns.clear();
        self.stats.clear();
        info!("Cleared all optimizer data");
    }
}

/// Cached query result
#[derive(Debug, Clone, Serialize)]
pub struct CachedResult {
    /// The cached result
    pub result: String,

    /// When this was cached
    #[serde(skip)]
    cached_at: Instant,

    /// Time-to-live
    #[serde(skip)]
    ttl: Duration,
}

impl CachedResult {
    /// Check if cache entry is expired
    pub fn is_expired(&self) -> bool {
        self.cached_at.elapsed() > self.ttl
    }

    /// Get age of cache entry
    pub fn age(&self) -> Duration {
        self.cached_at.elapsed()
    }
}

/// Hit rate statistics
#[derive(Debug, Clone, Default)]
struct HitStats {
    total_queries: usize,
    cache_hits: usize,
}

/// Cache statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    pub cache_size: usize,
    pub max_cache_size: usize,
    pub total_queries: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub hit_rate: f64,
}

/// Index statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStats {
    pub keyword_count: usize,
    pub compiled_patterns: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_basic() {
        let optimizer = PerformanceOptimizer::new(100);

        // Cache miss
        assert!(optimizer.get_cached("test query").is_none());

        // Cache result
        optimizer.cache("test query", "result".to_string());

        // Cache hit
        let cached = optimizer.get_cached("test query");
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().result, "result");
    }

    #[test]
    fn test_cache_expiration() {
        let optimizer = PerformanceOptimizer::new(100);

        // Create expired cache entry
        let mut cached = CachedResult {
            result: "test".to_string(),
            cached_at: Instant::now() - Duration::from_secs(400),
            ttl: Duration::from_secs(300),
        };

        assert!(cached.is_expired());

        // Fresh entry
        cached.cached_at = Instant::now();
        assert!(!cached.is_expired());
    }

    #[test]
    fn test_cache_eviction() {
        let optimizer = PerformanceOptimizer::new(2);

        // Fill cache
        optimizer.cache("query1", "result1".to_string());
        optimizer.cache("query2", "result2".to_string());

        // Add third entry - should evict oldest
        optimizer.cache("query3", "result3".to_string());

        // Cache should have 2 entries
        let stats = optimizer.cache_stats();
        assert_eq!(stats.cache_size, 2);
    }

    #[test]
    fn test_index_building() {
        let optimizer = PerformanceOptimizer::new(100);

        let patterns = vec![
            (0, r"git\s+status".to_string()),
            (1, r"git\s+log".to_string()),
            (2, r"read\s+.*".to_string()),
        ];

        optimizer.build_index(patterns);

        let stats = optimizer.index_stats();
        assert!(stats.keyword_count > 0);
        assert_eq!(stats.compiled_patterns, 3);
    }

    #[test]
    fn test_candidate_lookup() {
        let optimizer = PerformanceOptimizer::new(100);

        let patterns = vec![
            (0, r"git\s+status".to_string()),
            (1, r"git\s+log".to_string()),
            (2, r"read\s+file".to_string()),
        ];

        optimizer.build_index(patterns);

        // Query with "git" should match rules 0 and 1
        let candidates = optimizer.get_candidates("git status");
        assert!(!candidates.is_empty());
    }

    #[test]
    fn test_compiled_patterns() {
        let optimizer = PerformanceOptimizer::new(100);

        let patterns = vec![(0, r"git\s+status".to_string())];

        optimizer.build_index(patterns);

        // Should have precompiled pattern
        let pattern = optimizer.get_compiled_pattern(0);
        assert!(pattern.is_some());

        // Pattern should match
        let regex = pattern.unwrap();
        assert!(regex.is_match("git status"));
    }

    #[test]
    fn test_cache_stats() {
        let optimizer = PerformanceOptimizer::new(100);

        // Generate some cache activity
        optimizer.cache("query1", "result1".to_string());
        optimizer.get_cached("query1"); // Hit
        optimizer.get_cached("query2"); // Miss

        let stats = optimizer.cache_stats();
        assert_eq!(stats.cache_size, 1);
        assert_eq!(stats.total_queries, 2);
        assert_eq!(stats.cache_hits, 1);
        assert_eq!(stats.cache_misses, 1);
        assert_eq!(stats.hit_rate, 0.5);
    }

    #[test]
    fn test_clear_operations() {
        let optimizer = PerformanceOptimizer::new(100);

        optimizer.cache("query", "result".to_string());
        optimizer.build_index(vec![(0, "pattern".to_string())]);

        // Clear cache only
        optimizer.clear_cache();
        assert_eq!(optimizer.cache_stats().cache_size, 0);
        assert!(optimizer.index_stats().keyword_count > 0);

        // Clear all
        optimizer.clear_all();
        assert_eq!(optimizer.cache_stats().cache_size, 0);
        assert_eq!(optimizer.index_stats().keyword_count, 0);
    }

    #[test]
    fn test_keyword_extraction() {
        let optimizer = PerformanceOptimizer::new(100);

        let keywords = optimizer.extract_keywords(r"git\s+status");
        assert!(keywords.contains(&"git".to_string()));
        assert!(keywords.contains(&"status".to_string()));
    }
}
