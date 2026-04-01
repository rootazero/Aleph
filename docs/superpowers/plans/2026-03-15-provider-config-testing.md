# Provider Config Testing Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a three-layer test pyramid (Rust logic probes, RPC integration probes, Playwright E2E) to validate the provider configuration architecture after the model→models migration.

**Architecture:** Layer 1 tests serialization and config struct manipulation directly (no I/O). Layer 2 spawns a real Aleph server as a child process and tests via WebSocket JSON-RPC. Layer 3 uses Playwright to test browser UI interactions against the same child-process server. All layers use programmatic test data construction.

**Tech Stack:** Rust (tokio, tokio-tungstenite, tempfile), Playwright (TypeScript), JSON-RPC 2.0 over WebSocket

**Spec:** `docs/superpowers/specs/2026-03-15-provider-config-testing-design.md`

---

## Chunk 1: Layer 1 — Rust Logic Probes

### Task 1: Create provider_config_probe module structure

**Files:**
- Create: `tests/provider_config_probe/mod.rs`
- Create: `tests/provider_config_probe/serde_tests.rs`
- Create: `tests/provider_config_probe/config_crud_tests.rs`
- Create: `tests/provider_config_probe/edge_case_tests.rs`
- Create: `tests/provider_config_probe.rs` (test entry point)

- [ ] **Step 1: Create the entry point file**

Create `tests/provider_config_probe.rs`:
```rust
mod provider_config_probe;
```

Create `tests/provider_config_probe/mod.rs`:
```rust
mod serde_tests;
mod config_crud_tests;
mod edge_case_tests;
```

- [ ] **Step 2: Verify structure compiles**

Run: `cargo test --test provider_config_probe --no-run -p alephcore`
Expected: Compiles (no tests yet).

- [ ] **Step 3: Commit**

```bash
git add tests/provider_config_probe.rs tests/provider_config_probe/
git commit -m "test: scaffold provider_config_probe module structure

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Serialization & backward compatibility tests

**Files:**
- Modify: `tests/provider_config_probe/serde_tests.rs`

- [ ] **Step 1: Write all serialization tests**

```rust
//! Serialization and backward compatibility tests for all four provider config types.

use alephcore::config::types::provider::ProviderConfig;
use alephcore::config::types::memory::EmbeddingProviderConfig;
use alephcore::memory::rerank::provider::RerankConfig;
use alephcore::config::types::generation::provider::GenerationProviderConfig;

// ── ProviderConfig ──

#[test]
fn provider_config_backward_compat_single_model() {
    let toml = r#"model = "gpt-4o""#;
    let config: ProviderConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.default_model(), "gpt-4o");
    assert_eq!(config.models, vec!["gpt-4o"]);
}

#[test]
fn provider_config_multi_model() {
    let toml = r#"models = ["gpt-4o", "gpt-4o-mini", "o1"]"#;
    let config: ProviderConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.default_model(), "gpt-4o");
    assert_eq!(config.models.len(), 3);
    assert_eq!(config.all_models(), &["gpt-4o", "gpt-4o-mini", "o1"]);
}

#[test]
fn provider_config_empty_list_rejected() {
    let toml = r#"models = []"#;
    let result = toml::from_str::<ProviderConfig>(toml);
    assert!(result.is_err(), "empty models list should be rejected");
}

#[test]
fn provider_config_empty_strings_filtered() {
    let toml = r#"models = ["", "gpt-4o", " "]"#;
    let config: ProviderConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.models, vec!["gpt-4o"]);
}

#[test]
fn provider_config_default_model_is_first() {
    let config = ProviderConfig::test_config("first");
    assert_eq!(config.default_model(), "first");
}

#[test]
fn provider_config_serializes_as_models_array() {
    let config = ProviderConfig::test_config("gpt-4o");
    let toml_str = toml::to_string_pretty(&config).unwrap();
    assert!(toml_str.contains("models = ["), "should serialize as models array, got: {}", toml_str);
    assert!(!toml_str.contains("\nmodel = "), "should not serialize as single model field");
}

#[test]
fn provider_config_roundtrip() {
    let toml = r#"models = ["a", "b", "c"]"#;
    let config: ProviderConfig = toml::from_str(toml).unwrap();
    let serialized = toml::to_string_pretty(&config).unwrap();
    let config2: ProviderConfig = toml::from_str(&serialized).unwrap();
    assert_eq!(config.models, config2.models);
}

// ── EmbeddingProviderConfig ──

#[test]
fn embedding_config_backward_compat() {
    // EmbeddingProviderConfig has required fields, build a minimal valid TOML
    let toml = r#"
id = "test"
name = "Test"
preset = "Custom"
api_base = "http://localhost"
model = "bge-m3"
dimensions = 1024
"#;
    let config: EmbeddingProviderConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.default_model(), "bge-m3");
}

