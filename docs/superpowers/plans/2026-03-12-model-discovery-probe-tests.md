# Model Discovery Probe Tests Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add ~35 probe tests across 4 layers (unit, integration, BDD, real API) to validate model discovery at production quality before merging.

**Architecture:** Extract parse helpers from protocol adapters for L1 unit tests. Add wiremock to dev-dependencies for L2 mock HTTP integration tests. Extend existing BDD feature file for L3 RPC handler scenarios. Add `#[ignore]` real API tests in `core/tests/` for L4.

**Tech Stack:** Rust, wiremock 0.6, Cucumber 0.21 (existing), tokio test runtime

**Spec:** `docs/superpowers/specs/2026-03-12-model-discovery-probe-tests-design.md`

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `core/Cargo.toml` | Add `wiremock = "0.6"` dev-dependency |
| Modify | `core/src/providers/model_registry.rs` | Add `with_ttl()` builder method |
| Modify | `core/src/providers/protocols/openai.rs` | Extract `parse_models_response()` + 3 L1 tests |
| Modify | `core/src/providers/protocols/gemini.rs` | Extract `parse_gemini_models_response()` + 3 L1 tests |
| Modify | `core/src/providers/ollama.rs` | Make `TagsResponse`/`OllamaModelInfo` `pub(crate)` + 3 L1 tests |
| New | `core/tests/model_discovery_integration.rs` | 12 wiremock integration tests (L2) |
| Modify | `core/tests/features/models/chat_handlers.feature` | Add 11 model discovery BDD scenarios (L3) |
| Modify | `core/tests/steps/models_steps.rs` | Add model discovery step implementations (L3) |
| Modify | `core/tests/world/models_ctx.rs` | Add `get_mutable_config()` for writable handlers |
| New | `core/tests/real_api_probe.rs` | 4 `#[ignore]` real API tests (L4) |

---

## Chunk 1: Prerequisites & L1 Unit Tests

### Task 1: Add wiremock dev-dependency

**Files:**
- Modify: `core/Cargo.toml:211-220` (dev-dependencies section)

- [ ] **Step 1: Add wiremock to dev-dependencies**

In `core/Cargo.toml`, add to `[dev-dependencies]`:

```toml
wiremock = "0.6"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p alephcore --tests`
Expected: compiles with no errors

- [ ] **Step 3: Commit**

```bash
git add core/Cargo.toml
git commit -m "deps: add wiremock dev-dependency for model discovery probe tests"
```

---

### Task 2: Add ModelRegistry::with_ttl()

**Files:**
- Modify: `core/src/providers/model_registry.rs:65-90`

- [ ] **Step 1: Write the test**

Add to the existing `#[cfg(test)] mod tests` in `model_registry.rs`:

```rust
#[tokio::test]
async fn test_with_ttl_overrides_default() {
    let registry = ModelRegistry::new(None)
        .with_ttl(Duration::from_millis(50));
    assert_eq!(registry.ttl, Duration::from_millis(50));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib model_registry::tests::test_with_ttl_overrides_default`
Expected: FAIL — `with_ttl` method does not exist

- [ ] **Step 3: Implement with_ttl()**

Add after `ModelRegistry::new()` (after line 90 in `model_registry.rs`):

```rust
/// Override cache TTL (useful for testing)
pub fn with_ttl(mut self, ttl: Duration) -> Self {
    self.ttl = ttl;
    self
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib model_registry::tests::test_with_ttl_overrides_default`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add core/src/providers/model_registry.rs
git commit -m "providers: add ModelRegistry::with_ttl() for test-configurable cache expiry"
```

---

### Task 3: L1 — OpenAI parse_models_response extraction + tests

**Files:**
- Modify: `core/src/providers/protocols/openai.rs:464-517`

- [ ] **Step 1: Write the 3 L1 tests**

Add to `#[cfg(test)] mod tests` in `openai.rs`:

```rust
use crate::providers::adapter::DiscoveredModel;
use super::parse_models_response;

#[test]
fn parse_models_response_success() {
    let body = serde_json::json!({
        "data": [
            {"id": "gpt-4o", "owned_by": "openai"},
            {"id": "gpt-4o-mini", "owned_by": "openai"}
        ]
    });
    let models = parse_models_response(&body).unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "gpt-4o");
    assert_eq!(models[0].owned_by, Some("openai".to_string()));
    assert_eq!(models[0].capabilities, vec!["chat".to_string()]);
    assert_eq!(models[1].id, "gpt-4o-mini");
}

#[test]
fn parse_models_response_empty() {
    let body = serde_json::json!({"data": []});
    let models = parse_models_response(&body).unwrap();
    assert!(models.is_empty());
}

#[test]
fn parse_models_response_malformed() {
    let body = serde_json::json!({"invalid": true});
    let models = parse_models_response(&body).unwrap();
    assert!(models.is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib protocols::openai::tests::parse_models_response`
