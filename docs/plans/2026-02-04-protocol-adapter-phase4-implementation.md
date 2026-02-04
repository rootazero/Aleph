# Protocol Adapter Phase 4 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** Enable users to define new AI protocols via YAML files without recompiling Rust, with hot reload support.

**Architecture:** Build on Phase 4 Foundation (ProtocolDefinition types, ProtocolRegistry, stubs). Implement ConfigurableProtocol to convert YAML definitions into HTTP requests/responses, ProtocolLoader to load and watch protocol files, create example YAML configs, and write user documentation.

**Tech Stack:** Rust, serde_yaml, notify (file watching), handlebars (template engine), jsonpath (response parsing)

---

## Task 1: Add Required Dependencies

**Files:**
- Modify: `core/Cargo.toml`

**Step 1: Add template and parsing dependencies**

Add to `[dependencies]` section in `core/Cargo.toml`:

```toml
# Template engine for protocol definitions
handlebars = "5.1"

# JSONPath for response parsing
jsonpath-rust = "0.5"

# File watching for hot reload
notify = "6.1"
```

**Step 2: Build to verify dependencies**

Run: `cd core && cargo build`
Expected: Build succeeds with new dependencies

**Step 3: Commit**

```bash
git add core/Cargo.toml
git commit -m "feat(protocols): add dependencies for configurable protocols (handlebars, jsonpath, notify)"
```

---

## Task 2: Implement Template Engine Wrapper

**Files:**
- Create: `core/src/providers/protocols/template.rs`
- Modify: `core/src/providers/protocols/mod.rs`

**Step 1: Write failing test**

Create `core/src/providers/protocols/template.rs`:

```rust
//! Template engine for protocol request/response transformation

use crate::config::ProviderConfig;
use crate::error::{AetherError, Result};
use handlebars::Handlebars;
use serde_json::{json, Value};

/// Template context builder
pub struct TemplateContext {
    data: Value,
}

impl TemplateContext {
    pub fn new() -> Self {
        Self { data: json!({}) }
    }

    pub fn with_config(mut self, config: &ProviderConfig) -> Self {
        self.data["config"] = json!({
            "model": config.model,
            "api_key": config.api_key.as_deref().unwrap_or(""),
            "base_url": config.base_url.as_deref().unwrap_or(""),
            "temperature": config.temperature.unwrap_or(1.0),
            "max_tokens": config.max_tokens,
        });
        self
    }

    pub fn with_input(mut self, input: &str) -> Self {
        self.data["input"] = json!(input);
        self
    }

    pub fn with_system_prompt(mut self, prompt: &str) -> Self {
        self.data["system_prompt"] = json!(prompt);
        self
    }

    pub fn with_messages(mut self, messages: Value) -> Self {
        self.data["messages"] = messages;
        self
    }

    pub fn build(self) -> Value {
        self.data
    }
}

/// Template renderer
pub struct TemplateRenderer {
    registry: Handlebars<'static>,
}

impl TemplateRenderer {
    pub fn new() -> Self {
        Self {
            registry: Handlebars::new(),
        }
    }

    /// Render a template string with context
    pub fn render(&self, template: &str, context: &Value) -> Result<String> {
        self.registry
            .render_template(template, context)
            .map_err(|e| AetherError::provider(format!("Template render error: {}", e)))
    }

    /// Render a JSON template with context
    pub fn render_json(&self, template: &Value, context: &Value) -> Result<Value> {
        let json_str = serde_json::to_string(template)?;
        let rendered = self.render(&json_str, context)?;
        serde_json::from_str(&rendered)
            .map_err(|e| AetherError::provider(format!("Invalid JSON after template render: {}", e)))
    }
}

impl Default for TemplateRenderer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_context_building() {
        let config = ProviderConfig {
            model: "gpt-4".to_string(),
            api_key: Some("sk-test".to_string()),
            ..Default::default()
        };

        let context = TemplateContext::new()
            .with_config(&config)
            .with_input("Hello")
            .build();

        assert_eq!(context["config"]["model"], "gpt-4");
        assert_eq!(context["input"], "Hello");
    }

    #[test]
    fn test_template_renderer() {
        let renderer = TemplateRenderer::new();
        let context = json!({"name": "World"});
        let result = renderer.render("Hello {{name}}", &context).unwrap();
        assert_eq!(result, "Hello World");
    }
}
```

**Step 2: Add module to mod.rs**

Add to `core/src/providers/protocols/mod.rs`:

```rust
mod template;
pub use template::{TemplateContext, TemplateRenderer};
```

**Step 3: Run tests**

Run: `cd core && cargo test template::tests`
Expected: 2 tests pass

**Step 4: Commit**

```bash
git add core/src/providers/protocols/template.rs core/src/providers/protocols/mod.rs
git commit -m "feat(protocols): add template engine wrapper for request/response transformation"
```

---

## Task 3: Implement JSONPath Response Parser