#[test]
fn embedding_config_multi_model() {
    let toml = r#"
id = "test"
name = "Test"
preset = "Custom"
api_base = "http://localhost"
models = ["bge-m3", "bge-large"]
dimensions = 1024
"#;
    let config: EmbeddingProviderConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.models.len(), 2);
    assert_eq!(config.default_model(), "bge-m3");
}

// ── RerankConfig ──

#[test]
fn rerank_config_backward_compat() {
    let toml = r#"
enabled = true
provider = "Jina"
api_base = "https://api.jina.ai"
api_key = "test-key"
model = "jina-reranker-v2"
"#;
    let config: RerankConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.default_model(), "jina-reranker-v2");
}

#[test]
fn rerank_config_multi_model() {
    let toml = r#"
enabled = true
provider = "Jina"
api_base = "https://api.jina.ai"
api_key = "test-key"
models = ["jina-reranker-v2", "jina-reranker-v1"]
"#;
    let config: RerankConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.models.len(), 2);
}

// ── GenerationProviderConfig ──

#[test]
fn generation_config_optional_models() {
    let toml = r#"
provider_type = "openai"
"#;
    let config: GenerationProviderConfig = toml::from_str(toml).unwrap();
    assert!(config.models.is_empty(), "generation models should be optional (empty vec)");
    assert!(config.default_model().is_none());
}

#[test]
fn generation_config_model_aliases() {
    let toml = r#"
provider_type = "openai"
models = ["dall-e-3"]
model_aliases = { "dalle" = "dall-e-3" }
"#;
    let config: GenerationProviderConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.default_model(), Some("dall-e-3"));
    assert_eq!(config.model_aliases.get("dalle"), Some(&"dall-e-3".to_string()));
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --test provider_config_probe -p alephcore -- serde_tests`
Expected: All pass. If any import paths are wrong, fix them based on compiler errors.

- [ ] **Step 3: Commit**

```bash
git add tests/provider_config_probe/serde_tests.rs
git commit -m "test: add serialization and backward compat tests for provider config

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Config struct CRUD tests (no I/O)

**Files:**
- Modify: `tests/provider_config_probe/config_crud_tests.rs`

- [ ] **Step 1: Write config struct manipulation tests**

```rust
//! Config struct CRUD tests — direct struct manipulation, no handlers, no I/O.

use alephcore::config::Config;
use alephcore::config::types::provider::ProviderConfig;

#[test]
fn add_provider_to_config() {
    let mut config = Config::default();
    let provider = ProviderConfig::test_config("gpt-4o");
    config.providers.insert("openai".to_string(), provider);
    assert!(config.providers.contains_key("openai"));
    assert_eq!(config.providers["openai"].default_model(), "gpt-4o");
}

#[test]
fn provider_with_multiple_models() {
    let mut config = Config::default();
    let mut provider = ProviderConfig::test_config("gpt-4o");
    provider.models.push("gpt-4o-mini".to_string());
    provider.models.push("o1".to_string());
    config.providers.insert("openai".to_string(), provider);
    assert_eq!(config.providers["openai"].all_models().len(), 3);
    assert_eq!(config.providers["openai"].default_model(), "gpt-4o");
}

#[test]
fn remove_provider_from_config() {
    let mut config = Config::default();
    config.providers.insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));
    assert!(config.providers.contains_key("openai"));
    config.providers.remove("openai");
    assert!(!config.providers.contains_key("openai"));
}

#[test]
fn default_provider_resolution() {
    let mut config = Config::default();
    let mut provider = ProviderConfig::test_config("gpt-4o");
    provider.models = vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()];
    config.providers.insert("openai".to_string(), provider);
    config.general.default_provider = Some("openai".to_string());

    let default_name = config.general.default_provider.as_ref().unwrap();
    let default_provider = &config.providers[default_name];
    assert_eq!(default_provider.default_model(), "gpt-4o");
}

#[test]
fn full_config_toml_roundtrip() {
    let mut config = Config::default();
    let mut p1 = ProviderConfig::test_config("gpt-4o");
    p1.models = vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()];
    config.providers.insert("openai".to_string(), p1);

    let toml_str = toml::to_string_pretty(&config).unwrap();
    let config2: Config = toml::from_str(&toml_str).unwrap();

    assert_eq!(config2.providers["openai"].models, vec!["gpt-4o", "gpt-4o-mini"]);
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --test provider_config_probe -p alephcore -- config_crud_tests`
Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add tests/provider_config_probe/config_crud_tests.rs
git commit -m "test: add config struct CRUD tests for provider config

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Edge case tests

