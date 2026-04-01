# Simplify Model Configuration Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove all dynamic model discovery infrastructure and add multi-model support per provider, returning to simple text-based model configuration.

**Architecture:** Four separate provider config structs (`ProviderConfig`, `EmbeddingProviderConfig`, `RerankConfig`, `GenerationProviderConfig`) each get `model → models: Vec<String>` migration. All model discovery code (ModelRegistry, probe endpoints, presets, ModelSelector UI) is deleted. Frontend replaces ModelSelector with simple comma-separated text inputs.

**Tech Stack:** Rust (serde custom deserializer), Leptos (WASM panel UI), TOML config

**Spec:** `docs/superpowers/specs/2026-03-15-simplify-model-config-design.md`

---

## Chunk 1: Core Config Type Changes

### Task 1: Shared serde helper for models deserialization

**Files:**
- Create: `src/config/types/serde_helpers.rs`
- Modify: `src/config/types/mod.rs` (add `pub mod serde_helpers;`)

- [ ] **Step 1: Create serde_helpers.rs**

Create `src/config/types/serde_helpers.rs` with two deserializers used by all provider config types:

```rust
use serde::de;

/// Deserializer for required models field.
/// Accepts both `model = "xxx"` (String) and `models = ["xxx", ...]` (Vec<String>).
/// Rejects empty lists and empty strings.
pub fn deserialize_models<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: de::Deserializer<'de>,
{
    struct ModelsVisitor;

    impl<'de> de::Visitor<'de> for ModelsVisitor {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or array of strings")
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Vec<String>, E> {
            if value.is_empty() {
                Err(E::custom("model name cannot be empty"))
            } else {
                Ok(vec![value.to_string()])
            }
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<String>, A::Error> {
            let mut models = Vec::new();
            while let Some(s) = seq.next_element::<String>()? {
                let trimmed = s.trim().to_string();
                if !trimmed.is_empty() {
                    models.push(trimmed);
                }
            }
            if models.is_empty() {
                Err(de::Error::custom("models list cannot be empty"))
            } else {
                Ok(models)
            }
        }
    }

    deserializer.deserialize_any(ModelsVisitor)
}

/// Deserializer for optional models field (used by GenerationProviderConfig).
/// Accepts String, Vec<String>, or null/missing. Empty vec is valid.
pub fn deserialize_optional_models<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: de::Deserializer<'de>,
{
    struct OptionalModelsVisitor;

    impl<'de> de::Visitor<'de> for OptionalModelsVisitor {
        type Value = Vec<String>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string, array of strings, or null")
        }

        fn visit_none<E: de::Error>(self) -> Result<Vec<String>, E> {
            Ok(Vec::new())
        }

        fn visit_unit<E: de::Error>(self) -> Result<Vec<String>, E> {
            Ok(Vec::new())
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Vec<String>, E> {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(Vec::new())
            } else {
                Ok(vec![trimmed.to_string()])
            }
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<String>, A::Error> {
            let mut models = Vec::new();
            while let Some(s) = seq.next_element::<String>()? {
                let trimmed = s.trim().to_string();
                if !trimmed.is_empty() {
                    models.push(trimmed);
                }
            }
            Ok(models)
        }
    }

    deserializer.deserialize_any(OptionalModelsVisitor)
}
```

- [ ] **Step 2: Add module declaration**

In `src/config/types/mod.rs`, add `pub mod serde_helpers;`.

- [ ] **Step 3: Commit**

```bash
git add src/config/types/serde_helpers.rs src/config/types/mod.rs
git commit -m "config: add shared serde helpers for models deserialization"
```

---

### Task 2: ProviderConfig — model → models migration

**Files:**
- Modify: `src/config/types/provider.rs`
- Modify: `src/config/tests/serialization.rs`

Note: `ProviderConfig` does NOT have a `Default` impl. The `test_config()` helper manually constructs all fields.

- [ ] **Step 1: Update ProviderConfig struct**

In `src/config/types/provider.rs`, change:
```rust
// BEFORE (line 38)
pub model: String,

// AFTER
#[serde(deserialize_with = "crate::config::types::serde_helpers::deserialize_models", alias = "model")]
pub models: Vec<String>,
```

