# Config Externalization Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Externalize hardcoded provider presets, system prompts, and default values into `~/.aleph/presets.toml`, `~/.aleph/prompts.toml`, and `~/.aleph/defaults.toml`.

**Architecture:** Three new TOML files loaded at startup with merge-over-defaults semantics. Built-in values in Rust remain as fallback. New `PresetsOverride`, `PromptsOverride`, and `DefaultsOverride` types loaded in `Config::load()` flow. A shared `load_optional_toml()` helper handles file-not-found gracefully.

**Tech Stack:** Rust, serde, toml, once_cell, tracing

**Design Doc:** `docs/plans/2026-03-02-config-externalization-design.md`

---

## Phase 1: presets.toml (Provider + Generation Presets)

### Task 1: Create PresetsOverride types

**Files:**
- Create: `core/src/config/presets_override.rs`
- Modify: `core/src/config/mod.rs:14-25`

**Step 1: Write the failing test**

In `core/src/config/presets_override.rs`, create the module with types and tests:

```rust
//! Presets override loading from ~/.aleph/presets.toml
//!
//! Allows users to add new provider/generation presets or override
//! built-in presets without recompilation.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use tracing::warn;

// =============================================================================
// Provider Presets Override
// =============================================================================

/// Partial provider preset — all fields optional for merge semantics
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PartialProviderPreset {
    pub base_url: Option<String>,
    pub protocol: Option<String>,
    pub color: Option<String>,
    pub default_model: Option<String>,
    pub aliases: Option<Vec<String>>,
    /// Set to false to disable a built-in preset
    pub enabled: Option<bool>,
}

// =============================================================================
// Generation Presets Override
// =============================================================================

/// Partial generation preset — all fields optional for merge semantics
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PartialGenerationPreset {
    pub provider_type: Option<String>,
    pub default_model: Option<String>,
    pub base_url: Option<String>,
    /// Set to false to disable a built-in preset
    pub enabled: Option<bool>,
}

/// Generation presets grouped by type
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GenerationPresetsOverride {
    #[serde(default)]
    pub image: HashMap<String, PartialGenerationPreset>,
    #[serde(default)]
    pub video: HashMap<String, PartialGenerationPreset>,
    #[serde(default)]
    pub audio: HashMap<String, PartialGenerationPreset>,
}

// =============================================================================
// Top-level PresetsOverride
// =============================================================================

/// Root struct for ~/.aleph/presets.toml
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PresetsOverride {
    #[serde(default)]
    pub providers: HashMap<String, PartialProviderPreset>,
    #[serde(default)]
    pub generation: GenerationPresetsOverride,
}

// =============================================================================
// Loading
// =============================================================================

/// Load presets override from a TOML file.
/// Returns Default if file doesn't exist or fails to parse.
pub fn load_presets_override(path: &Path) -> PresetsOverride {
    if !path.exists() {
        return PresetsOverride::default();
    }

    match std::fs::read_to_string(path) {
        Ok(contents) => match toml::from_str(&contents) {
            Ok(overrides) => {
                tracing::info!(path = %path.display(), "Loaded presets override");
                overrides
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e, "Failed to parse presets.toml, using defaults");
                PresetsOverride::default()
            }
        },
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Failed to read presets.toml, using defaults");
            PresetsOverride::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_presets_override() {
        let overrides: PresetsOverride = toml::from_str("").unwrap();
        assert!(overrides.providers.is_empty());
        assert!(overrides.generation.image.is_empty());
    }

    #[test]
    fn test_provider_preset_partial_parse() {
        let toml_str = r#"
[providers.my-provider]
base_url = "https://api.example.com/v1"
protocol = "openai"
"#;
        let overrides: PresetsOverride = toml::from_str(toml_str).unwrap();
        let preset = overrides.providers.get("my-provider").unwrap();
        assert_eq!(preset.base_url.as_deref(), Some("https://api.example.com/v1"));
        assert_eq!(preset.protocol.as_deref(), Some("openai"));
        assert!(preset.color.is_none());
        assert!(preset.default_model.is_none());
    }

    #[test]
    fn test_generation_preset_parse() {
        let toml_str = r#"
[generation.image.my-dalle]
provider_type = "openai"
default_model = "dall-e-4"
base_url = "https://api.openai.com"

[generation.video.my-veo]
provider_type = "google_veo"
default_model = "veo-3"
"#;
        let overrides: PresetsOverride = toml::from_str(toml_str).unwrap();
        assert_eq!(overrides.generation.image.len(), 1);
        assert_eq!(overrides.generation.video.len(), 1);
        let img = overrides.generation.image.get("my-dalle").unwrap();
        assert_eq!(img.default_model.as_deref(), Some("dall-e-4"));
    }

    #[test]
    fn test_disable_builtin_preset() {
        let toml_str = r#"
[providers.openai]
enabled = false
"#;
        let overrides: PresetsOverride = toml::from_str(toml_str).unwrap();
        let preset = overrides.providers.get("openai").unwrap();
        assert_eq!(preset.enabled, Some(false));
    }

    #[test]
    fn test_load_nonexistent_file() {
        let result = load_presets_override(Path::new("/nonexistent/presets.toml"));
        assert!(result.providers.is_empty());
    }

    #[test]
    fn test_provider_with_aliases() {
        let toml_str = r#"
[providers.my-provider]
base_url = "https://api.example.com/v1"
aliases = ["alias1", "alias2"]
"#;
        let overrides: PresetsOverride = toml::from_str(toml_str).unwrap();
        let preset = overrides.providers.get("my-provider").unwrap();
        assert_eq!(preset.aliases.as_ref().unwrap().len(), 2);
    }
}
```