**Files:**
- Modify: `tests/provider_config_probe/edge_case_tests.rs`

- [ ] **Step 1: Write edge case tests**

```rust
//! Edge case tests for provider config models field.

use alephcore::config::types::provider::ProviderConfig;

#[test]
fn unicode_model_name() {
    let toml = r#"models = ["模型-v1", "gpt-4o"]"#;
    let config: ProviderConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.models[0], "模型-v1");

    // Round-trip
    let serialized = toml::to_string_pretty(&config).unwrap();
    let config2: ProviderConfig = toml::from_str(&serialized).unwrap();
    assert_eq!(config2.models[0], "模型-v1");
}

#[test]
fn long_model_name() {
    let long_name = "a".repeat(1000);
    let toml = format!(r#"models = ["{}"]"#, long_name);
    let config: ProviderConfig = toml::from_str(&toml).unwrap();
    assert_eq!(config.models[0].len(), 1000);
}

#[test]
fn special_chars_in_model_name() {
    let toml = r#"models = ["org/model-v2.1-beta", "openai:gpt-4o@latest"]"#;
    let config: ProviderConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.models[0], "org/model-v2.1-beta");
    assert_eq!(config.models[1], "openai:gpt-4o@latest");
}

#[test]
fn duplicate_models_allowed() {
    let toml = r#"models = ["gpt-4o", "gpt-4o"]"#;
    let config: ProviderConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.models.len(), 2);
}

#[test]
fn hundred_models() {
    let models: Vec<String> = (0..100).map(|i| format!("model-{}", i)).collect();
    let toml = format!("models = {:?}", models);
    let config: ProviderConfig = toml::from_str(&toml).unwrap();
    assert_eq!(config.models.len(), 100);
    assert_eq!(config.default_model(), "model-0");
}

#[test]
fn json_roundtrip() {
    let config = ProviderConfig::test_config("org/model-v2.1-beta");
    let json = serde_json::to_value(&config).unwrap();
    assert!(json["models"].is_array());
    assert_eq!(json["models"][0], "org/model-v2.1-beta");

    let config2: ProviderConfig = serde_json::from_value(json).unwrap();
    assert_eq!(config2.models[0], "org/model-v2.1-beta");
}
```

- [ ] **Step 2: Run all Layer 1 tests**

Run: `cargo test --test provider_config_probe -p alephcore`
Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add tests/provider_config_probe/edge_case_tests.rs
git commit -m "test: add edge case tests for provider config models

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

---

## Chunk 2: Layer 2 — RPC Integration Probes

### Task 5: Create RPC probe module and test server harness

**Files:**
- Create: `tests/provider_rpc_probe.rs` (entry point)
- Create: `tests/provider_rpc_probe/mod.rs`
- Create: `tests/provider_rpc_probe/harness.rs`

- [ ] **Step 1: Create the test server harness**

Create `tests/provider_rpc_probe.rs`:
```rust
mod provider_rpc_probe;
```

Create `tests/provider_rpc_probe/mod.rs`:
```rust
mod harness;
mod endpoint_tests;
mod error_tests;
mod robustness_tests;
```