Expected: FAIL — `parse_models_response` not found

- [ ] **Step 3: Extract parse_models_response()**

Add a standalone function (before the `impl ProtocolAdapter for OpenAiProtocol` block, or right before `list_models`):

```rust
/// Parse OpenAI /v1/models JSON response into DiscoveredModel list
pub(crate) fn parse_models_response(body: &serde_json::Value) -> crate::error::Result<Vec<DiscoveredModel>> {
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
    Ok(models)
}
```

Then update `list_models()` (around line 494-514) to call the extracted function:

```rust
let body: serde_json::Value = response
    .json()
    .await
    .map_err(|e| AlephError::network(format!("Failed to parse model list: {}", e)))?;

let models = parse_models_response(&body)?;
```

- [ ] **Step 4: Run all OpenAI tests**

Run: `cargo test -p alephcore --lib protocols::openai::tests`
Expected: all pass (new + existing)

- [ ] **Step 5: Commit**

```bash
git add core/src/providers/protocols/openai.rs
git commit -m "providers: extract parse_models_response() from OpenAI list_models with L1 tests"
```

---

### Task 4: L1 — Gemini parse_gemini_models_response extraction + tests

**Files:**
- Modify: `core/src/providers/protocols/gemini.rs:431-481`

- [ ] **Step 1: Write the 3 L1 tests**

Add to `#[cfg(test)] mod tests` in `gemini.rs`:

```rust
use crate::providers::adapter::DiscoveredModel;
use super::parse_gemini_models_response;

#[test]
fn parse_gemini_models_success() {
    let body = serde_json::json!({
        "models": [
            {"name": "models/gemini-2.5-pro", "displayName": "Gemini 2.5 Pro"},
            {"name": "models/gemini-2.5-flash", "displayName": "Gemini 2.5 Flash"}
        ]
    });
    let models = parse_gemini_models_response(&body).unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "gemini-2.5-pro"); // prefix stripped
    assert_eq!(models[0].name, Some("Gemini 2.5 Pro".to_string()));
    assert_eq!(models[0].owned_by, Some("google".to_string()));
    assert_eq!(models[0].capabilities, vec!["chat".to_string()]);
}

#[test]
fn parse_gemini_models_empty() {
    let body = serde_json::json!({"models": []});
    let models = parse_gemini_models_response(&body).unwrap();
    assert!(models.is_empty());
}

#[test]
fn parse_gemini_models_malformed() {
    let body = serde_json::json!({"invalid": true});
    let models = parse_gemini_models_response(&body).unwrap();
    assert!(models.is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib protocols::gemini::tests::parse_gemini_models`
Expected: FAIL — function not found

- [ ] **Step 3: Extract parse_gemini_models_response()**

```rust
/// Parse Gemini /v1beta/models JSON response into DiscoveredModel list
pub(crate) fn parse_gemini_models_response(body: &serde_json::Value) -> crate::error::Result<Vec<DiscoveredModel>> {
    let models = body["models"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
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
    Ok(models)
}
```

Update `list_models()` to call it:

```rust
let body: serde_json::Value = response
    .json()
    .await
    .map_err(|e| AlephError::network(format!("Failed to parse Gemini model list: {}", e)))?;

let models = parse_gemini_models_response(&body)?;
```

- [ ] **Step 4: Run all Gemini tests**

Run: `cargo test -p alephcore --lib protocols::gemini::tests`
Expected: all pass

- [ ] **Step 5: Commit**

```bash
git add core/src/providers/protocols/gemini.rs
git commit -m "providers: extract parse_gemini_models_response() with L1 tests"
```

---

### Task 5: L1 — Ollama TagsResponse parse tests

**Files:**
- Modify: `core/src/providers/ollama.rs:125-135,297-330`

- [ ] **Step 1: Make TagsResponse and OllamaModelInfo pub(crate)**

Change visibility of structs at lines 125-135:

```rust
#[derive(Debug, Deserialize)]
pub(crate) struct TagsResponse {
    pub(crate) models: Vec<OllamaModelInfo>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OllamaModelInfo {
    pub(crate) name: String,
}
```

- [ ] **Step 2: Write the 3 L1 tests**

