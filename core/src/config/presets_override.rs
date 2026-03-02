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
}
