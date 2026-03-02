//! Presets override types for ~/.aleph/presets.toml
//!
//! These types represent user overrides for built-in provider and generation presets.
//! All fields are Option<T> so users only need to specify the fields they want to change.
//! Missing fields are left as None and the built-in defaults are used instead.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use tracing::warn;

// =============================================================================
// Provider preset overrides
// =============================================================================

/// Partial override for a provider preset.
///
/// All fields are optional — users only specify what they want to change.
/// During merge, None fields fall back to the built-in preset value.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PartialProviderPreset {
    /// Override base URL
    #[serde(default)]
    pub base_url: Option<String>,
    /// Override protocol (e.g., "openai", "anthropic")
    #[serde(default)]
    pub protocol: Option<String>,
    /// Override UI color (hex string)
    #[serde(default)]
    pub color: Option<String>,
    /// Override default model
    #[serde(default)]
    pub default_model: Option<String>,
    /// Additional name aliases for this provider
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Whether this preset is enabled (set false to hide a built-in provider)
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

// =============================================================================
// Generation preset overrides
// =============================================================================

/// Partial override for a generation preset.
///
/// All fields are optional — users only specify what they want to change.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PartialGenerationPreset {
    /// Override provider type
    #[serde(default)]
    pub provider_type: Option<String>,
    /// Override default model
    #[serde(default)]
    pub default_model: Option<String>,
    /// Override base URL
    #[serde(default)]
    pub base_url: Option<String>,
    /// Whether this preset is enabled (set false to hide a built-in generation provider)
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

/// Generation presets overrides, grouped by media type.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GenerationPresetsOverride {
    /// Image generation provider overrides (keyed by preset name)
    #[serde(default)]
    pub image: HashMap<String, PartialGenerationPreset>,
    /// Video generation provider overrides (keyed by preset name)
    #[serde(default)]
    pub video: HashMap<String, PartialGenerationPreset>,
    /// Audio generation provider overrides (keyed by preset name)
    #[serde(default)]
    pub audio: HashMap<String, PartialGenerationPreset>,
}

// =============================================================================
// Root override struct
// =============================================================================

/// Root struct for ~/.aleph/presets.toml
///
/// Contains user overrides for both LLM provider presets and generation provider presets.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PresetsOverride {
    /// LLM provider preset overrides (keyed by provider name like "openai", "deepseek")
    #[serde(default)]
    pub providers: HashMap<String, PartialProviderPreset>,
    /// Generation provider preset overrides (image, video, audio)
    #[serde(default)]
    pub generation: GenerationPresetsOverride,
}

// =============================================================================
// Loading
// =============================================================================

/// Load presets override from a TOML file.
///
/// Returns `PresetsOverride::default()` if the file does not exist or cannot be parsed.
/// Logs warnings on parse errors.
pub fn load_presets_override(path: &Path) -> PresetsOverride {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return PresetsOverride::default();
        }
        Err(e) => {
            warn!("Failed to read presets override file {}: {}", path.display(), e);
            return PresetsOverride::default();
        }
    };

    match toml::from_str(&content) {
        Ok(parsed) => parsed,
        Err(e) => {
            warn!("Failed to parse presets override file {}: {}", path.display(), e);
            PresetsOverride::default()
        }
    }
}

// =============================================================================
// Owned preset types (for runtime-merged presets)
// =============================================================================

/// Owned version of ProviderPreset for runtime-merged presets.
///
/// Unlike `ProviderPreset` which uses `&'static str`, this uses `String`
/// so it can hold merged values from both built-in presets and user overrides.
#[derive(Debug, Clone)]
pub struct OwnedProviderPreset {
    pub base_url: String,
    pub protocol: String,
    pub color: String,
    pub default_model: String,
}

/// Owned version of GenerationPreset for runtime-merged presets.
#[derive(Debug, Clone)]
pub struct OwnedGenerationPreset {
    pub provider_type: String,
    pub default_model: String,
    pub base_url: Option<String>,
}

// =============================================================================
// Merge functions
// =============================================================================

