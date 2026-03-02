//! Generation provider presets registry
//!
//! Contains default configurations for known generation providers (image, video, audio, speech).

use once_cell::sync::Lazy;
use std::collections::HashMap;

/// Generation provider preset configuration
#[derive(Debug, Clone)]
pub struct GenerationPreset {
    /// Provider type identifier (e.g., "openai", "stability")
    pub provider_type: &'static str,
    /// Default model for the provider
    pub default_model: &'static str,
    /// Default base URL (None means provider-specific SDK default)
    pub base_url: Option<&'static str>,
}

/// Registry of known generation provider presets, keyed by preset ID
pub static PRESETS: Lazy<HashMap<&'static str, GenerationPreset>> = Lazy::new(|| {
    let mut m = HashMap::new();

    // Image providers
    m.insert(
        "openai-dalle",
        GenerationPreset {
            provider_type: "openai",
            default_model: "dall-e-3",
            base_url: Some("https://api.openai.com"),

        },
    );
    m.insert(
        "stability-ai",
        GenerationPreset {
            provider_type: "stability",
            default_model: "stable-diffusion-xl-1024-v1-0",
            base_url: Some("https://api.stability.ai"),

        },
    );
    m.insert(
        "google-imagen",
        GenerationPreset {
            provider_type: "google",
            default_model: "imagen-3.0-generate-002",
            base_url: None,

        },
    );
    m.insert(
        "replicate",
        GenerationPreset {
            provider_type: "replicate",
            default_model: "black-forest-labs/flux-schnell",
            base_url: Some("https://api.replicate.com"),

        },
    );

    // Video providers
    m.insert(
        "google-veo",
        GenerationPreset {
            provider_type: "google_veo",
            default_model: "veo-2.0-generate-001",
            base_url: None,

        },
    );
    m.insert(
        "runway",
        GenerationPreset {
            provider_type: "runway",
            default_model: "gen-3",
            base_url: Some("https://api.runwayml.com/v1"),

        },
    );
    m.insert(
        "pika",
        GenerationPreset {
            provider_type: "pika",
            default_model: "pika-1.0",
            base_url: Some("https://api.pika.art/v1"),

        },
    );

    // Audio/Speech providers
    m.insert(
        "openai-tts",
        GenerationPreset {
            provider_type: "openai_tts",
            default_model: "tts-1-hd",
            base_url: Some("https://api.openai.com"),

        },
    );
    m.insert(
        "elevenlabs",
        GenerationPreset {
            provider_type: "elevenlabs",
            default_model: "eleven_multilingual_v2",
            base_url: Some("https://api.elevenlabs.io"),

        },
    );

    m
});

/// Get a generation preset by name (exact match)
pub fn get_preset(name: &str) -> Option<&'static GenerationPreset> {
    PRESETS.get(name)
}

/// Find a generation preset by provider_type
pub fn get_preset_by_type(provider_type: &str) -> Option<&'static GenerationPreset> {
    PRESETS.values().find(|p| p.provider_type == provider_type)
}

