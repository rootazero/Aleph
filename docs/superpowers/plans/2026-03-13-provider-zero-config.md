# Provider Zero-Config UX Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Zero-learning-cost AI provider configuration via first-run wizard + enhanced settings form with automatic model discovery.

**Architecture:** Frontend (Leptos/WASM) owns wizard step orchestration. Core adds two aggregated RPC endpoints (`providers.probe`, `providers.needs_setup`). Shared UI Logic gets `ModelsApi` client. Three reusable components (model_selector, probe_indicator, api_key_input) are shared between wizard and settings form.

**Tech Stack:** Rust, Leptos 0.7 (WASM), JSON-RPC 2.0, wiremock (tests), reqwest

---

## Chunk 1: Backend RPC Endpoints

### Task 1: Add `providers.needs_setup` RPC handler

**Files:**
- Modify: `core/src/gateway/handlers/providers.rs` (append after `handle_test`)
- Modify: `core/src/bin/aleph/commands/start/builder/handlers.rs:552` (register new handler)

- [ ] **Step 1: Write the test for `handle_needs_setup`**

Add to `core/src/gateway/handlers/providers.rs` inside the existing `#[cfg(test)]` module:

```rust
#[tokio::test]
async fn test_needs_setup_empty_providers() {
    let config = Arc::new(RwLock::new(Config::default()));
    let request = JsonRpcRequest::with_id("providers.needsSetup", None, serde_json::json!(1));
    let response = handle_needs_setup(request, config).await;
    let result: serde_json::Value = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_eq!(result["needs_setup"], true);
    assert_eq!(result["provider_count"], 0);
    assert_eq!(result["has_verified"], false);
}

#[tokio::test]
async fn test_needs_setup_has_verified_provider() {
    let mut config = Config::default();
    let mut provider_cfg = ProviderConfig::test_config("gpt-4o");
    provider_cfg.enabled = true;
    provider_cfg.verified = true;
    config.providers.insert("openai".to_string(), provider_cfg);
    let config = Arc::new(RwLock::new(config));
    let request = JsonRpcRequest::with_id("providers.needsSetup", None, serde_json::json!(1));
    let response = handle_needs_setup(request, config).await;
    let result: serde_json::Value = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_eq!(result["needs_setup"], false);
    assert_eq!(result["provider_count"], 1);
    assert_eq!(result["has_verified"], true);
}

#[tokio::test]
async fn test_needs_setup_has_unverified_provider() {
    let mut config = Config::default();
    let mut provider_cfg = ProviderConfig::test_config("gpt-4o");
    provider_cfg.enabled = true;
    provider_cfg.verified = false;
    config.providers.insert("openai".to_string(), provider_cfg);
    let config = Arc::new(RwLock::new(config));
    let request = JsonRpcRequest::with_id("providers.needsSetup", None, serde_json::json!(1));
    let response = handle_needs_setup(request, config).await;
    let result: serde_json::Value = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_eq!(result["needs_setup"], true);
    assert_eq!(result["provider_count"], 1);
    assert_eq!(result["has_verified"], false);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib test_needs_setup -- --nocapture`
Expected: FAIL — `handle_needs_setup` not found

- [ ] **Step 3: Implement `handle_needs_setup`**

Add to `core/src/gateway/handlers/providers.rs`, after `handle_test` (after line ~706):

```rust
// ============================================================================
// Needs Setup
// ============================================================================

/// Check if first-run setup is needed
///
/// Returns true if no provider is both enabled and verified.
/// Panel calls this on startup to decide whether to show the setup wizard.
pub async fn handle_needs_setup(request: JsonRpcRequest, config_store: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let cfg = config_store.read().await;
    let provider_count = cfg.providers.len();
    let has_verified = cfg.providers.values().any(|p| p.enabled && p.verified);

    JsonRpcResponse::success(
        request.id,
        json!({
            "needs_setup": !has_verified,
            "provider_count": provider_count,
            "has_verified": has_verified,
        }),
    )
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib test_needs_setup -- --nocapture`
Expected: 3 tests PASS

- [ ] **Step 5: Register handler in binary**

In `core/src/bin/aleph/commands/start/builder/handlers.rs`, add after line 552 (`providers.test` registration):

```rust
    register_handler!(server, "providers.needsSetup", providers::handle_needs_setup, config);
```

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p alephcore && cargo check --bin aleph`
Expected: compiles

- [ ] **Step 7: Commit**

```bash
git add core/src/gateway/handlers/providers.rs core/src/bin/aleph/commands/start/builder/handlers.rs
git commit -m "providers: add providers.needsSetup RPC for first-run detection"
```

---

### Task 2: Add `providers.probe` RPC handler

**Files:**
- Modify: `core/src/gateway/handlers/providers.rs` (append after `handle_needs_setup`)
- Modify: `core/src/bin/aleph/commands/start/builder/handlers.rs` (register)

- [ ] **Step 1: Write the unit test for probe params and response types**

Add to the `#[cfg(test)]` module in `core/src/gateway/handlers/providers.rs`:

```rust
#[tokio::test]
async fn test_probe_needs_protocol() {
    let config = Arc::new(RwLock::new(Config::default()));
    // Missing protocol field
    let request = JsonRpcRequest::with_id(
        "providers.probe",
        Some(json!({})),
        serde_json::json!(1),
    );
    let response = handle_probe(request, config).await;
    assert!(response.error.is_some(), "Should fail without protocol");
}

#[tokio::test]
async fn test_probe_unknown_protocol() {
    let config = Arc::new(RwLock::new(Config::default()));
    let request = JsonRpcRequest::with_id(
        "providers.probe",
        Some(json!({"protocol": "nonexistent"})),
        serde_json::json!(1),
    );
    let response = handle_probe(request, config).await;
    // Should return success response with error field (not RPC error)
    let result: serde_json::Value = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_eq!(result["success"], false);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib test_probe -- --nocapture`
Expected: FAIL — `handle_probe` not found

- [ ] **Step 3: Implement `handle_probe`**

Add to `core/src/gateway/handlers/providers.rs`, after `handle_needs_setup`:

```rust
// ============================================================================
// Probe
// ============================================================================

/// Parameters for providers.probe
#[derive(Debug, Deserialize)]
pub struct ProbeParams {
    /// Protocol type: "openai", "anthropic", "gemini", "ollama"
    pub protocol: String,
    /// API key (not needed for Ollama)
    #[serde(default)]
    pub api_key: Option<String>,
    /// Custom base URL (None = protocol default)
    #[serde(default)]
    pub base_url: Option<String>,
}

/// Probe result combining connection test + model discovery
#[derive(Debug, Serialize)]
pub struct ProbeResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    pub models: Vec<crate::providers::adapter::DiscoveredModel>,
    pub model_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Probe a provider: test connection + discover available models
///
/// Combines connection verification and model discovery in a single call.
/// Used by the setup wizard and enhanced settings form.
pub async fn handle_probe(request: JsonRpcRequest, _config_store: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let params: ProbeParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let protocol = params.protocol.clone();

    // Build temporary config for probing
    // Note: test_config() sets the model field, not protocol. Set protocol explicitly.
    let mut probe_config = ProviderConfig::test_config("probe-placeholder");
    probe_config.protocol = Some(protocol.clone());
    if let Some(api_key) = params.api_key {
        probe_config.api_key = Some(api_key);
    }
    if let Some(base_url) = params.base_url {
        probe_config.base_url = Some(base_url);
    }

    let registry = &crate::providers::model_registry::MODEL_REGISTRY;
    let probe_name = format!("probe-{}", uuid::Uuid::new_v4());
    let start = std::time::Instant::now();

    // Attempt model discovery (implicitly tests connection)
    let (models, model_source, error) = if protocol == "ollama" {
        let ollama_adapter = super::models::OllamaDiscoveryAdapter::new(
            probe_name.clone(),
            probe_config.clone(),
        );
        match registry
            .list_models(&probe_name, &protocol, &ollama_adapter, &probe_config)
            .await
        {
            models if !models.is_empty() => {
                let source = registry
                    .get_source(&probe_name)
                    .await
                    .map(|s| match s {
                        crate::providers::model_registry::ModelSource::Api => "api".to_string(),
                        crate::providers::model_registry::ModelSource::Preset => "preset".to_string(),
                    })
                    .unwrap_or_else(|| "preset".to_string());
                (models, source, None)
            }
            _ => (vec![], "preset".to_string(), Some("No models found".to_string())),
        }
    } else {
        let protocol_registry = crate::providers::protocols::ProtocolRegistry::global();
        if protocol_registry.list_protocols().is_empty() {
            protocol_registry.register_builtin();
        }

        match protocol_registry.get(&protocol) {
            Some(adapter) => {
                let models = registry
                    .list_models(&probe_name, &protocol, adapter.as_ref(), &probe_config)
                    .await;
                if models.is_empty() {
                    (
                        vec![],
                        "preset".to_string(),
                        Some("No models discovered — check API key and endpoint".to_string()),
                    )
                } else {
                    let source = registry
                        .get_source(&probe_name)
                        .await
                        .map(|s| match s {
                            crate::providers::model_registry::ModelSource::Api => "api".to_string(),
                            crate::providers::model_registry::ModelSource::Preset => {
                                "preset".to_string()
                            }
                        })
                        .unwrap_or_else(|| "preset".to_string());
                    (models, source, None)
                }
            }
            None => (
                vec![],
                "preset".to_string(),
                Some(format!("Unknown protocol: {}", protocol)),
            ),
        }
    };

    let latency_ms = start.elapsed().as_millis() as u64;
    let success = error.is_none();

    JsonRpcResponse::success(
        request.id,
        json!(ProbeResult {
            success,
            latency_ms: Some(latency_ms),
            models,
            model_source,
            error,
        }),
    )
}
```

- [ ] **Step 4: Make `OllamaDiscoveryAdapter` accessible from providers module**

The `OllamaDiscoveryAdapter` is currently private in `models.rs`. Add `pub(super)` visibility:

In `core/src/gateway/handlers/models.rs`, change line 24:
```rust
// FROM:
struct OllamaDiscoveryAdapter {
// TO:
pub(super) struct OllamaDiscoveryAdapter {
```