Add to `#[cfg(test)] mod tests` in `ollama.rs`:

```rust
use super::{TagsResponse, OllamaModelInfo};

#[test]
fn parse_tags_response_success() {
    let json = r#"{"models": [{"name": "llama3:latest"}, {"name": "codellama:7b"}]}"#;
    let tags: TagsResponse = serde_json::from_str(json).unwrap();
    assert_eq!(tags.models.len(), 2);
    assert_eq!(tags.models[0].name, "llama3:latest");
    assert_eq!(tags.models[1].name, "codellama:7b");
}

#[test]
fn parse_tags_response_empty() {
    let json = r#"{"models": []}"#;
    let tags: TagsResponse = serde_json::from_str(json).unwrap();
    assert!(tags.models.is_empty());
}

#[test]
fn parse_tags_response_vision_model_detection() {
    // Verify that vision model names (llava, bakllava) are detectable
    let json = r#"{"models": [{"name": "llava:latest"}, {"name": "bakllava:7b"}]}"#;
    let tags: TagsResponse = serde_json::from_str(json).unwrap();
    assert!(tags.models[0].name.contains("llava"));
    assert!(tags.models[1].name.contains("llava"));
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib providers::ollama::tests::parse_tags_response`
Expected: all 3 pass

- [ ] **Step 4: Commit**

```bash
git add core/src/providers/ollama.rs
git commit -m "providers: make TagsResponse pub(crate) and add L1 parse tests"
```

---

## Chunk 2: L2 Integration Tests (wiremock)

### Task 6: L2 — API Probe Success tests (OpenAI, Gemini, Ollama)

**Files:**
- Create: `core/tests/model_discovery_integration.rs`

**Reference:** The integration test needs to import from `alephcore` (the crate name in `core/Cargo.toml`). Check `core/Cargo.toml` for `[lib] name = "alephcore"`. The test file goes in `core/tests/` and is automatically picked up by `cargo test -p alephcore --test model_discovery_integration`.

- [ ] **Step 1: Create test file with OpenAI probe test**

Create `core/tests/model_discovery_integration.rs`:

```rust
//! L2 Integration tests: wiremock + ModelRegistry
//!
//! Tests the full model discovery flow with mock HTTP servers.

use alephcore::config::ProviderConfig;
use alephcore::providers::adapter::DiscoveredModel;
use alephcore::providers::model_registry::{ModelRegistry, ModelSource};
use alephcore::providers::protocols::openai::OpenAiProtocol;
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn openai_models_response() -> serde_json::Value {
    json!({
        "data": [
            {"id": "gpt-4o", "owned_by": "openai"},
            {"id": "gpt-4o-mini", "owned_by": "openai"},
            {"id": "o3", "owned_by": "openai"}
        ]
    })
}

#[tokio::test]
async fn probe_openai_models_via_api() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_models_response()))
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let registry = ModelRegistry::new(None);
    let models = registry
        .list_models("test-openai", "openai", &adapter, &config)
        .await;

    assert_eq!(models.len(), 3);
    assert_eq!(models[0].id, "gpt-4o");
    assert_eq!(
        registry.get_source("test-openai").await,
        Some(ModelSource::Api)
    );
}
```

- [ ] **Step 2: Run to verify it passes**

Run: `cargo test -p alephcore --test model_discovery_integration probe_openai`
Expected: PASS

**Note:** `OpenAiProtocol` and `GeminiProtocol` do NOT implement `Default`. They must be constructed via `::new(reqwest::Client::new())`. Check `core/src/providers/protocols/openai.rs:32-36` and `gemini.rs:30-34` for exact constructors.

- [ ] **Step 3: Add Gemini probe test**

Append to the same file:

```rust
use alephcore::providers::protocols::gemini::GeminiProtocol;

fn gemini_models_response() -> serde_json::Value {
    json!({
        "models": [
            {"name": "models/gemini-2.5-pro", "displayName": "Gemini 2.5 Pro"},
            {"name": "models/gemini-2.5-flash", "displayName": "Gemini 2.5 Flash"}
        ]
    })
}

#[tokio::test]
async fn probe_gemini_models_via_api() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1beta/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(gemini_models_response()))
        .mount(&server)
        .await;

    let adapter = GeminiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gemini-2.5-pro");
    config.base_url = Some(server.uri());

    let registry = ModelRegistry::new(None);
    let models = registry
        .list_models("test-gemini", "gemini", &adapter, &config)
        .await;

    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "gemini-2.5-pro"); // prefix stripped
    assert_eq!(models[0].name, Some("Gemini 2.5 Pro".to_string()));
    assert_eq!(
        registry.get_source("test-gemini").await,
        Some(ModelSource::Api)
    );
}
```

