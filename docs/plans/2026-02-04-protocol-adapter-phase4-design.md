# Protocol Adapter Phase 4: Clean Architecture & Dynamic Protocols

**Date**: 2026-02-04
**Status**: 📝 Design Complete
**Branch**: TBD (will create during implementation)

---

## 1. Overview and Objectives

**Phase 4** focuses on two core objectives:
1. **Clean Technical Debt**: Remove legacy backward compatibility logic
2. **Extensibility**: Support user-defined protocols via configuration files

This is the natural evolution after successful Protocol Adapter architecture migration in Phases 1-3:
- **Phase 1**: OpenAI protocol adapter migration
- **Phase 2**: Claude and Gemini migration
- **Phase 3**: Added 11 OpenAI-compatible providers
- **Phase 4**: Cleanup + Dynamic protocols (this phase)
- **Phase 5**: Streaming optimization + Auto error recovery (future)

### Success Criteria

- ✅ `provider_type` field completely removed
- ✅ Configuration model simplified to `protocol` + preset
- ✅ Users can add new protocols via YAML without recompilation
- ✅ Hot reload: protocol file changes take effect automatically
- ✅ All existing tests pass
- ✅ Net code reduction (removing redundant logic)

### Non-Goals (Deferred to Phase 5)

- ❌ Extension/WASM protocols
- ❌ Independent process protocols
- ❌ Streaming performance optimization
- ❌ Automatic error recovery

---

## 2. Part A: Clean Backward Compatibility

### 2.1 Problem Diagnosis

Current code contains the following backward compatibility logic:

```rust
// core/src/config/types/provider.rs
pub struct ProviderConfig {
    pub provider_type: Option<String>,  // ← To be removed
    pub protocol: Option<String>,
    // ...
}

pub fn protocol(&self) -> String {
    if let Some(ref p) = self.protocol {
        return p.clone();
    }
    if let Some(ref t) = self.provider_type {
        return match t.to_lowercase().as_str() {
            "claude" => "anthropic".to_string(),  // ← Mapping to be removed
            _ => t.to_lowercase(),
        };
    }
    "openai".to_string()
}
```

**Issues:**
1. Two fields (`provider_type` and `protocol`) with overlapping semantics
2. Mapping logic increases cognitive burden
3. Users may be unclear which field to use
4. Test code mixes both approaches

### 2.2 Cleanup Plan

**To be deleted:**
1. `ProviderConfig::provider_type` field and related serialization logic
2. Mapping logic in `protocol()` method (`"claude" → "anthropic"`)
3. Technical aliases in Presets:
   - Remove `"anthropic"` (keep `"claude"`)
   - Remove `"google"` (keep `"gemini"`)
   - Keep `"volcengine"`, `"ark"` as aliases for `"doubao"` (brand diversity)

**Simplified logic:**
```rust
pub fn protocol(&self) -> String {
    self.protocol
        .clone()
        .unwrap_or_else(|| "openai".to_string())
}
```

**New configuration model:**
- User uses preset name (e.g., `"claude"`, `"deepseek"`) → factory auto-infers protocol
- User uses custom config → must explicitly specify `protocol: "anthropic"`

---

## 3. Part B: Dynamic Protocol Registration - Architecture

### 3.1 Layered Architecture

Dynamic protocol registration uses a three-layer design, ordered by increasing complexity:

```
Layer 1: Configurable Protocols (80% scenarios) ← Phase 4 implementation
├── Minimal configuration (extends existing protocols)
└── Full template (custom protocols)

Layer 2: Extension Protocols (15% scenarios) ← Reserved interface
└── WASM/Node.js plugins

Layer 3: Process Protocols (5% scenarios) ← Reserved interface
└── MCP/gRPC independent processes
```

### 3.2 Configurable Protocols (Layer 1)

Introduce new trait `ConfigurableProtocol` that implements `ProtocolAdapter`:

```rust
// core/src/providers/protocols/configurable.rs

/// Protocol adapter loaded from configuration file
pub struct ConfigurableProtocol {
    config: ProtocolDefinition,  // Parsed from YAML
    client: Client,
    base_protocol: Option<Arc<dyn ProtocolAdapter>>,  // Protocol being extended
}

/// Protocol definition (deserialized from YAML)
#[derive(Debug, Deserialize)]
pub struct ProtocolDefinition {
    pub name: String,
    pub extends: Option<String>,  // "openai", "anthropic", "gemini"

    #[serde(default)]
    pub base_url: Option<String>,

    #[serde(default)]
    pub differences: ProtocolDifferences,  // Minimal config mode

    #[serde(default)]
    pub custom: Option<CustomProtocol>,  // Full template mode
}
```

**Core concept:**
- If `extends` exists: Use base protocol + apply `differences`
- If `custom` exists: Fully custom (template-based)
- Two modes are mutually exclusive but can be combined (inherit then override)

---

## 4. Configurable Protocols - YAML Schema

### 4.1 Minimal Configuration Mode (Recommended)

For scenarios similar to existing protocols, only describe differences:

```yaml
# ~/.aleph/protocols/groq-custom.yaml
name: groq-custom
extends: openai
base_url: https://api.groq.com/openai/v1

differences:
  # Authentication differences
  auth:
    header: X-Custom-Key  # Override default "Authorization"
    prefix: ""            # No "Bearer " prefix

  # Request field differences
  request_fields:
    temperature:
      default: 0.7
      range: [0.0, 2.0]
    max_tokens:
      rename_to: max_completion_tokens  # Field name mapping

  # Response path differences
  response_paths:
    content: "data.choices[0].text"  # JSONPath
```

### 4.2 Full Template Mode (Advanced)

For completely different protocols:

```yaml
# ~/.aleph/protocols/exotic.yaml
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
    temperature: "{{config.temperature | default: 1.0}}"
    max_tokens: "{{config.max_tokens | default: 2048}}"

response_mapping:
  content: "output.generated_text"
  error: "error.message"

stream_config:
  format: sse
  event_prefix: "data: "
  done_marker: "[DONE]"
  content_path: "chunk.text"
```

### 4.3 Template Syntax

Using simple Handlebars-style syntax:
- `{{variable}}` - Variable substitution
- `{{variable | default: value}}` - With default value
- Supported context:
  - `config.*` - ProviderConfig fields
  - `input` - User input
  - `system_prompt` - System prompt
  - `messages` - Message array
  - `attachments` - Attachment array

---

## 5. Protocol Loading and Registration

### 5.1 Protocol Registry

Introduce a global registry to manage all protocols:

```rust
// core/src/providers/protocols/registry.rs

/// Protocol registry (singleton)
pub struct ProtocolRegistry {
    protocols: RwLock<HashMap<String, Arc<dyn ProtocolAdapter>>>,
    builtin: HashMap<String, fn(Client) -> Arc<dyn ProtocolAdapter>>,
}

impl ProtocolRegistry {
    pub fn global() -> &'static Self { /* ... */ }

    /// Register built-in protocols (on startup)
    pub fn register_builtin(&self) {
        self.builtin.insert("openai", |c| Arc::new(OpenAiProtocol::new(c)));
        self.builtin.insert("anthropic", |c| Arc::new(AnthropicProtocol::new(c)));
        self.builtin.insert("gemini", |c| Arc::new(GeminiProtocol::new(c)));
    }

    /// Register configurable protocol
    pub fn register_configurable(&self, name: String, def: ProtocolDefinition) -> Result<()> {
        let protocol = ConfigurableProtocol::from_definition(def, self)?;
        self.protocols.write().insert(name, Arc::new(protocol));
        Ok(())
    }

    /// Get protocol
    pub fn get(&self, name: &str) -> Option<Arc<dyn ProtocolAdapter>> {
        // 1. Check dynamically registered protocols
        if let Some(p) = self.protocols.read().get(name) {
            return Some(p.clone());
        }
        // 2. Fallback to built-in protocols
        self.builtin.get(name).map(|factory| factory(Client::new()))
    }
}
```

### 5.2 Hybrid Loading Strategy