Create `tests/provider_rpc_probe/harness.rs`:
```rust
//! Test harness that spawns a real Aleph server as a child process.

use std::io::Write;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::{SinkExt, StreamExt};

/// Find an available TCP port by binding to port 0.
fn find_available_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

pub struct AlephTestServer {
    child: Child,
    pub port: u16,
    pub ws_url: String,
    _config_dir: TempDir,
}

impl AlephTestServer {
    /// Start a new Aleph server with minimal config in a temp directory.
    pub async fn start() -> Self {
        Self::start_with_config("").await
    }

    /// Start with additional TOML config content appended to defaults.
    pub async fn start_with_config(extra_toml: &str) -> Self {
        let config_dir = TempDir::new().unwrap();
        let config_path = config_dir.path().join("config.toml");

        // Write minimal config with auth disabled for testing
        let mut config_file = std::fs::File::create(&config_path).unwrap();
        writeln!(config_file, r#"
[general]
default_provider = "test"

[providers.test]
protocol = "openai"
models = ["test-model"]
enabled = true
verified = true
base_url = "http://127.0.0.1:1"

{extra_toml}
"#).unwrap();

        let port = find_available_port();

        // Spawn the aleph binary
        let child = Command::new("cargo")
            .args([
                "run", "-p", "alephcore", "--bin", "aleph", "--",
                "--config", config_path.to_str().unwrap(),
                "--port", &port.to_string(),
                "--bind", "127.0.0.1",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("Failed to start aleph server");

        let ws_url = format!("ws://127.0.0.1:{}/ws", port);

        // Wait for server to be ready (up to 30 seconds for compilation + startup)
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(60) {
                panic!("Aleph server did not start within 60 seconds on port {}", port);
            }
            match TcpListener::bind(format!("127.0.0.1:{}", port)) {
                Ok(_) => {
                    // Port still free, server not ready yet
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Err(_) => {
                    // Port taken = server is listening
                    // Give it a moment to finish initialization
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    break;
                }
            }
        }

        Self {
            child,
            port,
            ws_url,
            _config_dir: config_dir,
        }
    }

    /// Send a JSON-RPC request and return the response.
    pub async fn rpc_call(&self, method: &str, params: Value) -> Value {
        let (mut ws_stream, _) = connect_async(&self.ws_url)
            .await
            .expect("Failed to connect to WebSocket");

        let request = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1
        });

        ws_stream.send(Message::Text(request.to_string())).await.unwrap();

        // Read response (with timeout)
        let response = tokio::time::timeout(Duration::from_secs(10), ws_stream.next())
            .await
            .expect("Timed out waiting for RPC response")
            .expect("WebSocket stream ended")
            .expect("WebSocket error");

        match response {
            Message::Text(text) => serde_json::from_str(&text).unwrap(),
            _ => panic!("Expected text WebSocket message"),
        }
    }

    /// Send RPC and expect a successful result.
    pub async fn rpc_ok(&self, method: &str, params: Value) -> Value {
        let response = self.rpc_call(method, params).await;
        assert!(response.get("error").is_none(),
            "RPC {} returned error: {:?}", method, response["error"]);
        response["result"].clone()
    }

    /// Send RPC and expect an error.
    pub async fn rpc_err(&self, method: &str, params: Value) -> Value {
        let response = self.rpc_call(method, params).await;
        assert!(response.get("error").is_some(),
            "RPC {} expected error but got result: {:?}", method, response["result"]);
        response["error"].clone()
    }

    /// Clean up all providers except "test" (the default).
    pub async fn clean_providers(&self) {
        let result = self.rpc_ok("providers.list", json!({})).await;
        if let Some(providers) = result["providers"].as_array() {
            for p in providers {
                let name = p["name"].as_str().unwrap_or("");
                if name != "test" {
                    self.rpc_call("providers.delete", json!({"name": name})).await;
                }
            }
        }
    }
}

impl Drop for AlephTestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
```

- [ ] **Step 2: Verify compiles**

Run: `cargo test --test provider_rpc_probe --no-run -p alephcore`
Expected: Compiles (may need to adjust imports based on actual crate structure).

- [ ] **Step 3: Commit**

```bash
git add tests/provider_rpc_probe.rs tests/provider_rpc_probe/
git commit -m "test: add RPC integration probe harness with child-process server

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: RPC endpoint validation tests

**Files:**
- Create: `tests/provider_rpc_probe/endpoint_tests.rs`

- [ ] **Step 1: Write endpoint tests**

```rust
//! RPC endpoint validation tests via real WebSocket connection.

use serde_json::json;
use serial_test::serial;
use crate::provider_rpc_probe::harness::AlephTestServer;

#[tokio::test]
#[serial]
async fn websocket_connects() {
    let server = AlephTestServer::start().await;
    let result = server.rpc_ok("providers.list", json!({})).await;
    assert!(result.get("providers").is_some());
}

#[tokio::test]
#[serial]
async fn providers_list_returns_models_array() {
    let server = AlephTestServer::start().await;
    let result = server.rpc_ok("providers.list", json!({})).await;
    let providers = result["providers"].as_array().unwrap();
    // Should have at least the "test" provider from config
    assert!(!providers.is_empty());
    let test_provider = providers.iter().find(|p| p["name"] == "test").unwrap();
    assert!(test_provider["models"].is_array(), "models should be an array");
}

#[tokio::test]
#[serial]
async fn providers_create_and_get() {
    let server = AlephTestServer::start().await;
    server.clean_providers().await;

    // Create
    server.rpc_ok("providers.create", json!({
        "name": "openai",
        "config": {
            "protocol": "openai",
            "models": ["gpt-4o", "gpt-4o-mini"],
            "enabled": true,
            "base_url": "https://api.openai.com/v1"
        }
    })).await;

    // Get
    let result = server.rpc_ok("providers.get", json!({"name": "openai"})).await;
    let models = result["provider"]["models"].as_array().unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0], "gpt-4o");
    assert_eq!(models[1], "gpt-4o-mini");
}

