//! Model registry with caching and preset fallback
//!
//! Provides centralized model discovery with three-layer resolution:
//! 1. API probe (via ProtocolAdapter::list_models)
//! 2. Preset fallback (from model-presets.toml)
//! 3. Empty list (caller handles manual input)

use crate::config::ProviderConfig;
use crate::providers::adapter::{DiscoveredModel, ProtocolAdapter};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::warn;

/// Default cache TTL: 24 hours
const DEFAULT_TTL_SECS: u64 = 86400;

/// Embed preset TOML at compile time — always available regardless of CWD
const PRESET_TOML: &str = include_str!("../../../shared/config/model-presets.toml");

/// Global model registry instance
pub static MODEL_REGISTRY: Lazy<ModelRegistry> = Lazy::new(|| {
    ModelRegistry::new(Some(PRESET_TOML))
});

/// Source of a cached model list
#[derive(Debug, Clone, PartialEq)]
pub enum ModelSource {
    /// Fetched from provider API
    Api,
    /// Loaded from preset file
    Preset,
}

/// Cached model list for a single provider
#[derive(Debug, Clone)]
struct CachedModelList {
    models: Vec<DiscoveredModel>,
    source: ModelSource,
    fetched_at: Instant,
}

/// Preset file structure
#[derive(Debug, Deserialize)]
struct PresetFile {
    #[serde(flatten)]
    protocols: HashMap<String, PresetProtocol>,
}

/// Per-protocol preset entry
#[derive(Debug, Deserialize)]
struct PresetProtocol {
    models: Vec<DiscoveredModel>,
}

/// Model registry with caching and preset fallback
pub struct ModelRegistry {
    cache: RwLock<HashMap<String, CachedModelList>>,
    presets: HashMap<String, Vec<DiscoveredModel>>,
    ttl: Duration,
}

impl ModelRegistry {
    /// Create a new registry with presets loaded from the given TOML string
    pub fn new(preset_toml: Option<&str>) -> Self {
        let presets = preset_toml
            .and_then(|toml_str| {
                toml::from_str::<PresetFile>(toml_str)
                    .map_err(|e| {
                        warn!("Failed to parse model presets: {}", e);
                        e
                    })
                    .ok()
            })
            .map(|file| {
                file.protocols
                    .into_iter()
                    .map(|(k, v)| (k, v.models))
                    .collect()
            })
            .unwrap_or_default();

        Self {
            cache: RwLock::new(HashMap::new()),
            presets,
            ttl: Duration::from_secs(DEFAULT_TTL_SECS),
        }
    }

    /// Get available models for a provider (with caching)
    ///
    /// Resolution order:
    /// 1. Return cached if valid
    /// 2. Try API probe via adapter.list_models()
    /// 3. Fall back to presets
    /// 4. Return empty list
    pub async fn list_models(
        &self,
        provider_name: &str,
        protocol: &str,
        adapter: &dyn ProtocolAdapter,
        config: &ProviderConfig,
    ) -> Vec<DiscoveredModel> {
        // Check cache
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(provider_name) {
                if cached.fetched_at.elapsed() < self.ttl {
                    return cached.models.clone();
                }
            }
        }