**Step 2: Register module in config/mod.rs**

Add to `core/src/config/mod.rs` after line 20 (`pub mod ui_hints;`):

```rust
pub mod presets_override;
```

**Step 3: Run test to verify it passes**

Run: `cargo test -p alephcore --lib presets_override`

Expected: All 6 tests PASS

**Step 4: Commit**

```bash
git add core/src/config/presets_override.rs core/src/config/mod.rs
git commit -m "config: add PresetsOverride types for ~/.aleph/presets.toml"
```

---

### Task 2: Integrate PresetsOverride into Config loading

**Files:**
- Modify: `core/src/config/structs.rs:16-104` (add field)
- Modify: `core/src/config/load.rs:38-148` (load presets)

**Step 1: Add presets_override field to Config struct**

In `core/src/config/structs.rs`, add after line 103 (before the closing `}` of Config):

```rust
    /// Presets override loaded from ~/.aleph/presets.toml
    /// Not serialized to config.toml — lives in its own file
    #[serde(skip)]
    pub presets_override: crate::config::presets_override::PresetsOverride,
```

In `Default for Config` (line 193, before closing `}`):

```rust
            presets_override: crate::config::presets_override::PresetsOverride::default(),
```

**Step 2: Load presets.toml in Config::load_from_file()**

In `core/src/config/load.rs`, add after line 91 (`config.merge_builtin_rules();`):

```rust
        // Load presets override from ~/.aleph/presets.toml
        if let Ok(config_dir) = crate::utils::paths::get_config_dir() {
            let presets_path = config_dir.join("presets.toml");
            config.presets_override =
                crate::config::presets_override::load_presets_override(&presets_path);
        }
```

Also in `Config::load()` (after line 176 `let config = Self::default();`), add the same loading for the default-config path:

```rust
            // Load presets override even when no config.toml exists
            if let Ok(config_dir) = crate::utils::paths::get_config_dir() {
                let presets_path = config_dir.join("presets.toml");
                config.presets_override =
                    crate::config::presets_override::load_presets_override(&presets_path);
            }
```

**Step 3: Run tests to verify compilation**

Run: `cargo test -p alephcore --lib config`

Expected: Existing tests still PASS, no compilation errors

**Step 4: Commit**

```bash
git add core/src/config/structs.rs core/src/config/load.rs
git commit -m "config: load presets.toml into Config on startup"
```

---

### Task 3: Add merge logic to provider presets

**Files:**
- Modify: `core/src/providers/presets.rs:296-298` (enhance get_preset)
- Modify: `core/src/config/presets_override.rs` (add merge helper)

**Step 1: Write merge test in presets_override.rs**

Add to the `tests` module in `core/src/config/presets_override.rs`:

```rust
    use crate::providers::presets::ProviderPreset;

    #[test]
    fn test_merge_provider_preset_override_field() {
        let builtin = ProviderPreset {
            base_url: "https://api.openai.com/v1",
            protocol: "openai",
            color: "#10a37f",
            default_model: "gpt-4o",
        };
        let partial = PartialProviderPreset {
            default_model: Some("gpt-4-turbo".to_string()),
            ..Default::default()
        };
        let merged = merge_provider_preset(&builtin, &partial);
        assert_eq!(merged.base_url, "https://api.openai.com/v1"); // unchanged
        assert_eq!(merged.default_model, "gpt-4-turbo");          // overridden
    }

    #[test]
    fn test_partial_to_full_provider_preset() {
        let partial = PartialProviderPreset {
            base_url: Some("https://api.new.com/v1".to_string()),
            protocol: Some("openai".to_string()),
            color: Some("#ff0000".to_string()),
            default_model: Some("new-model".to_string()),
            ..Default::default()
        };
        let full = partial_to_provider_preset(&partial);
        assert!(full.is_some());
        let full = full.unwrap();
        assert_eq!(full.base_url, "https://api.new.com/v1");
    }
```

**Step 2: Implement merge functions in presets_override.rs**

Add before `#[cfg(test)]`:

```rust
use crate::providers::presets::ProviderPreset;

/// Owned version of ProviderPreset for runtime-merged presets
#[derive(Debug, Clone)]
pub struct OwnedProviderPreset {
    pub base_url: String,
    pub protocol: String,
    pub color: String,
    pub default_model: String,
}

/// Merge a partial override onto a built-in preset, returning an owned copy
pub fn merge_provider_preset(builtin: &ProviderPreset, partial: &PartialProviderPreset) -> OwnedProviderPreset {
    OwnedProviderPreset {
        base_url: partial.base_url.clone().unwrap_or_else(|| builtin.base_url.to_string()),
        protocol: partial.protocol.clone().unwrap_or_else(|| builtin.protocol.to_string()),
        color: partial.color.clone().unwrap_or_else(|| builtin.color.to_string()),
        default_model: partial.default_model.clone().unwrap_or_else(|| builtin.default_model.to_string()),
    }
}

/// Convert a fully-specified partial preset to an owned preset.
/// Returns None if required fields (base_url, protocol) are missing.
pub fn partial_to_provider_preset(partial: &PartialProviderPreset) -> Option<OwnedProviderPreset> {
    Some(OwnedProviderPreset {
        base_url: partial.base_url.clone()?,
        protocol: partial.protocol.clone().unwrap_or_else(|| "openai".to_string()),
        color: partial.color.clone().unwrap_or_else(|| "#808080".to_string()),
        default_model: partial.default_model.clone().unwrap_or_default(),
    })
}
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib presets_override`

Expected: All tests PASS including new merge tests

**Step 4: Commit**

```bash
git add core/src/config/presets_override.rs
git commit -m "config: add provider preset merge logic"
```

---

### Task 4: Wire merged presets into gateway handlers

**Files:**
- Modify: `core/src/providers/presets.rs:296-298` (add override-aware lookup)
- Modify: `core/src/gateway/handlers/providers.rs` (use new lookup)

**Step 1: Add get_merged_preset function to providers/presets.rs**

Add after the existing `get_preset()` function (line 298):

```rust
/// Get a preset with override support.
/// Checks override first, then falls back to built-in.
/// Returns an OwnedProviderPreset (merged if override exists).
pub fn get_merged_preset(
    name: &str,
    overrides: &crate::config::presets_override::PresetsOverride,
) -> Option<crate::config::presets_override::OwnedProviderPreset> {
    let lower = name.to_lowercase();
    let builtin = PRESETS.get(lower.as_str());
    let partial = overrides.providers.get(&lower);

    // Check aliases in overrides
    let partial = partial.or_else(|| {
        overrides.providers.values().find(|p| {
            p.aliases.as_ref().map_or(false, |a| {
                a.iter().any(|alias| alias.to_lowercase() == lower)
            })
        })
    });

    match (builtin, partial) {
        // Override exists for built-in: merge
        (Some(b), Some(p)) => {
            if p.enabled == Some(false) {
                return None; // disabled
            }
            Some(crate::config::presets_override::merge_provider_preset(b, p))
        }
        // Only built-in exists
        (Some(b), None) => Some(crate::config::presets_override::OwnedProviderPreset {
            base_url: b.base_url.to_string(),
            protocol: b.protocol.to_string(),
            color: b.color.to_string(),
            default_model: b.default_model.to_string(),
        }),
        // Only override exists (new provider)
        (None, Some(p)) => {
            if p.enabled == Some(false) {
                return None;
            }
            crate::config::presets_override::partial_to_provider_preset(p)
        }
        // Neither exists
        (None, None) => None,
    }
}
```

**Step 2: Write test for get_merged_preset**

Add to the tests module in `core/src/providers/presets.rs`:

```rust
    #[test]
    fn test_get_merged_preset_builtin_only() {
        let overrides = crate::config::presets_override::PresetsOverride::default();
        let preset = get_merged_preset("openai", &overrides).unwrap();
        assert_eq!(preset.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn test_get_merged_preset_with_override() {
        let mut overrides = crate::config::presets_override::PresetsOverride::default();
        overrides.providers.insert("openai".to_string(), crate::config::presets_override::PartialProviderPreset {
            default_model: Some("gpt-4-turbo".to_string()),
            ..Default::default()
        });
        let preset = get_merged_preset("openai", &overrides).unwrap();
        assert_eq!(preset.default_model, "gpt-4-turbo");
        assert_eq!(preset.base_url, "https://api.openai.com/v1"); // unchanged
    }

    #[test]
    fn test_get_merged_preset_disabled() {
        let mut overrides = crate::config::presets_override::PresetsOverride::default();
        overrides.providers.insert("openai".to_string(), crate::config::presets_override::PartialProviderPreset {
            enabled: Some(false),
            ..Default::default()
        });
        assert!(get_merged_preset("openai", &overrides).is_none());
    }

    #[test]
    fn test_get_merged_preset_new_provider() {
        let mut overrides = crate::config::presets_override::PresetsOverride::default();
        overrides.providers.insert("my-custom".to_string(), crate::config::presets_override::PartialProviderPreset {
            base_url: Some("https://api.custom.com/v1".to_string()),
            protocol: Some("openai".to_string()),
            default_model: Some("custom-v1".to_string()),
            ..Default::default()
        });
        let preset = get_merged_preset("my-custom", &overrides).unwrap();
        assert_eq!(preset.base_url, "https://api.custom.com/v1");
    }
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib presets`

Expected: All tests PASS

**Step 4: Update providers handler to use merged presets**

In `core/src/gateway/handlers/providers.rs`, the `build_provider_config_for_persistence()` function calls `get_preset(provider_name)`. Update it to use `get_merged_preset()` when a `PresetsOverride` is available from the config context.

This involves passing the `PresetsOverride` from the handler's config context. The exact refactoring depends on how the handler accesses `Config` — look for `ctx.config()` or similar patterns in the handler functions.

The key change: wherever `get_preset(name)` is called in providers.rs, replace with:

```rust
// Before:
let preset = get_preset(provider_name);
let base_url = preset.map(|p| p.base_url.to_string());

// After:
let merged = get_merged_preset(provider_name, &config.presets_override);
let base_url = merged.as_ref().map(|p| p.base_url.clone());
```

**Step 5: Run tests**

Run: `cargo test -p alephcore --lib handlers::providers`

Expected: Existing handler tests still PASS

**Step 6: Commit**

```bash
git add core/src/providers/presets.rs core/src/gateway/handlers/providers.rs
git commit -m "config: wire merged provider presets into gateway handlers"
```

---

### Task 5: Add merge logic for generation presets

**Files:**
- Modify: `core/src/config/types/generation/presets.rs:113-121` (add override-aware lookup)
- Modify: `core/src/config/presets_override.rs` (add generation merge helpers)

**Step 1: Add generation merge functions to presets_override.rs**

```rust
use crate::config::types::generation::presets::GenerationPreset;

/// Owned version of GenerationPreset
#[derive(Debug, Clone)]
pub struct OwnedGenerationPreset {
    pub provider_type: String,
    pub default_model: String,
    pub base_url: Option<String>,
}

/// Merge a partial override onto a built-in generation preset
pub fn merge_generation_preset(
    builtin: &GenerationPreset,
    partial: &PartialGenerationPreset,
) -> OwnedGenerationPreset {
    OwnedGenerationPreset {
        provider_type: partial.provider_type.clone()
            .unwrap_or_else(|| builtin.provider_type.to_string()),
        default_model: partial.default_model.clone()
            .unwrap_or_else(|| builtin.default_model.to_string()),
        base_url: partial.base_url.clone()
            .or_else(|| builtin.base_url.map(|u| u.to_string())),
    }
}

/// Convert a fully-specified partial to owned generation preset
pub fn partial_to_generation_preset(partial: &PartialGenerationPreset) -> Option<OwnedGenerationPreset> {
    Some(OwnedGenerationPreset {
        provider_type: partial.provider_type.clone()?,
        default_model: partial.default_model.clone().unwrap_or_default(),
        base_url: partial.base_url.clone(),
    })
}
```

**Step 2: Add get_merged_preset to generation/presets.rs**

Add after existing `get_preset_by_type()`:

```rust
/// Get a generation preset with override support.
pub fn get_merged_generation_preset(
    name: &str,
    category: &str,
    overrides: &crate::config::presets_override::PresetsOverride,
) -> Option<crate::config::presets_override::OwnedGenerationPreset> {
    let builtin = PRESETS.get(name);
    let partial = match category {
        "image" => overrides.generation.image.get(name),
        "video" => overrides.generation.video.get(name),
        "audio" => overrides.generation.audio.get(name),
        _ => None,
    };

    match (builtin, partial) {
        (Some(b), Some(p)) => {
            if p.enabled == Some(false) { return None; }
            Some(crate::config::presets_override::merge_generation_preset(b, p))
        }
        (Some(b), None) => Some(crate::config::presets_override::OwnedGenerationPreset {
            provider_type: b.provider_type.to_string(),
            default_model: b.default_model.to_string(),
            base_url: b.base_url.map(|u| u.to_string()),
        }),
        (None, Some(p)) => {
            if p.enabled == Some(false) { return None; }
            crate::config::presets_override::partial_to_generation_preset(p)
        }
        (None, None) => None,
    }
}
```

**Step 3: Write tests**

Add tests in `core/src/config/types/generation/presets.rs`:

```rust
    #[test]
    fn test_get_merged_generation_preset_builtin() {
        let overrides = crate::config::presets_override::PresetsOverride::default();
        let preset = get_merged_generation_preset("openai-dalle", "image", &overrides).unwrap();
        assert_eq!(preset.default_model, "dall-e-3");
    }

    #[test]
    fn test_get_merged_generation_preset_new() {
        let mut overrides = crate::config::presets_override::PresetsOverride::default();
        overrides.generation.image.insert("my-img".to_string(),
            crate::config::presets_override::PartialGenerationPreset {
                provider_type: Some("custom".to_string()),
                default_model: Some("my-model".to_string()),
                ..Default::default()
            }
        );
        let preset = get_merged_generation_preset("my-img", "image", &overrides).unwrap();
        assert_eq!(preset.provider_type, "custom");
    }
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib generation::presets`

Expected: All tests PASS

**Step 5: Update generation_providers handler similarly to Task 4**

Update `core/src/gateway/handlers/generation_providers.rs` to use `get_merged_generation_preset()` where it currently calls `get_preset()`.

**Step 6: Commit**

```bash
git add core/src/config/presets_override.rs core/src/config/types/generation/presets.rs core/src/gateway/handlers/generation_providers.rs
git commit -m "config: wire merged generation presets into gateway handlers"
```

---

### Task 6: Verify Phase 1 end-to-end

**Step 1: Create a test presets.toml file**

```bash
mkdir -p /tmp/aleph-test
cat > /tmp/aleph-test/presets.toml << 'EOF'
[providers.my-test-provider]
base_url = "https://api.test.com/v1"
protocol = "openai"
color = "#ff0000"
default_model = "test-model-v1"

[providers.openai]
default_model = "gpt-4-turbo"

[generation.image.my-image]
provider_type = "openai"
default_model = "dall-e-4"
base_url = "https://api.openai.com"
EOF
```