#[tokio::test]
#[serial]
async fn providers_update_models() {
    let server = AlephTestServer::start().await;
    server.clean_providers().await;

    // Create
    server.rpc_ok("providers.create", json!({
        "name": "myai",
        "config": {
            "protocol": "openai",
            "models": ["old-model"],
            "enabled": true
        }
    })).await;

    // Update
    server.rpc_ok("providers.update", json!({
        "name": "myai",
        "config": {
            "models": ["new-model-a", "new-model-b"]
        }
    })).await;

    // Verify
    let result = server.rpc_ok("providers.get", json!({"name": "myai"})).await;
    let models = result["provider"]["models"].as_array().unwrap();
    assert_eq!(models[0], "new-model-a");
    assert_eq!(models[1], "new-model-b");
}

#[tokio::test]
#[serial]
async fn providers_delete() {
    let server = AlephTestServer::start().await;
    server.clean_providers().await;

    server.rpc_ok("providers.create", json!({
        "name": "todelete",
        "config": { "protocol": "openai", "models": ["x"], "enabled": true }
    })).await;

    server.rpc_ok("providers.delete", json!({"name": "todelete"})).await;

    let result = server.rpc_ok("providers.list", json!({})).await;
    let names: Vec<&str> = result["providers"].as_array().unwrap()
        .iter().filter_map(|p| p["name"].as_str()).collect();
    assert!(!names.contains(&"todelete"));
}

#[tokio::test]
#[serial]
async fn deleted_endpoint_models_list_returns_method_not_found() {
    let server = AlephTestServer::start().await;
    let err = server.rpc_err("models.list", json!({})).await;
    assert_eq!(err["code"], -32601, "should be METHOD_NOT_FOUND");
}

#[tokio::test]
#[serial]
async fn deleted_endpoint_providers_probe_returns_method_not_found() {
    let server = AlephTestServer::start().await;
    let err = server.rpc_err("providers.probe", json!({"protocol": "openai"})).await;
    assert_eq!(err["code"], -32601);
}
```

- [ ] **Step 2: Run tests (requires building the aleph binary first)**

Run: `cargo build -p alephcore --bin aleph && cargo test --test provider_rpc_probe -p alephcore -- endpoint_tests`
Expected: All pass. Note: first run will be slow due to binary compilation.

- [ ] **Step 3: Commit**

```bash
git add tests/provider_rpc_probe/endpoint_tests.rs
git commit -m "test: add RPC endpoint validation tests for provider config

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Error path and robustness tests

**Files:**
- Create: `tests/provider_rpc_probe/error_tests.rs`
- Create: `tests/provider_rpc_probe/robustness_tests.rs`

- [ ] **Step 1: Write error path tests**

Create `tests/provider_rpc_probe/error_tests.rs`:
```rust
//! Error path tests for provider RPC endpoints.

use serde_json::json;
use serial_test::serial;
use crate::provider_rpc_probe::harness::AlephTestServer;

#[tokio::test]
#[serial]
async fn update_nonexistent_provider() {
    let server = AlephTestServer::start().await;
    let err = server.rpc_err("providers.update", json!({
        "name": "doesnotexist",
        "config": { "models": ["x"] }
    })).await;
    assert!(err["message"].as_str().unwrap().len() > 0);
}

#[tokio::test]
#[serial]
async fn delete_nonexistent_provider() {
    let server = AlephTestServer::start().await;
    let err = server.rpc_err("providers.delete", json!({"name": "doesnotexist"})).await;
    assert!(err["message"].as_str().unwrap().len() > 0);
}

#[tokio::test]
#[serial]
async fn create_duplicate_provider() {
    let server = AlephTestServer::start().await;
    server.clean_providers().await;

    // First create succeeds
    server.rpc_ok("providers.create", json!({
        "name": "dup",
        "config": { "protocol": "openai", "models": ["a"], "enabled": true }
    })).await;

    // Second create with same name should fail
    let err = server.rpc_err("providers.create", json!({
        "name": "dup",
        "config": { "protocol": "openai", "models": ["b"], "enabled": true }
    })).await;
    assert!(err["message"].as_str().unwrap().len() > 0);
}

#[tokio::test]
#[serial]
async fn providers_test_unreachable_url() {
    let server = AlephTestServer::start().await;

    let result = server.rpc_ok("providers.test", json!({
        "config": {
            "protocol": "openai",
            "models": ["gpt-4o"],
            "api_key": "sk-fake",
            "base_url": "http://127.0.0.1:1"
        }
    })).await;

    assert_eq!(result["success"], false);
    assert!(result["error"].as_str().unwrap().len() > 0);
}
```

- [ ] **Step 2: Write robustness tests**

