//! Dynamic model discovery for providers that support runtime model listing.
//!
//! Providers like Ollama and LM Studio expose API endpoints to list
//! available models. This module defines a trait and caching layer.

use async_trait::async_trait;
use serde::Serialize;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// A model discovered at runtime from a provider's API.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredModel {
    pub id: String,
    pub display_name: Option<String>,
    pub size_bytes: Option<u64>,
    pub modified_at: Option<String>,
}

/// Trait for providers that support runtime model listing.
#[async_trait]
pub trait ModelDiscovery: Send + Sync {
    /// Provider name for this discovery source.
    fn provider_name(&self) -> &str;

    /// Fetch available models from the provider's API.
    async fn discover_models(&self) -> anyhow::Result<Vec<DiscoveredModel>>;
}

/// Cached model discovery wrapper.
/// Caches results for a configurable duration to avoid frequent API calls.
/// Uses `tokio::sync::RwLock` for async-safe shared reads when cache is warm.
pub struct CachedDiscovery {
    inner: Box<dyn ModelDiscovery>,
    cache: RwLock<Option<(Vec<DiscoveredModel>, Instant)>>,
    ttl: Duration,
}

impl CachedDiscovery {
    pub fn new(inner: Box<dyn ModelDiscovery>, ttl: Duration) -> Self {
        Self {
            inner,
            cache: RwLock::new(None),
            ttl,
        }
    }

    pub async fn discover(&self) -> anyhow::Result<Vec<DiscoveredModel>> {
        // Check cache (shared read — no contention when warm)
        {
            let cache = self.cache.read().await;
            if let Some((models, fetched_at)) = cache.as_ref() {
                if fetched_at.elapsed() < self.ttl {
                    return Ok(models.clone());
                }
            }
        }

        // Fetch fresh
        let models = self.inner.discover_models().await?;

        // Update cache (exclusive write)
        {
            let mut cache = self.cache.write().await;
            *cache = Some((models.clone(), Instant::now()));
        }

        Ok(models)
    }

    pub fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockDiscovery;

    #[async_trait]
    impl ModelDiscovery for MockDiscovery {
        fn provider_name(&self) -> &str {
            "mock"
        }
        async fn discover_models(&self) -> anyhow::Result<Vec<DiscoveredModel>> {
            Ok(vec![DiscoveredModel {
                id: "test-model".to_string(),
                display_name: Some("Test Model".to_string()),
                size_bytes: Some(1_000_000),
                modified_at: None,
            }])
        }
    }

    #[tokio::test]
    async fn test_cached_discovery() {
        let cached = CachedDiscovery::new(Box::new(MockDiscovery), Duration::from_secs(300));

        let models = cached.discover().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "test-model");

        // Second call should use cache
        let models2 = cached.discover().await.unwrap();
        assert_eq!(models2.len(), 1);
    }

    #[tokio::test]
    async fn test_cached_discovery_provider_name() {
        let cached = CachedDiscovery::new(Box::new(MockDiscovery), Duration::from_secs(300));
        assert_eq!(cached.provider_name(), "mock");
    }
}
