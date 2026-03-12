# Model Discovery Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace manual model string entry with automatic API-driven model discovery, cached with 24h TTL, with preset TOML fallback.

**Architecture:** Extend `ProtocolAdapter` trait with `list_models()` default method. New `ModelRegistry` singleton (static Lazy) manages per-provider cache + preset fallback. RPC handlers query `ModelRegistry` instead of reading config directly.

**Tech Stack:** Rust, tokio, reqwest, serde, TOML, async_trait

**Spec:** `docs/superpowers/specs/2026-03-12-model-discovery-design.md`

---

## File Structure

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `core/src/providers/adapter.rs` | Add `DiscoveredModel` struct + `list_models()` default method to `ProtocolAdapter` |
| New | `core/src/providers/model_registry.rs` | `ModelRegistry` cache service: TTL cache, preset loading, aggregation |
| New | `shared/config/model-presets.toml` | Preset model lists per protocol (Anthropic, OpenAI fallback, Gemini fallback) |
| Modify | `core/src/providers/protocols/openai.rs` | Implement `list_models()` via `GET /v1/models` |
| Modify | `core/src/providers/protocols/gemini.rs` | Implement `list_models()` via `GET /v1beta/models` |
| Modify | `core/src/providers/ollama.rs` | Add `list_models()` method via `GET /api/tags` |
| Modify | `core/src/providers/mod.rs` | Re-export `model_registry` module |
| Modify | `core/src/gateway/handlers/models.rs` | Rewrite RPC handlers to use `ModelRegistry`; add `models.refresh`, `models.set_default`, `models.set_model` |
| Modify | `core/src/gateway/handlers/mod.rs` | Register new RPC methods |

---

## Notes

- `AvailableModel` struct and `all_available_models()` from the spec are deferred — they are for Agent model routing which is out of scope.
- `models.set_default` and `models.set_model` require `Arc<tokio::sync::RwLock<Config>>` — same pattern as `providers.setDefault` in `providers.rs`. These handlers are defined but NOT registered in `HandlerRegistry::new()` — they'll be wired by Gateway startup code (same as other mutable-config handlers). Task 9 registers only `models.refresh` (read-only config).

---

## Chunk 1: Core Types and ModelRegistry

### Task 1: Add `DiscoveredModel` and `list_models()` to ProtocolAdapter

**Files:**
- Modify: `core/src/providers/adapter.rs`

- [ ] **Step 1: Write the test for `DiscoveredModel` serialization**

Add to the existing `#[cfg(test)] mod tests` at the bottom of `adapter.rs`:

```rust
#[test]
fn test_discovered_model_serialize() {
    let model = DiscoveredModel {
        id: "gpt-4o".to_string(),
        name: Some("GPT-4o".to_string()),
        owned_by: Some("openai".to_string()),
        capabilities: vec!["chat".to_string(), "vision".to_string()],
    };
    let json = serde_json::to_value(&model).unwrap();
    assert_eq!(json["id"], "gpt-4o");
    assert_eq!(json["name"], "GPT-4o");
    assert_eq!(json["capabilities"].as_array().unwrap().len(), 2);
}

#[test]
fn test_discovered_model_default_capabilities() {
    let model = DiscoveredModel {
        id: "some-model".to_string(),
        name: None,
        owned_by: None,
        capabilities: vec!["chat".to_string()],
    };
    assert_eq!(model.capabilities, vec!["chat"]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib adapter::tests::test_discovered_model`
Expected: FAIL — `DiscoveredModel` not defined

- [ ] **Step 3: Add `DiscoveredModel` struct and `list_models()` default method**

Add after the `TokenUsage` struct (before `#[cfg(test)]`):

```rust
/// A model discovered via API probe or preset list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredModel {
    /// Model ID (e.g., "gpt-4o")
    pub id: String,
    /// Display name (e.g., "GPT-4o")
    pub name: Option<String>,
    /// Owner/organization
    pub owned_by: Option<String>,
    /// Capabilities: "chat", "vision", "tools", "thinking"
    pub capabilities: Vec<String>,
}
```

Add to the `ProtocolAdapter` trait, after the `name()` method:

```rust
    /// Fetch available models from the provider API.
    /// Returns None if the protocol does not support model listing.
    async fn list_models(&self, _config: &ProviderConfig) -> Result<Option<Vec<DiscoveredModel>>> {
        Ok(None)
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib adapter::tests::test_discovered_model`
Expected: PASS

