# Generation Provider Isolation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split generation providers into 4 isolated categories (image/video/speech/audio) with smart URL auto-completion.

**Architecture:** Config adds `image_providers`/`video_providers`/`speech_providers`/`audio_providers` sections. Old `providers` format auto-mapped via `capabilities[0]`. Shared `ResolvedUrl` enum handles URL normalization for all provider types. Panel, RPC handlers, and slash commands adapted.

**Tech Stack:** Rust, serde, TOML config, Leptos WASM (Panel)

**Spec:** `docs/superpowers/specs/2026-03-24-generation-provider-isolation-design.md`

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `src/generation/providers/url_normalize.rs` | `ResolvedUrl`, `resolve_base_url()`, `needs_auto_complete()` |
| Modify | `src/config/types/generation/config.rs` | Add 4 typed provider maps + merge logic |
| Modify | `src/config/types/generation/provider.rs` | Make `capabilities` optional (derived from section) |
| Modify | `src/generation/providers/factory.rs` | Accept `GenerationType`, pass to provider constructors |
| Modify | `src/generation/providers/openai_tts.rs` | Use `ResolvedUrl` for `speech_url()` + `stt_url()` |
| Modify | `src/generation/providers/openai_image.rs` | Use `ResolvedUrl` for `generations_url()` + `edits_url()` |
| Modify | `src/generation/providers/openai_compat/builder.rs` | Replace `normalize_endpoint()` with shared `resolve_base_url()` |
| Modify | `src/generation/providers/mod.rs` | Add `pub mod url_normalize` |
| Modify | `src/bin/aleph-server/commands/start/builder/agent_init.rs` | Iterate 4 typed maps instead of single `providers` |
| Modify | `src/bin/aleph-server/commands/start/builder/subsystems.rs` | STT config from speech_providers |
| Modify | `src/gateway/handlers/generation_providers.rs` | RPC handlers use typed maps |
| Modify | `interfaces/webchat/src/api/generation_providers.rs` | API calls include generation_type |
| Modify | `interfaces/webchat/src/views/settings/generation_providers.rs` | Tab-based UI reads from typed maps |

---

### Task 1: Create shared URL normalization module

**Files:**
- Create: `src/generation/providers/url_normalize.rs`
- Modify: `src/generation/providers/mod.rs`

- [ ] **Step 1: Create `src/generation/providers/url_normalize.rs`**