        // Cache miss or expired — try API probe
        self.refresh_inner(provider_name, protocol, adapter, config).await
    }

    /// Force refresh a provider's model list
    pub async fn refresh(
        &self,
        provider_name: &str,
        protocol: &str,
        adapter: &dyn ProtocolAdapter,
        config: &ProviderConfig,
    ) -> Vec<DiscoveredModel> {
        self.refresh_inner(provider_name, protocol, adapter, config).await
    }

    /// Internal refresh logic
    async fn refresh_inner(
        &self,
        provider_name: &str,
        protocol: &str,
        adapter: &dyn ProtocolAdapter,
        config: &ProviderConfig,
    ) -> Vec<DiscoveredModel> {
        // Try API probe
        let (models, source) = match adapter.list_models(config).await {
            Ok(Some(models)) if !models.is_empty() => (models, ModelSource::Api),
            Ok(_) => {
                // API returned None or empty — use presets
                match self.presets.get(protocol) {
                    Some(preset_models) => (preset_models.clone(), ModelSource::Preset),
                    None => (vec![], ModelSource::Preset),
                }
            }
            Err(e) => {
                warn!("API probe failed for {}: {}, falling back to presets", provider_name, e);
                match self.presets.get(protocol) {
                    Some(preset_models) => (preset_models.clone(), ModelSource::Preset),
                    None => (vec![], ModelSource::Preset),
                }
            }
        };

        // Update cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(
                provider_name.to_string(),
                CachedModelList {
                    models: models.clone(),
                    source,
                    fetched_at: Instant::now(),
                },
            );
        }

        models
    }

    /// Get the source of a cached entry
    pub async fn get_source(&self, provider_name: &str) -> Option<ModelSource> {
        let cache = self.cache.read().await;
        cache.get(provider_name).map(|c| c.source.clone())
    }

    /// Get the last refresh time for a provider
    pub async fn last_refreshed(&self, provider_name: &str) -> Option<Instant> {
        let cache = self.cache.read().await;
        cache.get(provider_name).map(|c| c.fetched_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::error::Result;
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use futures::stream::BoxStream;

    /// Mock adapter that returns a fixed model list
    struct MockAdapter {
        models: Option<Vec<DiscoveredModel>>,
    }

    #[async_trait]
    impl ProtocolAdapter for MockAdapter {
        fn build_request(
            &self,
            _payload: &RequestPayload,
            _config: &ProviderConfig,
            _is_streaming: bool,
        ) -> Result<reqwest::RequestBuilder> {
            unimplemented!()
        }

        async fn parse_response(&self, _response: reqwest::Response) -> Result<ProviderResponse> {
            unimplemented!()
        }

        async fn parse_stream(
            &self,
            _response: reqwest::Response,
        ) -> Result<BoxStream<'static, Result<String>>> {
            unimplemented!()
        }

        fn name(&self) -> &'static str {
            "mock"
        }

        async fn list_models(&self, _config: &ProviderConfig) -> Result<Option<Vec<DiscoveredModel>>> {
            Ok(self.models.clone())
        }
    }

    fn test_config() -> ProviderConfig {
        ProviderConfig::test_config("test-model")
    }

    #[tokio::test]
    async fn test_preset_loading() {
        let toml = r#"
[anthropic]
models = [
    { id = "claude-sonnet", name = "Claude Sonnet", capabilities = ["chat"] },
]
"#;
        let registry = ModelRegistry::new(Some(toml));
        assert_eq!(registry.presets.len(), 1);
        assert_eq!(registry.presets["anthropic"][0].id, "claude-sonnet");
    }

    #[tokio::test]
    async fn test_api_probe_success() {
        let adapter = MockAdapter {
            models: Some(vec![DiscoveredModel {
                id: "gpt-4o".to_string(),
                name: Some("GPT-4o".to_string()),
                owned_by: None,
                capabilities: vec!["chat".to_string()],
            }]),
        };

        let registry = ModelRegistry::new(None);
        let models = registry
            .list_models("openai", "openai", &adapter, &test_config())
            .await;

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-4o");
        assert_eq!(registry.get_source("openai").await, Some(ModelSource::Api));
    }

    #[tokio::test]
    async fn test_fallback_to_presets() {
        let adapter = MockAdapter { models: None }; // API returns None

        let toml = r#"
[openai]
models = [
    { id = "gpt-4o", name = "GPT-4o", capabilities = ["chat"] },
    { id = "gpt-4o-mini", name = "GPT-4o Mini", capabilities = ["chat"] },
]
"#;
        let registry = ModelRegistry::new(Some(toml));
        let models = registry
            .list_models("my-openai", "openai", &adapter, &test_config())
            .await;

        assert_eq!(models.len(), 2);
        assert_eq!(registry.get_source("my-openai").await, Some(ModelSource::Preset));
    }

    #[tokio::test]
    async fn test_cache_hit() {
        let adapter = MockAdapter {
            models: Some(vec![DiscoveredModel {
                id: "model-1".to_string(),
                name: None,
                owned_by: None,
                capabilities: vec!["chat".to_string()],
            }]),
        };

        let registry = ModelRegistry::new(None);

        // First call — populates cache
        let models1 = registry
            .list_models("provider", "openai", &adapter, &test_config())
            .await;
        assert_eq!(models1.len(), 1);

        // Second call — should return cached
        let models2 = registry
            .list_models("provider", "openai", &adapter, &test_config())
            .await;
        assert_eq!(models2.len(), 1);
        assert_eq!(models2[0].id, "model-1");
    }

    #[tokio::test]
    async fn test_force_refresh() {
        let adapter = MockAdapter {
            models: Some(vec![DiscoveredModel {
                id: "model-v2".to_string(),
                name: None,
                owned_by: None,
                capabilities: vec!["chat".to_string()],
            }]),
        };

        let registry = ModelRegistry::new(None);
        let models = registry
            .refresh("provider", "openai", &adapter, &test_config())
            .await;

        assert_eq!(models[0].id, "model-v2");
    }

    #[tokio::test]
    async fn test_empty_when_no_presets_no_api() {
        let adapter = MockAdapter { models: None };
        let registry = ModelRegistry::new(None);

        let models = registry
            .list_models("unknown", "unknown", &adapter, &test_config())
            .await;
        assert!(models.is_empty());
    }

    #[test]
    fn test_invalid_toml_gracefully_handled() {
        let registry = ModelRegistry::new(Some("invalid { toml"));
        assert!(registry.presets.is_empty());
    }

    #[tokio::test]
    async fn test_embedded_presets_load() {
        // Test that the compile-time embedded presets parse correctly
        let registry = ModelRegistry::new(Some(PRESET_TOML));
        assert!(registry.presets.contains_key("anthropic"));
        assert!(registry.presets.contains_key("openai"));
        assert!(registry.presets.contains_key("gemini"));
        assert!(!registry.presets["anthropic"].is_empty());
    }
}