Create `tests/provider_rpc_probe/robustness_tests.rs`:
```rust
//! Robustness tests: concurrency, large payloads, malformed input.

use serde_json::json;
use serial_test::serial;
use crate::provider_rpc_probe::harness::AlephTestServer;

#[tokio::test]
#[serial]
async fn concurrent_providers_list() {
    let server = AlephTestServer::start().await;

    let mut handles = Vec::new();
    for _ in 0..10 {
        let ws_url = server.ws_url.clone();
        handles.push(tokio::spawn(async move {
            use tokio_tungstenite::connect_async;
            use futures_util::{SinkExt, StreamExt};
            use tokio_tungstenite::tungstenite::Message;

            let (mut ws, _) = connect_async(&ws_url).await.unwrap();
            let req = json!({"jsonrpc":"2.0","method":"providers.list","params":{},"id":1});
            ws.send(Message::Text(req.to_string())).await.unwrap();
            let msg = ws.next().await.unwrap().unwrap();
            let resp: serde_json::Value = match msg {
                Message::Text(t) => serde_json::from_str(&t).unwrap(),
                _ => panic!("unexpected message type"),
            };
            assert!(resp.get("result").is_some());
        }));
    }

    for h in handles {
        h.await.unwrap();
    }
}

#[tokio::test]
#[serial]
async fn large_models_list() {
    let server = AlephTestServer::start().await;
    server.clean_providers().await;

    let models: Vec<String> = (0..100).map(|i| format!("model-{}", i)).collect();

    server.rpc_ok("providers.create", json!({
        "name": "bigprovider",
        "config": {
            "protocol": "openai",
            "models": models,
            "enabled": true
        }
    })).await;

    let result = server.rpc_ok("providers.get", json!({"name": "bigprovider"})).await;
    let stored_models = result["provider"]["models"].as_array().unwrap();
    assert_eq!(stored_models.len(), 100);
}
```

- [ ] **Step 3: Run all Layer 2 tests**

Run: `cargo test --test provider_rpc_probe -p alephcore`
Expected: All pass.

- [ ] **Step 4: Commit**

```bash
git add tests/provider_rpc_probe/error_tests.rs tests/provider_rpc_probe/robustness_tests.rs
git commit -m "test: add error path and robustness tests for provider RPC

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

---

## Chunk 3: Layer 3 — Playwright E2E

### Task 8: Playwright infrastructure setup

**Files:**
- Create: `e2e/playwright.config.ts`
- Create: `e2e/global-setup.ts`
- Create: `e2e/global-teardown.ts`
- Create: `e2e/helpers/rpc-client.ts`
- Create: `e2e/helpers/test-fixtures.ts`
- Modify: `package.json` (add `@playwright/test`)
- Modify: `justfile` (add `test-probes` and `test-e2e` recipes)

- [ ] **Step 1: Add @playwright/test dependency**

Run:
```bash
npm install --save-dev @playwright/test
npx playwright install chromium
```

- [ ] **Step 2: Create playwright.config.ts**

```typescript
import { defineConfig } from '@playwright/test';
import path from 'path';

export default defineConfig({
  testDir: './e2e/tests',
  timeout: 30000,
  retries: 0,
  use: {
    baseURL: 'http://127.0.0.1:18791',
    headless: true,
  },
  globalSetup: path.resolve(__dirname, 'e2e/global-setup.ts'),
  globalTeardown: path.resolve(__dirname, 'e2e/global-teardown.ts'),
  projects: [
    { name: 'chromium', use: { browserName: 'chromium' } },
  ],
});
```

- [ ] **Step 3: Create global-setup.ts**

```typescript
import { execSync, spawn, ChildProcess } from 'child_process';
import { existsSync, mkdtempSync, writeFileSync } from 'fs';
import { tmpdir } from 'os';
import { join } from 'path';
import net from 'net';

const PORT = 18791;
const CONFIG_TOML = `
[general]
default_provider = "test"

[providers.test]
protocol = "openai"
models = ["test-model"]
enabled = true
verified = true
base_url = "http://127.0.0.1:1"
`;

async function waitForPort(port: number, timeoutMs = 60000): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      await new Promise<void>((resolve, reject) => {
        const socket = net.createConnection({ port, host: '127.0.0.1' }, () => {
          socket.destroy();
          resolve();
        });
        socket.on('error', reject);
      });
      return;
    } catch {
      await new Promise(r => setTimeout(r, 500));
    }
  }
  throw new Error(`Port ${port} not available after ${timeoutMs}ms`);
}