/// Merge a `PartialProviderPreset` (user override) onto a built-in `ProviderPreset`.
///
/// For each field, the user override wins if present; otherwise the built-in value is used.
pub fn merge_provider_preset(
    builtin: &crate::providers::presets::ProviderPreset,
    partial: &PartialProviderPreset,
) -> OwnedProviderPreset {
    OwnedProviderPreset {
        base_url: partial
            .base_url
            .clone()
            .unwrap_or_else(|| builtin.base_url.to_string()),
        protocol: partial
            .protocol
            .clone()
            .unwrap_or_else(|| builtin.protocol.to_string()),
        color: partial
            .color
            .clone()
            .unwrap_or_else(|| builtin.color.to_string()),
        default_model: partial
            .default_model
            .clone()
            .unwrap_or_else(|| builtin.default_model.to_string()),
    }
}

/// Create an `OwnedProviderPreset` from a partial-only override (no built-in to merge with).
///
/// This is used for entirely new user-defined providers. Returns `None` if `base_url` is missing,
/// since a provider cannot function without a URL.
pub fn partial_to_provider_preset(partial: &PartialProviderPreset) -> Option<OwnedProviderPreset> {
    Some(OwnedProviderPreset {
        base_url: partial.base_url.clone()?,
        protocol: partial
            .protocol
            .clone()
            .unwrap_or_else(|| "openai".to_string()),
        color: partial
            .color
            .clone()
            .unwrap_or_else(|| "#808080".to_string()),
        default_model: partial.default_model.clone().unwrap_or_default(),
    })
}

/// Merge a `PartialGenerationPreset` (user override) onto a built-in `GenerationPreset`.
pub fn merge_generation_preset(
    builtin: &crate::config::types::generation::presets::GenerationPreset,
    partial: &PartialGenerationPreset,
) -> OwnedGenerationPreset {
    OwnedGenerationPreset {
        provider_type: partial
            .provider_type
            .clone()
            .unwrap_or_else(|| builtin.provider_type.to_string()),
        default_model: partial
            .default_model
            .clone()
            .unwrap_or_else(|| builtin.default_model.to_string()),
        base_url: partial
            .base_url
            .clone()
            .or_else(|| builtin.base_url.map(|u| u.to_string())),
    }
}

/// Create an `OwnedGenerationPreset` from a partial-only override (no built-in to merge with).
///
/// Returns `None` if `provider_type` is missing, since a generation provider must have a type.
pub fn partial_to_generation_preset(
    partial: &PartialGenerationPreset,
) -> Option<OwnedGenerationPreset> {
    Some(OwnedGenerationPreset {
        provider_type: partial.provider_type.clone()?,
        default_model: partial.default_model.clone().unwrap_or_default(),
        base_url: partial.base_url.clone(),
    })
}

// =============================================================================
// Helpers
// =============================================================================