- [ ] **Step 5: Run full compile check**

Run: `cargo check -p alephcore`
Expected: SUCCESS — default method means no existing impl needs changes

- [ ] **Step 6: Commit**

```bash
git add core/src/providers/adapter.rs
git commit -m "model-discovery: add DiscoveredModel and list_models() to ProtocolAdapter"
```

---

### Task 2: Create preset TOML file

**Files:**
- Create: `shared/config/model-presets.toml`

- [ ] **Step 1: Create the preset file**

```toml
# Model Presets
#
# Fallback model lists for providers that don't support API-based model listing
# or when API probe fails. Protocol keys must match ProviderConfig.protocol() values.

[anthropic]
models = [
    { id = "claude-opus-4-20250514", name = "Claude Opus 4", capabilities = ["chat", "vision", "tools", "thinking"] },
    { id = "claude-sonnet-4-20250514", name = "Claude Sonnet 4", capabilities = ["chat", "vision", "tools", "thinking"] },
    { id = "claude-haiku-4-20250506", name = "Claude Haiku 4", capabilities = ["chat", "vision", "tools"] },
]

[openai]
# API probe takes priority; this is fallback for network failures
models = [
    { id = "gpt-4o", name = "GPT-4o", capabilities = ["chat", "vision", "tools"] },
    { id = "gpt-4o-mini", name = "GPT-4o Mini", capabilities = ["chat", "vision", "tools"] },
    { id = "o3", name = "O3", capabilities = ["chat", "tools", "thinking"] },
    { id = "o3-mini", name = "O3 Mini", capabilities = ["chat", "tools", "thinking"] },
]

[gemini]
models = [
    { id = "gemini-2.5-pro", name = "Gemini 2.5 Pro", capabilities = ["chat", "vision", "tools", "thinking"] },
    { id = "gemini-2.5-flash", name = "Gemini 2.5 Flash", capabilities = ["chat", "vision", "tools", "thinking"] },
]

# Ollama: no presets — models depend on local installation
```

- [ ] **Step 2: Commit**

```bash
git add shared/config/model-presets.toml
git commit -m "model-discovery: add model preset TOML file"
```

---

### Task 3: Create `ModelRegistry` with preset loading and cache

**Files:**
- Create: `core/src/providers/model_registry.rs`
- Modify: `core/src/providers/mod.rs`

- [ ] **Step 1: Write tests for `ModelRegistry`**

Create `core/src/providers/model_registry.rs` with test module first:

```rust
//! Model registry with caching and preset fallback
//!
//! Provides centralized model discovery with three-layer resolution:
//! 1. API probe (via ProtocolAdapter::list_models)
//! 2. Preset fallback (from model-presets.toml)
//! 3. Empty list (caller handles manual input)

use crate::config::ProviderConfig;
use crate::error::Result;
use crate::providers::adapter::{DiscoveredModel, ProtocolAdapter};
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::warn;

/// Default cache TTL: 24 hours
const DEFAULT_TTL_SECS: u64 = 86400;

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

        // Second call — should return cached (even if adapter changes underneath)
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
}
```

- [ ] **Step 2: Add module to `mod.rs`**

In `core/src/providers/mod.rs`, add after `pub mod presets;` (line 70):

```rust
pub mod model_registry;
```

And add re-export after line 96 (`pub use presets::{...};`):

```rust
pub use model_registry::ModelRegistry;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib model_registry::tests`
Expected: ALL PASS

- [ ] **Step 4: Commit**

```bash
git add core/src/providers/model_registry.rs core/src/providers/mod.rs
git commit -m "model-discovery: add ModelRegistry with cache and preset fallback"
```

---

## Chunk 2: Protocol Implementations

### Task 4: Implement `list_models()` for OpenAI protocol

**Files:**
- Modify: `core/src/providers/protocols/openai.rs`

- [ ] **Step 1: Add the `list_models` implementation**

Add this method to the `#[async_trait] impl ProtocolAdapter for OpenAiProtocol` block, after the `name()` method:

```rust
    async fn list_models(&self, config: &ProviderConfig) -> Result<Option<Vec<DiscoveredModel>>> {
        let base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

        // Normalize: ensure we have /v1 suffix for the models endpoint
        let url = if base_url.ends_with("/v1") {
            format!("{}/models", base_url)
        } else {
            format!("{}/v1/models", base_url)
        };

        let api_key = config.api_key.as_deref().unwrap_or("");

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| AlephError::network(format!("Model list request failed: {}", e)))?;

        if !response.status().is_success() {
            return Ok(None);
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AlephError::network(format!("Failed to parse model list: {}", e)))?;

        let models = body["data"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let id = m["id"].as_str()?;
                        Some(DiscoveredModel {
                            id: id.to_string(),
                            name: Some(id.to_string()),
                            owned_by: m["owned_by"].as_str().map(|s| s.to_string()),
                            capabilities: vec!["chat".to_string()],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(Some(models))
    }
```

Add the import at the top of the file (with existing imports from adapter):

```rust
use crate::providers::adapter::DiscoveredModel;
```

- [ ] **Step 2: Run compile check**

Run: `cargo check -p alephcore`
Expected: SUCCESS

- [ ] **Step 3: Commit**

```bash
git add core/src/providers/protocols/openai.rs
git commit -m "model-discovery: implement list_models for OpenAI protocol"
```

---

### Task 5: Implement `list_models()` for Gemini protocol

**Files:**
- Modify: `core/src/providers/protocols/gemini.rs`

- [ ] **Step 1: Add the `list_models` implementation**

Add to the `#[async_trait] impl ProtocolAdapter for GeminiProtocol` block, after the `name()` method:

```rust
    async fn list_models(&self, config: &ProviderConfig) -> Result<Option<Vec<DiscoveredModel>>> {
        let base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string());

        let api_key = config.api_key.as_deref().unwrap_or("");
        let url = format!("{}/v1beta/models?key={}", base_url, api_key);

        let response = self
            .client
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| AlephError::network(format!("Gemini model list request failed: {}", e)))?;

        if !response.status().is_success() {
            return Ok(None);
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AlephError::network(format!("Failed to parse Gemini model list: {}", e)))?;

        let models = body["models"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        // Gemini returns "models/gemini-1.5-pro" format
                        let full_name = m["name"].as_str()?;
                        let id = full_name.strip_prefix("models/").unwrap_or(full_name);
                        let display_name = m["displayName"].as_str().map(|s| s.to_string());

                        Some(DiscoveredModel {
                            id: id.to_string(),
                            name: display_name,
                            owned_by: Some("google".to_string()),
                            capabilities: vec!["chat".to_string()],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(Some(models))
    }
```

Add the import at the top:

```rust
use crate::providers::adapter::DiscoveredModel;
```

- [ ] **Step 2: Run compile check**

Run: `cargo check -p alephcore`
Expected: SUCCESS

- [ ] **Step 3: Commit**

```bash
git add core/src/providers/protocols/gemini.rs
git commit -m "model-discovery: implement list_models for Gemini protocol"
```

---

### Task 6: Add `list_models()` to `OllamaProvider`

**Files:**
- Modify: `core/src/providers/ollama.rs`

- [ ] **Step 1: Add Ollama tags response types and `list_models` method**

Add the response type near the other response structs (after `OllamaError`):

```rust
/// Response from Ollama /api/tags endpoint
#[derive(Debug, Deserialize)]
struct TagsResponse {
    models: Vec<OllamaModelInfo>,
}

/// Model info from Ollama tags
#[derive(Debug, Deserialize)]
struct OllamaModelInfo {
    name: String,
}
```

Add the `list_models` method to `impl OllamaProvider` (not the `AiProvider` impl — just the inherent impl):

```rust
    /// Fetch available models from the local Ollama server
    pub async fn list_models(&self) -> Result<Vec<DiscoveredModel>> {
        let url = format!("{}/api/tags", self.endpoint);

        let response = self
            .client
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| AlephError::network(format!("Ollama tags request failed: {}", e)))?;

        if !response.status().is_success() {
            return Ok(vec![]);
        }

        let tags: TagsResponse = response
            .json()
            .await
            .map_err(|e| AlephError::network(format!("Failed to parse Ollama tags: {}", e)))?;

        let models = tags
            .models
            .into_iter()
            .map(|m| DiscoveredModel {
                id: m.name.clone(),
                name: Some(m.name),
                owned_by: Some("local".to_string()),
                capabilities: vec!["chat".to_string()],
            })
            .collect();

        Ok(models)
    }
```