**Step 2: Write integration test**

Add to `core/src/config/presets_override.rs` tests:

```rust
    #[test]
    fn test_load_valid_presets_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("presets.toml");
        std::fs::write(&path, r#"
[providers.my-test]
base_url = "https://api.test.com/v1"
protocol = "openai"

[generation.image.my-img]
provider_type = "openai"
default_model = "dall-e-4"
"#).unwrap();
        let overrides = load_presets_override(&path);
        assert_eq!(overrides.providers.len(), 1);
        assert_eq!(overrides.generation.image.len(), 1);
    }

    #[test]
    fn test_load_malformed_toml_returns_default() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("presets.toml");
        std::fs::write(&path, "this is not valid toml {{{{").unwrap();
        let overrides = load_presets_override(&path);
        assert!(overrides.providers.is_empty());
    }
```

**Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib`

Expected: All tests PASS (including pre-existing failures in markdown_skill which are known)

**Step 4: Commit**

```bash
git add core/src/config/presets_override.rs
git commit -m "config: add presets.toml integration tests"
```

---

## Phase 2: prompts.toml (System Prompts & Templates)

### Task 7: Create PromptsOverride types

**Files:**
- Create: `core/src/config/prompts_override.rs`
- Modify: `core/src/config/mod.rs` (add module)

**Step 1: Write module with types and tests**

Create `core/src/config/prompts_override.rs`:

```rust
//! Prompts override loading from ~/.aleph/prompts.toml
//!
//! Allows users to customize system prompts and templates without recompilation.

use serde::Deserialize;
use std::path::Path;
use tracing::warn;

// =============================================================================
// Prompt Sections
// =============================================================================

/// Planning system prompt override
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlannerPrompts {
    pub system_prompt: Option<String>,
}

/// Bootstrap ritual prompt overrides
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BootstrapPrompts {
    /// Complete bootstrap prompt (replaces entire BOOTSTRAP_PROMPT const)
    pub prompt: Option<String>,
}

/// Scratchpad template override
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ScratchpadPrompts {
    /// Markdown template for new scratchpads
    pub template: Option<String>,
}

/// Memory-related prompt overrides
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MemoryPrompts {
    pub compression_prompt: Option<String>,
    pub extraction_prompt: Option<String>,
}

/// Agent loop prompt overrides
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentPrompts {
    pub system_prefix: Option<String>,
    pub observation_prompt: Option<String>,
}

// =============================================================================
// Top-level PromptsOverride
// =============================================================================

/// Root struct for ~/.aleph/prompts.toml
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PromptsOverride {
    #[serde(default)]
    pub planner: Option<PlannerPrompts>,
    #[serde(default)]
    pub bootstrap: Option<BootstrapPrompts>,
    #[serde(default)]
    pub scratchpad: Option<ScratchpadPrompts>,
    #[serde(default)]
    pub memory: Option<MemoryPrompts>,
    #[serde(default)]
    pub agent: Option<AgentPrompts>,
}

// =============================================================================
// Loading
// =============================================================================

/// Load prompts override from a TOML file.
/// Returns Default if file doesn't exist or fails to parse.
pub fn load_prompts_override(path: &Path) -> PromptsOverride {
    if !path.exists() {
        return PromptsOverride::default();
    }

    match std::fs::read_to_string(path) {
        Ok(contents) => match toml::from_str(&contents) {
            Ok(overrides) => {
                tracing::info!(path = %path.display(), "Loaded prompts override");
                overrides
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e, "Failed to parse prompts.toml, using defaults");
                PromptsOverride::default()
            }
        },
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Failed to read prompts.toml, using defaults");
            PromptsOverride::default()
        }
    }
}

// =============================================================================
// Accessor helpers
// =============================================================================

impl PromptsOverride {
    /// Get the planner system prompt override, if set
    pub fn planner_system_prompt(&self) -> Option<&str> {
        self.planner.as_ref()?.system_prompt.as_deref()
    }

    /// Get the bootstrap prompt override, if set
    pub fn bootstrap_prompt(&self) -> Option<&str> {
        self.bootstrap.as_ref()?.prompt.as_deref()
    }