```rust
//! Shared URL normalization for generation providers.
//!
//! Standard URLs (domain-only or domain+/v1) get auto-completed with
//! the appropriate endpoint path. Custom full URLs are used as-is.

use crate::generation::GenerationType;

/// Resolved URL — either a standard base that derives endpoints,
/// or a custom full URL used as-is.
#[derive(Debug, Clone)]
pub enum ResolvedUrl {
    /// Standard OpenAI-compatible base URL.
    /// All operation endpoints derived automatically.
    Standard(String),
    /// Custom full URL. Used as-is for primary operation only.
    Custom(String),
}

impl ResolvedUrl {
    /// Get the primary endpoint URL for the given generation type.
    pub fn primary_endpoint(&self, gen_type: GenerationType) -> String {
        match self {
            ResolvedUrl::Custom(url) => url.clone(),
            ResolvedUrl::Standard(base) => {
                let suffix = match gen_type {
                    GenerationType::Image => "/v1/images/generations",
                    GenerationType::Video => "/v1/videos/generations",
                    GenerationType::Speech => "/v1/audio/speech",
                    GenerationType::Audio => "/v1/audio/generations",
                };
                format!("{}{}", base, suffix)
            }
        }
    }

    /// Get the secondary endpoint URL (edit for image, STT for speech).
    /// Returns None for custom URLs or types without secondary endpoints.
    pub fn secondary_endpoint(&self, gen_type: GenerationType) -> Option<String> {
        match self {
            ResolvedUrl::Custom(_) => None,
            ResolvedUrl::Standard(base) => {
                let suffix = match gen_type {
                    GenerationType::Image => Some("/v1/images/edits"),
                    GenerationType::Speech => Some("/v1/audio/transcriptions"),
                    _ => None,
                };
                suffix.map(|s| format!("{}{}", base, s))
            }
        }
    }
}

/// Resolve a user-configured URL into a ResolvedUrl.
///
/// Rules:
/// - Domain-only (no path after scheme) → Standard (auto-complete)
/// - Domain + /v1 → Standard (auto-complete)
/// - Anything else → Custom (use as-is)
pub fn resolve_base_url(url: &str) -> ResolvedUrl {
    let trimmed = url.trim_end_matches('/');
    if needs_auto_complete(trimmed) {
        let base = trimmed.trim_end_matches("/v1").trim_end_matches('/');
        ResolvedUrl::Standard(base.to_string())
    } else {
        ResolvedUrl::Custom(trimmed.to_string())
    }
}

/// Check if a URL is a standard base that needs endpoint path auto-completion.
///
/// Standard: domain-only (no `/` in path) or domain + `/v1`.
/// Everything else is treated as a custom full URL.
fn needs_auto_complete(url: &str) -> bool {
    let after_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    // No slash at all = pure domain, or ends with /v1 = standard base
    !after_scheme.contains('/') || after_scheme.ends_with("/v1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_only_is_standard() {
        let r = resolve_base_url("https://api.example.com");
        assert!(matches!(r, ResolvedUrl::Standard(ref b) if b == "https://api.example.com"));
    }

    #[test]
    fn test_domain_with_v1_is_standard() {
        let r = resolve_base_url("https://api.example.com/v1");
        assert!(matches!(r, ResolvedUrl::Standard(ref b) if b == "https://api.example.com"));
    }

    #[test]
    fn test_domain_with_v1_trailing_slash() {
        let r = resolve_base_url("https://api.example.com/v1/");
        assert!(matches!(r, ResolvedUrl::Standard(ref b) if b == "https://api.example.com"));
    }

    #[test]
    fn test_full_path_is_custom() {
        let r = resolve_base_url("https://api.example.com/v2/videos/generations");
        assert!(matches!(r, ResolvedUrl::Custom(ref u) if u == "https://api.example.com/v2/videos/generations"));
    }

    #[test]
    fn test_custom_path_is_custom() {
        let r = resolve_base_url("https://api.example.com/custom/tts");
        assert!(matches!(r, ResolvedUrl::Custom(ref u) if u == "https://api.example.com/custom/tts"));
    }

    #[test]
    fn test_primary_endpoint_image() {
        let r = ResolvedUrl::Standard("https://api.example.com".into());
        assert_eq!(r.primary_endpoint(GenerationType::Image), "https://api.example.com/v1/images/generations");
    }

    #[test]
    fn test_primary_endpoint_speech() {
        let r = ResolvedUrl::Standard("https://api.example.com".into());
        assert_eq!(r.primary_endpoint(GenerationType::Speech), "https://api.example.com/v1/audio/speech");
    }

    #[test]
    fn test_primary_endpoint_video() {
        let r = ResolvedUrl::Standard("https://api.example.com".into());
        assert_eq!(r.primary_endpoint(GenerationType::Video), "https://api.example.com/v1/videos/generations");
    }

    #[test]
    fn test_primary_endpoint_audio() {
        let r = ResolvedUrl::Standard("https://api.example.com".into());
        assert_eq!(r.primary_endpoint(GenerationType::Audio), "https://api.example.com/v1/audio/generations");
    }

    #[test]
    fn test_secondary_endpoint_image_edit() {
        let r = ResolvedUrl::Standard("https://api.example.com".into());
        assert_eq!(r.secondary_endpoint(GenerationType::Image), Some("https://api.example.com/v1/images/edits".into()));
    }

    #[test]
    fn test_secondary_endpoint_speech_stt() {
        let r = ResolvedUrl::Standard("https://api.example.com".into());
        assert_eq!(r.secondary_endpoint(GenerationType::Speech), Some("https://api.example.com/v1/audio/transcriptions".into()));
    }

    #[test]
    fn test_secondary_endpoint_video_none() {
        let r = ResolvedUrl::Standard("https://api.example.com".into());
        assert_eq!(r.secondary_endpoint(GenerationType::Video), None);
    }

    #[test]
    fn test_custom_url_primary() {
        let r = ResolvedUrl::Custom("https://custom.api.com/my/endpoint".into());
        assert_eq!(r.primary_endpoint(GenerationType::Speech), "https://custom.api.com/my/endpoint");
    }

    #[test]
    fn test_custom_url_no_secondary() {
        let r = ResolvedUrl::Custom("https://custom.api.com/my/endpoint".into());
        assert_eq!(r.secondary_endpoint(GenerationType::Image), None);
    }
}
```