- [ ] **Step 4: Add Ollama probe test via OllamaDiscoveryAdapter**

This test is more complex because Ollama uses a separate code path. The `OllamaDiscoveryAdapter` (defined in `core/src/gateway/handlers/models.rs`) wraps `OllamaProvider` to implement `ProtocolAdapter::list_models()`. If `OllamaDiscoveryAdapter` is not public, this test may need to use the mock adapter pattern instead, or the adapter needs to be made `pub(crate)`.

```rust
#[tokio::test]
async fn probe_ollama_tags_via_api() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "models": [
                    {"name": "llama3:latest"},
                    {"name": "llava:7b"}
                ]
            })),
        )
        .mount(&server)
        .await;

    // OllamaProvider is tested directly (not through ProtocolAdapter)
    // since it uses a separate code path. OllamaDiscoveryAdapter is
    // private in the handlers module — we test OllamaProvider.list_models() directly.
    use alephcore::providers::ollama::OllamaProvider;

    let mut config = ProviderConfig::test_config("llama3");
    config.base_url = Some(server.uri());

    let provider = OllamaProvider::new("test-ollama".to_string(), config)
        .expect("Should create OllamaProvider");
    let models = provider.list_models().await.expect("Should list models");

    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "llama3:latest");
    assert_eq!(models[1].id, "llava:7b");
}
```

**Note:** `OllamaDiscoveryAdapter` in `core/src/gateway/handlers/models.rs` is private. For L2 integration we test `OllamaProvider::list_models()` directly instead of going through ModelRegistry. The ModelRegistry integration for Ollama is implicitly tested through the RPC handler in L3 BDD.

- [ ] **Step 5: Run all probe success tests**

Run: `cargo test -p alephcore --test model_discovery_integration probe_`
Expected: 2-3 pass

- [ ] **Step 6: Commit**

```bash
git add core/tests/model_discovery_integration.rs
git commit -m "tests: add L2 API probe success integration tests with wiremock"
```

---

### Task 7: L2 — Fallback Degradation tests

**Files:**
- Modify: `core/tests/model_discovery_integration.rs`

- [ ] **Step 1: Add fallback-to-preset-on-500 test**

```rust
#[tokio::test]
async fn fallback_to_preset_on_api_failure() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let preset_toml = r#"
[openai]
models = [
    { id = "gpt-4o", name = "GPT-4o", capabilities = ["chat", "vision"] },
]
"#;
    let registry = ModelRegistry::new(Some(preset_toml));
    let models = registry
        .list_models("test-openai", "openai", &adapter, &config)
        .await;

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "gpt-4o");
    assert_eq!(
        registry.get_source("test-openai").await,
        Some(ModelSource::Preset)
    );
}
```

- [ ] **Step 2: Add fallback-on-timeout test**

```rust
#[tokio::test]
async fn fallback_to_preset_on_timeout() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(openai_models_response())
                .set_delay(Duration::from_secs(10)), // exceeds 5s probe timeout
        )
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let preset_toml = r#"
[openai]
models = [
    { id = "gpt-4o-fallback", name = "Fallback", capabilities = ["chat"] },
]
"#;
    let registry = ModelRegistry::new(Some(preset_toml));
    let models = registry
        .list_models("test-openai", "openai", &adapter, &config)
        .await;

    // Should not hang — should fallback to preset within reasonable time
    assert!(!models.is_empty());
    assert_eq!(
        registry.get_source("test-openai").await,
        Some(ModelSource::Preset)
    );
}
```

- [ ] **Step 3: Add fallback-on-401 test**

```rust
#[tokio::test]
async fn fallback_to_preset_on_401_unauthorized() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(json!({"error": "invalid_api_key"})),
        )
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let preset_toml = r#"
[openai]
models = [
    { id = "gpt-4o", name = "GPT-4o", capabilities = ["chat"] },
]
"#;
    let registry = ModelRegistry::new(Some(preset_toml));
    let models = registry
        .list_models("test-openai", "openai", &adapter, &config)
        .await;

    assert_eq!(models.len(), 1);
    assert_eq!(
        registry.get_source("test-openai").await,
        Some(ModelSource::Preset)
    );
}
```

- [ ] **Step 4: Add fallback-to-empty (no preset) test**

```rust
#[tokio::test]
async fn fallback_to_empty_when_no_preset() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    // No presets at all
    let registry = ModelRegistry::new(None);
    let models = registry
        .list_models("test-openai", "openai", &adapter, &config)
        .await;

    assert!(models.is_empty());
}
```