Add the `default_model()` convenience method in the existing `impl ProviderConfig`:
```rust
/// Returns the default model (first in the list)
pub fn default_model(&self) -> &str {
    debug_assert!(!self.models.is_empty(), "models should never be empty after deserialization");
    &self.models[0]
}

/// Returns all configured models
pub fn all_models(&self) -> &[String] {
    &self.models
}
```

Update `test_config` helper — this manually constructs all fields (no `..Default::default()`):
```rust
pub fn test_config(model: &str) -> Self {
    Self {
        models: vec![model.to_string()],
        // ... all other fields remain the same as before
    }
}
```

- [ ] **Step 2: Update serialization tests**

In `src/config/tests/serialization.rs`, update any test that references `.model` to use `.models` or `.default_model()`. Key locations:
- Line 141: `ProviderConfig::test_config("gpt-4o")` — already uses helper, should work
- Lines 207-229: TOML round-trip test — verify `models = ["gpt-4o"]` appears in output

Add backward compatibility tests:
```rust
#[test]
fn test_provider_config_model_backward_compat() {
    let toml_str = r#"model = "gpt-4o""#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.default_model(), "gpt-4o");
    assert_eq!(config.models, vec!["gpt-4o".to_string()]);
}

#[test]
fn test_provider_config_models_vec() {
    let toml_str = r#"models = ["gpt-4o", "gpt-4o-mini", "o1"]"#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.default_model(), "gpt-4o");
    assert_eq!(config.models.len(), 3);
}

#[test]
fn test_provider_config_empty_models_rejected() {
    let toml_str = r#"models = []"#;
    let result = toml::from_str::<ProviderConfig>(toml_str);
    assert!(result.is_err());
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib -- config::tests::serialization`
Expected: All tests pass including new backward compat tests.

- [ ] **Step 4: Commit**

```bash
git add src/config/types/provider.rs src/config/tests/serialization.rs
git commit -m "config: migrate ProviderConfig model to models Vec with backward compat"
```

---

### Task 3: EmbeddingProviderConfig — model → models migration

**Files:**
- Modify: `src/config/types/memory.rs`

- [ ] **Step 1: Update EmbeddingProviderConfig struct**

In `src/config/types/memory.rs` (around line 190-221), change:
```rust
// BEFORE
pub model: String,

// AFTER
#[serde(deserialize_with = "crate::config::types::serde_helpers::deserialize_models", alias = "model")]
pub models: Vec<String>,
```

Add `default_model()` method:
```rust
impl EmbeddingProviderConfig {
    pub fn default_model(&self) -> &str {
        debug_assert!(!self.models.is_empty());
        &self.models[0]
    }
}
```

Update any Default impl or constructor to use `models: vec![...]`.

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib -- config`
Expected: Pass (fix any test that references `.model` on EmbeddingProviderConfig).

- [ ] **Step 3: Commit**

```bash
git add src/config/types/memory.rs
git commit -m "config: migrate EmbeddingProviderConfig model to models Vec"
```

---

### Task 4: RerankConfig — model → models migration

**Files:**
- Modify: `src/memory/rerank/provider.rs`
- Modify: `src/memory/rerank/mod.rs` (test assertions)

- [ ] **Step 1: Update RerankConfig struct**

In `src/memory/rerank/provider.rs` (around line 58-114), change:
```rust
// BEFORE
pub model: String,

// AFTER
#[serde(deserialize_with = "crate::config::types::serde_helpers::deserialize_models", alias = "model")]
pub models: Vec<String>,
```

Add `default_model()` method to `impl RerankConfig`.

- [ ] **Step 2: Update rerank mod.rs test assertions**

In `src/memory/rerank/mod.rs` (line 167), update test:
```rust
// BEFORE
assert_eq!(config.model, ...);
// AFTER
assert_eq!(config.default_model(), ...);
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib -- memory::rerank`
Expected: Pass.

- [ ] **Step 4: Commit**

```bash
git add src/memory/rerank/provider.rs src/memory/rerank/mod.rs
git commit -m "config: migrate RerankConfig model to models Vec"
```

---

### Task 5: GenerationProviderConfig — model → models migration

**Files:**
- Modify: `src/config/types/generation/provider.rs`
- Modify: `src/config/types/generation/mod.rs` (tests)

Note: `GenerationProviderConfig` has `model: Option<String>` (optional) AND a separate `models: HashMap<String, String>` field for model aliases. The `models` HashMap field needs to be renamed to `model_aliases` to free the name for the new `models: Vec<String>`.

- [ ] **Step 1: Update GenerationProviderConfig struct**

In `src/config/types/generation/provider.rs`:
```rust
// BEFORE
pub model: Option<String>,
pub models: HashMap<String, String>,  // model aliases