- [ ] **Step 2: Add `pub mod url_normalize;` to `src/generation/providers/mod.rs`**

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib generation::providers::url_normalize`
Expected: All 14 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/generation/providers/url_normalize.rs src/generation/providers/mod.rs
git commit -m "generation: add shared URL normalization with ResolvedUrl"
```

---

### Task 2: Add 4 typed provider maps to GenerationConfig

**Files:**
- Modify: `src/config/types/generation/config.rs`
- Modify: `src/config/types/generation/provider.rs`

- [ ] **Step 1: Read both files completely before modifying**

Read `src/config/types/generation/config.rs` and `src/config/types/generation/provider.rs`.

- [ ] **Step 2: Add 4 typed maps to `GenerationConfig`**

In `config.rs`, add 4 new fields to the `GenerationConfig` struct (after the existing `providers` field):

```rust
    /// Image generation providers (new typed format)
    #[serde(default)]
    pub image_providers: HashMap<String, GenerationProviderConfig>,

    /// Video generation providers (new typed format)
    #[serde(default)]
    pub video_providers: HashMap<String, GenerationProviderConfig>,

    /// Speech generation providers (new typed format — TTS/STT)
    #[serde(default)]
    pub speech_providers: HashMap<String, GenerationProviderConfig>,

    /// Audio/music generation providers (new typed format)
    #[serde(default)]
    pub audio_providers: HashMap<String, GenerationProviderConfig>,
```

- [ ] **Step 3: Add `merged_providers()` method to `GenerationConfig`**

This method merges old `providers` (by `capabilities[0]`) + new typed maps into a unified `Vec<(String, GenerationProviderConfig, GenerationType)>`. New format takes priority.

```rust
    /// Merge all provider sources into a unified list with resolved type.
    /// New typed maps take priority over legacy `providers` section.
    pub fn merged_providers(&self) -> Vec<(String, GenerationProviderConfig, GenerationType)> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // New format first (priority)
        for (name, cfg) in &self.image_providers {
            seen.insert(name.clone());
            let mut cfg = cfg.clone();
            cfg.capabilities = vec![GenerationType::Image];
            result.push((name.clone(), cfg, GenerationType::Image));
        }
        for (name, cfg) in &self.video_providers {
            seen.insert(name.clone());
            let mut cfg = cfg.clone();
            cfg.capabilities = vec![GenerationType::Video];
            result.push((name.clone(), cfg, GenerationType::Video));
        }
        for (name, cfg) in &self.speech_providers {
            seen.insert(name.clone());
            let mut cfg = cfg.clone();
            cfg.capabilities = vec![GenerationType::Speech];
            result.push((name.clone(), cfg, GenerationType::Speech));
        }
        for (name, cfg) in &self.audio_providers {
            seen.insert(name.clone());
            let mut cfg = cfg.clone();
            cfg.capabilities = vec![GenerationType::Audio];
            result.push((name.clone(), cfg, GenerationType::Audio));
        }

        // Legacy format: map by capabilities[0], skip if already seen
        for (name, cfg) in &self.providers {
            if seen.contains(name) { continue; }
            if let Some(gen_type) = cfg.capabilities.first().copied() {
                result.push((name.clone(), cfg.clone(), gen_type));
            }
        }

        result
    }
```