Also change line 30 (`fn new`) to:
```rust
// FROM:
    fn new(name: String, config: crate::config::ProviderConfig) -> Self {
// TO:
    pub(super) fn new(name: String, config: crate::config::ProviderConfig) -> Self {
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib test_probe -- --nocapture`
Expected: 2 tests PASS

- [ ] **Step 6: Register handler in binary**

In `core/src/bin/aleph/commands/start/builder/handlers.rs`, add after the `providers.needsSetup` line:

```rust
    register_handler!(server, "providers.probe", providers::handle_probe, config);
```

- [ ] **Step 7: Verify compilation**

Run: `cargo check -p alephcore && cargo check --bin aleph`
Expected: compiles

- [ ] **Step 8: Commit**

```bash
git add core/src/gateway/handlers/providers.rs core/src/gateway/handlers/models.rs core/src/bin/aleph/commands/start/builder/handlers.rs
git commit -m "providers: add providers.probe RPC combining connection test + model discovery"
```

---

### Task 3: L2 Integration tests for probe and needs_setup

**Files:**
- Modify: `core/tests/model_discovery_integration.rs` (append new tests)

- [ ] **Step 1: Write wiremock integration tests**

Append to `core/tests/model_discovery_integration.rs`:

```rust
// ── providers.probe + providers.needs_setup integration tests ────────────────

use alephcore::gateway::handlers::providers::{handle_probe, handle_needs_setup, ProbeResult};
use alephcore::gateway::protocol::JsonRpcRequest;
use alephcore::config::Config;
use tokio::sync::RwLock;
use std::sync::Arc;

#[tokio::test]
async fn probe_openai_discovers_models_via_mock() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_models_response()))
        .mount(&server)
        .await;

    let config = Arc::new(RwLock::new(Config::default()));
    let request = JsonRpcRequest::with_id(
        "providers.probe",
        Some(json!({
            "protocol": "openai",
            "api_key": "test-key",
            "base_url": server.uri()
        })),
        serde_json::json!(1),
    );

    let response = handle_probe(request, config).await;
    let result: ProbeResult = serde_json::from_value(response.result.unwrap()).unwrap();

    assert!(result.success);
    assert!(!result.models.is_empty());
    assert_eq!(result.model_source, "api");
    assert!(result.error.is_none());
    assert!(result.latency_ms.is_some());
}

#[tokio::test]
async fn probe_falls_back_to_preset_on_failure() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let config = Arc::new(RwLock::new(Config::default()));
    let request = JsonRpcRequest::with_id(
        "providers.probe",
        Some(json!({
            "protocol": "openai",
            "api_key": "bad-key",
            "base_url": server.uri()
        })),
        serde_json::json!(1),
    );

    let response = handle_probe(request, config).await;
    let result: ProbeResult = serde_json::from_value(response.result.unwrap()).unwrap();

    // ModelRegistry falls back to preset models on API failure.
    // If presets exist for this protocol, success=true with source="preset".
    // If no presets, success=false.
    // With embedded model-presets.toml, OpenAI presets exist, so:
    assert_eq!(result.model_source, "preset");
    // Models may or may not be empty depending on preset config
}

#[tokio::test]
async fn needs_setup_returns_true_for_empty_config() {
    let config = Arc::new(RwLock::new(Config::default()));
    let request = JsonRpcRequest::with_id("providers.needsSetup", None, serde_json::json!(1));
    let response = handle_needs_setup(request, config).await;
    let result: serde_json::Value = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_eq!(result["needs_setup"], true);
}
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test -p alephcore --test model_discovery_integration -- --nocapture`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add core/tests/model_discovery_integration.rs
git commit -m "tests: add L2 integration tests for providers.probe and providers.needsSetup"
```

---

## Chunk 2: Shared UI Logic API Layer

### Task 4: Add `ModelsApi` to shared_ui_logic

**Files:**
- Create: `shared_ui_logic/src/api/models.rs`
- Modify: `shared_ui_logic/src/api/mod.rs`

- [ ] **Step 1: Create `ModelsApi` RPC client**

Create `shared_ui_logic/src/api/models.rs`:

```rust
//! Models API
//!
//! High-level API for model discovery and selection operations.

use crate::protocol::rpc::{RpcClient, RpcError};
use serde::{Deserialize, Serialize};

/// Refresh result from models.refresh (single provider)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshResult {
    /// Provider name
    pub provider: String,
    /// Number of models found
    pub count: usize,
    /// Source: "api", "preset", "config"
    pub source: String,
    /// Discovered models (simpler shape than ModelInfo)
    pub models: Vec<RefreshModelEntry>,
}

/// Model entry in refresh response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshModelEntry {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Model information from discovery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model identifier (e.g., "gpt-4o")
    pub id: String,
    /// Provider name
    pub provider: String,
    /// Provider type (e.g., "openai", "anthropic")
    pub provider_type: String,
    /// Whether the provider is enabled
    pub enabled: bool,
    /// Whether this is the default model
    pub is_default: bool,
    /// Whether this is the provider's currently configured model
    pub is_current: bool,
    /// Model capabilities: "chat", "vision", "tools", "thinking"
    pub capabilities: Vec<String>,
    /// Source: "api", "preset", "config"
    pub source: String,
}

/// Models API client
///
/// Provides model discovery and selection operations.
pub struct ModelsApi<C: crate::connection::AlephConnector> {
    rpc: RpcClient<C>,
}