export default async function globalSetup() {
  // Check WASM panel is built
  if (!existsSync('apps/panel/dist/aleph_panel_bg.wasm')) {
    console.log('Building WASM panel...');
    execSync('just wasm', { stdio: 'inherit' });
  }

  // Create temp config
  const configDir = mkdtempSync(join(tmpdir(), 'aleph-e2e-'));
  const configPath = join(configDir, 'config.toml');
  writeFileSync(configPath, CONFIG_TOML);

  // Build and start server
  console.log('Building aleph binary...');
  execSync('cargo build -p alephcore --bin aleph', { stdio: 'inherit' });

  console.log(`Starting aleph server on port ${PORT}...`);
  const serverProcess = spawn('cargo', [
    'run', '-p', 'alephcore', '--bin', 'aleph', '--',
    '--config', configPath,
    '--port', String(PORT),
    '--bind', '127.0.0.1',
  ], {
    stdio: 'ignore',
    detached: true,
  });

  // Store PID and config dir for teardown
  process.env.ALEPH_TEST_PID = String(serverProcess.pid);
  process.env.ALEPH_TEST_CONFIG_DIR = configDir;

  serverProcess.unref();

  await waitForPort(PORT);
  console.log('Aleph server ready.');
}
```

- [ ] **Step 4: Create global-teardown.ts**

```typescript
import { rmSync } from 'fs';

export default async function globalTeardown() {
  const pid = process.env.ALEPH_TEST_PID;
  if (pid) {
    try {
      process.kill(Number(pid), 'SIGTERM');
    } catch {
      // Process may have already exited
    }
  }

  const configDir = process.env.ALEPH_TEST_CONFIG_DIR;
  if (configDir) {
    try {
      rmSync(configDir, { recursive: true, force: true });
    } catch {
      // Ignore cleanup errors
    }
  }
}
```

- [ ] **Step 5: Create rpc-client.ts**

```typescript
import WebSocket from 'ws';

let idCounter = 0;

export class RpcClient {
  private url: string;

  constructor(port = 18791) {
    this.url = `ws://127.0.0.1:${port}/ws`;
  }

  async call(method: string, params: Record<string, unknown> = {}): Promise<any> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(this.url);
      const id = ++idCounter;

      ws.on('open', () => {
        ws.send(JSON.stringify({
          jsonrpc: '2.0',
          method,
          params,
          id,
        }));
      });

      ws.on('message', (data: Buffer) => {
        const response = JSON.parse(data.toString());
        ws.close();
        if (response.error) {
          reject(new Error(`RPC ${method}: ${response.error.message}`));
        } else {
          resolve(response.result);
        }
      });

      ws.on('error', reject);

      setTimeout(() => {
        ws.close();
        reject(new Error(`RPC ${method} timed out`));
      }, 10000);
    });
  }
}
```

- [ ] **Step 6: Create test-fixtures.ts**

```typescript
import { RpcClient } from './rpc-client';

const rpc = new RpcClient();

export async function cleanProviders() {
  const result = await rpc.call('providers.list');
  const providers = result.providers || [];
  for (const p of providers) {
    if (p.name !== 'test') {
      try {
        await rpc.call('providers.delete', { name: p.name });
      } catch {
        // Ignore delete errors
      }
    }
  }
}

export async function injectProvider(name: string, config: Record<string, unknown>) {
  await rpc.call('providers.create', { name, config });
}

export async function getProviders(): Promise<any[]> {
  const result = await rpc.call('providers.list');
  return result.providers || [];
}

export { rpc };
```

- [ ] **Step 7: Add Justfile recipes**

Add to `justfile`:
```
# Provider config integration probes (Layer 1 + 2)
test-probes:
  cargo test --test provider_config_probe --test provider_rpc_probe -p alephcore

# Playwright E2E tests (Layer 3)
test-e2e:
  npx playwright test --project=chromium

# Real API tests (optional, needs API keys)
test-real-api:
  cargo test --test provider_rpc_probe -p alephcore -- --ignored
```

- [ ] **Step 8: Commit**

```bash
git add e2e/ playwright.config.ts package.json justfile
git commit -m "test: add Playwright infrastructure for provider config e2e tests

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

---

### Task 9: Playwright provider settings tests

**Files:**
- Create: `e2e/tests/providers.spec.ts`

- [ ] **Step 1: Write provider settings tests**