// AFTER
#[serde(deserialize_with = "crate::config::types::serde_helpers::deserialize_optional_models", alias = "model", default)]
pub models: Vec<String>,
#[serde(default)]
pub model_aliases: HashMap<String, String>,
```

Add convenience method:
```rust
impl GenerationProviderConfig {
    pub fn default_model(&self) -> Option<&str> {
        self.models.first().map(|s| s.as_str())
    }
}
```

- [ ] **Step 2: Update generation/mod.rs tests**

In `src/config/types/generation/mod.rs`, update test assertions from `.model` to `.default_model()` or `.models`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib -- config::types::generation`
Expected: Pass.

- [ ] **Step 4: Commit**

```bash
git add src/config/types/generation/
git commit -m "config: migrate GenerationProviderConfig model to models Vec, rename models HashMap to model_aliases"
```

---

## Chunk 2: Backend .model Reference Migration

### Task 6: Protocol implementations — .model → .default_model()

**Files:**
- Modify: `src/providers/protocols/openai.rs` (lines 266, 333)
- Modify: `src/providers/protocols/anthropic.rs` (lines 244, 261)
- Modify: `src/providers/protocols/chatgpt.rs` (lines 176, 184)
- Modify: `src/providers/protocols/gemini.rs` (lines 52, 57, 281)
- Modify: `src/providers/protocols/template.rs` (line 60)
- Modify: `src/providers/protocols/configurable.rs` (line 433, if applicable)

- [ ] **Step 1: Update all protocol files**

In each file, replace `config.model` with `config.default_model()` and `config.model.clone()` with `config.default_model().to_string()`.

For `template.rs` line 60, the template context must still expose `"model"` key:
```rust
// BEFORE
"model": config.model,
// AFTER
"model": config.default_model(),
```

This preserves backward compatibility for custom protocol templates using `{{config.model}}`.

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib -- providers::protocols`
Expected: Pass.

- [ ] **Step 3: Commit**

```bash
git add src/providers/protocols/
git commit -m "providers: migrate protocol implementations from .model to .default_model()"
```

---

### Task 7: Provider implementations — .model → .default_model()

**Files:**
- Modify: `src/providers/http_provider.rs` (lines 47, 237)
- Modify: `src/providers/ollama.rs` (lines 156, 183, 191, 607, 642, 647, 659)
- Modify: `src/providers/openai/request.rs` (lines 98, 236)
- Modify: `src/providers/profile_manager/mod.rs` (line 299)
- Modify: `src/providers/auth_profile_registry.rs` (line 169)
- Modify: `src/providers/mod.rs` (provider creation)

- [ ] **Step 1: Update all provider files**

Replace `config.model` → `config.default_model()` and `config.model.clone()` → `config.default_model().to_string()`.

Special case in `ollama.rs` line 607 (test setup):
```rust
// BEFORE
config.model = "";
// AFTER
config.models = vec!["test-model".to_string()];
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib -- providers`
Expected: Pass.

- [ ] **Step 3: Commit**

```bash
git add src/providers/
git commit -m "providers: migrate provider implementations from .model to .default_model()"
```

---

### Task 8: Embedding & Reranking providers — .model → .default_model()

**Files:**
- Modify: `src/memory/embedding_provider.rs` (line 77)
- Modify: `src/memory/rerank/jina.rs` (line 73)
- Modify: `src/memory/rerank/siliconflow.rs` (line 73)
- Modify: `src/memory/rerank/vllm.rs` (line 73)
- Modify: `src/memory/rerank/pinecone.rs` (line 83)
- Modify: `src/memory/rerank/voyage.rs` (line 73)

- [ ] **Step 1: Update all files**

In each file, replace `config.model` / `self.config.model` with the appropriate `.default_model()` call.

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib -- memory`
Expected: Pass.