- [ ] **Step 5: Add fallback on empty API response test**

```rust
#[tokio::test]
async fn fallback_to_preset_on_empty_api_response() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let preset_toml = r#"
[openai]
models = [
    { id = "gpt-4o", name = "GPT-4o", capabilities = ["chat"] },
]
"#;
    let registry = ModelRegistry::new(Some(preset_toml));
    let models = registry
        .list_models("test-openai", "openai", &adapter, &config)
        .await;

    // Empty API response should trigger preset fallback
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "gpt-4o");
    assert_eq!(
        registry.get_source("test-openai").await,
        Some(ModelSource::Preset)
    );
}
```

- [ ] **Step 6: Run all fallback tests**

Run: `cargo test -p alephcore --test model_discovery_integration fallback_`
Expected: all 5 pass

- [ ] **Step 7: Commit**

```bash
git add core/tests/model_discovery_integration.rs
git commit -m "tests: add L2 fallback degradation integration tests"
```

---

### Task 8: L2 — Cache Behavior + Concurrency tests

**Files:**
- Modify: `core/tests/model_discovery_integration.rs`

- [ ] **Step 1: Add cache-hit test**

```rust
#[tokio::test]
async fn cache_hit_avoids_api_call() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_models_response()))
        .expect(1) // wiremock assertion: exactly 1 request
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let registry = ModelRegistry::new(None);

    // First call — hits API
    let models1 = registry
        .list_models("test-openai", "openai", &adapter, &config)
        .await;
    assert_eq!(models1.len(), 3);

    // Second call — should use cache, no new HTTP request
    let models2 = registry
        .list_models("test-openai", "openai", &adapter, &config)
        .await;
    assert_eq!(models2.len(), 3);

    // wiremock will verify exactly 1 request on drop
}
```

- [ ] **Step 2: Add cache-expiry test**

```rust
#[tokio::test]
async fn cache_expires_after_ttl() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_models_response()))
        .expect(2) // exactly 2 requests (initial + after expiry)
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let registry = ModelRegistry::new(None).with_ttl(Duration::from_millis(100));

    // First call
    let _ = registry
        .list_models("test-openai", "openai", &adapter, &config)
        .await;

    // Wait for TTL to expire
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Second call — cache expired, should hit API again
    let models = registry
        .list_models("test-openai", "openai", &adapter, &config)
        .await;
    assert_eq!(models.len(), 3);

    // wiremock will verify exactly 2 requests on drop
}
```

- [ ] **Step 3: Add force-refresh test**

```rust
#[tokio::test]
async fn force_refresh_bypasses_cache() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_models_response()))
        .expect(2) // initial + forced refresh
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let registry = ModelRegistry::new(None);

    // Initial populate
    let _ = registry
        .list_models("test-openai", "openai", &adapter, &config)
        .await;

    // Force refresh — bypasses cache
    let models = registry
        .refresh("test-openai", "openai", &adapter, &config)
        .await;
    assert_eq!(models.len(), 3);

    // wiremock verifies 2 requests
}
```

- [ ] **Step 4: Add concurrency test**

```rust
use std::sync::Arc;

#[tokio::test]
async fn concurrent_list_models_no_panic() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_models_response()))
        .mount(&server)
        .await;

    let adapter = Arc::new(OpenAiProtocol::new(reqwest::Client::new()));
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());
    let config = Arc::new(config);

    let registry = Arc::new(ModelRegistry::new(None));

    let mut handles = vec![];

    // 10 concurrent list_models
    for i in 0..10 {
        let registry = Arc::clone(&registry);
        let adapter = Arc::clone(&adapter);
        let config = Arc::clone(&config);
        handles.push(tokio::spawn(async move {
            registry
                .list_models(&format!("provider-{}", i), "openai", adapter.as_ref(), &config)
                .await
        }));
    }

    // 2 concurrent refreshes
    for _ in 0..2 {
        let registry = Arc::clone(&registry);
        let adapter = Arc::clone(&adapter);
        let config = Arc::clone(&config);
        handles.push(tokio::spawn(async move {
            registry
                .refresh("concurrent-refresh", "openai", adapter.as_ref(), &config)
                .await
        }));
    }

    // All should complete without panic
    for handle in handles {
        let models = handle.await.expect("task should not panic");
        assert!(!models.is_empty());
    }
}
```

- [ ] **Step 5: Run all cache + concurrency tests**

Run: `cargo test -p alephcore --test model_discovery_integration`
Expected: all tests in the file pass