fn default_enabled() -> bool {
    true
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_presets_override() {
        let parsed: PresetsOverride = toml::from_str("").unwrap();
        assert!(parsed.providers.is_empty());
        assert!(parsed.generation.image.is_empty());
        assert!(parsed.generation.video.is_empty());
        assert!(parsed.generation.audio.is_empty());
    }

    #[test]
    fn test_provider_preset_partial_parse() {
        let toml_str = r#"
[providers.my-provider]
base_url = "https://custom.example.com/v1"
protocol = "openai"
"#;
        let parsed: PresetsOverride = toml::from_str(toml_str).unwrap();

        let preset = parsed.providers.get("my-provider").unwrap();
        assert_eq!(preset.base_url.as_deref(), Some("https://custom.example.com/v1"));
        assert_eq!(preset.protocol.as_deref(), Some("openai"));
        // Unset fields remain None / default
        assert!(preset.color.is_none());
        assert!(preset.default_model.is_none());
        assert!(preset.aliases.is_empty());
        assert!(preset.enabled);
    }

    #[test]
    fn test_generation_preset_parse() {
        let toml_str = r#"
[generation.image.my-dalle]
provider_type = "openai"
default_model = "dall-e-3"
base_url = "https://api.openai.com"

[generation.video.my-runway]
provider_type = "runway"
default_model = "gen-4"
"#;
        let parsed: PresetsOverride = toml::from_str(toml_str).unwrap();

        // Image preset
        let img = parsed.generation.image.get("my-dalle").unwrap();
        assert_eq!(img.provider_type.as_deref(), Some("openai"));
        assert_eq!(img.default_model.as_deref(), Some("dall-e-3"));
        assert_eq!(img.base_url.as_deref(), Some("https://api.openai.com"));
        assert!(img.enabled);

        // Video preset
        let vid = parsed.generation.video.get("my-runway").unwrap();
        assert_eq!(vid.provider_type.as_deref(), Some("runway"));
        assert_eq!(vid.default_model.as_deref(), Some("gen-4"));
        assert!(vid.base_url.is_none());
        assert!(vid.enabled);
    }

    #[test]
    fn test_disable_builtin_preset() {
        let toml_str = r#"
[providers.openai]
enabled = false
"#;
        let parsed: PresetsOverride = toml::from_str(toml_str).unwrap();

        let preset = parsed.providers.get("openai").unwrap();
        assert!(!preset.enabled);
        // Other fields remain None
        assert!(preset.base_url.is_none());
        assert!(preset.protocol.is_none());
    }

    #[test]
    fn test_load_nonexistent_file() {
        let result = load_presets_override(Path::new("/tmp/does-not-exist-aleph-presets.toml"));
        assert!(result.providers.is_empty());
        assert!(result.generation.image.is_empty());
    }

    #[test]
    fn test_provider_with_aliases() {
        let toml_str = r#"
[providers.volcengine]
base_url = "https://ark.cn-beijing.volces.com/api/v3"
aliases = ["doubao", "ark"]
"#;
        let parsed: PresetsOverride = toml::from_str(toml_str).unwrap();

        let preset = parsed.providers.get("volcengine").unwrap();
        assert_eq!(preset.aliases, vec!["doubao".to_string(), "ark".to_string()]);
        assert_eq!(preset.base_url.as_deref(), Some("https://ark.cn-beijing.volces.com/api/v3"));
    }

    // =========================================================================
    // Merge function tests
    // =========================================================================

    #[test]
    fn test_merge_provider_preset_all_overrides() {
        let builtin = crate::providers::presets::ProviderPreset {
            base_url: "https://api.example.com",
            protocol: "openai",
            color: "#111111",
            default_model: "model-v1",
        };
        let partial = PartialProviderPreset {
            base_url: Some("https://custom.example.com".to_string()),
            protocol: Some("anthropic".to_string()),
            color: Some("#222222".to_string()),
            default_model: Some("model-v2".to_string()),
            ..Default::default()
        };

        let merged = merge_provider_preset(&builtin, &partial);
        assert_eq!(merged.base_url, "https://custom.example.com");
        assert_eq!(merged.protocol, "anthropic");
        assert_eq!(merged.color, "#222222");
        assert_eq!(merged.default_model, "model-v2");
    }

    #[test]
    fn test_merge_provider_preset_no_overrides() {
        let builtin = crate::providers::presets::ProviderPreset {
            base_url: "https://api.example.com",
            protocol: "openai",
            color: "#111111",
            default_model: "model-v1",
        };
        let partial = PartialProviderPreset::default();

        let merged = merge_provider_preset(&builtin, &partial);
        assert_eq!(merged.base_url, "https://api.example.com");
        assert_eq!(merged.protocol, "openai");
        assert_eq!(merged.color, "#111111");
        assert_eq!(merged.default_model, "model-v1");
    }

    #[test]
    fn test_merge_provider_preset_partial_overrides() {
        let builtin = crate::providers::presets::ProviderPreset {
            base_url: "https://api.example.com",
            protocol: "openai",
            color: "#111111",
            default_model: "model-v1",
        };
        let partial = PartialProviderPreset {
            base_url: Some("https://custom.example.com".to_string()),
            // Only override base_url
            ..Default::default()
        };

        let merged = merge_provider_preset(&builtin, &partial);
        assert_eq!(merged.base_url, "https://custom.example.com");
        assert_eq!(merged.protocol, "openai"); // from builtin
        assert_eq!(merged.color, "#111111"); // from builtin
        assert_eq!(merged.default_model, "model-v1"); // from builtin
    }

    #[test]
    fn test_partial_to_provider_preset_complete() {
        let partial = PartialProviderPreset {
            base_url: Some("https://my-api.com/v1".to_string()),
            protocol: Some("openai".to_string()),
            color: Some("#ff0000".to_string()),
            default_model: Some("my-model".to_string()),
            ..Default::default()
        };

        let owned = partial_to_provider_preset(&partial).unwrap();
        assert_eq!(owned.base_url, "https://my-api.com/v1");
        assert_eq!(owned.protocol, "openai");
        assert_eq!(owned.color, "#ff0000");
        assert_eq!(owned.default_model, "my-model");
    }

    #[test]
    fn test_partial_to_provider_preset_minimal() {
        let partial = PartialProviderPreset {
            base_url: Some("https://my-api.com/v1".to_string()),
            // Only base_url is required
            ..Default::default()
        };

        let owned = partial_to_provider_preset(&partial).unwrap();
        assert_eq!(owned.base_url, "https://my-api.com/v1");
        assert_eq!(owned.protocol, "openai"); // default
        assert_eq!(owned.color, "#808080"); // default
        assert_eq!(owned.default_model, ""); // default empty
    }

    #[test]
    fn test_partial_to_provider_preset_missing_base_url() {
        let partial = PartialProviderPreset {
            protocol: Some("openai".to_string()),
            // No base_url — should return None
            ..Default::default()
        };

        assert!(partial_to_provider_preset(&partial).is_none());
    }

    #[test]
    fn test_merge_generation_preset_all_overrides() {
        let builtin = crate::config::types::generation::presets::GenerationPreset {
            provider_type: "openai",
            default_model: "dall-e-3",
            base_url: Some("https://api.openai.com"),
        };
        let partial = PartialGenerationPreset {
            provider_type: Some("custom".to_string()),
            default_model: Some("custom-model".to_string()),
            base_url: Some("https://custom.com".to_string()),
            enabled: true,
        };

        let merged = merge_generation_preset(&builtin, &partial);
        assert_eq!(merged.provider_type, "custom");
        assert_eq!(merged.default_model, "custom-model");
        assert_eq!(merged.base_url.as_deref(), Some("https://custom.com"));
    }

    #[test]
    fn test_merge_generation_preset_no_overrides() {
        let builtin = crate::config::types::generation::presets::GenerationPreset {
            provider_type: "openai",
            default_model: "dall-e-3",
            base_url: Some("https://api.openai.com"),
        };
        let partial = PartialGenerationPreset::default();

        let merged = merge_generation_preset(&builtin, &partial);
        assert_eq!(merged.provider_type, "openai");
        assert_eq!(merged.default_model, "dall-e-3");
        assert_eq!(merged.base_url.as_deref(), Some("https://api.openai.com"));
    }

    #[test]
    fn test_merge_generation_preset_builtin_no_base_url() {
        let builtin = crate::config::types::generation::presets::GenerationPreset {
            provider_type: "google",
            default_model: "imagen-3",
            base_url: None,
        };
        let partial = PartialGenerationPreset::default();

        let merged = merge_generation_preset(&builtin, &partial);
        assert_eq!(merged.provider_type, "google");
        assert!(merged.base_url.is_none());
    }

    #[test]
    fn test_partial_to_generation_preset_complete() {
        let partial = PartialGenerationPreset {
            provider_type: Some("custom".to_string()),
            default_model: Some("my-model".to_string()),
            base_url: Some("https://my-api.com".to_string()),
            enabled: true,
        };

        let owned = partial_to_generation_preset(&partial).unwrap();
        assert_eq!(owned.provider_type, "custom");
        assert_eq!(owned.default_model, "my-model");
        assert_eq!(owned.base_url.as_deref(), Some("https://my-api.com"));
    }

    #[test]
    fn test_partial_to_generation_preset_minimal() {
        let partial = PartialGenerationPreset {
            provider_type: Some("custom".to_string()),
            ..Default::default()
        };

        let owned = partial_to_generation_preset(&partial).unwrap();
        assert_eq!(owned.provider_type, "custom");
        assert_eq!(owned.default_model, "");
        assert!(owned.base_url.is_none());
    }

    #[test]
    fn test_partial_to_generation_preset_missing_type() {
        let partial = PartialGenerationPreset {
            default_model: Some("my-model".to_string()),
            // No provider_type — should return None
            ..Default::default()
        };

        assert!(partial_to_generation_preset(&partial).is_none());
    }
}