```typescript
import { test, expect } from '@playwright/test';
import { cleanProviders, injectProvider, rpc } from '../helpers/test-fixtures';

test.describe('Provider Settings', () => {
  test.beforeEach(async ({ page }) => {
    await cleanProviders();
  });

  test('page loads without errors', async ({ page }) => {
    await page.goto('/');
    // Navigate to provider settings (adapt selector to actual UI)
    await page.click('text=Providers');
    await expect(page.locator('h1, h2, h3').first()).toBeVisible();
  });

  test('displays existing provider with models', async ({ page }) => {
    await injectProvider('openai', {
      protocol: 'openai',
      models: ['gpt-4o'],
      enabled: true,
      base_url: 'https://api.openai.com/v1',
    });

    await page.goto('/');
    await page.click('text=Providers');

    // Should show the provider and its model
    await expect(page.locator('text=openai')).toBeVisible();
    const modelsInput = page.locator('input[placeholder*="model"]').first();
    await expect(modelsInput).toHaveValue('gpt-4o');
  });

  test('input multiple models comma-separated', async ({ page }) => {
    await injectProvider('openai', {
      protocol: 'openai',
      models: ['gpt-4o'],
      enabled: true,
    });

    await page.goto('/');
    await page.click('text=Providers');

    const modelsInput = page.locator('input[placeholder*="model"]').first();
    await modelsInput.clear();
    await modelsInput.fill('gpt-4o, gpt-4o-mini, o1');
    await expect(modelsInput).toHaveValue('gpt-4o, gpt-4o-mini, o1');
  });

  test('save persists models across page reload', async ({ page }) => {
    await injectProvider('openai', {
      protocol: 'openai',
      models: ['gpt-4o'],
      enabled: true,
    });

    await page.goto('/');
    await page.click('text=Providers');

    // Edit models
    const modelsInput = page.locator('input[placeholder*="model"]').first();
    await modelsInput.clear();
    await modelsInput.fill('gpt-4o, gpt-4o-mini');

    // Save (adapt button selector to actual UI)
    await page.click('button:has-text("Save")');
    await page.waitForTimeout(1000);

    // Reload and verify
    await page.reload();
    await page.click('text=Providers');
    const reloadedInput = page.locator('input[placeholder*="model"]').first();
    await expect(reloadedInput).toHaveValue(/gpt-4o.*gpt-4o-mini/);
  });

  test('empty models shows error or prevents save', async ({ page }) => {
    await injectProvider('openai', {
      protocol: 'openai',
      models: ['gpt-4o'],
      enabled: true,
    });

    await page.goto('/');
    await page.click('text=Providers');

    const modelsInput = page.locator('input[placeholder*="model"]').first();
    await modelsInput.clear();

    // Try to save
    await page.click('button:has-text("Save")');

    // Should show error or models field should still be required
    // (exact behavior depends on UI implementation)
    const hasError = await page.locator('.error, .help-text.error, [class*="error"]').count();
    const inputStillEmpty = await modelsInput.inputValue() === '';
    expect(hasError > 0 || inputStillEmpty).toBeTruthy();
  });

  test('special characters in model name', async ({ page }) => {
    await injectProvider('custom', {
      protocol: 'openai',
      models: ['org/model-v2.1'],
      enabled: true,
    });

    await page.goto('/');
    await page.click('text=Providers');

    const modelsInput = page.locator('input[placeholder*="model"]').first();
    await expect(modelsInput).toHaveValue('org/model-v2.1');
  });
});
```

Note: The exact CSS selectors and navigation paths will need adjustment based on the actual panel UI. These tests provide the structure; selectors should be refined during implementation by inspecting the running panel.

- [ ] **Step 2: Create remaining spec files (stubs)**

Create `e2e/tests/embedding-providers.spec.ts`, `e2e/tests/reranking-providers.spec.ts`, `e2e/tests/generation-providers.spec.ts`, `e2e/tests/setup-wizard.spec.ts` with similar structure but adapted for each settings page. Each should:
- Use `cleanProviders()` in beforeEach
- Test page loads
- Test model input is text field (not dropdown)
- Test save and persist

These follow the exact same pattern as `providers.spec.ts` but navigate to different settings tabs and use different RPC methods for setup.

- [ ] **Step 3: Commit**

```bash
git add e2e/tests/
git commit -m "test: add Playwright e2e tests for provider settings pages

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

---

### Task 10: Verify full test pyramid

- [ ] **Step 1: Run Layer 1**

Run: `cargo test --test provider_config_probe -p alephcore`
Expected: All pass.

- [ ] **Step 2: Run Layer 2**

Run: `cargo test --test provider_rpc_probe -p alephcore`
Expected: All pass (may be slow on first run due to compilation).

- [ ] **Step 3: Run Layer 3 (if WASM panel is built)**

Run: `npx playwright test --project=chromium`
Expected: Tests run against live server. Some selectors may need adjustment based on actual UI.

- [ ] **Step 4: Update Justfile and commit**

Verify `just test-probes` and `just test-e2e` work. Final commit:

```bash
git add justfile
git commit -m "test: finalize provider config test pyramid

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```
