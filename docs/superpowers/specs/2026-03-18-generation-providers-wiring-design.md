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

**File:** `src/bin/aleph/commands/start/builder/agent_init.rs`

Create and populate the generation registry **unconditionally** (independent of whether an AI chat provider exists, since generation and chat are separate capabilities):

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

Then inject into `BuiltinToolConfig`:

```rust
let tool_config = BuiltinToolConfig {
    generation_registry: Some(generation_registry.clone()),
    // ... rest unchanged
};
```

**Rules:**
- Created at top level of `register_agent_handlers`, outside the AI provider conditional block
- Skip `enabled: false` and empty API key providers
- Warn on creation failure, never panic
- Uses existing `Arc<std::sync::RwLock<GenerationProviderRegistry>>` type

### 2. Hot-Reload

**File:** `src/bin/aleph/commands/start/builder/agent_init.rs`

After startup wiring, spawn a background task. The task needs access to the vault (`SharedTokenManager`) because RPC handlers set `api_key = None` in config and store keys in the vault separately.

```rust
{
    let gen_reg = generation_registry.clone();
    let config_handle = config.clone();       // Arc<tokio::sync::RwLock<Config>>
    let vault = shared_token_mgr.clone();     // Arc<SharedTokenManager>
    let mut rx = event_bus.subscribe();

    tokio::spawn(async move {
        while let Ok(event_json) = rx.recv().await {
            // Only react to generation provider changes
            let is_gen_event = serde_json::from_str::<serde_json::Value>(&event_json)
                .ok()
                .and_then(|v| v.get("topic")?.as_str().map(|s| s.to_string()))
                == Some("config.generation.providers.changed".to_string());
            if !is_gen_event { continue; }

            // Snapshot config (drop read guard before creating providers)
            let providers_snapshot = {
                let cfg = config_handle.read().await;
                cfg.generation.providers.clone()
            };

            // Rebuild registry with vault-resolved API keys
            let mut new_registry = GenerationProviderRegistry::new();
            for (name, mut provider_cfg) in providers_snapshot {
                if !provider_cfg.enabled { continue; }

                // Resolve API key from vault (RPC handlers store keys there, not in config)
                if provider_cfg.api_key.is_none() {
                    if let Ok(Some(secret)) = vault.get_secret(&format!("gen:{}", name)) {
                        provider_cfg.api_key = Some(secret.expose().to_string());
                    }
                }
                if provider_cfg.api_key.as_ref().map(|k| k.is_empty()).unwrap_or(true) {
                    continue;
                }

                match generation::providers::create_provider(&name, &provider_cfg) {
                    Ok(provider) => { new_registry.register(name.clone(), provider).ok(); }
                    Err(e) => { tracing::warn!(provider = %name, error = %e, "Skip gen provider reload"); }
                }
            }

            // Atomic swap
            let mut guard = gen_reg.write().unwrap_or_else(|e| e.into_inner());
            *guard = new_registry;
            tracing::info!("Generation provider registry reloaded ({} providers)", guard.len());
        }
    });
}
```

**Strategy:** Full registry rebuild on every change event.

**Key points:**
- Config read guard is dropped before provider creation (defensive, avoids holding lock during I/O)
- API keys resolved from vault via `SharedTokenManager` (same pattern as startup in `start/mod.rs`)
- Lock poison handled with `unwrap_or_else(|e| e.into_inner())`

### 3. openai_compat URL Fix

#### 3a. Config change

**File:** `src/config/types/generation/provider.rs`

Add `edit_url` field to `GenerationProviderConfig`:

```rust
/// Optional explicit edit endpoint URL (for openai_compat providers)
#[serde(default, skip_serializing_if = "Option::is_none")]
pub edit_url: Option<String>,
```

#### 3b. Provider struct change

**File:** `src/generation/providers/openai_compat/provider.rs`

Add `edit_endpoint: Option<String>` field to `OpenAiCompatProvider` struct.

#### 3c. Builder change

**File:** `src/generation/providers/openai_compat/builder.rs`

Two changes:

1. Add `edit_endpoint: Option<String>` field and `edit_endpoint()` builder method
2. **Remove the `/v1` normalization** in `build()`. Currently the builder strips `/v1` from the URL:

```rust
// REMOVE this normalization:
let endpoint = self.base_url
    .trim_end_matches('/')
    .trim_end_matches("/v1")
    .trim_end_matches('/')
    .to_string();

// REPLACE with simple trailing-slash strip:
let endpoint = self.base_url.trim_end_matches('/').to_string();
```

This is necessary because `base_url` is now the full endpoint URL. The old normalization would destroy paths like `/v1/images/generations` → `/images/generations`.

#### 3d. URL logic change

**File:** `src/generation/providers/openai_compat/helpers.rs`

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
    // Heuristic: replace "/generations" with "/edits" for standard OpenAI-style URLs.
    // For non-standard URLs (e.g. /suno/generate), this returns unchanged — which is fine
    // because those providers typically don't support editing. Users with non-standard edit
    // endpoints should set `edit_url` explicitly.
    self.endpoint.replace("/generations", "/edits")
}
```

#### 3e. Factory change

**File:** `src/generation/providers/mod.rs`

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
| `src/bin/aleph/commands/start/builder/agent_init.rs` | Startup wiring + hot-reload task (needs vault access) |
| `src/generation/providers/openai_compat/helpers.rs` | URL logic: use endpoint directly |
| `src/generation/providers/openai_compat/provider.rs` | Add `edit_endpoint` field |
| `src/generation/providers/openai_compat/builder.rs` | Add `edit_endpoint()` method; remove `/v1` normalization |
| `src/generation/providers/mod.rs` | Pass `edit_url` in factory |
| `src/config/types/generation/provider.rs` | Add `edit_url` field |
| `apps/panel/src/views/settings/generation_providers.rs` | Label change + edit_url input |
| `apps/panel/src/api.rs` | Add `edit_url` field |

## Files NOT Changed

- `GenerationProvider` trait
- `GenerationProviderRegistry`
- RPC handlers (already complete)
- 10 specialized provider implementations (openai_image, stability, etc. keep their own URL logic)
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

~120 lines Rust + ~20 lines Leptos UI adjustment