- [ ] **Step 4: Update `get_providers_for_type()` to use `merged_providers()`**

Replace the existing implementation to iterate `merged_providers()` filtered by type.

- [ ] **Step 5: Make `capabilities` default to empty in `GenerationProviderConfig`**

In `provider.rs`, ensure `capabilities` has `#[serde(default)]` (should already have it). When using new typed maps, `capabilities` is set by `merged_providers()`, not by the user.

- [ ] **Step 6: Run compile check**

Run: `cargo check -p alephcore`
Expected: Success (existing code still uses `providers` which still exists).

- [ ] **Step 7: Commit**

```bash
git add src/config/types/generation/
git commit -m "config: add 4 typed provider maps to GenerationConfig"
```

---

### Task 3: Adapt provider factory to accept GenerationType

**Files:**
- Modify: `src/generation/providers/factory.rs`
- Modify: `src/generation/providers/openai_tts.rs`
- Modify: `src/generation/providers/openai_image.rs`
- Modify: `src/generation/providers/openai_compat/builder.rs`

- [ ] **Step 1: Read all 4 files**

- [ ] **Step 2: Update `create_provider` signature in `factory.rs`**

Add `gen_type: GenerationType` parameter:

```rust
pub fn create_provider(
    name: &str,
    config: &GenerationProviderConfig,
    gen_type: GenerationType,
) -> GenerationResult<Arc<dyn GenerationProvider>>
```

Inside the function, call `resolve_base_url()` on the base_url and pass to providers:

```rust
use super::url_normalize::resolve_base_url;

// At the top of create_provider():
let resolved_url = config.base_url.as_deref()
    .map(|url| resolve_base_url(url));
```

Pass `gen_type` and `resolved_url` to each provider constructor as needed.

- [ ] **Step 3: Update `openai_tts.rs` to use `ResolvedUrl`**

Replace the manual endpoint normalization in `new()` (lines 193-198) with:

```rust
use super::url_normalize::{ResolvedUrl, resolve_base_url};

// In new():
let resolved = resolve_base_url(&base_url);
let endpoint = resolved.primary_endpoint(GenerationType::Speech);
```

Update `speech_url()` to return `self.endpoint.clone()` (already normalized).

Add `stt_url()` method:
```rust
fn stt_url(&self) -> Option<String> {
    self.resolved.secondary_endpoint(GenerationType::Speech)
}
```

Store `resolved: ResolvedUrl` in the struct for `stt_url()` derivation.

- [ ] **Step 4: Update `openai_image.rs` to use `ResolvedUrl`**

Same pattern — replace manual normalization with `resolve_base_url()`. Store `ResolvedUrl` for `edits_url()` derivation.

- [ ] **Step 5: Update `openai_compat/builder.rs`**

Delete the private `normalize_endpoint()` method entirely. Replace with:

```rust
use super::super::url_normalize::resolve_base_url;

// In build():
let resolved = resolve_base_url(&self.base_url);
let endpoint = resolved.primary_endpoint(
    self.supported_types.first().copied().unwrap_or(GenerationType::Image)
);
```

- [ ] **Step 6: Update all `create_provider()` call sites**

Search for `create_provider(name, ` and add the `gen_type` parameter. Key call sites:
- `src/bin/aleph-server/commands/start/builder/agent_init.rs` — startup registration
- `src/gateway/handlers/generation_providers.rs` — test connection handler

For the startup registration (agent_init.rs), use the `gen_type` from `merged_providers()` (Task 4).
For test connection handler, derive `gen_type` from the provider's first capability.

- [ ] **Step 7: Compile and test**

Run: `cargo check -p alephcore`
Then: `cargo test -p alephcore --lib generation`

- [ ] **Step 8: Commit**

```bash
git add src/generation/providers/ src/bin/aleph-server/ src/gateway/handlers/
git commit -m "generation: use ResolvedUrl in all provider constructors, delete old normalize_endpoint"
```

