//! Dynamic model discovery for providers that support runtime model listing.
//!
//! Providers like Ollama and LM Studio expose API endpoints to list
//! available models. This module defines a trait and a caching layer with
//! a declarative fallback path — when the live `/models` call errors out,
//! the cached layer can synthesise entries from a curated list (typically
//! `ProviderPreset.fallback_models`) so callers always get *something*.

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

impl DiscoveredModel {
    /// Build a minimal entry from a model id only.
    ///
    /// Used by [`CachedDiscovery`]'s fallback path when the live discovery
    /// call errors out and we substitute `ProviderPreset.fallback_models`.
    pub fn from_id(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            display_name: None,
            size_bytes: None,
            modified_at: None,
        }
    }
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
///
/// Caches results for a configurable duration to avoid frequent API calls.
/// Uses `tokio::sync::RwLock` for async-safe shared reads when cache is warm.
///
/// When `fallback` is non-empty and the inner [`ModelDiscovery::discover_models`]
/// call returns an error, the wrapper substitutes synthetic entries built
/// from each fallback model id, still writing them to cache so subsequent
/// reads hit fast paths. This matches openclaw's
/// `catalog.run + staticCatalog.run` ordered-fallback semantics.
pub struct CachedDiscovery {
    inner: Box<dyn ModelDiscovery>,
    cache: RwLock<Option<(Vec<DiscoveredModel>, Instant)>>,
    ttl: Duration,
    fallback: &'static [&'static str],
}

impl CachedDiscovery {
    /// Build a cache without any fallback.
    ///
    /// Equivalent to legacy behaviour: discovery failures propagate as errors.
    #[must_use]
    pub fn new(inner: Box<dyn ModelDiscovery>, ttl: Duration) -> Self {
        Self {
            inner,
            cache: RwLock::new(None),
            ttl,
            fallback: &[],
        }
    }

    /// Attach a static fallback list (typically `preset.fallback_models`).
    pub const fn with_fallback(mut self, fallback: &'static [&'static str]) -> Self {
        self.fallback = fallback;
        self
    }

    /// Convenience constructor that wires fallback from a preset name.
    ///
    /// Returns the same `CachedDiscovery` shape as [`Self::new`] but with
    /// `preset.fallback_models` already attached. Unknown preset names
    /// silently keep an empty fallback (matches legacy `new`).
    #[must_use]
    pub fn from_preset(inner: Box<dyn ModelDiscovery>, preset_name: &str, ttl: Duration) -> Self {
        let fallback = crate::providers::presets::get_preset(preset_name)
            .map(|p| p.fallback_models)
            .unwrap_or(&[]);
        Self {
            inner,
            cache: RwLock::new(None),
            ttl,
            fallback,
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

        // Fetch fresh; on error fall back to the static list when one exists.
        let models = match self.inner.discover_models().await {
            Ok(models) => models,
            Err(err) => {
                if self.fallback.is_empty() {
                    return Err(err);
                }
                tracing::warn!(
                    provider = self.inner.provider_name(),
                    error = %err,
                    fallback_count = self.fallback.len(),
                    "model discovery failed; using preset fallback list"
                );
                self.fallback
                    .iter()
                    .map(|id| DiscoveredModel::from_id(*id))
                    .collect()
            }
        };

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

    /// Always errors — used to exercise the fallback path.
    struct FailingDiscovery;

    #[async_trait]
    impl ModelDiscovery for FailingDiscovery {
        fn provider_name(&self) -> &str {
            "failing"
        }
        async fn discover_models(&self) -> anyhow::Result<Vec<DiscoveredModel>> {
            Err(anyhow::anyhow!("network unreachable"))
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

    #[tokio::test]
    async fn falls_back_to_preset_models_on_failure() {
        // With no fallback, the error propagates.
        let bare = CachedDiscovery::new(Box::new(FailingDiscovery), Duration::from_secs(300));
        assert!(bare.discover().await.is_err());

        // With a fallback list, we synthesize entries.
        let with_fb = CachedDiscovery::new(Box::new(FailingDiscovery), Duration::from_secs(300))
            .with_fallback(&["model-a", "model-b"]);
        let models = with_fb.discover().await.unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "model-a");
        assert_eq!(models[1].id, "model-b");

        // Second call must hit cache (no further failure).
        let again = with_fb.discover().await.unwrap();
        assert_eq!(again.len(), 2);
    }

    #[tokio::test]
    async fn from_preset_inherits_fallback_list() {
        // `openai` preset declares `gpt-5.6, gpt-5.5, gpt-5.4-mini, o4-mini`.
        let cached = CachedDiscovery::from_preset(
            Box::new(FailingDiscovery),
            "openai",
            Duration::from_secs(300),
        );
        let models = cached.discover().await.unwrap();
        assert!(models.iter().any(|m| m.id == "gpt-5.6"));
        assert!(models.iter().any(|m| m.id == "gpt-5.4-mini"));
    }

    #[tokio::test]
    async fn from_preset_unknown_name_keeps_error_path() {
        let cached = CachedDiscovery::from_preset(
            Box::new(FailingDiscovery),
            "definitely-not-a-real-preset",
            Duration::from_secs(300),
        );
        assert!(cached.discover().await.is_err());
    }
}