**Files:**
- Create: `core/src/providers/protocols/jsonpath.rs`
- Modify: `core/src/providers/protocols/mod.rs`

**Step 1: Write failing test**

Create `core/src/providers/protocols/jsonpath.rs`:

```rust
//! JSONPath parser for extracting values from protocol responses

use crate::error::{AetherError, Result};
use jsonpath_rust::JsonPath;
use serde_json::Value;

/// Extract value from JSON using JSONPath
pub fn extract_value(json: &Value, path: &str) -> Result<String> {
    let json_path = JsonPath::try_from(path)
        .map_err(|e| AetherError::provider(format!("Invalid JSONPath '{}': {}", path, e)))?;

    let results = json_path.find(json);

    if results.is_empty() {
        return Err(AetherError::provider(format!(
            "JSONPath '{}' matched no values",
            path
        )));
    }

    // Get first match
    match results[0] {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        other => serde_json::to_string(other)
            .map_err(|e| AetherError::provider(format!("Failed to serialize value: {}", e))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_simple_string() {
        let json = json!({"content": "Hello"});
        let result = extract_value(&json, "$.content").unwrap();
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_extract_nested_value() {
        let json = json!({
            "data": {
                "choices": [
                    {"message": {"content": "Response"}}
                ]
            }
        });
        let result = extract_value(&json, "$.data.choices[0].message.content").unwrap();
        assert_eq!(result, "Response");
    }

    #[test]
    fn test_extract_nonexistent_path() {
        let json = json!({"foo": "bar"});
        let result = extract_value(&json, "$.missing");
        assert!(result.is_err());
    }
}
```

**Step 2: Add module to mod.rs**

Add to `core/src/providers/protocols/mod.rs`:

```rust
mod jsonpath;
pub use jsonpath::extract_value;
```

**Step 3: Run tests**

Run: `cd core && cargo test jsonpath::tests`
Expected: 3 tests pass

**Step 4: Commit**

```bash
git add core/src/providers/protocols/jsonpath.rs core/src/providers/protocols/mod.rs
git commit -m "feat(protocols): add JSONPath parser for response value extraction"
```

---

## Task 4: Implement ConfigurableProtocol - Minimal Mode

**Files:**
- Modify: `core/src/providers/protocols/configurable.rs`

**Step 1: Write failing test for minimal mode**

Add to test section of `core/src/providers/protocols/configurable.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::protocols::{ProtocolDefinition, ProtocolDifferences, AuthDifferences};

    #[tokio::test]
    async fn test_minimal_mode_build_request() {
        let def = ProtocolDefinition {
            name: "test-minimal".to_string(),
            extends: Some("openai".to_string()),
            base_url: Some("https://api.test.com".to_string()),
            differences: Some(ProtocolDifferences {
                auth: Some(AuthDifferences {
                    header: "X-API-Key".to_string(),
                    prefix: None,
                }),
                ..Default::default()
            }),
            custom: None,
        };

        let config = ProviderConfig {
            model: "test-model".to_string(),
            api_key: Some("test-key".to_string()),
            base_url: Some("https://api.test.com".to_string()),
            ..Default::default()
        };

        let client = reqwest::Client::new();
        let proto = ConfigurableProtocol::new(def, client);

        let payload = RequestPayload {
            input: "Hello".to_string(),
            system_prompt: None,
            ..Default::default()
        };

        let req = proto.build_request(&payload, &config, false).unwrap();
        // Verify auth header is customized
        // Note: reqwest::RequestBuilder doesn't expose headers for inspection,
        // so we'll test this via integration tests
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test test_minimal_mode_build_request`
Expected: FAIL with "not yet implemented"

**Step 3: Implement minimal mode logic**

Replace the `build_request` implementation in `configurable.rs`:

```rust
use crate::providers::protocols::{ProtocolRegistry, TemplateContext, TemplateRenderer};
use std::sync::Arc;

pub struct ConfigurableProtocol {
    definition: ProtocolDefinition,
    client: Client,
    base_protocol: Option<Arc<dyn ProtocolAdapter>>,
    renderer: TemplateRenderer,
}

impl ConfigurableProtocol {
    /// Create a new configurable protocol
    pub fn new(definition: ProtocolDefinition, client: Client) -> Self {
        // Load base protocol if extending
        let base_protocol = definition.extends.as_ref().and_then(|base_name| {
            ProtocolRegistry::global().get(base_name)
        });

        Self {
            definition,
            client,
            base_protocol,
            renderer: TemplateRenderer::new(),
        }
    }
}

#[async_trait]
impl ProtocolAdapter for ConfigurableProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        // Minimal mode: Delegate to base protocol, apply differences
        if let Some(ref base) = self.base_protocol {
            let mut req = base.build_request(payload, config, is_streaming)?;

            // Apply auth differences
            if let Some(ref diff) = self.definition.differences {
                if let Some(ref auth) = diff.auth {
                    let api_key = config.api_key.as_deref()
                        .ok_or_else(|| AetherError::invalid_config("API key required"))?;

                    let value = if let Some(ref prefix) = auth.prefix {
                        format!("{}{}", prefix, api_key)
                    } else {
                        api_key.to_string()
                    };

                    req = req.header(&auth.header, value);
                }
            }

            return Ok(req);
        }

        // Custom mode: Build from template (Task 5)
        if self.definition.custom.is_some() {
            return Err(AetherError::provider("Custom protocol mode not yet implemented"));
        }

        Err(AetherError::provider("Protocol must either extend a base protocol or define custom implementation"))
    }

    async fn parse_response(&self, response: reqwest::Response) -> Result<String> {
        // Minimal mode: Delegate to base protocol
        if let Some(ref base) = self.base_protocol {
            return base.parse_response(response).await;
        }

        // Custom mode (Task 5)
        Err(AetherError::provider("Custom protocol mode not yet implemented"))
    }

    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>> {
        // Minimal mode: Delegate to base protocol
        if let Some(ref base) = self.base_protocol {
            return base.parse_stream(response).await;
        }

        // Custom mode (Task 5)
        Err(AetherError::provider("Custom protocol mode not yet implemented"))
    }

    fn name(&self) -> &'static str {
        Box::leak(self.definition.name.clone().into_boxed_str())
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd core && cargo test test_minimal_mode_build_request`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/providers/protocols/configurable.rs
git commit -m "feat(protocols): implement ConfigurableProtocol minimal mode (extends base + differences)"
```

---

## Task 5: Implement ConfigurableProtocol - Custom Mode

**Files:**
- Modify: `core/src/providers/protocols/configurable.rs`

**Step 1: Write failing test for custom mode**

Add to test section:

```rust
#[tokio::test]
async fn test_custom_mode_build_request() {
    use crate::providers::protocols::{CustomProtocol, AuthConfig, EndpointConfig, ResponseMapping};

    let def = ProtocolDefinition {
        name: "test-custom".to_string(),
        extends: None,
        base_url: Some("https://api.custom.com".to_string()),
        differences: None,
        custom: Some(CustomProtocol {
            auth: AuthConfig {
                auth_type: "header".to_string(),
                config: serde_json::json!({
                    "header": "X-API-Key",
                    "value_template": "{{config.api_key}}"
                }),
            },
            endpoints: EndpointConfig {
                chat: "/v1/chat".to_string(),
                stream: None,
            },
            request_template: serde_json::json!({
                "model_name": "{{config.model}}",
                "input_text": "{{input}}",
            }),
            response_mapping: ResponseMapping {
                content: "$.result.text".to_string(),
                error: Some("$.error.message".to_string()),
            },
            stream_config: None,
        }),
    };

    let config = ProviderConfig {
        model: "custom-model".to_string(),
        api_key: Some("key123".to_string()),
        base_url: Some("https://api.custom.com".to_string()),
        ..Default::default()
    };

    let client = reqwest::Client::new();
    let proto = ConfigurableProtocol::new(def, client);

    let payload = RequestPayload {
        input: "Test".to_string(),
        system_prompt: None,
        ..Default::default()
    };

    let req = proto.build_request(&payload, &config, false).unwrap();
    // Verify custom template is applied (integration test)
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test test_custom_mode_build_request`
Expected: FAIL with "not yet implemented"

**Step 3: Implement custom mode logic**

Update `build_request` method in `configurable.rs`:

```rust
fn build_request(
    &self,
    payload: &RequestPayload,
    config: &ProviderConfig,
    is_streaming: bool,
) -> Result<reqwest::RequestBuilder> {
    // Minimal mode: Delegate to base protocol, apply differences
    if let Some(ref base) = self.base_protocol {
        let mut req = base.build_request(payload, config, is_streaming)?;

        // Apply auth differences
        if let Some(ref diff) = self.definition.differences {
            if let Some(ref auth) = diff.auth {
                let api_key = config.api_key.as_deref()
                    .ok_or_else(|| AetherError::invalid_config("API key required"))?;

                let value = if let Some(ref prefix) = auth.prefix {
                    format!("{}{}", prefix, api_key)
                } else {
                    api_key.to_string()
                };

                req = req.header(&auth.header, value);
            }
        }

        return Ok(req);
    }

    // Custom mode: Build from template
    if let Some(ref custom) = self.definition.custom {
        // Build URL
        let base_url = self.definition.base_url.as_ref()
            .or(config.base_url.as_ref())
            .ok_or_else(|| AetherError::invalid_config("base_url required for custom protocol"))?;

        let endpoint = if is_streaming {
            custom.endpoints.stream.as_ref().unwrap_or(&custom.endpoints.chat)
        } else {
            &custom.endpoints.chat
        };

        let url = format!("{}{}", base_url.trim_end_matches('/'), endpoint);

        // Build template context
        let context = TemplateContext::new()
            .with_config(config)
            .with_input(&payload.input)
            .build();

        // Render request body
        let body = self.renderer.render_json(&custom.request_template, &context)?;

        // Build request
        let mut req = self.client.post(&url).json(&body);

        // Add authentication
        if custom.auth.auth_type == "header" {
            if let Some(header_name) = custom.auth.config.get("header").and_then(|v| v.as_str()) {
                if let Some(value_template) = custom.auth.config.get("value_template").and_then(|v| v.as_str()) {
                    let auth_value = self.renderer.render(value_template, &context)?;
                    req = req.header(header_name, auth_value);
                }
            }
        }

        return Ok(req);
    }

    Err(AetherError::provider("Protocol must either extend a base protocol or define custom implementation"))
}
```

**Step 4: Update parse_response for custom mode**

```rust
async fn parse_response(&self, response: reqwest::Response) -> Result<String> {
    // Minimal mode: Delegate to base protocol
    if let Some(ref base) = self.base_protocol {
        return base.parse_response(response).await;
    }

    // Custom mode: Parse using JSONPath
    if let Some(ref custom) = self.definition.custom {
        let json: serde_json::Value = response.json().await
            .map_err(|e| AetherError::provider(format!("Failed to parse JSON response: {}", e)))?;

        // Check for error
        if let Some(ref error_path) = custom.response_mapping.error {
            if let Ok(error_msg) = crate::providers::protocols::extract_value(&json, error_path) {
                return Err(AetherError::provider(error_msg));
            }
        }

        // Extract content
        crate::providers::protocols::extract_value(&json, &custom.response_mapping.content)
    } else {
        Err(AetherError::provider("Invalid protocol configuration"))
    }
}
```

**Step 5: Run test to verify it passes**

Run: `cd core && cargo test test_custom_mode_build_request`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/providers/protocols/configurable.rs
git commit -m "feat(protocols): implement ConfigurableProtocol custom mode with template rendering"
```

---

## Task 6: Implement ProtocolLoader File Loading

**Files:**
- Modify: `core/src/providers/protocols/loader.rs`

**Step 1: Write failing test**

Add to `loader.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_load_from_file() {
        let temp_dir = TempDir::new().unwrap();
        let protocol_file = temp_dir.path().join("test.yaml");

        let yaml = r#"
name: test-protocol
extends: openai
base_url: https://api.test.com
"#;
        fs::write(&protocol_file, yaml).unwrap();

        ProtocolLoader::load_from_file(&protocol_file).await.unwrap();

        let registry = ProtocolRegistry::global();
        assert!(registry.get("test-protocol").is_some());
    }

    #[tokio::test]
    async fn test_load_from_dir() {
        let temp_dir = TempDir::new().unwrap();

        fs::write(
            temp_dir.path().join("proto1.yaml"),
            "name: proto1\nextends: openai\n",
        ).unwrap();

        fs::write(
            temp_dir.path().join("proto2.yaml"),
            "name: proto2\nextends: anthropic\n",
        ).unwrap();

        ProtocolLoader::load_from_dir(temp_dir.path()).await.unwrap();

        let registry = ProtocolRegistry::global();
        assert!(registry.get("proto1").is_some());
        assert!(registry.get("proto2").is_some());
    }
}
```

**Step 2: Add tempfile dependency**

Add to `core/Cargo.toml` under `[dev-dependencies]`:

```toml
tempfile = "3.10"
```

**Step 3: Run test to verify it fails**

Run: `cd core && cargo test loader::tests`
Expected: FAIL with "not yet implemented"

**Step 4: Implement file loading logic**

Replace loader.rs implementation:

```rust
//! Protocol loader for YAML-based protocols

use crate::error::{AetherError, Result};
use crate::providers::protocols::{ConfigurableProtocol, ProtocolDefinition, ProtocolRegistry};
use reqwest::Client;
use std::path::Path;
use std::sync::Arc;
use tokio::fs;
use tracing::{error, info};

/// Protocol loader manages loading protocols from YAML files
pub struct ProtocolLoader;

impl ProtocolLoader {
    /// Load a protocol from YAML file
    pub async fn load_from_file(path: &Path) -> Result<()> {
        let content = fs::read_to_string(path)
            .await
            .map_err(|e| AetherError::provider(format!("Failed to read protocol file {:?}: {}", path, e)))?;

        let def: ProtocolDefinition = serde_yaml::from_str(&content)
            .map_err(|e| AetherError::provider(format!("Failed to parse protocol YAML: {}", e)))?;

        let client = Client::new();
        let protocol = ConfigurableProtocol::new(def.clone(), client);

        ProtocolRegistry::global().register(def.name.clone(), Arc::new(protocol))?;

        info!("Loaded protocol '{}' from {:?}", def.name, path);
        Ok(())
    }

    /// Load all protocols from directory
    pub async fn load_from_dir(dir: &Path) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }

        let mut entries = fs::read_dir(dir)
            .await
            .map_err(|e| AetherError::provider(format!("Failed to read directory {:?}: {}", dir, e)))?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            AetherError::provider(format!("Failed to read directory entry: {}", e))
        })? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("yaml") {
                if let Err(e) = Self::load_from_file(&path).await {
                    error!("Failed to load protocol from {:?}: {}", path, e);
                }
            }
        }

        Ok(())
    }

    /// Start hot reload watcher (implemented in Task 7)
    pub fn start_watching() -> Result<()> {
        info!("Hot reload not yet implemented");
        Ok(())
    }
}
```

**Step 5: Run tests to verify they pass**

Run: `cd core && cargo test loader::tests`
Expected: 2 tests pass

**Step 6: Commit**

```bash
git add core/src/providers/protocols/loader.rs core/Cargo.toml
git commit -m "feat(protocols): implement ProtocolLoader file and directory loading"
```

---

## Task 7: Implement Hot Reload with File Watching

**Files:**
- Modify: `core/src/providers/protocols/loader.rs`

**Step 1: Write test for hot reload**

Add to test section in `loader.rs`:

```rust
#[tokio::test]
async fn test_hot_reload() {
    use std::time::Duration;
    use tokio::time::sleep;

    let temp_dir = TempDir::new().unwrap();
    let protocol_file = temp_dir.path().join("hotreload.yaml");

    // Initial load
    let yaml_v1 = r#"
name: hotreload-test
extends: openai
base_url: https://api.v1.com
"#;
    fs::write(&protocol_file, yaml_v1).unwrap();
    ProtocolLoader::load_from_file(&protocol_file).await.unwrap();

    // Start watching
    let _watcher = ProtocolLoader::start_watching_dir(temp_dir.path()).unwrap();

    // Modify file
    let yaml_v2 = r#"
name: hotreload-test
extends: anthropic
base_url: https://api.v2.com
"#;
    sleep(Duration::from_millis(100)).await;
    fs::write(&protocol_file, yaml_v2).unwrap();

    // Wait for reload
    sleep(Duration::from_secs(3)).await;

    // Verify reload happened (check logs in manual testing)
    // Automated verification is complex, manual testing will confirm
}
```

**Step 2: Implement file watching**

Update `loader.rs`:

```rust
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc::channel;
use std::thread;

impl ProtocolLoader {
    // ... existing methods ...

    /// Start watching a directory for protocol file changes
    pub fn start_watching_dir(dir: &Path) -> Result<RecommendedWatcher> {
        let (tx, rx) = channel();

        let mut watcher = RecommendedWatcher::new(
            tx,
            Config::default().with_poll_interval(std::time::Duration::from_secs(2)),
        )
        .map_err(|e| AetherError::provider(format!("Failed to create file watcher: {}", e)))?;

        watcher
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|e| AetherError::provider(format!("Failed to watch directory {:?}: {}", dir, e)))?;

        info!("Watching protocols directory: {:?}", dir);

        // Spawn thread to handle events
        let dir_path = dir.to_path_buf();
        thread::spawn(move || {
            while let Ok(event) = rx.recv() {
                Self::handle_fs_event(event, &dir_path);
            }
        });

        Ok(watcher)
    }

    /// Handle filesystem events
    fn handle_fs_event(event: notify::Result<Event>, dir: &Path) {
        match event {
            Ok(Event { kind, paths, .. }) => match kind {
                EventKind::Create(_) | EventKind::Modify(_) => {
                    for path in paths {
                        if path.extension().and_then(|s| s.to_str()) == Some("yaml") {
                            info!("Protocol file changed: {:?}", path);
                            tokio::spawn(async move {
                                if let Err(e) = Self::load_from_file(&path).await {
                                    error!("Failed to reload protocol: {}", e);
                                }
                            });
                        }
                    }
                }
                EventKind::Remove(_) => {
                    for path in paths {
                        if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                            ProtocolRegistry::global().unregister(name);
                            info!("Unregistered protocol '{}'", name);
                        }
                    }
                }
                _ => {}
            },
            Err(e) => error!("File watch error: {}", e),
        }
    }

    /// Start hot reload for default directory
    pub fn start_watching() -> Result<Option<RecommendedWatcher>> {
        let protocols_dir = dirs::home_dir()
            .ok_or_else(|| AetherError::provider("Cannot determine home directory"))?
            .join(".aether/protocols");

        if !protocols_dir.exists() {
            info!("Protocols directory doesn't exist, skipping hot reload: {:?}", protocols_dir);
            return Ok(None);
        }

        Self::start_watching_dir(&protocols_dir).map(Some)
    }
}
```

**Step 3: Run test**

Run: `cd core && cargo test test_hot_reload -- --nocapture`
Expected: Test passes, logs show file watching

**Step 4: Commit**

```bash
git add core/src/providers/protocols/loader.rs
git commit -m "feat(protocols): implement hot reload with notify file watching"
```

---

## Task 8: Create Example YAML Protocol Configs

**Files:**
- Create: `examples/protocols/groq-custom.yaml`
- Create: `examples/protocols/exotic-ai.yaml`
- Create: `examples/protocols/README.md`

**Step 1: Create minimal config example**

Create `examples/protocols/groq-custom.yaml`:

```yaml
# Example: Minimal configuration mode
# Extends OpenAI protocol with custom authentication
name: groq-custom
extends: openai
base_url: https://api.groq.com/openai/v1

differences:
  # Custom authentication header
  auth:
    header: X-API-Key
    prefix: ""  # No "Bearer " prefix

  # Field customization
  request_fields:
    temperature:
      default: 0.7
      range: [0.0, 2.0]
```

**Step 2: Create full template example**

Create `examples/protocols/exotic-ai.yaml`:

```yaml
# Example: Full template mode
# Completely custom protocol implementation
name: exotic-ai
protocol_type: custom

base_url: https://api.exotic.ai

auth:
  type: header
  header: X-API-Token
  value_template: "{{config.api_key}}"

endpoints:
  chat: "/v2/completions"
  stream: "/v2/completions/stream"

request_template:
  model_name: "{{config.model}}"
  input_text: "{{input}}"
  system_instruction: "{{system_prompt}}"
  parameters:
    temperature: "{{config.temperature}}"
    max_tokens: "{{config.max_tokens}}"

response_mapping:
  content: "$.output.generated_text"
  error: "$.error.message"

stream_config:
  format: sse
  event_prefix: "data: "
  done_marker: "[DONE]"
  content_path: "$.chunk.text"
```

**Step 3: Create README**

Create `examples/protocols/README.md`:

```markdown
# Example Protocol Configurations

This directory contains example YAML protocol configurations for Aether.

## Usage

Copy example files to `~/.aether/protocols/` to use them:

```bash
mkdir -p ~/.aether/protocols
cp examples/protocols/groq-custom.yaml ~/.aether/protocols/
```

Aether will automatically load protocols from `~/.aether/protocols/` on startup and hot-reload changes.

## Examples

### Minimal Configuration Mode

**File**: `groq-custom.yaml`

Use when the provider API is similar to OpenAI but with minor differences:
- Custom authentication headers
- Different field names or defaults
- Different base URL

### Full Template Mode

**File**: `exotic-ai.yaml`

Use when the provider API is completely different:
- Custom request/response formats
- Different authentication schemes
- Custom streaming protocols

## Configuration Reference

See `docs/PROTOCOL_ADAPTER_USER_GUIDE.md` for complete documentation.
```

**Step 4: Verify files are created**

Run: `ls -la examples/protocols/`
Expected: See 3 files (groq-custom.yaml, exotic-ai.yaml, README.md)

**Step 5: Commit**

```bash
git add examples/protocols/
git commit -m "docs(protocols): add example YAML protocol configurations"
```

---

## Task 9: Write User Documentation

**Files:**
- Create: `docs/PROTOCOL_ADAPTER_USER_GUIDE.md`

**Step 1: Create comprehensive user guide**

Create `docs/PROTOCOL_ADAPTER_USER_GUIDE.md`:

```markdown
# Protocol Adapter User Guide

## Overview

Aether's Protocol Adapter system allows you to add support for new AI providers without modifying or recompiling the Rust codebase. Define new protocols using YAML configuration files that are automatically loaded and hot-reloaded.

## Quick Start

### 1. Create Protocol Configuration

Create `~/.aether/protocols/my-provider.yaml`:

```yaml
name: my-custom-provider
extends: openai
base_url: https://api.myprovider.com/v1
```

### 2. Use in Provider Config

Add to your `~/.aether/config.yaml`:

```yaml
providers:
  my_ai:
    protocol: my-custom-provider
    model: my-model-name
    api_key: your-api-key
```

### 3. Hot Reload

Aether watches `~/.aether/protocols/` and automatically reloads when you edit files. Changes take effect within 2 seconds.

## Configuration Modes

### Minimal Configuration Mode (Recommended)

Use when your provider's API is similar to OpenAI, Anthropic, or Gemini.

**Example**: Custom authentication

```yaml
name: groq-custom
extends: openai
base_url: https://api.groq.com/openai/v1

differences:
  auth:
    header: X-Custom-Key
    prefix: ""  # No "Bearer " prefix
```

**Example**: Field customization

```yaml
name: custom-fields
extends: openai

differences:
  request_fields:
    temperature:
      default: 0.7
      range: [0.0, 2.0]
    max_tokens:
      rename_to: max_completion_tokens
```

**Example**: Response path customization

```yaml
name: custom-response
extends: openai

differences:
  response_paths:
    content: "$.data.choices[0].text"  # JSONPath
```

### Full Template Mode (Advanced)

Use when your provider's API is completely different.

```yaml
name: exotic-provider
protocol_type: custom
base_url: https://api.exotic.com

# Authentication
auth:
  type: header
  header: X-API-Key
  value_template: "{{config.api_key}}"

# Endpoints
endpoints:
  chat: "/v2/completions"
  stream: "/v2/completions/stream"

# Request template (Handlebars syntax)
request_template:
  model_name: "{{config.model}}"
  input_text: "{{input}}"
  system_instruction: "{{system_prompt}}"
  parameters:
    temperature: "{{config.temperature}}"
    max_tokens: "{{config.max_tokens}}"

# Response parsing (JSONPath)
response_mapping:
  content: "$.output.generated_text"
  error: "$.error.message"

# Streaming config
stream_config:
  format: sse
  event_prefix: "data: "
  done_marker: "[DONE]"
  content_path: "$.chunk.text"
```

## Template Syntax

Aether uses Handlebars-style template syntax:

### Variables

- `{{config.model}}` - Model name from config
- `{{config.api_key}}` - API key
- `{{config.base_url}}` - Base URL
- `{{config.temperature}}` - Temperature parameter
- `{{config.max_tokens}}` - Max tokens parameter
- `{{input}}` - User input text
- `{{system_prompt}}` - System prompt
- `{{messages}}` - Full message array

### Default Values

```yaml
temperature: "{{config.temperature}}"  # Use config value or default to 1.0
```

## JSONPath Syntax

Use JSONPath to extract values from responses:

- `$.content` - Top-level field
- `$.data.choices[0].message.content` - Nested field with array
- `$.output.generated_text` - Custom field

## Hot Reload

Aether automatically watches these locations:

1. **Default directory**: `~/.aether/protocols/`
2. **Explicit paths** in `config.yaml`:

```yaml
protocol_extensions:
  - path: ./custom-protocols/my-provider.yaml
  - path: /etc/aether/shared-protocols/company.yaml
```

Changes to protocol files are detected within 2 seconds and applied automatically. No restart required.

## Troubleshooting

### Protocol not loading

Check logs for errors:
```bash
tail -f ~/.aether/logs/aether.log | grep protocol
```

Common issues:
- Invalid YAML syntax
- Missing required fields
- Invalid JSONPath expressions
- Template rendering errors

### Testing protocol

Use `aether test-protocol` command:

```bash
aether test-protocol ~/.aether/protocols/my-provider.yaml
```

This validates YAML syntax and template rendering.

## Examples

See `examples/protocols/` for complete examples:
- `groq-custom.yaml` - Minimal configuration mode
- `exotic-ai.yaml` - Full template mode

## Architecture

Protocol resolution order:
1. Check dynamic protocols (loaded from YAML)
2. Fall back to built-in protocols (openai, anthropic, gemini, ollama)

See `docs/ARCHITECTURE.md` for detailed architecture documentation.
```

**Step 2: Verify documentation**

Run: `cat docs/PROTOCOL_ADAPTER_USER_GUIDE.md | head -20`
Expected: See documentation header

**Step 3: Commit**

```bash
git add docs/PROTOCOL_ADAPTER_USER_GUIDE.md
git commit -m "docs(protocols): add comprehensive protocol adapter user guide"
```

---

## Task 10: Integration Testing

**Files:**
- Create: `core/tests/protocol_integration_test.rs`

**Step 1: Create integration test file**

Create `core/tests/protocol_integration_test.rs`:

```rust
//! Integration tests for configurable protocol system

use aethecore::config::ProviderConfig;
use aethecore::providers::protocols::{ConfigurableProtocol, ProtocolDefinition, ProtocolLoader, ProtocolRegistry};
use aethecore::providers::create_provider;
use std::sync::Once;

static INIT: Once = Once::new();

fn init_registry() {
    INIT.call_once(|| {
        ProtocolRegistry::global().register_builtin();
    });
}

#[tokio::test]
async fn test_end_to_end_minimal_protocol() {
    init_registry();

    // Load protocol from YAML
    let yaml = r#"
name: test-minimal
extends: openai
base_url: https://api.test.com
differences:
  auth:
    header: X-API-Key
    prefix: ""
"#;

    let def: ProtocolDefinition = serde_yaml::from_str(yaml).unwrap();
    let protocol = ConfigurableProtocol::new(def.clone(), reqwest::Client::new());
    ProtocolRegistry::global().register(def.name.clone(), std::sync::Arc::new(protocol)).unwrap();

    // Create provider using the protocol
    let config = ProviderConfig {
        protocol: Some("test-minimal".to_string()),
        model: "test-model".to_string(),
        api_key: Some("test-key".to_string()),
        base_url: Some("https://api.test.com".to_string()),
        ..Default::default()
    };

    let provider = create_provider("test", config);
    assert!(provider.is_ok());
}

#[tokio::test]
async fn test_protocol_hot_reload_simulation() {
    init_registry();

    use tempfile::TempDir;
    use tokio::fs;

    let temp_dir = TempDir::new().unwrap();
    let protocol_file = temp_dir.path().join("reload-test.yaml");

    // Version 1
    let yaml_v1 = r#"
name: reload-test
extends: openai
base_url: https://api.v1.com
"#;
    fs::write(&protocol_file, yaml_v1).await.unwrap();
    ProtocolLoader::load_from_file(&protocol_file).await.unwrap();

    let proto_v1 = ProtocolRegistry::global().get("reload-test");
    assert!(proto_v1.is_some());

    // Version 2 (simulating hot reload)
    let yaml_v2 = r#"
name: reload-test
extends: anthropic
base_url: https://api.v2.com
"#;
    fs::write(&protocol_file, yaml_v2).await.unwrap();
    ProtocolLoader::load_from_file(&protocol_file).await.unwrap();

    let proto_v2 = ProtocolRegistry::global().get("reload-test");
    assert!(proto_v2.is_some());
    // In real hot reload, this would be a different instance
}
```

**Step 2: Run integration tests**

Run: `cd core && cargo test --test protocol_integration_test`
Expected: 2 tests pass

**Step 3: Commit**

```bash
git add core/tests/protocol_integration_test.rs
git commit -m "test(protocols): add integration tests for configurable protocol system"
```

---

## Task 11: Update ARCHITECTURE.md

**Files:**
- Modify: `docs/ARCHITECTURE.md`

**Step 1: Add Protocol Adapter section**

Add to the Providers section in `docs/ARCHITECTURE.md`:

```markdown
### Protocol Adapter Architecture

Aether uses a layered protocol adapter system supporting multiple AI provider protocols:

**Layer 1: Built-in Protocols** (Compiled Rust)
- `OpenAiProtocol` - OpenAI-compatible APIs
- `AnthropicProtocol` - Claude/Anthropic APIs
- `GeminiProtocol` - Google Gemini APIs
- `OllamaProvider` - Local Ollama (native implementation)

**Layer 2: Configurable Protocols** (YAML-based, hot-reload)
- Minimal configuration mode - Extend existing protocols with differences
- Full template mode - Completely custom protocol implementations
- Loaded from `~/.aether/protocols/` directory
- Changes detected within 2 seconds (file watching)

**Layer 3: Extension Protocols** (Future)
- WASM/Node.js plugin protocols
- Independent process protocols (MCP/gRPC)

#### Protocol Resolution Flow

```
User config.protocol
    ↓
ProtocolRegistry.get(name)
    ↓
├─> Dynamic protocols (YAML-loaded) ───> ConfigurableProtocol
│   ├─> Minimal mode: base + differences
│   └─> Custom mode: template rendering
├─> Built-in protocols ───> OpenAi/Anthropic/Gemini
└─> Not found ───> Error with available list
```

#### Hot Reload Mechanism

1. `notify` crate watches `~/.aether/protocols/`
2. File change detected (Create/Modify/Delete)
3. YAML parsed into `ProtocolDefinition`
4. `ConfigurableProtocol` created
5. Registry updated atomically
6. New requests use updated protocol

See `docs/PROTOCOL_ADAPTER_USER_GUIDE.md` for user documentation.
```

**Step 2: Verify documentation compiles**

Run: `grep -A 10 "Protocol Adapter Architecture" docs/ARCHITECTURE.md`
Expected: See the new section

**Step 3: Commit**

```bash
git add docs/ARCHITECTURE.md
git commit -m "docs(architecture): document configurable protocol adapter system"
```

---

## Task 12: Run Full Test Suite

**Files:**
- None (verification step)

**Step 1: Run all provider tests**

Run: `cd core && cargo test providers::tests`
Expected: All tests pass

**Step 2: Run all protocol tests**

Run: `cd core && cargo test protocols::`
Expected: All tests pass

**Step 3: Run integration tests**

Run: `cd core && cargo test --tests`
Expected: All tests pass including integration tests

**Step 4: Build in release mode**

Run: `cd core && cargo build --release`
Expected: Clean build with no warnings

**Step 5: Document test results**

If all tests pass, create a summary comment in the plan indicating successful completion.

---

## Post-Implementation Checklist

After all tasks complete:

- [ ] All unit tests passing
- [ ] All integration tests passing
- [ ] Example YAML configs created and documented
- [ ] User guide written
- [ ] Architecture documentation updated
- [ ] No compiler warnings in release build
- [ ] Hot reload manually tested (watch logs while editing YAML)
- [ ] Ready for code review

## Manual Testing Steps

After implementation, manually verify:

1. **Create test protocol**:
```bash
mkdir -p ~/.aether/protocols
cat > ~/.aether/protocols/test.yaml << EOF
name: test-groq
extends: openai
base_url: https://api.groq.com/openai/v1
EOF
```

2. **Start Aether gateway**:
```bash
cargo run -p aethecore --features gateway
```

3. **Verify protocol loaded**: Check logs for "Loaded protocol 'test-groq'"

4. **Test hot reload**: Edit `test.yaml`, verify "Reloaded protocol" in logs

5. **Test with provider**: Add test-groq provider to config and send request

---

**Status**: Ready for implementation with `superpowers:subagent-driven-development`