---

### Task 4: Adapt startup registration to use merged_providers()

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init.rs`
- Modify: `src/bin/aleph-server/commands/start/builder/subsystems.rs`

- [ ] **Step 1: Read both files**

- [ ] **Step 2: Update startup registration loop in `agent_init.rs`**

Replace the current loop (lines ~191-212):

```rust
// Old: for (name, provider_cfg) in &app_config.generation.providers { ... }
```

With:

```rust
// New: iterate merged providers with resolved types
for (name, provider_cfg, gen_type) in app_config.generation.merged_providers() {
    if !provider_cfg.enabled { continue; }
    // ... resolve api_key from vault ...
    match gen_providers::create_provider(&name, &provider_cfg, gen_type) {
        Ok(provider) => {
            registry.register(name.clone(), provider).ok();
            tracing::info!(provider = %name, gen_type = ?gen_type, "Registered generation provider");
        }
        Err(e) => {
            tracing::warn!(provider = %name, error = %e, "Skip generation provider");
        }
    }
}
```

- [ ] **Step 3: Update hot-reload handler in `agent_init.rs`**

The hot-reload (lines ~214-277) also iterates `config.generation.providers`. Change to use `config.generation.merged_providers()`.

- [ ] **Step 4: Update STT config in `subsystems.rs`**

The STT config (lines ~388-414) searches `config.generation.providers` for speech providers. Change to:

```rust
// Search speech_providers first, fall back to legacy providers
let speech_providers: Vec<_> = config.generation.merged_providers()
    .into_iter()
    .filter(|(_, cfg, t)| *t == GenerationType::Speech && cfg.enabled)
    .collect();
```

Use `ResolvedUrl` for STT endpoint:

```rust
use alephcore::generation::providers::url_normalize::resolve_base_url;

let resolved = resolve_base_url(base_url);
let stt_endpoint = resolved.secondary_endpoint(GenerationType::Speech)
    .unwrap_or_else(|| format!("{}/v1/audio/transcriptions", base_url));
```

- [ ] **Step 5: Compile and test**

Run: `cargo check -p alephcore`
Run: `cargo check --bin aleph-server`

- [ ] **Step 6: Commit**

```bash
git add src/bin/aleph-server/
git commit -m "startup: use merged_providers() for registration and STT config"
```

---

### Task 5: Adapt RPC handlers for typed provider maps

**Files:**
- Modify: `src/gateway/handlers/generation_providers.rs`

- [ ] **Step 1: Read the full file**

- [ ] **Step 2: Update `handle_list()`**

Change to iterate `config.generation.merged_providers()` instead of `config.generation.providers`. Include `generation_type` in the response JSON for each provider.

- [ ] **Step 3: Update `handle_create()`**

When creating a provider, the RPC params should include `generation_type` (string: "image"/"video"/"speech"/"audio"). Store into the correct typed map:

```rust
let gen_type_str = params.get("generation_type").and_then(|v| v.as_str()).unwrap_or("image");
// Insert into the correct typed map based on gen_type_str
match gen_type_str {
    "image" => config.generation.image_providers.insert(name, provider_cfg),
    "video" => config.generation.video_providers.insert(name, provider_cfg),
    "speech" => config.generation.speech_providers.insert(name, provider_cfg),
    "audio" => config.generation.audio_providers.insert(name, provider_cfg),
    _ => config.generation.image_providers.insert(name, provider_cfg),
};
```

- [ ] **Step 4: Update `handle_update()` and `handle_delete()`**

Same pattern — operate on the correct typed map. For update/delete, search across all 4 maps to find the provider by name.

- [ ] **Step 5: Update `handle_set_default()`**

Already works with `generation_type` — just ensure it reads from typed maps.

- [ ] **Step 6: Migrate existing config on first access**

Add a helper that moves entries from old `providers` map to typed maps if they have `capabilities` set. This is a one-time migration that happens when config is saved.

- [ ] **Step 7: Compile and test**

Run: `cargo check -p alephcore`

- [ ] **Step 8: Commit**

```bash
git add src/gateway/handlers/generation_providers.rs
git commit -m "handlers: adapt generation provider RPC to typed maps"
```

---

### Task 6: Adapt Panel webchat UI

**Files:**
- Modify: `interfaces/webchat/src/api/generation_providers.rs`
- Modify: `interfaces/webchat/src/views/settings/generation_providers.rs`

- [ ] **Step 1: Read both files**

- [ ] **Step 2: Update API calls to include `generation_type`**

In `generation_providers.rs`, the `create()` and `update()` calls need to pass `generation_type` in params. The `list()` response now includes `generation_type` per provider.

- [ ] **Step 3: Update provider list view**

The Panel already has category tabs (Image/Video/Audio/Speech). Ensure:
- Each tab filters providers by `generation_type` from the list response
- Create new provider within a tab auto-sets the correct `generation_type`
- Provider cards show which category they belong to

- [ ] **Step 4: Remove `capabilities` field from provider edit form**

Since type is determined by which tab the user is in, the `capabilities` field in the edit form is unnecessary. Remove it to reduce confusion.

- [ ] **Step 5: Build WASM**

Run: `just wasm` or the webchat build command
Expected: Successful WASM build.

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/
git commit -m "panel: adapt generation provider UI to typed categories"
```