impl<C: crate::connection::AlephConnector> ModelsApi<C> {
    /// Create a new models API client
    pub fn new(rpc: RpcClient<C>) -> Self {
        Self { rpc }
    }

    /// List available models
    pub async fn list(
        &self,
        provider: Option<&str>,
        refresh: bool,
    ) -> Result<Vec<ModelInfo>, RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        struct Params<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            provider: Option<&'a str>,
            #[serde(skip_serializing_if = "std::ops::Not::not")]
            refresh: bool,
        }

        #[derive(Deserialize)]
        struct Response {
            models: Vec<ModelInfo>,
        }

        let response: Response = self
            .rpc
            .call("models.list", &Params { provider, refresh })
            .await?;
        Ok(response.models)
    }

    /// Force refresh model list for a specific provider
    ///
    /// Note: models.refresh returns a different shape than models.list.
    /// Single provider: `{ provider, count, source, models: [{id, name, capabilities}] }`
    /// Multiple providers: `{ results: [{ provider, count, source, models }] }`
    /// We only support single-provider refresh here (which is the wizard/form use case).
    pub async fn refresh(
        &self,
        provider: &str,
    ) -> Result<RefreshResult, RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        struct Params<'a> {
            provider: &'a str,
        }

        let result: RefreshResult = self
            .rpc
            .call("models.refresh", &Params { provider })
            .await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_info_deserialization() {
        let json = serde_json::json!({
            "id": "gpt-4o",
            "provider": "openai",
            "provider_type": "openai",
            "enabled": true,
            "is_default": true,
            "is_current": true,
            "capabilities": ["chat", "vision", "tools"],
            "source": "api"
        });
        let info: ModelInfo = serde_json::from_value(json).unwrap();
        assert_eq!(info.id, "gpt-4o");
        assert_eq!(info.capabilities.len(), 3);
    }
}
```

- [ ] **Step 2: Export from mod.rs**

In `shared_ui_logic/src/api/mod.rs`, add:

After line `pub mod providers;`:
```rust
pub mod models;
```

After line `pub use providers::{ProviderConfig, ProviderInfo, ProvidersApi, TestResult};`:
```rust
pub use models::{ModelInfo, ModelsApi, RefreshResult, RefreshModelEntry};
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p aleph-ui-logic`
Expected: compiles (note: the shared_ui_logic crate may have a different package name — check Cargo.toml)

- [ ] **Step 4: Commit**

```bash
git add shared_ui_logic/src/api/models.rs shared_ui_logic/src/api/mod.rs
git commit -m "shared-ui-logic: add ModelsApi RPC client for model discovery"
```

---

### Task 5: Add `probe()` and `needs_setup()` to `ProvidersApi` + `verified` field

**Files:**
- Modify: `shared_ui_logic/src/api/providers.rs`
- Modify: `shared_ui_logic/src/api/mod.rs` (update re-exports)

- [ ] **Step 1: Add `verified` field to `ProviderInfo`**

In `shared_ui_logic/src/api/providers.rs`, add to the `ProviderInfo` struct (after `is_default`):

```rust
    /// Whether the provider has been verified via connection test
    #[serde(default)]
    pub verified: bool,
```

- [ ] **Step 2: Add `ProbeResult` and `NeedsSetupResult` types**

Add after `TestResult` struct:

```rust
/// Discovered model from provider API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredModel {
    /// Model identifier
    pub id: String,
    /// Display name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Who owns this model
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<String>,
    /// Model capabilities
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Provider probe result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    /// Whether probe succeeded (models discovered from API)
    pub success: bool,
    /// Connection latency in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// Discovered models
    pub models: Vec<DiscoveredModel>,
    /// Source of models: "api" or "preset"
    pub model_source: String,
    /// Error message if probe failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Needs-setup check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeedsSetupResult {
    /// Whether first-run setup is needed
    pub needs_setup: bool,
    /// Number of configured providers
    pub provider_count: usize,
    /// Whether any provider is verified
    pub has_verified: bool,
}
```

- [ ] **Step 3: Add `probe()` and `needs_setup()` methods**

Add to `impl<C: AlephConnector> ProvidersApi<C>`, after `set_default()`:

```rust
    /// Probe a provider: test connection + discover models
    pub async fn probe(
        &self,
        protocol: &str,
        api_key: Option<&str>,
        base_url: Option<&str>,
    ) -> Result<ProbeResult, RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        struct Params<'a> {
            protocol: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            api_key: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            base_url: Option<&'a str>,
        }

        let result: ProbeResult = self
            .rpc
            .call("providers.probe", &Params { protocol, api_key, base_url })
            .await?;
        Ok(result)
    }

    /// Check if first-run setup wizard is needed
    pub async fn needs_setup(&self) -> Result<NeedsSetupResult, RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        let result: NeedsSetupResult = self.rpc.call("providers.needsSetup", &()).await?;
        Ok(result)
    }
