//! Generation provider presets registry
//!
//! Contains default configurations for known generation providers (image, video, audio, speech).

use crate::providers::metadata::{Modality, ProviderMetadata};
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

    // -------------------------------------------------------------------------
    // Phase C — Fal.ai aggregator. One provider, many model endpoints. Each
    // preset below points at a Fal-hosted model and enables the right
    // modality. `capabilities` on the GenerationProviderConfig (set via
    // user config.toml or the factory default) controls which generation
    // type the preset will accept.
    // -------------------------------------------------------------------------
    m.insert(
        "fal",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/flux/dev",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-flux",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/flux/dev",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-kling-video",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/kling-video/v1/pro/text-to-video",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-luma",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/luma-dream-machine",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-runway",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/runway-gen3/turbo/image-to-video",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-pika",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/pika/v2.2/text-to-video",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-hailuo",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/minimax/video-01",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-jimeng",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/bytedance/seedance-1-pro",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-musicgen",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/musicgen",
            base_url: Some("https://queue.fal.run"),
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

// =============================================================================
// Generation provider metadata (modality / display name / homepage)
// =============================================================================

const IMAGE_ONLY: &[Modality] = &[Modality::Image];
const VIDEO_ONLY: &[Modality] = &[Modality::Video];
const SPEECH_ONLY: &[Modality] = &[Modality::Speech];

/// Per-preset metadata for generation providers — display name, modalities,
/// homepage. Mirror of `crate::providers::presets::PRESET_METADATA` for the
/// image/video/audio layer.
pub static GENERATION_METADATA: Lazy<HashMap<&'static str, ProviderMetadata>> = Lazy::new(|| {
    let mut m: HashMap<&'static str, ProviderMetadata> = HashMap::new();

    m.insert(
        "openai-dalle",
        ProviderMetadata {
            display_name: "OpenAI DALL-E",
            modalities: IMAGE_ONLY,
            homepage: Some("https://platform.openai.com/docs/guides/images"),
            notes: None,
        },
    );
    m.insert(
        "stability-ai",
        ProviderMetadata {
            display_name: "Stability AI",
            modalities: IMAGE_ONLY,
            homepage: Some("https://platform.stability.ai"),
            notes: Some("Stable Diffusion XL family"),
        },
    );
    m.insert(
        "google-imagen",
        ProviderMetadata {
            display_name: "Google Imagen",
            modalities: IMAGE_ONLY,
            homepage: Some("https://ai.google.dev/gemini-api/docs/imagen"),
            notes: None,
        },
    );
    m.insert(
        "replicate",
        ProviderMetadata {
            display_name: "Replicate",
            modalities: &[Modality::Image, Modality::Video, Modality::Music],
            homepage: Some("https://replicate.com"),
            notes: Some("OSS model hosting (Flux, SDXL, MusicGen, …)"),
        },
    );
    m.insert(
        "google-veo",
        ProviderMetadata {
            display_name: "Google Veo",
            modalities: VIDEO_ONLY,
            homepage: Some("https://deepmind.google/technologies/veo"),
            notes: None,
        },
    );
    m.insert(
        "runway",
        ProviderMetadata {
            display_name: "Runway Gen-3",
            modalities: VIDEO_ONLY,
            homepage: Some("https://docs.dev.runwayml.com"),
            notes: None,
        },
    );
    m.insert(
        "pika",
        ProviderMetadata {
            display_name: "Pika Labs",
            modalities: VIDEO_ONLY,
            homepage: Some("https://pika.art"),
            notes: None,
        },
    );
    m.insert(
        "openai-tts",
        ProviderMetadata {
            display_name: "OpenAI TTS",
            modalities: SPEECH_ONLY,
            homepage: Some("https://platform.openai.com/docs/guides/text-to-speech"),
            notes: None,
        },
    );
    m.insert(
        "elevenlabs",
        ProviderMetadata {
            display_name: "ElevenLabs",
            modalities: SPEECH_ONLY,
            homepage: Some("https://elevenlabs.io"),
            notes: None,
        },
    );

    // Phase C — Fal.ai aggregator family
    m.insert(
        "fal",
        ProviderMetadata {
            display_name: "Fal.ai",
            modalities: &[Modality::Image, Modality::Video, Modality::Music],
            homepage: Some("https://fal.ai"),
            notes: Some("Aggregator — image/video/music via queue API"),
        },
    );
    m.insert(
        "fal-flux",
        ProviderMetadata {
            display_name: "Flux (via Fal)",
            modalities: IMAGE_ONLY,
            homepage: Some("https://fal.ai/models/fal-ai/flux/dev"),
            notes: None,
        },
    );
    m.insert(
        "fal-kling-video",
        ProviderMetadata {
            display_name: "Kling Video (via Fal) / 可灵",
            modalities: VIDEO_ONLY,
            homepage: Some("https://fal.ai/models/fal-ai/kling-video"),
            notes: None,
        },
    );
    m.insert(
        "fal-luma",
        ProviderMetadata {
            display_name: "Luma Dream Machine (via Fal)",
            modalities: VIDEO_ONLY,
            homepage: Some("https://fal.ai/models/fal-ai/luma-dream-machine"),
            notes: None,
        },
    );
    m.insert(
        "fal-runway",
        ProviderMetadata {
            display_name: "Runway Gen-3 (via Fal)",
            modalities: VIDEO_ONLY,
            homepage: Some("https://fal.ai/models/fal-ai/runway-gen3"),
            notes: None,
        },
    );
    m.insert(
        "fal-pika",
        ProviderMetadata {
            display_name: "Pika (via Fal)",
            modalities: VIDEO_ONLY,
            homepage: Some("https://fal.ai/models/fal-ai/pika"),
            notes: None,
        },
    );
    m.insert(
        "fal-hailuo",
        ProviderMetadata {
            display_name: "Hailuo (via Fal) / 海螺",
            modalities: VIDEO_ONLY,
            homepage: Some("https://fal.ai/models/fal-ai/minimax/video-01"),
            notes: Some("MiniMax video model"),
        },
    );
    m.insert(
        "fal-jimeng",
        ProviderMetadata {
            display_name: "Jimeng / Seedance (via Fal) / 即梦",
            modalities: VIDEO_ONLY,
            homepage: Some("https://fal.ai/models/fal-ai/bytedance/seedance-1-pro"),
            notes: Some("ByteDance Seedance"),
        },
    );
    m.insert(
        "fal-musicgen",
        ProviderMetadata {
            display_name: "MusicGen (via Fal)",
            modalities: &[Modality::Music],
            homepage: Some("https://fal.ai/models/fal-ai/musicgen"),
            notes: None,
        },
    );

    m
});