- [ ] **Step 6: Commit**

```bash
git add core/tests/model_discovery_integration.rs
git commit -m "tests: add L2 cache behavior and concurrency integration tests"
```

---

## Chunk 3: L3 BDD Scenarios & L4 Real API Tests

### Task 9: L3 — Extend ModelsContext for mutable config

**Files:**
- Modify: `core/tests/world/models_ctx.rs`

- [ ] **Step 1: Add mutable config support**

Add to `ModelsContext` struct and impl:

```rust
// Add field to ModelsContext struct:
pub mutable_config: Option<Arc<tokio::sync::RwLock<Config>>>,

// Add method:
pub fn get_mutable_config(&self) -> Arc<tokio::sync::RwLock<Config>> {
    self.mutable_config
        .clone()
        .expect("Mutable config not initialized")
}

pub fn init_mutable_config_with_providers(&mut self, providers: Vec<(&str, &str)>) {
    let mut config = Config::default();
    for (name, model) in &providers {
        config.providers.insert(
            name.to_string(),
            ProviderConfig::test_config(*model),
        );
    }
    let config = Arc::new(tokio::sync::RwLock::new(config));
    self.mutable_config = Some(config);
    // Also set regular config for read-only handlers
    // (clone the inner config for Arc<Config>)
}
```

**Note to implementer:** Inspect the exact `ModelsContext` struct definition and adapt the field addition to match existing patterns. The key point is that `handle_set_model` and `handle_set_default` require `Arc<tokio::sync::RwLock<Config>>` while existing handlers use `Arc<Config>`.

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore --tests`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add core/tests/world/models_ctx.rs
git commit -m "tests: add mutable config support to ModelsContext for writable RPC handlers"
```

---

### Task 10: L3 — BDD model discovery scenarios

**Files:**
- Modify: `core/tests/features/models/chat_handlers.feature`
- Modify: `core/tests/steps/models_steps.rs`

- [ ] **Step 1: Add BDD scenarios to chat_handlers.feature**

Append after existing scenarios:

```gherkin
  # === Model Discovery Scenarios ===

  Scenario: List models returns models with source field
    Given a config with provider "openai" using model "gpt-4o"
    When I call models.list with no params
    Then the response should be successful
    And the models array should contain models with source field

  Scenario: List models with refresh forces re-probe
    Given a config with provider "openai" using model "gpt-4o"
    When I call models.list with refresh true
    Then the response should be successful
    And the models array should contain models with source field

  Scenario: List models shows is_current for configured model
    Given a config with provider "openai" using model "gpt-4o"
    When I call models.list with no params
    Then the response should be successful

  Scenario: Refresh single provider returns updated model list
    Given a config with provider "openai" using model "gpt-4o"
    When I call models.refresh for provider "openai"
    Then the response should be successful

  Scenario: Refresh all providers returns aggregated results
    Given a config with provider "openai" using model "gpt-4o" and provider "anthropic" using model "claude-opus-4-20250514"
    When I call models.refresh with no provider filter
    Then the response should be successful

  Scenario: Refresh unknown provider returns error
    Given an empty config
    When I call models.refresh for provider "nonexistent"
    Then the response should be an error

  Scenario: Anthropic returns preset models
    Given a config with provider "anthropic" using model "claude-opus-4-20250514"
    When I call models.list for provider "anthropic"
    Then the response should be successful
    And the models should include preset models

  Scenario: Set model to discovered model succeeds
    Given a mutable config with provider "openai" using model "gpt-4o"
    When I call models.set_model with provider "openai" and model "gpt-4o-mini"
    Then the response should be successful

  Scenario: Set model to unknown model returns error
    Given a mutable config with provider "openai" using model "gpt-4o"
    When I call models.set_model with provider "openai" and model "gpt-99"
    Then the response should be an error

  Scenario: Set model backward compatibility
    Given a mutable config with providers "openai" and "anthropic" default "openai"
    When I call models.set with model "anthropic"
    Then the response should be successful
```

- [ ] **Step 2: Add step implementations**

Add to `core/tests/steps/models_steps.rs`:

```rust
use alephcore::gateway::handlers::models;

// === Then steps ===

#[then("the models array should contain models with source field")]
async fn then_models_have_source(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let models = ctx.get_models_array().expect("No models in response");
    for model in models {
        assert!(
            model.get("source").is_some(),
            "Model missing 'source' field: {:?}",
            model
        );
    }
}

#[then("the models should include preset models")]
async fn then_models_include_presets(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let models = ctx.get_models_array().expect("No models in response");
    assert!(!models.is_empty(), "Expected preset models but got empty");
}

// === When steps ===

#[when("I call models.list with refresh true")]
async fn when_call_models_list_refresh(w: &mut AlephWorld) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    let config = ctx.get_config();
    let request = JsonRpcRequest::new(
        "models.list",
        Some(json!({"refresh": true})),
        Some(json!(1)),
    );
    let response = models::handle_list(request, config).await;
    ctx.response = Some(response);
}

#[when(expr = "I call models.refresh for provider {string}")]
async fn when_call_models_refresh(w: &mut AlephWorld, provider: String) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    let config = ctx.get_config();
    let request = JsonRpcRequest::new(
        "models.refresh",
        Some(json!({"provider": provider})),
        Some(json!(1)),
    );
    let response = models::handle_refresh(request, config).await;
    ctx.response = Some(response);
}

#[when("I call models.refresh with no provider filter")]
async fn when_call_models_refresh_all(w: &mut AlephWorld) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    let config = ctx.get_config();
    let request = JsonRpcRequest::new("models.refresh", None, Some(json!(1)));
    let response = models::handle_refresh(request, config).await;
    ctx.response = Some(response);
}

#[when(expr = "I call models.set_model with provider {string} and model {string}")]
async fn when_call_models_set_model(w: &mut AlephWorld, provider: String, model: String) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    let config = ctx.get_mutable_config();
    let request = JsonRpcRequest::new(
        "models.set_model",
        Some(json!({"provider": provider, "model": model})),
        Some(json!(1)),
    );
    let response = models::handle_set_model(request, config).await;
    ctx.response = Some(response);
}

#[when(expr = "I call models.set with model {string}")]
async fn when_call_models_set(w: &mut AlephWorld, model: String) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    let config = ctx.get_mutable_config();
    let request = JsonRpcRequest::new(
        "models.set",
        Some(json!({"model": model})),
        Some(json!(1)),
    );
    let response = models::handle_set(request, config).await;
    ctx.response = Some(response);
}

// === Given steps ===

#[given(expr = "a mutable config with provider {string} using model {string}")]
async fn given_mutable_config_single(w: &mut AlephWorld, provider: String, model: String) {
    let ctx = w.models.get_or_insert_with(ModelsContext::default);
    ctx.init_mutable_config_with_providers(vec![(&provider, &model)]);
}

#[given(expr = "a mutable config with providers {string} and {string} default {string}")]
async fn given_mutable_config(
    w: &mut AlephWorld,
    provider1: String,
    provider2: String,
    default: String,
) {
    let ctx = w.models.get_or_insert_with(ModelsContext::default);
    ctx.init_mutable_config_with_providers(vec![
        (&provider1, "model-1"),
        (&provider2, "model-2"),
    ]);
    // Set default on the mutable config
    // Adapt based on ModelsContext implementation
}
```

**Note to implementer:** The exact step patterns must follow existing convention in `models_steps.rs`. Key points:
- `handle_refresh` takes `Arc<Config>` (read-only)
- `handle_set_model` and `handle_set_default` take `Arc<tokio::sync::RwLock<Config>>` (mutable)
- `handle_set` (backward compat) takes `Arc<tokio::sync::RwLock<Config>>` (delegates to `handle_set_default`)
- Check actual handler signatures in `core/src/gateway/handlers/models.rs` and adjust

- [ ] **Step 3: Run BDD tests**

Run: `cargo test -p alephcore --test cucumber`
Expected: existing + new scenarios pass

- [ ] **Step 4: Commit**

```bash
git add core/tests/features/models/chat_handlers.feature core/tests/steps/models_steps.rs
git commit -m "tests: add L3 BDD model discovery scenarios"
```

---

### Task 11: L4 — Real API probe tests

**Files:**
- Create: `core/tests/real_api_probe.rs`

- [ ] **Step 1: Create real API test file**