---

### Task 7: Clean up dead code and migrate user config

**Files:**
- Modify: `src/config/types/generation/config.rs`
- Modify: `~/.aleph/config.toml` (user config)

- [ ] **Step 1: Remove dead helper functions**

In `config.rs`, the old `get_providers_for_type()` that iterates `self.providers` directly should be replaced with the new `merged_providers()` based version. Remove any code that is no longer called.

- [ ] **Step 2: Remove `extract_base_url()` from `main.rs` and webchat**

The `extract_base_url()` function in `src/bin/aleph-server/main.rs` and `interfaces/webchat/src/views/settings/generation_providers.rs` is superseded by `resolve_base_url()`. Delete it if no longer used, or keep if Panel still needs it for display purposes.

- [ ] **Step 3: Migrate user config**

Update `~/.aleph/config.toml` to use new format:

```toml
# Move [generation.providers.T8Star] → [generation.speech_providers.T8Star]
# Move [generation.providers.T8StarVideo] → [generation.video_providers.T8StarVideo]
# Move [generation.providers.T8StariMage] → [generation.image_providers.T8StariMage]
# Delete old [generation.providers.*] sections
```

- [ ] **Step 4: Full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass.

- [ ] **Step 5: Clippy**

Run: `cargo clippy -p alephcore -- -W warnings 2>&1 | grep -E "generation|url_normalize|provider" | head -20`
Expected: No new warnings from our changes.

- [ ] **Step 6: Commit**

```bash
git add core/ interfaces/
git commit -m "generation: clean up dead code, migrate config to typed format"
```

---

### Task 8: Integration verification

- [ ] **Step 1: Build and restart**

```bash
just build
pkill -f "target/release/aleph-server"; sleep 3
target/release/aleph-server start
```

- [ ] **Step 2: Verify slash commands**

Test via Telegram:
- `/speech 测试语音` → should work (speech provider registered)
- `/image test` → should work or show proper "no provider" error
- `/video test` → should work or show proper "no provider" error
- `/audio test` → should work or show proper "no provider" error

- [ ] **Step 3: Verify Panel UI**

Open `http://127.0.0.1:18790/` → Settings → Generation Providers:
- 4 tabs visible (Image/Video/Audio/Speech)
- Each tab shows only providers of that type
- Can create/edit/delete within each tab
- Default provider selection works per type

- [ ] **Step 4: Verify hot-reload**

Edit a provider in Panel → config should save → provider re-registered without restart.

- [ ] **Step 5: Commit if any fixes needed**

```bash
git add -A
git commit -m "generation: fix integration issues"
```
