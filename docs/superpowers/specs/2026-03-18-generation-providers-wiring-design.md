# Generation Providers Wiring Design

**Date:** 2026-03-18
**Scope:** Wire up existing generation provider system at startup, add hot-reload, fix openai_compat URL flexibility

## Problem

The generation provider system (trait, 10 implementations, registry, config, Panel UI, RPC handlers) is fully implemented but **never initialized at runtime**. In `agent_init.rs`, the `BuiltinToolConfig` is built with `generation_registry: None`, so `generate_image`, `generate_video`, and `generate_audio` tools always return "not available".

Additionally, `openai_compat` hardcodes `/v1/images/generations` path onto `base_url`, making it impossible to use custom endpoints like `https://ai.t8star.cn/suno/generate`.

## Solution Overview

Three changes:

1. **Startup wiring** — Read `config.generation.providers`, create provider instances, inject into `BuiltinToolConfig`
2. **Hot-reload** — Background task listens for `config.generation.providers.changed` events, rebuilds entire registry
3. **openai_compat URL fix** — `base_url` becomes the full endpoint URL (no path appending)

## Design

### 1. Startup Wiring

**File:** `core/src/bin/aleph/commands/start/builder/agent_init.rs`

Before building `BuiltinToolConfig`, create and populate the generation registry:

```rust
let generation_registry = {
    let mut registry = GenerationProviderRegistry::new();
    for (name, provider_cfg) in &app_config.generation.providers {
        if !provider_cfg.enabled { continue; }
        if provider_cfg.api_key.as_ref().map(|k| k.is_empty()).unwrap_or(true) { continue; }
        match generation::providers::create_provider(name, provider_cfg) {
            Ok(provider) => { registry.register(name.clone(), provider).ok(); }
            Err(e) => { tracing::warn!(provider = %name, error = %e, "Skip generation provider"); }
        }
    }
    Arc::new(std::sync::RwLock::new(registry))
};
```

Then inject:

```rust
let tool_config = BuiltinToolConfig {
    generation_registry: Some(generation_registry.clone()),
    // ... rest unchanged
};
```

**Rules:**
- Skip `enabled: false` and empty API key providers
- Warn on creation failure, never panic
- Uses existing `Arc<std::sync::RwLock<GenerationProviderRegistry>>` type

### 2. Hot-Reload

**File:** `core/src/bin/aleph/commands/start/builder/agent_init.rs`

After startup wiring, spawn a background task:

```rust
{
    let gen_reg = generation_registry.clone();
    let config_handle = config.clone();  // Arc<RwLock<Config>>
    let mut rx = event_bus.subscribe();

    tokio::spawn(async move {
        while let Ok(event_json) = rx.recv().await {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&event_json) {
                if val.get("topic").and_then(|t| t.as_str())
                    != Some("config.generation.providers.changed")
                {
                    continue;
                }
            } else {
                continue;
            }

            let cfg = config_handle.read().await;
            let mut new_registry = GenerationProviderRegistry::new();
            for (name, provider_cfg) in &cfg.generation.providers {
                if !provider_cfg.enabled { continue; }
                if provider_cfg.api_key.as_ref().map(|k| k.is_empty()).unwrap_or(true) {
                    continue;
                }
                match generation::providers::create_provider(name, provider_cfg) {
                    Ok(provider) => { new_registry.register(name.clone(), provider).ok(); }
                    Err(e) => { tracing::warn!(provider = %name, error = %e, "Skip gen provider reload"); }
                }
            }

            let mut guard = gen_reg.write().unwrap_or_else(|e| e.into_inner());
            *guard = new_registry;
            tracing::info!("Generation provider registry reloaded ({} providers)", guard.len());
        }
    });
}
```

**Strategy:** Full registry rebuild on every change event.

**Rationale:**
- Provider instance creation is cheap (just HTTP client construction)
- Full rebuild avoids partial state risks
- Simplest possible code

### 3. openai_compat URL Fix

#### 3a. Config change

**File:** `core/src/config/types/generation/provider.rs`

Add `edit_url` field to `GenerationProviderConfig`:

```rust
/// Optional explicit edit endpoint URL (for openai_compat providers)
#[serde(default, skip_serializing_if = "Option::is_none")]
pub edit_url: Option<String>,
```

#### 3b. Provider struct change

**File:** `core/src/generation/providers/openai_compat/provider.rs`

Add `edit_endpoint: Option<String>` field to `OpenAiCompatProvider` struct.

#### 3c. Builder change

**File:** `core/src/generation/providers/openai_compat/builder.rs`

Add `edit_endpoint()` method to builder, wire it through to struct construction.

#### 3d. URL logic change

**File:** `core/src/generation/providers/openai_compat/helpers.rs`

```rust
// Before:
pub(crate) fn generations_url(&self) -> String {
    format!("{}/v1/images/generations", self.endpoint)
}
pub(crate) fn edits_url(&self) -> String {
    format!("{}/v1/images/edits", self.endpoint)
}

// After:
pub(crate) fn generations_url(&self) -> String {
    self.endpoint.clone()
}
pub(crate) fn edits_url(&self) -> String {
    if let Some(ref edit_url) = self.edit_endpoint {
        return edit_url.clone();
    }
    self.endpoint.replace("/generations", "/edits")
}
```

#### 3e. Factory change

**File:** `core/src/generation/providers/mod.rs`

In `create_provider()`, pass `config.edit_url` to `OpenAiCompatProviderBuilder`:

```rust
"openai_compat" => {
    let base_url = config.base_url.clone().ok_or_else(|| ...)?;
    let mut builder = OpenAiCompatProvider::builder(name, &api_key, &base_url);
    if let Some(ref edit_url) = config.edit_url {
        builder = builder.edit_endpoint(edit_url);
    }
    // ... rest unchanged
}
```

#### 3f. Panel UI change

**File:** `apps/panel/src/views/settings/generation_providers.rs`

- Rename `base_url` input label to "API Endpoint URL"
- For `openai_compat` type, show optional "Edit Endpoint URL" input

**File:** `apps/panel/src/api.rs`

- Add `edit_url: Option<String>` to Panel's `GenerationProviderConfig`

## Files Changed

| File | Change |
|------|--------|
| `core/src/bin/aleph/commands/start/builder/agent_init.rs` | Startup wiring + hot-reload task |
| `core/src/generation/providers/openai_compat/helpers.rs` | URL logic: use endpoint directly |
| `core/src/generation/providers/openai_compat/provider.rs` | Add `edit_endpoint` field |
| `core/src/generation/providers/openai_compat/builder.rs` | Add `edit_endpoint()` method |
| `core/src/generation/providers/mod.rs` | Pass `edit_url` in factory |
| `core/src/config/types/generation/provider.rs` | Add `edit_url` field |
| `apps/panel/src/views/settings/generation_providers.rs` | Label change + edit_url input |
| `apps/panel/src/api.rs` | Add `edit_url` field |

## Files NOT Changed

- `GenerationProvider` trait
- `GenerationProviderRegistry`
- RPC handlers (already complete)
- 10 specialized provider implementations
- `response_parser`
- Builtin tools (`generate_image` / `generate_video` / `generate_audio`)

## Configuration Example

```toml
[generation.providers.t8star-image]
provider_type = "openai_compat"
base_url = "https://ai.t8star.cn/v1/images/generations"
edit_url = "https://ai.t8star.cn/v1/images/edits"
capabilities = ["image"]
color = "#FF6B35"

[generation.providers.t8star-video]
provider_type = "openai_compat"
base_url = "https://ai.t8star.cn/v2/videos/generations"
capabilities = ["video"]
color = "#FF6B35"

[generation.providers.t8star-music]
provider_type = "openai_compat"
base_url = "https://ai.t8star.cn/suno/generate"
capabilities = ["audio"]
color = "#FF6B35"

[generation.providers.dalle]
provider_type = "openai"
capabilities = ["image"]
model = "dall-e-3"
```

## Estimated Size

~100 lines Rust + ~20 lines Leptos UI adjustment