```rust
// core/src/providers/protocols/loader.rs

pub struct ProtocolLoader {
    registry: &'static ProtocolRegistry,
    watcher: Option<RecommendedWatcher>,  // notify crate
}

impl ProtocolLoader {
    /// Load all protocols
    pub async fn load_all(&mut self) -> Result<()> {
        // 1. Auto-scan convention path
        let default_dir = dirs::home_dir()
            .unwrap_or_default()
            .join(".aether/protocols");

        if default_dir.exists() {
            self.load_from_dir(&default_dir).await?;
        }

        // 2. Load explicitly declared paths in config
        if let Some(extensions) = Config::global().protocol_extensions {
            for path in extensions {
                self.load_from_file(&path).await?;
            }
        }

        Ok(())
    }

    /// Load all .yaml files from directory
    async fn load_from_dir(&self, dir: &Path) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension() == Some("yaml".as_ref()) {
                self.load_from_file(&path).await?;
            }
        }
        Ok(())
    }

    /// Load protocol definition from file
    async fn load_from_file(&self, path: &Path) -> Result<()> {
        let content = fs::read_to_string(path)?;
        let def: ProtocolDefinition = serde_yaml::from_str(&content)?;
        self.registry.register_configurable(def.name.clone(), def)?;
        info!("Loaded protocol '{}' from {:?}", def.name, path);
        Ok(())
    }
}
```

### 5.3 Configuration File Extension

```yaml
# ~/.aleph/config.yaml
protocol_extensions:
  - path: ./custom-protocols/my-provider.yaml
  - path: /etc/aether/shared-protocols/company.yaml
```

---

## 6. Hot Reload Mechanism

### 6.1 File Watching

Use `notify` crate for filesystem monitoring:

```rust
// core/src/providers/protocols/loader.rs (continued)

impl ProtocolLoader {
    /// Start hot reload (watch file changes)
    pub fn start_watching(&mut self) -> Result<()> {
        let (tx, rx) = std::sync::mpsc::channel();

        let mut watcher = RecommendedWatcher::new(
            tx,
            notify::Config::default()
                .with_poll_interval(Duration::from_secs(2)),
        )?;

        // Watch convention directory
        let protocols_dir = dirs::home_dir()
            .unwrap_or_default()
            .join(".aether/protocols");

        if protocols_dir.exists() {
            watcher.watch(&protocols_dir, RecursiveMode::NonRecursive)?;
            info!("Watching protocols directory: {:?}", protocols_dir);
        }

        // Start event handling thread
        tokio::spawn(async move {
            while let Ok(event) = rx.recv() {
                Self::handle_fs_event(event).await;
            }
        });

        self.watcher = Some(watcher);
        Ok(())
    }

    /// Handle filesystem events
    async fn handle_fs_event(event: notify::Result<Event>) {
        match event {
            Ok(Event { kind, paths, .. }) => {
                match kind {
                    EventKind::Create(_) | EventKind::Modify(_) => {
                        for path in paths {
                            if path.extension() == Some("yaml".as_ref()) {
                                info!("Protocol file changed: {:?}", path);
                                if let Err(e) = Self::reload_protocol(&path).await {
                                    error!("Failed to reload protocol: {}", e);
                                }
                            }
                        }
                    }
                    EventKind::Remove(_) => {
                        for path in paths {
                            Self::unregister_protocol(&path);
                        }
                    }
                    _ => {}
                }
            }
            Err(e) => error!("File watch error: {}", e),
        }
    }

    /// Reload single protocol
    async fn reload_protocol(path: &Path) -> Result<()> {
        let content = fs::read_to_string(path)?;
        let def: ProtocolDefinition = serde_yaml::from_str(&content)?;

        // Re-register (overwrite old)
        ProtocolRegistry::global().register_configurable(def.name.clone(), def)?;
        info!("Reloaded protocol from {:?}", path);
        Ok(())
    }

    /// Unregister protocol
    fn unregister_protocol(path: &Path) {
        // Infer protocol name from filename
        if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
            ProtocolRegistry::global().unregister(name);
            info!("Unregistered protocol '{}'", name);
        }
    }
}
```

### 6.2 Hot Reload Safety

- ✅ **No service interruption**: Re-registration doesn't affect running requests
- ✅ **Atomic replacement**: New protocol instance only replaces old after successful creation
- ✅ **Error isolation**: Load failures don't affect existing protocols
- ⚠️ **Cache invalidation**: Provider instances using this protocol get new protocol on next request