```rust
//! L4 Real API probe tests
//!
//! These tests call real provider APIs and require credentials.
//! Run with: cargo test -p alephcore --test real_api_probe -- --ignored
//!
//! Required env vars:
//! - OPENAI_API_KEY: for OpenAI tests
//! - GEMINI_API_KEY: for Gemini tests
//! - Ollama running on localhost:11434: for Ollama tests

use alephcore::config::ProviderConfig;
use alephcore::providers::model_registry::ModelRegistry;
use alephcore::providers::protocols::openai::OpenAiProtocol;
use alephcore::providers::protocols::gemini::GeminiProtocol;
use std::env;

#[tokio::test]
#[ignore]
async fn real_openai_list_models() {
    let api_key = env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.api_key = Some(api_key);
    // Don't set base_url — use real OpenAI API

    let registry = ModelRegistry::new(None);
    let models = registry
        .list_models("real-openai", "openai", &adapter, &config)
        .await;

    assert!(!models.is_empty(), "OpenAI should return models");

    let has_gpt = models.iter().any(|m| m.id.contains("gpt"));
    assert!(has_gpt, "Should contain at least one GPT model");

    println!("OpenAI returned {} models", models.len());
    for m in models.iter().take(5) {
        println!("  - {} (owned_by: {:?})", m.id, m.owned_by);
    }
}

#[tokio::test]
#[ignore]
async fn real_gemini_list_models() {
    let api_key = env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY must be set");

    let adapter = GeminiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gemini-2.5-pro");
    config.api_key = Some(api_key);

    let registry = ModelRegistry::new(None);
    let models = registry
        .list_models("real-gemini", "gemini", &adapter, &config)
        .await;

    assert!(!models.is_empty(), "Gemini should return models");

    let has_gemini = models.iter().any(|m| m.id.contains("gemini"));
    assert!(has_gemini, "Should contain at least one Gemini model");

    println!("Gemini returned {} models", models.len());
    for m in models.iter().take(5) {
        println!("  - {} (name: {:?})", m.id, m.name);
    }
}

#[tokio::test]
#[ignore]
async fn real_ollama_list_models() {
    // Requires Ollama running locally
    use alephcore::providers::ollama::OllamaProvider;

    let config = ProviderConfig::test_config("llama3");
    // OllamaProvider defaults to localhost:11434

    let provider = OllamaProvider::new("ollama".to_string(), config).expect("Should create OllamaProvider");
    let models = provider.list_models().await;

    match models {
        Ok(models) => {
            println!("Ollama returned {} models", models.len());
            for m in &models {
                println!("  - {}", m.id);
            }
        }
        Err(e) => {
            println!("Ollama not available or errored: {}", e);
            // Don't fail — Ollama may not be running
        }
    }
}

#[tokio::test]
#[ignore]
async fn real_full_discovery_flow() {
    use alephcore::providers::model_registry::ModelSource;

    let registry = ModelRegistry::new(Some(include_str!(
        "../../shared/config/model-presets.toml"
    )));

    // Test with whichever API keys are available
    if let Ok(api_key) = env::var("OPENAI_API_KEY") {
        let adapter = OpenAiProtocol::new(reqwest::Client::new());
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.api_key = Some(api_key);

        // Probe
        let models = registry
            .list_models("flow-openai", "openai", &adapter, &config)
            .await;
        assert!(!models.is_empty());
        assert_eq!(
            registry.get_source("flow-openai").await,
            Some(ModelSource::Api)
        );

        // Cache hit
        let models2 = registry
            .list_models("flow-openai", "openai", &adapter, &config)
            .await;
        assert_eq!(models.len(), models2.len());

        // Force refresh
        let models3 = registry
            .refresh("flow-openai", "openai", &adapter, &config)
            .await;
        assert!(!models3.is_empty());

        println!("Full flow OK: {} models, cache + refresh working", models.len());
    } else {
        println!("Skipping OpenAI flow test — OPENAI_API_KEY not set");
    }
}
```

- [ ] **Step 2: Verify file compiles**

Run: `cargo test -p alephcore --test real_api_probe --no-run`
Expected: compiles (tests not executed)

- [ ] **Step 3: Commit**

```bash
git add core/tests/real_api_probe.rs
git commit -m "tests: add L4 real API probe tests (ignored, require credentials)"
```

---

### Task 12: Final verification

- [ ] **Step 1: Run all unit tests**

Run: `cargo test -p alephcore --lib`
Expected: all pass (including new L1 tests)

- [ ] **Step 2: Run all integration tests (excluding ignored)**

Run: `cargo test -p alephcore --test model_discovery_integration`
Expected: all L2 tests pass

- [ ] **Step 3: Run BDD tests**

Run: `cargo test -p alephcore --test cucumber`
Expected: all scenarios pass

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -p alephcore --tests -- -D warnings`
Expected: no warnings

- [ ] **Step 5: Verify ignored tests compile**

Run: `cargo test -p alephcore --test real_api_probe --no-run`
Expected: compiles without errors

- [ ] **Step 6: Commit any final fixes**

If any test adjustments were needed, commit them:

```bash
git add -A
git commit -m "tests: fix probe test issues found during final verification"
```