/// Look up rich metadata for a generation preset by name.
pub fn generation_metadata(name: &str) -> Option<&'static ProviderMetadata> {
    GENERATION_METADATA.get(name)
}

/// All generation preset names (sorted) that serve the requested modality.
///
/// Presets without a metadata entry are excluded — generation providers
/// must opt in explicitly (unlike chat, which defaults to `Chat`-only).
pub fn generation_presets_by_modality(modality: Modality) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = PRESETS
        .keys()
        .copied()
        .filter(|name| {
            GENERATION_METADATA
                .get(*name)
                .map(|m| m.supports(modality))
                .unwrap_or(false)
        })
        .collect();
    out.sort_unstable();
    out
}

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
            Some(crate::config::presets_override::merge_generation_preset(
                b, p,
            ))
        }
        (Some(b), None) => Some(crate::config::presets_override::OwnedGenerationPreset {
            provider_type: b.provider_type.to_string(),
            default_model: b.default_model.to_string(),
            base_url: b.base_url.map(|u| u.to_string()),
        }),
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

        // Phase C — Fal.ai family
        assert!(PRESETS.contains_key("fal"));
        assert!(PRESETS.contains_key("fal-flux"));
        assert!(PRESETS.contains_key("fal-kling-video"));
        assert!(PRESETS.contains_key("fal-luma"));
        assert!(PRESETS.contains_key("fal-runway"));
        assert!(PRESETS.contains_key("fal-pika"));
        assert!(PRESETS.contains_key("fal-hailuo"));
        assert!(PRESETS.contains_key("fal-jimeng"));
        assert!(PRESETS.contains_key("fal-musicgen"));
    }

    #[test]
    fn test_fal_presets_share_provider_type_and_endpoint() {
        for name in [
            "fal",
            "fal-flux",
            "fal-kling-video",
            "fal-luma",
            "fal-runway",
            "fal-pika",
            "fal-hailuo",
            "fal-jimeng",
            "fal-musicgen",
        ] {
            let p = get_preset(name).unwrap_or_else(|| panic!("missing fal preset {name}"));
            assert_eq!(p.provider_type, "fal");
            assert_eq!(p.base_url, Some("https://queue.fal.run"));
        }
    }

    #[test]
    fn test_fal_video_presets_advertise_video_modality() {
        let video_names = ["fal-kling-video", "fal-luma", "fal-runway", "fal-pika", "fal-hailuo", "fal-jimeng"];
        for name in video_names {
            let meta = generation_metadata(name).expect(name);
            assert!(meta.supports(Modality::Video), "{name} should support Video");
            assert!(!meta.supports(Modality::Image), "{name} should not advertise Image");
        }
    }

    #[test]
    fn test_fal_musicgen_advertises_music_only() {
        let m = generation_metadata("fal-musicgen").unwrap();
        assert!(m.supports(Modality::Music));
        assert!(!m.supports(Modality::Video));
        assert!(!m.supports(Modality::Image));
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

        let preset =
            get_merged_generation_preset("my-video-gen", "custom-video", &overrides).unwrap();
        assert_eq!(preset.provider_type, "custom-video");
        assert_eq!(preset.default_model, "video-v1");
        assert_eq!(
            preset.base_url.as_deref(),
            Some("https://video-gen.example.com")
        );
    }

    #[test]
    fn test_get_merged_generation_preset_not_found() {
        let overrides = crate::config::presets_override::GenerationPresetsOverride::default();
        assert!(
            get_merged_generation_preset("nonexistent", "nonexistent-type", &overrides).is_none()
        );
    }

    #[test]
    fn test_generation_metadata_lookup() {
        let dalle = generation_metadata("openai-dalle").expect("dalle metadata");
        assert_eq!(dalle.display_name, "OpenAI DALL-E");
        assert!(dalle.supports(Modality::Image));
        assert!(!dalle.supports(Modality::Video));

        let veo = generation_metadata("google-veo").expect("veo metadata");
        assert!(veo.supports(Modality::Video));

        let replicate = generation_metadata("replicate").expect("replicate metadata");
        assert!(replicate.supports(Modality::Image));
        assert!(replicate.supports(Modality::Video));
        assert!(replicate.supports(Modality::Music));
    }

    #[test]
    fn test_generation_presets_by_modality() {
        let images = generation_presets_by_modality(Modality::Image);
        assert!(images.contains(&"openai-dalle"));
        assert!(images.contains(&"stability-ai"));
        assert!(images.contains(&"google-imagen"));
        assert!(images.contains(&"replicate"));
        // Should not include video/audio-only providers
        assert!(!images.contains(&"google-veo"));
        assert!(!images.contains(&"elevenlabs"));

        let videos = generation_presets_by_modality(Modality::Video);
        assert!(videos.contains(&"google-veo"));
        assert!(videos.contains(&"runway"));
        assert!(videos.contains(&"pika"));
        assert!(videos.contains(&"replicate"));

        let speech = generation_presets_by_modality(Modality::Speech);
        assert!(speech.contains(&"openai-tts"));
        assert!(speech.contains(&"elevenlabs"));

        // Chat is not a generation modality
        assert!(generation_presets_by_modality(Modality::Chat).is_empty());
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