```

- [ ] **Step 4: Update re-exports in `mod.rs`**

In `shared_ui_logic/src/api/mod.rs`, update the providers re-export line:

```rust
// FROM:
pub use providers::{ProviderConfig, ProviderInfo, ProvidersApi, TestResult};
// TO:
pub use providers::{DiscoveredModel, NeedsSetupResult, ProbeResult, ProviderConfig, ProviderInfo, ProvidersApi, TestResult};
```

- [ ] **Step 5: Update test_provider_info_serialization**

Update the test in `shared_ui_logic/src/api/providers.rs` to include `verified`:

```rust
    #[test]
    fn test_provider_info_serialization() {
        let info = ProviderInfo {
            name: "openai".to_string(),
            enabled: true,
            model: "gpt-4".to_string(),
            provider_type: Some("openai".to_string()),
            is_default: true,
            verified: true,
        };

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["name"], "openai");
        assert_eq!(json["verified"], true);
    }
```

- [ ] **Step 6: Add tests for new types**

Add to the `#[cfg(test)]` module:

```rust
    #[test]
    fn test_probe_result_deserialization() {
        let json = serde_json::json!({
            "success": true,
            "latency_ms": 234,
            "models": [
                {"id": "gpt-4o", "name": "GPT-4o", "capabilities": ["chat", "vision"]}
            ],
            "model_source": "api"
        });
        let result: ProbeResult = serde_json::from_value(json).unwrap();
        assert!(result.success);
        assert_eq!(result.models.len(), 1);
        assert_eq!(result.models[0].id, "gpt-4o");
    }

    #[test]
    fn test_needs_setup_result_deserialization() {
        let json = serde_json::json!({
            "needs_setup": true,
            "provider_count": 0,
            "has_verified": false
        });
        let result: NeedsSetupResult = serde_json::from_value(json).unwrap();
        assert!(result.needs_setup);
        assert_eq!(result.provider_count, 0);
    }
```

- [ ] **Step 7: Verify compilation and tests**

Run: `cargo check -p aleph-ui-logic && cargo test -p aleph-ui-logic`
Expected: compiles + tests pass

- [ ] **Step 8: Commit**

```bash
git add shared_ui_logic/src/api/providers.rs shared_ui_logic/src/api/mod.rs
git commit -m "shared-ui-logic: add probe(), needs_setup() to ProvidersApi + verified field"
```

---

## Chunk 3: Panel UI Components & Wizard

### Task 6: Create shared `model_selector` component

**Files:**
- Create: `apps/panel/src/components/model_selector.rs`
- Create or modify: `apps/panel/src/components/mod.rs` (check if exists, extend)

This component displays a grouped dropdown for model selection. Models are grouped by capability (Chat / Vision / Tools / Other).

- [ ] **Step 1: Check if `apps/panel/src/components/mod.rs` exists and what it exports**

Run: `cat apps/panel/src/components/mod.rs`

- [ ] **Step 2: Create the model selector component**

Create `apps/panel/src/components/model_selector.rs`:

```rust
//! Grouped model selector dropdown
//!
//! Displays discovered models grouped by capability (Chat, Vision, Tools, Other).
//! Shared between the setup wizard and the settings form.

use leptos::prelude::*;

/// A discovered model for display
#[derive(Debug, Clone, PartialEq)]
pub struct ModelOption {
    pub id: String,
    pub name: Option<String>, // Fall back to id when None (per spec)
    pub capabilities: Vec<String>,
    pub source: String, // "api" or "preset"
}

/// Props for the ModelSelector component
#[derive(Clone)]
pub struct ModelSelectorProps {
    /// Available models to display
    pub models: Signal<Vec<ModelOption>>,
    /// Currently selected model ID
    pub selected: RwSignal<Option<String>>,
    /// Default/recommended model ID (highlighted)
    pub recommended: Signal<Option<String>>,
    /// Whether to show the refresh button
    pub show_refresh: bool,
    /// Callback when refresh is clicked
    pub on_refresh: Option<Callback<()>>,
    /// Whether a refresh is in progress
    pub refreshing: Signal<bool>,
}

/// Group models by capability
fn group_models(models: &[ModelOption]) -> Vec<(&'static str, Vec<&ModelOption>)> {
    let groups = [
        ("Chat", "chat"),
        ("Vision", "vision"),
        ("Tools", "tools"),
    ];

    let mut result: Vec<(&'static str, Vec<&ModelOption>)> = Vec::new();
    let mut categorized = std::collections::HashSet::new();

    for (label, cap) in &groups {
        let group_models: Vec<&ModelOption> = models
            .iter()
            .filter(|m| m.capabilities.iter().any(|c| c == cap))
            .collect();
        if !group_models.is_empty() {
            for m in &group_models {
                categorized.insert(&m.id);
            }
            result.push((label, group_models));
        }
    }

    // "Other" for uncategorized
    let other: Vec<&ModelOption> = models
        .iter()
        .filter(|m| !categorized.contains(&m.id))
        .collect();
    if !other.is_empty() {
        result.push(("Other", other));
    }

    result
}

/// Grouped model dropdown selector
///
/// NOTE: The spec mentions "search" capability. The initial implementation uses
/// native `<select>` with `<optgroup>` for simplicity. If the model list is large
/// (50+ models), the implementer should upgrade to a custom dropdown with a
/// text filter input at the top. For MVP, native select is acceptable since
/// most providers return <20 models.
#[component]
pub fn ModelSelector(
    /// Available models
    models: Signal<Vec<ModelOption>>,
    /// Currently selected model ID
    selected: RwSignal<Option<String>>,
    /// Recommended/default model ID
    #[prop(optional)]
    recommended: Option<Signal<Option<String>>>,
    /// Show refresh button
    #[prop(default = false)]
    show_refresh: bool,
    /// Refresh callback
    #[prop(optional)]
    on_refresh: Option<Callback<()>>,
    /// Whether refresh is in progress
    #[prop(optional)]
    refreshing: Option<Signal<bool>>,
    /// Show "custom model" input fallback
    #[prop(default = true)]
    allow_custom: bool,
) -> impl IntoView {
    let show_custom_input = RwSignal::new(false);
    let custom_model = RwSignal::new(String::new());
    let recommended = recommended.unwrap_or(Signal::derive(|| None));
    let refreshing = refreshing.unwrap_or(Signal::derive(|| false));

    view! {
        <div class="model-selector">
            <div class="model-selector-header">
                <label class="form-label">"Model"</label>
                {move || show_refresh.then(|| {
                    let on_refresh = on_refresh.clone();
                    view! {
                        <button
                            class="btn-icon btn-xs"
                            title="Refresh model list"
                            disabled=move || refreshing.get()
                            on:click=move |_| {
                                if let Some(ref cb) = on_refresh {
                                    cb.run(());
                                }
                            }
                        >
                            {move || if refreshing.get() { "⟳" } else { "↻" }}
                        </button>
                    }
                })}
            </div>

            {move || {
                if show_custom_input.get() {
                    view! {
                        <div class="custom-model-input">
                            <input
                                type="text"
                                class="form-input"
                                placeholder="Enter model name..."
                                prop:value=move || custom_model.get()
                                on:input=move |ev| {
                                    let val = event_target_value(&ev);
                                    custom_model.set(val.clone());
                                    selected.set(Some(val));
                                }
                            />
                            <button
                                class="btn-link btn-xs"
                                on:click=move |_| show_custom_input.set(false)
                            >
                                "Back to list"
                            </button>
                        </div>
                    }.into_any()
                } else {
                    let models_val = models.get();
                    let groups = group_models(&models_val);
                    view! {
                        <select
                            class="form-select"
                            on:change=move |ev| {
                                let val = event_target_value(&ev);
                                if val == "__custom__" {
                                    show_custom_input.set(true);
                                } else if !val.is_empty() {
                                    selected.set(Some(val));
                                }
                            }
                        >
                            <option value="" disabled selected=move || selected.get().is_none()>
                                "Select a model..."
                            </option>
                            {groups.into_iter().map(|(label, group)| {
                                let recommended_id = recommended.get();
                                view! {
                                    <optgroup label=label>
                                        {group.into_iter().map(|m| {
                                            let is_recommended = recommended_id.as_ref() == Some(&m.id);
                                            let is_selected = selected.get().as_ref() == Some(&m.id);
                                            let base_name = m.name.clone().unwrap_or_else(|| m.id.clone());
                                            let display = if m.source == "preset" {
                                                format!("{} [Preset]", base_name)
                                            } else {
                                                base_name
                                            };
                                            let display = if is_recommended {
                                                format!("⭐ {}", display)
                                            } else {
                                                display
                                            };
                                            let id = m.id.clone();
                                            view! {
                                                <option
                                                    value=id
                                                    selected=is_selected
                                                >
                                                    {display}
                                                </option>
                                            }
                                        }).collect_view()}
                                    </optgroup>
                                }
                            }).collect_view()}
                            {allow_custom.then(|| view! {
                                <option value="__custom__">"✏️ Enter custom model name..."</option>
                            })}
                        </select>
                    }.into_any()
                }
            }}
        </div>
    }
}
```

- [ ] **Step 3: Add to components mod.rs**

Add `pub mod model_selector;` to `apps/panel/src/components/mod.rs`.

- [ ] **Step 4: Verify compilation**

Run: `cd apps/panel && trunk build` (or whatever build command the panel uses)
Expected: compiles

- [ ] **Step 5: Commit**

```bash
git add apps/panel/src/components/model_selector.rs apps/panel/src/components/mod.rs
git commit -m "panel: add grouped ModelSelector component"
```

---

### Task 7: Create `probe_indicator` and `api_key_input` components

**Files:**
- Create: `apps/panel/src/components/probe_indicator.rs`
- Create: `apps/panel/src/components/api_key_input.rs`
- Modify: `apps/panel/src/components/mod.rs`

- [ ] **Step 1: Create probe_indicator component**

Create `apps/panel/src/components/probe_indicator.rs`:

```rust
//! Probe status indicator
//!
//! Displays connection test result: loading spinner, success checkmark, or error.

use leptos::prelude::*;

/// Probe status for display
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeStatus {
    /// No probe performed yet
    Idle,
    /// Probe in progress
    Loading,
    /// Probe succeeded
    Success { latency_ms: u64 },
    /// Probe failed
    Error { message: String },
}

/// Probe status indicator
#[component]
pub fn ProbeIndicator(
    /// Current probe status
    status: Signal<ProbeStatus>,
) -> impl IntoView {
    view! {
        <span class="probe-indicator">
            {move || match status.get() {
                ProbeStatus::Idle => view! { <span></span> }.into_any(),
                ProbeStatus::Loading => view! {
                    <span class="probe-loading" title="Testing connection...">
                        "⟳"
                    </span>
                }.into_any(),
                ProbeStatus::Success { latency_ms } => view! {
                    <span class="probe-success" title=format!("Connected ({}ms)", latency_ms)>
                        {format!("✓ {}ms", latency_ms)}
                    </span>
                }.into_any(),
                ProbeStatus::Error { message } => view! {
                    <span class="probe-error" title=message.clone()>
                        "✗"
                    </span>
                }.into_any(),
            }}
        </span>
    }
}
```

- [ ] **Step 2: Create api_key_input component**

Create `apps/panel/src/components/api_key_input.rs`:

**Important**: The existing `SecretInput` component (`apps/panel/src/components/ui/secret_input.rs`) already provides a password input with show/hide toggle. This component wraps it with auto-probe behavior rather than reimplementing from scratch.

```rust
//! API key input with auto-probe
//!
//! Wraps the existing SecretInput with debounced auto-probe behavior
//! and a ProbeIndicator next to it.

use leptos::prelude::*;
use super::probe_indicator::{ProbeIndicator, ProbeStatus};
use super::ui::secret_input::SecretInput;

/// API key input with auto-probe and status indicator
#[component]
pub fn ApiKeyInput(
    /// Current API key value
    value: RwSignal<String>,
    /// Placeholder text
    #[prop(default = "Enter API key...")]
    placeholder: &'static str,
    /// Probe status signal (controlled externally)
    probe_status: Signal<ProbeStatus>,
    /// Called when key changes (after debounce) — caller triggers probe
    #[prop(optional)]
    on_key_change: Option<Callback<String>>,
    /// Debounce delay in milliseconds
    #[prop(default = 500)]
    debounce_ms: u32,
) -> impl IntoView {
    // Use web_sys for debounce timer (standard WASM approach)
    let timer_id = StoredValue::new(None::<i32>);

    let on_input = move |ev: leptos::ev::Event| {
        let val = event_target_value(&ev);
        value.set(val.clone());

        // Cancel previous timer
        if let Some(id) = timer_id.get_value() {
            web_sys::window().unwrap().clear_timeout_with_handle(id);
        }

        // Start new debounce timer
        if !val.is_empty() {
            if let Some(ref cb) = on_key_change {
                let cb = cb.clone();
                let closure = wasm_bindgen::closure::Closure::once_into_js(move || {
                    cb.run(val);
                });
                let id = web_sys::window()
                    .unwrap()
                    .set_timeout_with_callback_and_timeout_i32(
                        closure.as_ref().unchecked_ref(),
                        debounce_ms as i32,
                    )
                    .unwrap_or(0);
                timer_id.set_value(Some(id));
            }
        }
    };

    view! {
        <div class="api-key-input">
            <div class="input-with-indicator">
                <SecretInput
                    value=value
                    placeholder=placeholder
                    on_input=on_input
                />
                <ProbeIndicator status=probe_status />
            </div>
        </div>
    }
}
```

**Note for implementer**: Check `SecretInput`'s exact prop API — it may use different prop names or callback signatures. Adapt the wrapping code to match. If `SecretInput` doesn't support an `on_input` callback prop, add one or use a wrapper `<div>` with `on:input` event delegation.

- [ ] **Step 3: Export from mod.rs**

Add to `apps/panel/src/components/mod.rs`:

```rust
pub mod probe_indicator;
pub mod api_key_input;
```

- [ ] **Step 4: Verify compilation**

Run: `cd apps/panel && trunk build`
Expected: compiles

- [ ] **Step 5: Commit**

```bash
git add apps/panel/src/components/probe_indicator.rs apps/panel/src/components/api_key_input.rs apps/panel/src/components/mod.rs
git commit -m "panel: add ProbeIndicator and ApiKeyInput shared components"
```

---

### Task 8: Create Setup Wizard view

**Files:**
- Create: `apps/panel/src/views/wizard/mod.rs`
- Create: `apps/panel/src/views/wizard/setup_wizard.rs`
- Modify: `apps/panel/src/views/mod.rs` (add wizard module)

- [ ] **Step 1: Create wizard module**

Create `apps/panel/src/views/wizard/mod.rs`:

```rust
pub mod setup_wizard;
pub use setup_wizard::SetupWizard;
```

- [ ] **Step 2: Create the wizard component**

Create `apps/panel/src/views/wizard/setup_wizard.rs`. This is the main wizard component with 3 steps. Due to size, implement step-by-step:

The wizard uses the presets from `providers.rs` (same source) and calls `ProvidersApi::probe()` / `ProvidersApi::create()` / `ProvidersApi::set_default()`.

Key signals:
```rust
enum WizardStep { SelectProvider, EnterCredentials, SelectModel, Complete }

struct WizardState {
    step: RwSignal<WizardStep>,
    selected_preset: RwSignal<Option<ProviderPresetInfo>>,
    api_key: RwSignal<String>,
    probe_status: RwSignal<ProbeStatus>,
    probe_result: RwSignal<Option<ProbeResult>>,
    selected_model: RwSignal<Option<String>>,
    is_saving: RwSignal<bool>,
}
```