Add the import at the top:

```rust
use crate::providers::adapter::DiscoveredModel;
```

- [ ] **Step 2: Run compile check**

Run: `cargo check -p alephcore`
Expected: SUCCESS

- [ ] **Step 3: Commit**

```bash
git add core/src/providers/ollama.rs
git commit -m "model-discovery: add list_models to OllamaProvider"
```

---

## Chunk 3: RPC Handler Integration

### Task 7: Create global `ModelRegistry` singleton

**Files:**
- Modify: `core/src/providers/model_registry.rs`

- [ ] **Step 1: Add the global singleton**

Add at the top of the file, after the imports:

```rust
use once_cell::sync::Lazy;

/// Embed preset TOML at compile time — always available regardless of CWD
const PRESET_TOML: &str = include_str!("../../../shared/config/model-presets.toml");

/// Global model registry instance
pub static MODEL_REGISTRY: Lazy<ModelRegistry> = Lazy::new(|| {
    ModelRegistry::new(Some(PRESET_TOML))
});
```

Note: `include_str!` path is relative to the source file (`core/src/providers/model_registry.rs`), so `../../../shared/config/model-presets.toml` resolves to the project root's `shared/config/` directory. This ensures presets are always available regardless of working directory at runtime.

- [ ] **Step 2: Run compile check**

Run: `cargo check -p alephcore`
Expected: SUCCESS

- [ ] **Step 3: Commit**

```bash
git add core/src/providers/model_registry.rs
git commit -m "model-discovery: add MODEL_REGISTRY global singleton"
```

---

### Task 8: Rewrite RPC handlers to use `ModelRegistry`

**Files:**
- Modify: `core/src/gateway/handlers/models.rs`

- [ ] **Step 1: Update `ModelInfo` struct**