- [ ] **Step 3: Commit**

```bash
git add src/memory/
git commit -m "memory: migrate embedding and reranking providers from .model to .default_model()"
```

---

### Task 9: Generation providers — .model → .default_model() and .models → .model_aliases

**Files:**
- Modify: `src/generation/providers/mod.rs` (lines 139, 144, 157, 173, 178, 183, 193, 207, 218, 241)
- Modify: `src/dispatcher/analyzer.rs` (uses `config.models.keys()` on GenerationProviderConfig HashMap — must change to `config.model_aliases.keys()`)
- Modify: any other generation provider files referencing `.model` or `.models` HashMap

- [ ] **Step 1: Update generation provider files**

For `GenerationProviderConfig`, `default_model()` returns `Option<&str>`. Callers that previously did `config.model.clone()` (where model was `Option<String>`) should now use `config.default_model().map(|s| s.to_string())` or `config.default_model()`.

**Critical**: Also update all references to the old `config.models` HashMap (now renamed to `config.model_aliases`):
- `src/generation/providers/mod.rs` line 198: `for (alias, version) in &config.models` → `&config.model_aliases`
- `src/dispatcher/analyzer.rs`: `provider_config.models.keys().cloned()` → `provider_config.model_aliases.keys().cloned()`

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib -- generation`
Expected: Pass.

- [ ] **Step 3: Commit**

```bash
git add src/generation/ src/dispatcher/
git commit -m "generation: migrate from .model to .default_model() and .models to .model_aliases"
```

---

### Task 10: Gateway handlers and DTO types — .model → .models migration

**Files:**
- Modify: `src/gateway/handlers/providers/types.rs` (update `ProviderInfo.model` and `ProviderConfigJson.model` to `models: Vec<String>`)
- Modify: `src/gateway/handlers/providers/handlers.rs` (line 362 and construction of ProviderInfo)
- Modify: `src/gateway/handlers/providers/tests.rs` (test assertions)
- Modify: `src/gateway/handlers/generation_providers.rs` (line 85)
- Modify: `src/gateway/handlers/embedding_providers.rs` (lines 45-46, 522)
- Modify: `src/bin/aleph/commands/start/builder/agent_init.rs` (line 358)
- Modify: `src/agents/thinking_adapter.rs` (lines 104, 154)

- [ ] **Step 1: Update DTO types**

In `src/gateway/handlers/providers/types.rs`:
```rust
// ProviderInfo: change model: String → models: Vec<String>
// ProviderConfigJson: change model: String → models: Vec<String> (with same serde backward compat)
```

Where handlers construct `ProviderInfo`, use `models: config.models.clone()` instead of `model: config.model.clone()`.

- [ ] **Step 2: Update remaining handler files**

In each file listed above, replace `.model` references on provider configs with `.default_model()` or `.models` as appropriate.

Note: `src/gateway/agent_instance.rs` line 780 uses `.model` on `AgentInstanceConfig`, NOT `ProviderConfig` — do NOT change this.

- [ ] **Step 3: Use cargo check to find any remaining references**

Run: `cargo check -p alephcore 2>&1 | grep "no field.*model"`
Fix any remaining compilation errors from the migration.

- [ ] **Step 4: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: Pass (except pre-existing `tools::markdown_skill::loader::tests` failures).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "core: migrate all remaining .model references to .default_model() and update DTOs"
```

---

## Chunk 3: Delete Model Discovery Infrastructure

All three tasks in this chunk MUST be done together in a single commit to avoid intermediate build breakage. The probe handlers reference `OllamaDiscoveryAdapter` and `MODEL_REGISTRY` from the models module, so deleting one without the other breaks compilation.

### Task 11: Delete all model discovery code (single atomic operation)