### 6.3 Gateway Integration

```rust
// core/src/gateway/mod.rs

pub async fn start_gateway() -> Result<()> {
    // ... existing logic

    // Load protocols
    let mut loader = ProtocolLoader::new();
    loader.load_all().await?;
    loader.start_watching()?;  // Start hot reload

    // ... start WebSocket server
}
```

---

## 7. Integration with Existing System

### 7.1 Factory Function Update

Update `create_provider` to support dynamic protocols:

```rust
// core/src/providers/mod.rs

pub fn create_provider(name: &str, mut config: ProviderConfig) -> Result<Arc<dyn AiProvider>> {
    let name_lower = name.to_lowercase();

    // 1. Apply preset configuration
    if let Some(preset) = presets::get_preset(&name_lower) {
        if config.base_url.is_none() || config.base_url.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
            config.base_url = Some(preset.base_url.to_string());
        }
        if config.protocol.is_none() {
            config.protocol = Some(preset.protocol.to_string());
        }
        if config.color == "#808080" {
            config.color = preset.color.to_string();
        }
    }

    // 2. Determine protocol name
    let protocol_name = config.protocol();

    // 3. Get protocol adapter from registry
    let adapter = ProtocolRegistry::global()
        .get(&protocol_name)
        .ok_or_else(|| {
            AlephError::invalid_config(format!(
                "Unknown protocol: '{}'. Available: {:?}",
                protocol_name,
                ProtocolRegistry::global().list_protocols()
            ))
        })?;

    // 4. Special case: Ollama still uses native implementation
    if protocol_name == "ollama" {
        return Ok(Arc::new(OllamaProvider::new(name.to_string(), config)?));
    }

    // 5. Use HttpProvider + dynamic protocol
    let provider = HttpProvider::new(name.to_string(), config, adapter)?;
    Ok(Arc::new(provider))
}
```

### 7.2 ProviderConfig Simplification

```rust
// core/src/config/types/provider.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    // ❌ Deleted: pub provider_type: Option<String>,

    /// Protocol name (openai, anthropic, gemini, or custom)
    pub protocol: Option<String>,

    pub model: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,

    // ... other fields remain unchanged
}

impl ProviderConfig {
    /// Get effective protocol name
    pub fn protocol(&self) -> String {
        self.protocol
            .clone()
            .unwrap_or_else(|| "openai".to_string())
    }
}
```

### 7.3 Presets Cleanup

```rust
// core/src/providers/presets.rs

pub static PRESETS: Lazy<HashMap<&'static str, ProviderPreset>> = Lazy::new(|| {
    let mut m = HashMap::new();

    // Brand names (keep)
    m.insert("claude", ProviderPreset { /* ... */ });
    m.insert("gemini", ProviderPreset { /* ... */ });
    m.insert("kimi", ProviderPreset { /* ... */ });

    // ❌ Delete technical aliases
    // m.insert("anthropic", ...);
    // m.insert("google", ...);

    // ✅ Keep valuable aliases
    m.insert("moonshot", ProviderPreset { /* ... */ });
    m.insert("volcengine", ProviderPreset { /* ... */ });
    m.insert("ark", ProviderPreset { /* ... */ });

    m
});
```

---

## 8. Implementation Steps and Testing Strategy

### 8.1 Implementation Order

Phase 4 is divided into two sub-phases, each independently testable:

**Phase 4A: Clean Backward Compatibility** (Expected: 1-2 days)
1. Delete `ProviderConfig::provider_type` field
2. Simplify `protocol()` method (remove mapping logic)
3. Clean technical aliases in presets
4. Update all test cases
5. Update documentation and example configs
6. Commit: `refactor(providers): remove provider_type field and simplify protocol resolution`

**Phase 4B: Dynamic Protocol Registration** (Expected: 2-3 days)
1. Implement `ProtocolDefinition` and `ConfigurableProtocol`
2. Implement `ProtocolRegistry` registry
3. Implement `ProtocolLoader` loader and hot reload
4. Update `create_provider` factory function
5. Add configurable protocol tests
6. Commit: `feat(providers): add configurable protocol system with hot reload`

### 8.2 Testing Strategy