Add new fields to `ModelInfo`:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    pub provider_type: String,
    pub enabled: bool,
    pub is_default: bool,
    /// Whether this is the provider's currently configured model
    pub is_current: bool,
    pub capabilities: Vec<String>,
    /// Source of this model info: "api", "preset", "config"
    pub source: String,
}
```

- [ ] **Step 2: Update `ListParams` with `refresh` field**

```rust
#[derive(Debug, Deserialize, Default)]
pub struct ListParams {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub enabled_only: bool,
    /// Force cache refresh before listing
    #[serde(default)]
    pub refresh: bool,
}
```

- [ ] **Step 3: Rewrite `handle_list` to use `ModelRegistry`**

```rust
pub async fn handle_list(request: JsonRpcRequest, config: Arc<Config>) -> JsonRpcResponse {
    let params: ListParams = match &request.params {
        Some(p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => ListParams::default(),
    };

    let default_provider = config.general.default_provider.clone();
    let registry = &crate::providers::model_registry::MODEL_REGISTRY;
    let protocol_registry = crate::providers::protocols::ProtocolRegistry::global();
    if protocol_registry.list_protocols().is_empty() {
        protocol_registry.register_builtin();
    }

    let mut all_models = Vec::new();

    for (name, cfg) in &config.providers {
        // Apply filters
        if let Some(ref filter) = params.provider {
            if name != filter {
                continue;
            }
        }
        if params.enabled_only && !cfg.enabled {
            continue;
        }

        let protocol = cfg.protocol();
        let is_default = default_provider.as_ref() == Some(name);

        // Get discovered models from registry
        let discovered = if protocol == "ollama" {
            // Ollama uses its own provider, not ProtocolAdapter.
            // We wrap its list_models() through an OllamaDiscoveryAdapter
            // that implements ProtocolAdapter::list_models() by delegating
            // to OllamaProvider::list_models(). This lets ModelRegistry
            // handle caching uniformly for all providers.
            let ollama_adapter = OllamaDiscoveryAdapter::new(name.clone(), cfg.clone());
            if params.refresh {
                registry.refresh(name, &protocol, &ollama_adapter, cfg).await
            } else {
                registry.list_models(name, &protocol, &ollama_adapter, cfg).await
            }
        } else {
            match protocol_registry.get(&protocol) {
                Some(adapter) => {
                    if params.refresh {
                        registry.refresh(name, &protocol, adapter.as_ref(), cfg).await
                    } else {
                        registry
                            .list_models(name, &protocol, adapter.as_ref(), cfg)
                            .await
                    }
                }
                None => vec![],
            }
        };

        if discovered.is_empty() {
            // No discovered models — fall back to showing the configured model
            all_models.push(ModelInfo {
                id: cfg.model.clone(),
                provider: name.clone(),
                provider_type: protocol.clone(),
                enabled: cfg.enabled,
                is_default,
                is_current: true,
                capabilities: infer_capabilities(&protocol, &cfg.model),
                source: "config".to_string(),
            });
        } else {
            let source = registry
                .get_source(name)
                .await
                .map(|s| match s {
                    crate::providers::model_registry::ModelSource::Api => "api",
                    crate::providers::model_registry::ModelSource::Preset => "preset",
                })
                .unwrap_or("config");

            for model in discovered {
                let is_current = model.id == cfg.model;
                let capabilities = if model.capabilities.is_empty() {
                    infer_capabilities(&protocol, &model.id)
                } else {
                    model.capabilities.clone()
                };

                all_models.push(ModelInfo {
                    id: model.id,
                    provider: name.clone(),
                    provider_type: protocol.clone(),
                    enabled: cfg.enabled,
                    is_default: is_default && is_current,
                    is_current,
                    capabilities,
                    source: source.to_string(),
                });
            }
        }
    }

    JsonRpcResponse::success(request.id, json!({ "models": all_models }))
}
```

Add an `OllamaDiscoveryAdapter` at the top of the file — wraps `OllamaProvider::list_models()` into the `ProtocolAdapter` interface so ModelRegistry can cache Ollama results uniformly:

```rust
/// Adapter that wraps OllamaProvider::list_models() for ModelRegistry caching
struct OllamaDiscoveryAdapter {
    name: String,
    config: ProviderConfig,
}

impl OllamaDiscoveryAdapter {
    fn new(name: String, config: ProviderConfig) -> Self {
        Self { name, config }
    }
}

#[async_trait::async_trait]
impl crate::providers::ProtocolAdapter for OllamaDiscoveryAdapter {
    fn build_request(
        &self,
        _payload: &crate::providers::RequestPayload,
        _config: &ProviderConfig,
        _is_streaming: bool,
    ) -> crate::error::Result<reqwest::RequestBuilder> {
        unimplemented!("OllamaDiscoveryAdapter is only used for list_models")
    }
    async fn parse_response(
        &self,
        _response: reqwest::Response,
    ) -> crate::error::Result<crate::providers::ProviderResponse> {
        unimplemented!("OllamaDiscoveryAdapter is only used for list_models")
    }
    async fn parse_stream(
        &self,
        _response: reqwest::Response,
    ) -> crate::error::Result<futures::stream::BoxStream<'static, crate::error::Result<String>>> {
        unimplemented!("OllamaDiscoveryAdapter is only used for list_models")
    }
    fn name(&self) -> &'static str {
        "ollama-discovery"
    }
    async fn list_models(
        &self,
        _config: &ProviderConfig,
    ) -> crate::error::Result<Option<Vec<crate::providers::adapter::DiscoveredModel>>> {
        match crate::providers::OllamaProvider::new(self.name.clone(), self.config.clone()) {
            Ok(provider) => Ok(Some(provider.list_models().await.unwrap_or_default())),
            Err(_) => Ok(None),
        }
    }
}
```

- [ ] **Step 3b: Update `handle_get` for new `ModelInfo` fields**

The existing `handle_get` constructs `ModelInfo` without `is_current` and `source`. Update it:

```rust
pub async fn handle_get(request: JsonRpcRequest, config: Arc<Config>) -> JsonRpcResponse {
    let params: GetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match config.providers.get(&params.provider) {
        Some(cfg) => {
            let default_provider = config.general.default_provider.clone();
            let protocol = cfg.protocol();
            let capabilities = infer_capabilities(&protocol, &cfg.model);

            let info = ModelInfo {
                id: cfg.model.clone(),
                provider: params.provider.clone(),
                provider_type: protocol,
                enabled: cfg.enabled,
                is_default: default_provider.as_ref() == Some(&params.provider),
                is_current: true,
                capabilities,
                source: "config".to_string(),
            };

            JsonRpcResponse::success(request.id, json!({ "model": info }))
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Model not found for provider: {}", params.provider),
        ),
    }
}
```

- [ ] **Step 4: Add `handle_refresh` handler**

```rust
/// Parameters for models.refresh
#[derive(Debug, Deserialize, Default)]
pub struct RefreshParams {
    /// Provider to refresh (omit to refresh all)
    #[serde(default)]
    pub provider: Option<String>,
}