    /// Get the scratchpad template override, if set
    pub fn scratchpad_template(&self) -> Option<&str> {
        self.scratchpad.as_ref()?.template.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_prompts_override() {
        let overrides: PromptsOverride = toml::from_str("").unwrap();
        assert!(overrides.planner.is_none());
        assert!(overrides.bootstrap.is_none());
        assert!(overrides.scratchpad.is_none());
    }

    #[test]
    fn test_planner_prompt_parse() {
        let toml_str = r#"
[planner]
system_prompt = "You are a custom planner."
"#;
        let overrides: PromptsOverride = toml::from_str(toml_str).unwrap();
        assert_eq!(overrides.planner_system_prompt(), Some("You are a custom planner."));
    }

    #[test]
    fn test_multiline_prompt() {
        let toml_str = r#"
[planner]
system_prompt = """
Line 1
Line 2
Line 3
"""
"#;
        let overrides: PromptsOverride = toml::from_str(toml_str).unwrap();
        let prompt = overrides.planner_system_prompt().unwrap();
        assert!(prompt.contains("Line 1"));
        assert!(prompt.contains("Line 3"));
    }

    #[test]
    fn test_scratchpad_template_parse() {
        let toml_str = r#"
[scratchpad]
template = """
# My Custom Template
## Status
## Notes
"""
"#;
        let overrides: PromptsOverride = toml::from_str(toml_str).unwrap();
        assert!(overrides.scratchpad_template().unwrap().contains("My Custom Template"));
    }

    #[test]
    fn test_bootstrap_prompt_parse() {
        let toml_str = r#"
[bootstrap]
prompt = "Custom bootstrap ritual."
"#;
        let overrides: PromptsOverride = toml::from_str(toml_str).unwrap();
        assert_eq!(overrides.bootstrap_prompt(), Some("Custom bootstrap ritual."));
    }

    #[test]
    fn test_partial_override_only_some_sections() {
        let toml_str = r#"
[planner]
system_prompt = "Custom planner"
"#;
        let overrides: PromptsOverride = toml::from_str(toml_str).unwrap();
        assert!(overrides.planner_system_prompt().is_some());
        assert!(overrides.bootstrap_prompt().is_none());
        assert!(overrides.scratchpad_template().is_none());
    }

    #[test]
    fn test_load_nonexistent_prompts_file() {
        let result = load_prompts_override(Path::new("/nonexistent/prompts.toml"));
        assert!(result.planner.is_none());
    }
}
```

**Step 2: Register module in config/mod.rs**

Add after `pub mod presets_override;`:

```rust
pub mod prompts_override;
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib prompts_override`

Expected: All 7 tests PASS

**Step 4: Commit**

```bash
git add core/src/config/prompts_override.rs core/src/config/mod.rs
git commit -m "config: add PromptsOverride types for ~/.aleph/prompts.toml"
```

---

### Task 8: Integrate PromptsOverride into Config and consumers

**Files:**
- Modify: `core/src/config/structs.rs` (add field)
- Modify: `core/src/config/load.rs` (load prompts)
- Modify: `core/src/dispatcher/planner/prompt.rs` (use override)
- Modify: `core/src/agent_loop/bootstrap.rs` (use override)
- Modify: `core/src/memory/scratchpad/template.rs` (use override)

**Step 1: Add prompts_override field to Config**

In `core/src/config/structs.rs`, add after the `presets_override` field:

```rust
    /// Prompts override loaded from ~/.aleph/prompts.toml
    #[serde(skip)]
    pub prompts_override: crate::config::prompts_override::PromptsOverride,
```

In `Default for Config`, add:

```rust
            prompts_override: crate::config::prompts_override::PromptsOverride::default(),
```

**Step 2: Load prompts.toml in Config::load_from_file()**

In `core/src/config/load.rs`, add after the presets loading block:

```rust
        // Load prompts override from ~/.aleph/prompts.toml
        if let Ok(config_dir) = crate::utils::paths::get_config_dir() {
            let prompts_path = config_dir.join("prompts.toml");
            config.prompts_override =
                crate::config::prompts_override::load_prompts_override(&prompts_path);
        }
```

(Same pattern in `Config::load()` default branch.)

**Step 3: Update planner/prompt.rs to support override**

In `core/src/dispatcher/planner/prompt.rs`, add a new function:

```rust
/// Get the planning system prompt, checking override first
pub fn get_planning_system_prompt(
    overrides: &crate::config::prompts_override::PromptsOverride,
) -> &str {
    overrides.planner_system_prompt().unwrap_or(PLANNING_SYSTEM_PROMPT)
}
```

The caller of `PLANNING_SYSTEM_PROMPT` should switch to using `get_planning_system_prompt(&config.prompts_override)`.

**Step 4: Update bootstrap.rs to support override**

In `core/src/agent_loop/bootstrap.rs`, modify `bootstrap_prompt()`:

```rust
    /// Generate the bootstrap prompt, checking override first
    pub fn bootstrap_prompt_with_override(
        &self,
        overrides: &crate::config::prompts_override::PromptsOverride,
    ) -> Option<String> {
        match self.detect_phase() {
            BootstrapPhase::Uninitialized => {
                let prompt = overrides.bootstrap_prompt()
                    .unwrap_or(BOOTSTRAP_PROMPT);
                Some(prompt.to_string())
            }
            BootstrapPhase::Complete => None,
        }
    }
```

**Step 5: Update scratchpad/template.rs to support override**

In `core/src/memory/scratchpad/template.rs`, add:

```rust
/// Get the scratchpad template, checking override first
pub fn get_template(
    overrides: &crate::config::prompts_override::PromptsOverride,
) -> &str {
    overrides.scratchpad_template().unwrap_or(DEFAULT_TEMPLATE)
}
```

**Step 6: Run tests**

Run: `cargo test -p alephcore --lib`

Expected: All tests PASS

**Step 7: Commit**

```bash
git add core/src/config/structs.rs core/src/config/load.rs \
    core/src/dispatcher/planner/prompt.rs core/src/agent_loop/bootstrap.rs \
    core/src/memory/scratchpad/template.rs
git commit -m "config: integrate prompts.toml into planner, bootstrap, and scratchpad"
```

---

## Phase 3: defaults.toml (Default Value Overrides)

### Task 9: Create DefaultsOverride types

**Files:**
- Create: `core/src/config/defaults_override.rs`
- Modify: `core/src/config/mod.rs` (add module)

**Step 1: Write module with types and tests**

Create `core/src/config/defaults_override.rs`:

```rust
//! Defaults override loading from ~/.aleph/defaults.toml
//!
//! Allows users to override compiled-in default values for all subsystems.
//! Priority chain: compiled defaults → defaults.toml → config.toml

use serde::Deserialize;
use std::path::Path;
use std::sync::OnceLock;
use tracing::warn;

// =============================================================================
// Default Override Sections
// =============================================================================

/// Memory system defaults
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MemoryDefaultsOverride {
    pub similarity_threshold: Option<f32>,
    pub retention_days: Option<u32>,
    pub max_context_items: Option<u32>,
    pub compression_threshold: Option<f32>,
}

/// Provider defaults
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProviderDefaultsOverride {
    pub timeout_seconds: Option<u64>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
}

/// Generation defaults
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GenerationDefaultsOverride {
    pub timeout_seconds: Option<u64>,
}

// =============================================================================
// Top-level DefaultsOverride
// =============================================================================

/// Root struct for ~/.aleph/defaults.toml
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DefaultsOverride {
    #[serde(default)]
    pub memory: Option<MemoryDefaultsOverride>,
    #[serde(default)]
    pub provider: Option<ProviderDefaultsOverride>,
    #[serde(default)]
    pub generation: Option<GenerationDefaultsOverride>,
}

// =============================================================================
// Global singleton
// =============================================================================

/// Global defaults override, set once during Config::load()
static DEFAULTS_OVERRIDE: OnceLock<DefaultsOverride> = OnceLock::new();

/// Initialize the global defaults override. Called once during startup.
pub fn init_defaults_override(overrides: DefaultsOverride) {
    let _ = DEFAULTS_OVERRIDE.set(overrides);
}

/// Get a reference to the global defaults override.
pub fn get_defaults_override() -> &'static DefaultsOverride {
    DEFAULTS_OVERRIDE.get_or_init(DefaultsOverride::default)
}