**Files:**
- Delete: `src/providers/model_registry.rs`
- Delete: `shared/config/model-presets.toml`
- Delete: `src/gateway/handlers/models/` (entire directory)
- Modify: `src/providers/mod.rs` (remove `pub mod model_registry;` AND `pub use model_registry::ModelRegistry;` re-export)
- Modify: `src/providers/adapter.rs` (remove `list_models()` method + `DiscoveredModel` struct)
- Modify: `src/providers/protocols/openai.rs` (remove `list_models()` override if any)
- Modify: `src/providers/protocols/anthropic.rs` (remove `list_models()` override)
- Modify: `src/providers/protocols/chatgpt.rs` (remove `list_models()` override)
- Modify: `src/providers/protocols/gemini.rs` (remove `list_models()` override)
- Modify: `src/providers/ollama.rs` (remove `list_models()` method)
- Modify: `src/extension/provider_adapter.rs` (remove `list_models()` and `static_models()`)
- Modify: `src/gateway/handlers/mod.rs` (remove `pub mod models;` and models handler registrations at lines 196-217)
- Modify: `src/gateway/handlers/providers/handlers.rs` (remove `handle_probe()` at line 467)
- Modify: `src/gateway/handlers/providers/types.rs` (remove `ProbeParams`, `ProbeResult`)
- Modify: `src/gateway/handlers/providers/mod.rs` (remove probe re-exports)
- Modify: `src/gateway/handlers/embedding_providers.rs` (remove `handle_probe()`, `EmbeddingProbeParams`, `EmbeddingProbeResult`, `handle_presets()`)
- Modify: `src/bin/aleph/commands/start/builder/handlers.rs`:
  - Delete `register_models_handlers` function (lines 435-479)
  - Remove call to `register_models_handlers` (also check `src/bin/aleph/commands/start/mod.rs` line 452)
  - Remove import: `use alephcore::gateway::handlers::models as models_handlers;`
  - Remove `providers.probe` registration (line 556)
  - Remove `embedding_providers.probe` registration (line 605)
  - Remove `embedding_providers.presets` registration (line 606)

- [ ] **Step 1: Delete files and directories**

```bash
rm src/providers/model_registry.rs
rm shared/config/model-presets.toml
rm -rf src/gateway/handlers/models/
```

- [ ] **Step 2: Remove module declarations and re-exports**

In `src/providers/mod.rs`:
- Remove `pub mod model_registry;` line
- Remove `pub use model_registry::ModelRegistry;` re-export (if exists)

In `src/gateway/handlers/mod.rs`:
- Remove `pub mod models;`
- Remove the models handler registration block (lines 196-217 approximately)

- [ ] **Step 3: Remove ProtocolAdapter::list_models() and DiscoveredModel**

In `src/providers/adapter.rs`:
- Delete the `DiscoveredModel` struct (lines 303-314)
- Delete the `list_models()` method from the `ProtocolAdapter` trait (lines 164-229)
- Remove any imports only used by `list_models`

In each protocol file, remove `list_models()` overrides:
- `anthropic.rs`: Remove `list_models() -> Ok(None)`
- `chatgpt.rs`: Remove `list_models() -> Ok(None)`
- `gemini.rs`: Remove the full Gemini model listing implementation
- `openai.rs`: Check for overrides

In `src/providers/ollama.rs`: Remove `list_models()` method.

In `src/extension/provider_adapter.rs`: Remove `list_models()` and `static_models()` methods.

- [ ] **Step 4: Remove probe handlers**

In `src/gateway/handlers/providers/handlers.rs`: Delete `handle_probe()` function.

In `src/gateway/handlers/providers/types.rs`: Delete `ProbeParams` and `ProbeResult`.

In `src/gateway/handlers/providers/mod.rs`: Remove probe re-exports.

In `src/gateway/handlers/embedding_providers.rs`: Delete `handle_probe()`, `EmbeddingProbeParams`, `EmbeddingProbeResult`, and `handle_presets()`.

- [ ] **Step 5: Remove handler registrations**

In `src/bin/aleph/commands/start/builder/handlers.rs`:
- Delete `register_models_handlers` function (lines 435-479)
- Remove `use alephcore::gateway::handlers::models as models_handlers;` import
- Remove `providers.probe` registration (line 556)
- Remove `embedding_providers.probe` registration (line 605)
- Remove `embedding_providers.presets` registration (line 606)