/// Force refresh model list for a provider
pub async fn handle_refresh(request: JsonRpcRequest, config: Arc<Config>) -> JsonRpcResponse {
    let params: RefreshParams = match &request.params {
        Some(p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => RefreshParams::default(),
    };

    let registry = &crate::providers::model_registry::MODEL_REGISTRY;
    let protocol_registry = crate::providers::protocols::ProtocolRegistry::global();
    if protocol_registry.list_protocols().is_empty() {
        protocol_registry.register_builtin();
    }

    let providers_to_refresh: Vec<_> = config
        .providers
        .iter()
        .filter(|(name, _)| {
            params
                .provider
                .as_ref()
                .map_or(true, |filter| name.as_str() == filter)
        })
        .collect();

    let mut results = Vec::new();

    for (name, cfg) in providers_to_refresh {
        let protocol = cfg.protocol();

        let models = if protocol == "ollama" {
            let ollama_adapter = OllamaDiscoveryAdapter::new(name.clone(), cfg.clone());
            registry.refresh(name, &protocol, &ollama_adapter, cfg).await
        } else {
            match protocol_registry.get(&protocol) {
                Some(adapter) => {
                    registry.refresh(name, &protocol, adapter.as_ref(), cfg).await
                }
                None => vec![],
            }
        };

        let source = registry
            .get_source(name)
            .await
            .map(|s| match s {
                crate::providers::model_registry::ModelSource::Api => "api",
                crate::providers::model_registry::ModelSource::Preset => "preset",
            })
            .unwrap_or("config");

        results.push(json!({
            "provider": name,
            "count": models.len(),
            "source": source,
            "models": models.iter().map(|m| json!({
                "id": m.id,
                "name": m.name,
                "capabilities": m.capabilities,
            })).collect::<Vec<_>>(),
        }));
    }

    if results.len() == 1 {
        JsonRpcResponse::success(request.id, results.into_iter().next().unwrap())
    } else {
        JsonRpcResponse::success(request.id, json!({ "results": results }))
    }
}
```

- [ ] **Step 5: Add `handle_set_default` and `handle_set_model`**

```rust
/// Parameters for models.set_default
#[derive(Debug, Deserialize)]
pub struct SetDefaultParams {
    pub provider: String,
}

/// Set the default provider
pub async fn handle_set_default(
    request: JsonRpcRequest,
    config: Arc<tokio::sync::RwLock<Config>>,
) -> JsonRpcResponse {
    let params: SetDefaultParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let mut cfg = config.write().await;

    if !cfg.providers.contains_key(&params.provider) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Provider not found: {}", params.provider),
        );
    }

    cfg.general.default_provider = Some(params.provider.clone());

    JsonRpcResponse::success(
        request.id,
        json!({ "message": format!("Default provider set to {}", params.provider) }),
    )
}

/// Parameters for models.set_model
#[derive(Debug, Deserialize)]
pub struct SetModelParams {
    pub provider: String,
    pub model: String,
}