/// Get a generation preset with override support.
///
/// Resolution order:
/// 1. Look up by `name` in built-in presets, then merge with any user override.
/// 2. Fall back to `provider_type` lookup in built-in presets.
/// 3. If only a user override exists (new generation provider), create from partial.
/// 4. Returns `None` if disabled or not found.
///
/// The `generation_overrides` parameter is the generation section of `PresetsOverride`,
/// which groups overrides by media type. We search across all media type maps (image, video, audio).
pub fn get_merged_generation_preset(
    name: &str,
    provider_type: &str,
    generation_overrides: &crate::config::presets_override::GenerationPresetsOverride,
) -> Option<crate::config::presets_override::OwnedGenerationPreset> {
    let builtin = get_preset(name).or_else(|| get_preset_by_type(provider_type));

    // Search across all generation override maps for a matching name
    let partial = generation_overrides
        .image
        .get(name)
        .or_else(|| generation_overrides.video.get(name))
        .or_else(|| generation_overrides.audio.get(name));

    match (builtin, partial) {
        (Some(b), Some(p)) => {
            if !p.enabled {
                return None;
            }
            Some(crate::config::presets_override::merge_generation_preset(b, p))
        }
        (Some(b), None) => {
            Some(crate::config::presets_override::OwnedGenerationPreset {
                provider_type: b.provider_type.to_string(),
                default_model: b.default_model.to_string(),
                base_url: b.base_url.map(|u| u.to_string()),
            })
        }
        (None, Some(p)) => {
            if !p.enabled {
                return None;
            }
            crate::config::presets_override::partial_to_generation_preset(p)
        }
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_presets_contain_known_providers() {
        assert!(PRESETS.contains_key("openai-dalle"));
        assert!(PRESETS.contains_key("stability-ai"));
        assert!(PRESETS.contains_key("google-imagen"));
        assert!(PRESETS.contains_key("replicate"));
        assert!(PRESETS.contains_key("google-veo"));
        assert!(PRESETS.contains_key("openai-tts"));
        assert!(PRESETS.contains_key("elevenlabs"));
    }

    #[test]
    fn test_get_preset() {
        let preset = get_preset("openai-dalle").unwrap();
        assert_eq!(preset.provider_type, "openai");
        assert_eq!(preset.default_model, "dall-e-3");
    }

    #[test]
    fn test_get_preset_by_type() {
        let preset = get_preset_by_type("openai").unwrap();
        assert_eq!(preset.default_model, "dall-e-3");
    }

    // =========================================================================
    // get_merged_generation_preset tests
    // =========================================================================

    #[test]
    fn test_get_merged_generation_preset_builtin_only() {
        let overrides = crate::config::presets_override::GenerationPresetsOverride::default();
        let preset = get_merged_generation_preset("openai-dalle", "openai", &overrides).unwrap();
        assert_eq!(preset.provider_type, "openai");
        assert_eq!(preset.default_model, "dall-e-3");
        assert_eq!(preset.base_url.as_deref(), Some("https://api.openai.com"));
    }

    #[test]
    fn test_get_merged_generation_preset_builtin_by_type() {
        let overrides = crate::config::presets_override::GenerationPresetsOverride::default();
        // Name doesn't match any preset, but provider_type "openai" does
        let preset = get_merged_generation_preset("my-custom-dalle", "openai", &overrides).unwrap();
        assert_eq!(preset.provider_type, "openai");
        assert_eq!(preset.default_model, "dall-e-3");
    }

    #[test]
    fn test_get_merged_generation_preset_with_override() {
        let mut overrides = crate::config::presets_override::GenerationPresetsOverride::default();
        overrides.image.insert(
            "openai-dalle".to_string(),
            crate::config::presets_override::PartialGenerationPreset {
                default_model: Some("dall-e-4".to_string()),
                enabled: true,
                ..Default::default()
            },
        );

        let preset = get_merged_generation_preset("openai-dalle", "openai", &overrides).unwrap();
        assert_eq!(preset.provider_type, "openai"); // from builtin
        assert_eq!(preset.default_model, "dall-e-4"); // overridden
        assert_eq!(preset.base_url.as_deref(), Some("https://api.openai.com")); // from builtin
    }

    #[test]
    fn test_get_merged_generation_preset_disabled() {
        let mut overrides = crate::config::presets_override::GenerationPresetsOverride::default();
        overrides.image.insert(
            "openai-dalle".to_string(),
            crate::config::presets_override::PartialGenerationPreset {
                enabled: false,
                ..Default::default()
            },
        );

        assert!(get_merged_generation_preset("openai-dalle", "openai", &overrides).is_none());
    }

    #[test]
    fn test_get_merged_generation_preset_new_provider() {
        let mut overrides = crate::config::presets_override::GenerationPresetsOverride::default();
        overrides.video.insert(
            "my-video-gen".to_string(),
            crate::config::presets_override::PartialGenerationPreset {
                provider_type: Some("custom-video".to_string()),
                default_model: Some("video-v1".to_string()),
                base_url: Some("https://video-gen.example.com".to_string()),
                enabled: true,
            },
        );

        let preset = get_merged_generation_preset("my-video-gen", "custom-video", &overrides).unwrap();
        assert_eq!(preset.provider_type, "custom-video");
        assert_eq!(preset.default_model, "video-v1");
        assert_eq!(preset.base_url.as_deref(), Some("https://video-gen.example.com"));
    }

    #[test]
    fn test_get_merged_generation_preset_not_found() {
        let overrides = crate::config::presets_override::GenerationPresetsOverride::default();
        assert!(get_merged_generation_preset("nonexistent", "nonexistent-type", &overrides).is_none());
    }

    #[test]
    fn test_get_merged_generation_preset_audio_override() {
        let mut overrides = crate::config::presets_override::GenerationPresetsOverride::default();
        overrides.audio.insert(
            "elevenlabs".to_string(),
            crate::config::presets_override::PartialGenerationPreset {
                default_model: Some("eleven_turbo_v2".to_string()),
                enabled: true,
                ..Default::default()
            },
        );

        let preset = get_merged_generation_preset("elevenlabs", "elevenlabs", &overrides).unwrap();
        assert_eq!(preset.provider_type, "elevenlabs"); // from builtin
        assert_eq!(preset.default_model, "eleven_turbo_v2"); // overridden
    }
}