In `src/bin/aleph/commands/start/mod.rs`:
- Remove the call to `register_models_handlers` (line 452)

- [ ] **Step 6: Clean up any remaining imports**

Search for and remove any remaining imports of deleted modules:
```bash
cargo check -p alephcore 2>&1 | grep -i "unresolved\|cannot find"
```
Fix all compilation errors.

- [ ] **Step 7: Compile and test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: Pass.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "core: delete all model discovery infrastructure (ModelRegistry, probes, presets, list_models)"
```

**Note**: The OpenAI-compatible `/v1/models` endpoint in `src/gateway/openai_api/routes.rs` is intentionally NOT deleted — it serves external tool integration, not internal model discovery.

---

## Chunk 4: Frontend Changes

### Task 12: Remove ModelSelector component and probe types from API

**Files:**
- Delete: `apps/panel/src/components/model_selector.rs`
- Delete: `apps/panel/src/components/probe_indicator.rs`
- Modify: `apps/panel/src/components/mod.rs` (remove `pub mod model_selector;` and `pub mod probe_indicator;`)
- Modify: `apps/panel/src/api.rs` (remove probe types, methods, and migrate `.model` → `.models` in DTO types)

- [ ] **Step 1: Delete component files**

```bash
rm apps/panel/src/components/model_selector.rs
rm apps/panel/src/components/probe_indicator.rs
```

- [ ] **Step 2: Remove module declarations**

In `apps/panel/src/components/mod.rs`:
- Remove `pub mod model_selector;`
- Remove `pub mod probe_indicator;`

- [ ] **Step 3: Remove probe types and migrate API DTOs**

In `apps/panel/src/api.rs`:
- Delete `ProbeModelInfo` struct (lines 448-454)
- Delete `ProbeResultInfo` struct (lines 458-468)
- Delete `ProvidersApi::probe()` method (lines 597-614)
- Delete `EmbeddingProvidersApi::probe()` method (lines 1961-1978)
- Update all provider API response types that have `model: String` to `models: Vec<String>`:
  - Line 381: `ProviderInfo` struct
  - Line 409: any other response DTO
  - Lines 1011, 1832, 1856, 1884: embedding/reranking/generation DTOs

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "panel: remove ModelSelector, ProbeIndicator components, probe API methods, and migrate DTOs"
```

---

### Task 13: Update Providers settings view

**Files:**
- Modify: `apps/panel/src/views/settings/providers.rs`

- [ ] **Step 1: Remove all probe logic**

Remove:
- `probe_status` signal (line 504)
- Auto-probe on mount (lines 570-618)
- Manual `trigger_probe` closure (lines 769-841)
- Refresh/API key change callbacks (lines 831-841)
- `ProbeStatus` imports
- `ModelSelector` component usage (line 1147+)

- [ ] **Step 2: Replace with simple text input**

Where the ModelSelector was, add a simple text input for models:
```rust
// Models input (comma-separated)
<div class="form-group">
    <label>"Models"</label>
    <input
        type="text"
        placeholder="e.g. gpt-4o, gpt-4o-mini, o1"
        prop:value=move || models_value.get()
        on:input=move |ev| {
            let val = event_target_value(&ev);
            models_value.set(val);
        }
    />
    <small class="help-text">"Comma-separated. First model is the default."</small>
</div>
```

The `models_value` signal holds the comma-separated string. On save, split and trim:
```rust
let models: Vec<String> = models_value.get()
    .split(',')
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .collect();
```

On load, join from vec:
```rust
let models_str = provider.models.join(", ");
models_value.set(models_str);
```

- [ ] **Step 3: Commit**

```bash
git add apps/panel/src/views/settings/providers.rs
git commit -m "panel: replace ModelSelector with simple text input in providers settings"
```

---

### Task 14: Update Embedding Providers settings view

**Files:**
- Modify: `apps/panel/src/views/settings/embedding_providers.rs`

- [ ] **Step 1: Remove probe logic and ModelSelector**