/// Change a provider's configured model with validation
pub async fn handle_set_model(
    request: JsonRpcRequest,
    config: Arc<tokio::sync::RwLock<Config>>,
) -> JsonRpcResponse {
    let params: SetModelParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let mut cfg = config.write().await;

    let provider_cfg = match cfg.providers.get(&params.provider) {
        Some(c) => c.clone(),
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Provider not found: {}", params.provider),
            );
        }
    };

    // Validate model exists in available models
    let registry = &crate::providers::model_registry::MODEL_REGISTRY;
    let protocol = provider_cfg.protocol();
    let protocol_registry = crate::providers::protocols::ProtocolRegistry::global();
    if protocol_registry.list_protocols().is_empty() {
        protocol_registry.register_builtin();
    }

    let available = if protocol == "ollama" {
        let ollama_adapter = OllamaDiscoveryAdapter::new(params.provider.clone(), provider_cfg.clone());
        registry.list_models(&params.provider, &protocol, &ollama_adapter, &provider_cfg).await
    } else {
        match protocol_registry.get(&protocol) {
            Some(adapter) => {
                registry
                    .list_models(&params.provider, &protocol, adapter.as_ref(), &provider_cfg)
                    .await
            }
            None => vec![],
        }
    };

    // If we have a model list, validate against it
    if !available.is_empty() && !available.iter().any(|m| m.id == params.model) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!(
                "Model '{}' not found in {}'s available models. Available: {}",
                params.model,
                params.provider,
                available.iter().map(|m| m.id.as_str()).collect::<Vec<_>>().join(", ")
            ),
        );
    }

    // Update the model
    if let Some(provider) = cfg.providers.get_mut(&params.provider) {
        provider.model = params.model.clone();
    }

    JsonRpcResponse::success(
        request.id,
        json!({ "message": format!("Provider {} model set to {}", params.provider, params.model) }),
    )
}
```

- [ ] **Step 6: Keep `handle_set` as alias for backward compatibility**

Rename the existing `handle_set` to delegate to `handle_set_default`:

```rust
/// Set the active model (default provider) — backward compatibility alias
///
/// DEPRECATED: Use models.set_default instead
pub async fn handle_set(
    request: JsonRpcRequest,
    config: Arc<tokio::sync::RwLock<Config>>,
) -> JsonRpcResponse {
    // Translate old params format { "model": "openai" } to new { "provider": "openai" }
    let new_params = request.params.as_ref().and_then(|p| {
        p.get("model").map(|m| json!({ "provider": m }))
    });

    let new_request = JsonRpcRequest::new(
        "models.set_default",
        new_params,
        request.id.clone(),
    );

    handle_set_default(new_request, config).await
}
```

- [ ] **Step 7: Update existing tests for new `ModelInfo` fields**

In `test_model_info_serialize`, add the two new fields to the `ModelInfo` construction:

```rust
    let info = ModelInfo {
        id: "gpt-4o".to_string(),
        provider: "openai".to_string(),
        provider_type: "openai".to_string(),
        enabled: true,
        is_default: true,
        is_current: true,
        capabilities: vec![
            "chat".to_string(),
            "vision".to_string(),
            "tools".to_string(),
        ],
        source: "config".to_string(),
    };
```

And add assertions:

```rust
    assert!(json["is_current"].as_bool().unwrap());
    assert_eq!(json["source"], "config");
```

The `handle_list` integration tests (`test_handle_list_empty_config`, `test_handle_list_with_providers`, `test_handle_list_with_filter`) will work because the rewritten `handle_list` falls back to creating `ModelInfo` from config when `ModelRegistry` returns empty (which it will in tests since no real API is available and preset TOML contains only known protocols). Verify by running the tests.

- [ ] **Step 8: Run tests**

Run: `cargo test -p alephcore --lib models::tests`
Expected: ALL PASS

- [ ] **Step 9: Commit**

```bash
git add core/src/gateway/handlers/models.rs
git commit -m "model-discovery: rewrite RPC handlers to use ModelRegistry"
```

---

### Task 9: Register new RPC methods in handler registry

**Files:**
- Modify: `core/src/gateway/handlers/mod.rs`

- [ ] **Step 1: Register the new methods**

After the existing `models.capabilities` registration (around line 213), add:

```rust
        let cfg = models_config.clone();
        registry.register("models.refresh", move |req| {
            let config = cfg.clone();
            async move { models::handle_refresh(req, config).await }
        });
```

Note: `models.set_default` and `models.set_model` require writable config (`Arc<tokio::sync::RwLock<Config>>`). Check how `models.set` is currently registered — if it's not registered in `register_defaults()`, it may be wired elsewhere. Search for "models.set" registration and add the new handlers in the same location.

- [ ] **Step 2: Run compile check**

Run: `cargo check -p alephcore`
Expected: SUCCESS

- [ ] **Step 3: Run all tests**

Run: `cargo test -p alephcore --lib`
Expected: ALL PASS (pre-existing failures in `markdown_skill::loader` are known)

- [ ] **Step 4: Commit**

```bash
git add core/src/gateway/handlers/mod.rs
git commit -m "model-discovery: register models.refresh and new RPC methods"
```

---

### Task 10: Final integration test

**Files:**
- No new files — validation only

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: ALL PASS (minus known pre-existing failures)

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -50`
Expected: No new warnings

- [ ] **Step 3: Final commit (if any clippy fixes needed)**

```bash
git add -A
git commit -m "model-discovery: fix clippy warnings"
```
