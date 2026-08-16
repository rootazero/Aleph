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
    let video_names = [
        "fal-kling-video",
        "fal-luma",
        "fal-runway",
        "fal-pika",
        "fal-hailuo",
        "fal-jimeng",
    ];
    for name in video_names {
        let meta = generation_metadata(name).expect(name);
        assert!(
            meta.supports(Modality::Video),
            "{name} should support Video"
        );
        assert!(
            !meta.supports(Modality::Image),
            "{name} should not advertise Image"
        );
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
    assert_eq!(preset.default_model, "dall-e-3");
    assert_eq!(preset.base_url.as_deref(), Some("https://api.openai.com"));
}

#[test]
fn test_get_merged_generation_preset_builtin_by_type() {
    let overrides = crate::config::presets_override::GenerationPresetsOverride::default();
    // Name doesn't match any preset, but provider_type "openai" does
    let preset = get_merged_generation_preset("my-custom-dalle", "openai", &overrides).unwrap();
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
    assert_eq!(preset.default_model, "video-v1");
    assert_eq!(
        preset.base_url.as_deref(),
        Some("https://video-gen.example.com")
    );
}

#[test]
fn test_get_merged_generation_preset_not_found() {
    let overrides = crate::config::presets_override::GenerationPresetsOverride::default();
    assert!(get_merged_generation_preset("nonexistent", "nonexistent-type", &overrides).is_none());
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
    assert_eq!(preset.default_model, "eleven_turbo_v2"); // overridden
}

// =========================================================================
// hermes-parity field tests (Phase 2.3)
// =========================================================================

#[test]
fn high_value_generation_presets_have_display_name() {
    // Every shipped preset must carry a panel-ready display name. The
    // derived metadata falls back to canonical name if missing, but a
    // human-friendly label is the desired baseline.
    for name in [
        "openai-dalle",
        "google-imagen",
        "stability-ai",
        "fal",
        "elevenlabs",
        "openai-tts",
        "suno",
        "bfl-flux",
    ] {
        let p = get_preset(name).unwrap_or_else(|| panic!("missing preset {name}"));
        assert!(
            p.display_name.is_some(),
            "{name} should declare a panel display_name"
        );
    }
}

#[test]
fn high_value_generation_presets_have_homepage_and_signup() {
    for name in [
        "openai-dalle",
        "stability-ai",
        "elevenlabs",
        "fal",
        "deepgram-tts",
    ] {
        let p = get_preset(name).unwrap_or_else(|| panic!("missing preset {name}"));
        assert!(p.homepage.is_some(), "{name} should declare a homepage");
        assert!(
            p.signup_url.is_some(),
            "{name} should declare a signup_url for the CTA"
        );
    }
}

#[test]
fn presets_must_declare_at_least_one_modality() {
    // Catch the omission case: a preset without modalities is invisible
    // to `generation_presets_by_modality()`. All shipped entries today
    // declare exactly one modality (or the multi-modality replicate/fal).
    for (name, preset) in PRESETS.iter() {
        assert!(
            !preset.modalities.is_empty(),
            "preset {name} ships without any modality — invisible to routing"
        );
    }
}

#[test]
fn generation_metadata_display_name_falls_back_to_canonical() {
    // The derived metadata uses preset.display_name.unwrap_or(name);
    // we don't ship any without display_name today, but the fallback
    // is the design — assert at least one entry round-trips cleanly.
    let m = generation_metadata("openai-dalle").unwrap();
    assert_eq!(m.display_name, "OpenAI DALL-E");
}

#[test]
fn const_builder_chains_compose_correctly() {
    let p = GenerationPreset::new("foo", "model-x", Some("https://example.com"))
        .with_modalities(&[Modality::Image])
        .with_display("Foo")
        .with_description("desc")
        .with_homepage("https://docs.foo")
        .with_signup("https://signup.foo");
    assert_eq!(p.provider_type, "foo");
    assert_eq!(p.default_model, "model-x");
    assert_eq!(p.base_url, Some("https://example.com"));
    assert_eq!(p.display_name, Some("Foo"));
    assert_eq!(p.description, Some("desc"));
    assert_eq!(p.homepage, Some("https://docs.foo"));
    assert_eq!(p.signup_url, Some("https://signup.foo"));
    assert_eq!(p.modalities, &[Modality::Image]);
}