Remove:
- `probe_status` signals (line 378)
- `trigger_probe` closures (lines 391-436, 813-855)
- Auto-probe on mount (lines 438-450)
- ModelSelector usages (lines 631-637, 981-988)
- All ProbeStatus/ProbeIndicator imports

- [ ] **Step 2: Replace with simple text input**

Same pattern as Task 13 — comma-separated text input for models with split/join logic.

- [ ] **Step 3: Commit**

```bash
git add apps/panel/src/views/settings/embedding_providers.rs
git commit -m "panel: replace ModelSelector with simple text input in embedding providers settings"
```

---

### Task 15: Update Reranking Providers settings view

**Files:**
- Modify: `apps/panel/src/views/settings/reranking_providers.rs`

- [ ] **Step 1: Remove probe logic and ModelSelector**

Remove:
- `probe_loading` signals (line 261, 596)
- Auto-probe on mount (lines 268-309)
- `trigger_custom_probe` closure (lines 601-637)
- "Discover Models" button (lines 806-812)
- ModelSelector usages (lines 446-450, 784-788)

- [ ] **Step 2: Replace with simple text input**

Same pattern — comma-separated text input.

- [ ] **Step 3: Commit**

```bash
git add apps/panel/src/views/settings/reranking_providers.rs
git commit -m "panel: replace ModelSelector with simple text input in reranking providers settings"
```

---

### Task 16: Update Generation Providers settings view

**Files:**
- Modify: `apps/panel/src/views/settings/generation_providers.rs`

- [ ] **Step 1: Update model input**

This view uses hardcoded presets (lines 14-44 `generation_models_for_type()`), not dynamic probe. Remove the preset function and replace with comma-separated text input for models.

- [ ] **Step 2: Commit**

```bash
git add apps/panel/src/views/settings/generation_providers.rs
git commit -m "panel: replace hardcoded model presets with text input in generation providers settings"
```

---

### Task 17: Update Setup Wizard

**Files:**
- Modify: `apps/panel/src/views/wizard/setup_wizard.rs`

- [ ] **Step 1: Remove probe flow from wizard**

Remove:
- `probe_status` signal (line 33)
- `do_probe` closure (lines 48-95)
- Auto-probe triggers in SelectProvider/EnterCredentials steps
- ModelSelector in SelectModel step

- [ ] **Step 2: Replace model selection step**

The SelectModel wizard step should show a simple text input instead of ModelSelector. The user types their model name(s) directly.

- [ ] **Step 3: Commit**

```bash
git add apps/panel/src/views/wizard/setup_wizard.rs
git commit -m "panel: simplify setup wizard model selection to text input"
```

---

## Chunk 5: Final Cleanup and Validation

### Task 18: Full compile, test, and zombie code cleanup

- [ ] **Step 1: Full compile check**

Run: `cargo check -p alephcore`
Expected: Clean compile, no errors.

- [ ] **Step 2: Run all core tests**

Run: `cargo test -p alephcore --lib`
Expected: Pass (except pre-existing `tools::markdown_skill::loader::tests` failures).

- [ ] **Step 3: Build WASM panel**

Run: `just dev` or the WASM build command
Expected: Panel compiles successfully.

- [ ] **Step 4: Grep for zombie code**

Search for any remaining references to deleted modules:
```bash
rg "model_registry|ModelRegistry|MODEL_REGISTRY|DiscoveredModel|list_models|ProbeStatus|ProbeResult|ProbeParams|ModelSelector|model_selector|probe_indicator|ProbeIndicator|model.presets" --type rust
```
Expected: No matches (or only in unrelated contexts like comments/docs).

- [ ] **Step 5: Grep for remaining .model references on provider configs**

```bash
rg "config\.model[^s_]|\.model\.clone|\.model\.to_" --type rust src/
```
Expected: No matches on provider config types (some may exist on unrelated structs like `AgentInstanceConfig` — verify each is not a provider config).

- [ ] **Step 6: Update default-config.toml**

In `shared/config/default-config.toml`, update any `model = "xxx"` fields to `models = ["xxx"]` format for all provider sections.

- [ ] **Step 7: Final commit**

```bash
git add -A
git commit -m "cleanup: remove zombie code and update default config for models Vec"
```