**Unit Tests (Phase 4A):**
```rust
#[test]
fn test_protocol_resolution_without_provider_type() {
    let config = ProviderConfig {
        protocol: Some("anthropic".to_string()),
        model: "claude-3-5-sonnet".to_string(),
        ..Default::default()
    };
    assert_eq!(config.protocol(), "anthropic");
}

#[test]
fn test_preset_inference() {
    let config = ProviderConfig {
        protocol: None,
        model: "gpt-4".to_string(),
        ..Default::default()
    };
    assert_eq!(config.protocol(), "openai");  // Default value
}

#[test]
fn test_brand_presets_exist() {
    assert!(get_preset("claude").is_some());
    assert!(get_preset("gemini").is_some());
    assert!(get_preset("kimi").is_some());
}

#[test]
fn test_technical_aliases_removed() {
    assert!(get_preset("anthropic").is_none());
    assert!(get_preset("google").is_none());
}
```

**Integration Tests (Phase 4B):**
```rust
#[tokio::test]
async fn test_load_minimal_config_protocol() {
    let yaml = r#"
name: test-groq
extends: openai
base_url: https://api.groq.com/openai/v1
differences:
  auth:
    header: X-Custom-Key
"#;

    let def: ProtocolDefinition = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(def.name, "test-groq");
    assert_eq!(def.extends, Some("openai".to_string()));
}

#[tokio::test]
async fn test_protocol_registry() {
    let registry = ProtocolRegistry::global();
    registry.register_builtin();

    assert!(registry.get("openai").is_some());
    assert!(registry.get("anthropic").is_some());
    assert!(registry.get("gemini").is_some());
}

#[tokio::test]
async fn test_configurable_protocol_creation() {
    let def = ProtocolDefinition {
        name: "test-custom".to_string(),
        extends: Some("openai".to_string()),
        // ...
    };

    let protocol = ConfigurableProtocol::from_definition(def, &registry).unwrap();
    assert_eq!(protocol.name(), "test-custom");
}
```

**End-to-End Tests:**
```rust
#[tokio::test]
async fn test_create_provider_with_custom_protocol() {
    // 1. Register custom protocol
    let yaml = r#"
name: my-custom
extends: openai
base_url: https://api.example.com
"#;
    ProtocolLoader::load_from_str(yaml).await.unwrap();

    // 2. Create provider
    let config = ProviderConfig {
        protocol: Some("my-custom".to_string()),
        model: "test-model".to_string(),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };

    let provider = create_provider("custom", config).unwrap();
    assert_eq!(provider.name(), "custom");
}
```

---

## 9. Documentation and User Guide

### 9.1 Breaking Changes Notice

Clearly state in project README and CHANGELOG:

```markdown
## Breaking Changes in Phase 4

### Removed: `provider_type` field

**Before:**
```yaml
providers:
  my_claude:
    provider_type: "claude"
    model: "claude-3-5-sonnet-20241022"
    api_key: "sk-xxx"
```

**After (Option 1 - Using preset):**
```yaml
providers:
  my_claude:  # Preset name "claude" auto-infers protocol
    model: "claude-3-5-sonnet-20241022"
    api_key: "sk-xxx"
```

**After (Option 2 - Explicit protocol):**
```yaml
providers:
  my_claude:
    protocol: "anthropic"
    model: "claude-3-5-sonnet-20241022"
    api_key: "sk-xxx"
```

### Removed: Technical alias presets

- ❌ `"anthropic"` preset removed → Use `"claude"`
- ❌ `"google"` preset removed → Use `"gemini"`
- ✅ Brand aliases retained: `"kimi"`, `"moonshot"`, `"volcengine"`, `"ark"`
```

### 9.2 Custom Protocol Guide

```markdown
## Creating Custom Protocol Adapters

### Minimal Configuration (Recommended)

For providers similar to OpenAI:

```yaml
# ~/.aleph/protocols/groq-custom.yaml
name: groq-custom
extends: openai
base_url: https://api.groq.com/openai/v1

differences:
  auth:
    header: Authorization
    prefix: "Bearer "

  request_fields:
    temperature:
      default: 0.7
```

Usage:
```yaml
providers:
  my_groq:
    protocol: "groq-custom"
    model: "mixtral-8x7b"
    api_key: "gsk_xxx"
