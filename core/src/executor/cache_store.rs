//! Tool result cache store with LRU eviction

use lru::LruCache;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

use crate::agent_loop::ActionResult;

use super::cache_config::ToolCacheConfig;

/// Cache key for tool calls
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct ToolCallCacheKey {
    tool_name: String,
    args_hash: u64,
}

impl ToolCallCacheKey {
    fn new(tool_name: String, arguments: &serde_json::Value) -> Self {
        let args_str = serde_json::to_string(arguments).unwrap_or_default();
        let mut hasher = DefaultHasher::new();
        args_str.hash(&mut hasher);

        Self {
            tool_name,
            args_hash: hasher.finish(),
        }
    }
}

/// Cached tool result with metadata
#[derive(Debug, Clone)]
struct CachedToolResult {
    result: ActionResult,
    cached_at: Instant,
    hit_count: u32,
}

/// LRU cache store for tool results
pub struct ToolResultCache {
    cache: Arc<RwLock<LruCache<ToolCallCacheKey, CachedToolResult>>>,
    config: ToolCacheConfig,
}

impl ToolResultCache {
    /// Create a new cache store
    pub fn new(config: ToolCacheConfig) -> Self {
        let capacity = NonZeroUsize::new(config.capacity)
            .unwrap_or(NonZeroUsize::new(100).unwrap());
        let cache = Arc::new(RwLock::new(LruCache::new(capacity)));

        Self { cache, config }
    }

    /// Lookup a tool call result in cache
    pub async fn lookup(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Option<ActionResult> {
        if !self.config.should_cache(tool_name) {
            return None;
        }

        let key = ToolCallCacheKey::new(tool_name.to_string(), arguments);
        let mut cache = self.cache.write().await;

        if let Some(cached) = cache.get_mut(&key) {
            // Check TTL
            if cached.cached_at.elapsed() > self.config.ttl() {
                cache.pop(&key); // Expired
                return None;
            }

            // Update hit count
            cached.hit_count += 1;

            tracing::debug!(
                tool_name = tool_name,
                hit_count = cached.hit_count,
                age_secs = cached.cached_at.elapsed().as_secs(),
                "Tool result cache HIT"
            );

            return Some(cached.result.clone());
        }

        tracing::debug!(tool_name = tool_name, "Tool result cache MISS");
        None
    }

    /// Store a tool call result in cache
    pub async fn store(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
        result: &ActionResult,
    ) {
        if !self.config.should_cache(tool_name) {
            return;
        }

        // Only cache successful results if configured
        if self.config.cache_only_success && !result.is_success() {
            return;
        }

        let key = ToolCallCacheKey::new(tool_name.to_string(), arguments);
        let cached = CachedToolResult {
            result: result.clone(),
            cached_at: Instant::now(),
            hit_count: 0,
        };

        let mut cache = self.cache.write().await;
        cache.put(key, cached);

        tracing::debug!(
            tool_name = tool_name,
            cache_size = cache.len(),
            "Tool result cached"
        );
    }

    /// Clear all cache entries
    pub async fn clear(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    /// Get cache statistics
    pub async fn stats(&self) -> CacheStats {
        let cache = self.cache.read().await;
        let entries: Vec<_> = cache.iter().collect();

        let total_hits: u32 = entries.iter().map(|(_, v)| v.hit_count).sum();

        CacheStats {
            size: cache.len(),
            capacity: cache.cap().get(),
            total_hits,
        }
    }
}

#[derive(Debug)]
pub struct CacheStats {
    pub size: usize,
    pub capacity: usize,
    pub total_hits: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_hit() {
        let config = ToolCacheConfig::default();
        let cache = ToolResultCache::new(config);

        let tool_name = "file_ops";
        let args = serde_json::json!({"path": "/test.txt", "operation": "read"});
        let result = ActionResult::ToolSuccess {
            output: serde_json::json!("file content"),
            duration_ms: 100,
        };

        // Store
        cache.store(tool_name, &args, &result).await;

        // Lookup - should hit
        let cached = cache.lookup(tool_name, &args).await;
        assert!(cached.is_some());
        assert!(matches!(
            cached.unwrap(),
            ActionResult::ToolSuccess { .. }
        ));
    }

    #[tokio::test]
    async fn test_cache_miss_different_args() {
        let config = ToolCacheConfig::default();
        let cache = ToolResultCache::new(config);

        let tool_name = "file_ops";
        let args1 = serde_json::json!({"path": "/test1.txt"});
        let args2 = serde_json::json!({"path": "/test2.txt"});
        let result = ActionResult::ToolSuccess {
            output: serde_json::json!("content"),
            duration_ms: 100,
        };

        // Store with args1
        cache.store(tool_name, &args1, &result).await;

        // Lookup with args2 - should miss
        let cached = cache.lookup(tool_name, &args2).await;
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn test_ttl_expiration() {
        let mut config = ToolCacheConfig::default();
        config.ttl_seconds = 1; // 1 second TTL
        let cache = ToolResultCache::new(config);

        let tool_name = "file_ops";
        let args = serde_json::json!({"path": "/test.txt"});
        let result = ActionResult::ToolSuccess {
            output: serde_json::json!("content"),
            duration_ms: 100,
        };

        // Store
        cache.store(tool_name, &args, &result).await;

        // Wait for expiration
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        // Lookup - should miss due to TTL
        let cached = cache.lookup(tool_name, &args).await;
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn test_exclude_tools() {
        let mut config = ToolCacheConfig::default();
        config.exclude_tools = vec!["bash".to_string()];
        let cache = ToolResultCache::new(config);

        let tool_name = "bash";
        let args = serde_json::json!({"command": "ls"});
        let result = ActionResult::ToolSuccess {
            output: serde_json::json!("output"),
            duration_ms: 100,
        };

        // Store - should not cache
        cache.store(tool_name, &args, &result).await;

        // Lookup - should miss (not cached)
        let cached = cache.lookup(tool_name, &args).await;
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn test_cache_only_success() {
        let config = ToolCacheConfig::default();
        let cache = ToolResultCache::new(config);

        let tool_name = "file_ops";
        let args = serde_json::json!({"path": "/test.txt"});
        let result = ActionResult::ToolError {
            error: "File not found".to_string(),
            retryable: false,
        };

        // Store error result - should not cache
        cache.store(tool_name, &args, &result).await;

        // Lookup - should miss (not cached)
        let cached = cache.lookup(tool_name, &args).await;
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn test_cache_stats() {
        let config = ToolCacheConfig::default();
        let cache = ToolResultCache::new(config);

        let tool_name = "file_ops";
        let args = serde_json::json!({"path": "/test.txt"});
        let result = ActionResult::ToolSuccess {
            output: serde_json::json!("content"),
            duration_ms: 100,
        };

        // Store
        cache.store(tool_name, &args, &result).await;

        // Lookup multiple times
        cache.lookup(tool_name, &args).await;
        cache.lookup(tool_name, &args).await;

        let stats = cache.stats().await;
        assert_eq!(stats.size, 1);
        assert_eq!(stats.capacity, 100);
        assert_eq!(stats.total_hits, 2);
    }
}