// =============================================================================
// Loading
// =============================================================================

/// Load defaults override from a TOML file.
/// Returns Default if file doesn't exist or fails to parse.
pub fn load_defaults_override(path: &Path) -> DefaultsOverride {
    if !path.exists() {
        return DefaultsOverride::default();
    }

    match std::fs::read_to_string(path) {
        Ok(contents) => match toml::from_str(&contents) {
            Ok(overrides) => {
                tracing::info!(path = %path.display(), "Loaded defaults override");
                overrides
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e, "Failed to parse defaults.toml, using compiled defaults");
                DefaultsOverride::default()
            }
        },
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Failed to read defaults.toml, using compiled defaults");
            DefaultsOverride::default()
        }
    }
}

// =============================================================================
// Accessor helpers (used by fn default_*() functions)
// =============================================================================

impl DefaultsOverride {
    pub fn provider_timeout_seconds(&self) -> Option<u64> {
        self.provider.as_ref()?.timeout_seconds
    }

    pub fn memory_similarity_threshold(&self) -> Option<f32> {
        self.memory.as_ref()?.similarity_threshold
    }

    pub fn memory_retention_days(&self) -> Option<u32> {
        self.memory.as_ref()?.retention_days
    }

    pub fn generation_timeout_seconds(&self) -> Option<u64> {
        self.generation.as_ref()?.timeout_seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_defaults_override() {
        let overrides: DefaultsOverride = toml::from_str("").unwrap();
        assert!(overrides.memory.is_none());
        assert!(overrides.provider.is_none());
    }

    #[test]
    fn test_memory_defaults_parse() {
        let toml_str = r#"
[memory]
similarity_threshold = 0.8
retention_days = 60
"#;
        let overrides: DefaultsOverride = toml::from_str(toml_str).unwrap();
        assert_eq!(overrides.memory_similarity_threshold(), Some(0.8));
        assert_eq!(overrides.memory_retention_days(), Some(60));
    }

    #[test]
    fn test_provider_defaults_parse() {
        let toml_str = r#"
[provider]
timeout_seconds = 120
temperature = 0.5
"#;
        let overrides: DefaultsOverride = toml::from_str(toml_str).unwrap();
        assert_eq!(overrides.provider_timeout_seconds(), Some(120));
        let prov = overrides.provider.unwrap();
        assert_eq!(prov.temperature, Some(0.5));
    }

    #[test]
    fn test_partial_override() {
        let toml_str = r#"
[memory]
similarity_threshold = 0.6
"#;
        let overrides: DefaultsOverride = toml::from_str(toml_str).unwrap();
        assert_eq!(overrides.memory_similarity_threshold(), Some(0.6));
        assert!(overrides.memory_retention_days().is_none()); // not overridden
    }

    #[test]
    fn test_load_nonexistent_defaults_file() {
        let result = load_defaults_override(Path::new("/nonexistent/defaults.toml"));
        assert!(result.memory.is_none());
    }
}
```

**Step 2: Register module in config/mod.rs**

Add: `pub mod defaults_override;`

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib defaults_override`

Expected: All 5 tests PASS

**Step 4: Commit**

```bash
git add core/src/config/defaults_override.rs core/src/config/mod.rs
git commit -m "config: add DefaultsOverride types for ~/.aleph/defaults.toml"
```

---

### Task 10: Integrate DefaultsOverride into Config loading and fn default_*()

**Files:**
- Modify: `core/src/config/structs.rs` (add field)
- Modify: `core/src/config/load.rs` (load defaults FIRST, before config.toml)
- Modify: `core/src/config/types/provider.rs` (update default_timeout_seconds)
- Modify: `core/src/config/types/memory.rs` (update default thresholds)

**Step 1: Add defaults_override field to Config**

In `core/src/config/structs.rs`, add:

```rust
    /// Defaults override loaded from ~/.aleph/defaults.toml
    #[serde(skip)]
    pub defaults_override: crate::config::defaults_override::DefaultsOverride,
```

**Step 2: Load defaults.toml BEFORE config.toml**

This is critical: defaults.toml must be loaded and initialized before config.toml parsing, because `fn default_*()` functions are called during serde deserialization.

In `core/src/config/load.rs`, modify `load_from_file()` to load defaults.toml before TOML parsing (before line 72):

```rust
        // Load defaults override BEFORE parsing config.toml
        // because serde calls fn default_*() during deserialization
        if let Ok(config_dir) = crate::utils::paths::get_config_dir() {
            let defaults_path = config_dir.join("defaults.toml");
            let defaults = crate::config::defaults_override::load_defaults_override(&defaults_path);
            crate::config::defaults_override::init_defaults_override(defaults);
        }
```

**Step 3: Update example default function**

In `core/src/config/types/provider.rs`, update `default_timeout_seconds`:

```rust
pub fn default_timeout_seconds() -> u64 {
    crate::config::defaults_override::get_defaults_override()
        .provider_timeout_seconds()
        .unwrap_or(300)
}
```

This pattern can be applied incrementally to other `fn default_*()` functions as needed. Start with the most commonly tweaked ones:
- `default_timeout_seconds` in provider.rs
- `similarity_threshold` default in memory.rs
- `retention_days` default in memory.rs

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib`

Expected: All tests PASS

**Step 5: Commit**

```bash
git add core/src/config/structs.rs core/src/config/load.rs \
    core/src/config/types/provider.rs core/src/config/defaults_override.rs
git commit -m "config: integrate defaults.toml into startup and default functions"
```

---

### Task 11: Final integration test and cleanup

**Step 1: Write a comprehensive integration test**

Create a test that exercises all three files together. Add to `core/src/config/tests.rs` (or create if missing):

```rust
#[test]
fn test_all_override_files_loaded() {
    // Verify that Config::default() has empty overrides
    let config = Config::default();
    assert!(config.presets_override.providers.is_empty());
    assert!(config.prompts_override.planner.is_none());
    assert!(config.defaults_override.memory.is_none());
}
```

**Step 2: Run full test suite**

Run: `cargo test -p alephcore --lib`

Expected: All tests PASS

**Step 3: Run cargo check to verify no warnings**

Run: `cargo check -p alephcore 2>&1 | head -20`

Expected: No errors. Warnings are acceptable for unused fields (they'll be wired in by consumers later).

**Step 4: Final commit**

```bash
git add -A
git commit -m "config: complete ~/.aleph config externalization (presets, prompts, defaults)"
```

---

## Summary of All Commits

| # | Commit Message | Phase |
|---|---------------|-------|
| 1 | `config: add PresetsOverride types for ~/.aleph/presets.toml` | P1 |
| 2 | `config: load presets.toml into Config on startup` | P1 |
| 3 | `config: add provider preset merge logic` | P1 |
| 4 | `config: wire merged provider presets into gateway handlers` | P1 |
| 5 | `config: wire merged generation presets into gateway handlers` | P1 |
| 6 | `config: add presets.toml integration tests` | P1 |
| 7 | `config: add PromptsOverride types for ~/.aleph/prompts.toml` | P2 |
| 8 | `config: integrate prompts.toml into planner, bootstrap, and scratchpad` | P2 |
| 9 | `config: add DefaultsOverride types for ~/.aleph/defaults.toml` | P3 |
| 10 | `config: integrate defaults.toml into startup and default functions` | P3 |
| 11 | `config: complete ~/.aleph config externalization (presets, prompts, defaults)` | Final |

## Key Design Notes for Implementer

1. **`#[serde(skip)]`** on override fields — they are NOT part of config.toml serialization
2. **Defaults.toml loads BEFORE config.toml parsing** — because serde calls `fn default_*()` during deserialization
3. **OnceLock for DefaultsOverride** — global singleton allows `fn default_*()` to access overrides without passing context
4. **All load functions are graceful** — file not found or parse error returns Default, never blocks startup
5. **Merge is field-level** — only non-None fields in overrides replace built-in values
6. **Aliases in presets** — lookup checks both direct key and aliases array
7. **`enabled = false`** — the only way to disable a built-in preset at runtime