**Pre-requisite: Extract preset data to shared module.**

The `ProviderPreset` struct and `PRESETS` array are currently private in `providers.rs`. Before building the wizard:

1. Create `apps/panel/src/preset_data.rs` with `pub struct ProviderPreset` and `pub static PRESETS: &[ProviderPreset]`
2. Move the preset definitions from `providers.rs` to this new file
3. Update `providers.rs` to import from `preset_data`
4. The wizard will also import from `preset_data`

The implementer should:
1. Create the wizard as a modal overlay (position: fixed, z-index on top)
2. Step 1: Render preset cards (from shared `preset_data::PRESETS`)
3. Step 2: Render `ApiKeyInput` + `ProbeIndicator`, auto-probe on key change
4. Step 3: Render `ModelSelector` with probe results, confirm button
5. Completion: success message + "configure generation?" prompt
6. Close button (X) in top-right + Escape key handler (use `on:keydown` on the overlay `<div>` with tabindex, or `window().add_event_listener` with cleanup on unmount via `on_cleanup`)
7. On confirm: call `ProvidersApi::create()` then `ProvidersApi::set_default()`

**Important implementation notes:**
- For Ollama (no API key, `needs_api_key == false`), skip step 2 and go directly to step 3
- For OAuth providers (`auth_type == "oauth"`), step 2 shows the existing `SubscriptionLoginSection` instead of key input
- Use CSS classes from the existing panel theme (not emoji characters) for icons — check existing components for the pattern

- [ ] **Step 3: Add wizard module to views**

In `apps/panel/src/views/mod.rs`, add:
```rust
pub mod wizard;
```

- [ ] **Step 4: Verify compilation**

Run: `cd apps/panel && trunk build`

- [ ] **Step 5: Commit**

```bash
git add apps/panel/src/views/wizard/
git commit -m "panel: add SetupWizard view with 3-step provider configuration"
```

---

### Task 9: Wire wizard trigger in app startup

**Files:**
- Modify: `apps/panel/src/app.rs`

- [ ] **Step 1: Add wizard trigger logic**

In `apps/panel/src/app.rs`, after the Gateway connection is established:

1. Call `ProvidersApi::needs_setup()`
2. If `needs_setup == true`, show the wizard overlay
3. Use a `RwSignal<bool>` like `show_wizard` to control visibility

Add a signal and the wizard component in `AppContent` or `MainContent`:

```rust
let show_wizard = RwSignal::new(false);

// After connection is established:
spawn_local(async move {
    if let Ok(result) = ProvidersApi::needs_setup(&state).await {
        if result.needs_setup {
            show_wizard.set(true);
        }
    }
});

// In the view:
{move || show_wizard.get().then(|| view! {
    <SetupWizard on_close=move || show_wizard.set(false) />
})}
```

- [ ] **Step 2: Verify compilation**

Run: `cd apps/panel && trunk build`

- [ ] **Step 3: Commit**

```bash
git add apps/panel/src/app.rs
git commit -m "panel: wire setup wizard trigger on first-run detection"
```

---

### Task 10: Enhance settings form with ModelSelector

**Files:**
- Modify: `apps/panel/src/views/settings/providers.rs`

- [ ] **Step 1: Replace model text input with ModelSelector**

In `ProviderDetailPanel` (around line 1137-1149), replace the model `<input>` with:

1. Add a `models_list: RwSignal<Vec<ModelOption>>` signal
2. On provider selection (or API key change), call `ModelsApi::list(provider_name, false)` to populate
3. Replace `<input>` with `<ModelSelector>` component
4. Keep form_model signal synced with ModelSelector's selected signal

- [ ] **Step 2: Add API key auto-probe**

Replace the existing SecretInput for API key with `ApiKeyInput` component:
1. On key change callback → call `ProvidersApi::probe()`
2. Update probe_status signal
3. On success → refresh models_list

- [ ] **Step 3: Add provider status visualization**

In the configured providers list, add:
- Verified dot indicator (green/gray based on `verified` field)
- Model source tag (API/Preset/Manual)

- [ ] **Step 4: Verify compilation**

Run: `cd apps/panel && trunk build`

- [ ] **Step 5: Commit**

```bash
git add apps/panel/src/views/settings/providers.rs
git commit -m "panel: enhance provider form with ModelSelector and auto-probe"
```

---

### Task 11: Final verification

- [ ] **Step 1: Run all backend tests**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -5`
Expected: our new tests pass (pre-existing failures may exist)

- [ ] **Step 2: Run integration tests**

Run: `cargo test -p alephcore --test model_discovery_integration -- --nocapture`
Expected: all tests pass

- [ ] **Step 3: Run shared_ui_logic tests**

Run: `cargo test -p aleph-ui-logic`
Expected: all tests pass

- [ ] **Step 4: Build panel**

Run: `cd apps/panel && trunk build`
Expected: WASM builds successfully

- [ ] **Step 5: Run clippy**

Run: `cargo clippy -p alephcore --tests -- -D warnings 2>&1 | grep -E 'providers|models|probe|needs_setup'`
Expected: no warnings in our new code

- [ ] **Step 6: Commit any fixes**

If any issues found, fix and commit.