```

### Full Template Configuration (Advanced)

For completely different protocols:

```yaml
# ~/.aleph/protocols/custom-api.yaml
name: custom-api
protocol_type: custom
base_url: https://api.custom.com

auth:
  type: header
  header: X-API-Key
  value_template: "{{config.api_key}}"

request_template:
  model: "{{config.model}}"
  prompt: "{{input}}"
  params:
    temp: "{{config.temperature | default: 1.0}}"

response_mapping:
  content: "result.text"
```

### Hot Reload

Aleph automatically watches `~/.aleph/protocols/` for changes.
Edit your protocol file and it will reload within 2 seconds.
```

### 9.3 Architecture Documentation Update

Update `docs/ARCHITECTURE.md` Providers section:

```markdown
## Provider Architecture

### Protocol Resolution

1. **Preset Lookup**: Check if provider name matches a preset
   - Preset provides: `base_url`, `protocol`, `color`

2. **Protocol Resolution**: Determine protocol adapter
   - Priority: `config.protocol` > preset.protocol > "openai"

3. **Adapter Lookup**: Query `ProtocolRegistry`
   - Built-in: `openai`, `anthropic`, `gemini`, `ollama`
   - Custom: User-defined in `~/.aleph/protocols/`

### Protocol Registry

- **Built-in Protocols**: Compiled Rust implementations
- **Configurable Protocols**: YAML-defined, hot-reloadable
- **Extension Protocols**: (Future) WASM/Node.js plugins
- **Process Protocols**: (Future) gRPC/MCP external processes
```

---

## 10. Summary and Future Outlook

### 10.1 Phase 4 Expected Outcomes

**Code Metrics:**
- Code deleted: ~100 lines (`provider_type` related logic)
- Code added: ~800 lines (configurable protocol system)
- Net addition: ~700 lines
- Test coverage: 20+ new test cases

**Functional Metrics:**
- ✅ Simplified configuration model: Single `protocol` field
- ✅ Extensibility: Users can add protocols via YAML
- ✅ Hot reload: Automatic effect within 2 seconds
- ✅ Backward compatible: All brand aliases retained

**User Value:**
1. **Lower contribution barrier**: Add new AI services without compiling Rust
2. **Fast experimentation**: Protocol config changes take effect immediately
3. **Clear configuration**: Remove redundant fields, clearer semantics
4. **Community friendly**: Users can share protocol config files

### 10.2 Phase 5 Outlook

Based on Phase 4's foundation, Phase 5 will focus on runtime optimization:

**C. Streaming Optimization**
- SSE parser performance optimization
- Smart buffering strategies
- Progress indication and cancellation support
- Enhanced streaming error handling

**D. Automatic Error Recovery**
- Smart retry strategies (exponential backoff)
- Cross-protocol failover
- Health checks and circuit breakers
- Automatic rate limit management

**Reserved Extension Points:**
- Layer 2: Extension protocols (WASM/Node.js)
- Layer 3: Process protocols (MCP/gRPC)
- Protocol version management
- A/B testing framework

### 10.3 Architecture Evolution Path

```
Phase 1-3: Protocol adapter migration (Complete)
    ↓
Phase 4: Cleanup + Dynamic (This phase)
    ↓
Phase 5: Runtime optimization
    ↓
Future: Complete plugin ecosystem
```

---

## Implementation Checklist

### Phase 4A: Clean Backward Compatibility
- [ ] Delete `provider_type` field from `ProviderConfig`
- [ ] Simplify `protocol()` method
- [ ] Remove `"anthropic"` and `"google"` presets
- [ ] Update all test cases
- [ ] Update documentation
- [ ] Commit and verify all tests pass

### Phase 4B: Dynamic Protocol Registration
- [ ] Implement `ProtocolDefinition` struct
- [ ] Implement `ConfigurableProtocol` adapter
- [ ] Implement `ProtocolRegistry` singleton
- [ ] Implement `ProtocolLoader` with hot reload
- [ ] Update `create_provider` factory
- [ ] Add comprehensive tests
- [ ] Write user guide and examples
- [ ] Commit and verify integration

---

**Status**: ✅ Design Complete, Ready for Implementation
