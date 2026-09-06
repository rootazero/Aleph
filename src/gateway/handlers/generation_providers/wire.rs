//! Conversions between the stored generation config and the shared wire
//! shapes in `aleph_protocol::providers`.
//!
//! The wire type is a **lossy view** of the stored one on purpose (see
//! [`aleph_protocol::providers::GenerationProviderConfigJson`]), so these
//! conversions are asymmetric and each direction has a different job:
//!
//! * outbound ([`config_to_wire`]) drops nothing a client is entitled to see,
//!   and never carries the API key;
//! * inbound ([`config_from_wire`]) produces only the fields a client can set,
//!   and is meaningless on its own — the caller must merge it onto the stored
//!   entry, which is what `build_generation_provider_for_persistence`'s `prior`
//!   argument is for. Persisting the result of this function directly is the
//!   bug the `prior` argument exists to prevent.
//!
//! The guard against a field getting dropped in either direction is the
//! round-trip test below, which builds both structs with **exhaustive literals
//! and no `..Default::default()`** — so a field added to either type stops
//! this module compiling instead of silently going missing at runtime.

use aleph_protocol::providers::{
    GenerationDefaultsJson, GenerationProviderConfigJson, GenerationProviderRow,
};

use crate::config::types::generation::GenerationDefaults;
use crate::config::GenerationProviderConfig;
use crate::generation::GenerationType;

/// The snake_case modality string a `GenerationType` takes on the wire.
///
/// The exact inverse of [`super::helpers::parse_generation_type`], and the
/// only place the mapping is written down in this direction — the handlers
/// used to derive it with `format!("{gen_type:?}").to_lowercase()`, which is a
/// second derivation of the same fact that happens to agree only while every
/// variant is a single word.
pub(crate) const fn modality_str(gen_type: GenerationType) -> &'static str {
    match gen_type {
        GenerationType::Image => "image",
        GenerationType::Video => "video",
        GenerationType::Audio => "audio",
        GenerationType::Speech => "speech",
        GenerationType::Transcription => "transcription",
    }
}

fn defaults_to_wire(d: &GenerationDefaults) -> GenerationDefaultsJson {
    GenerationDefaultsJson {
        width: d.width,
        height: d.height,
        aspect_ratio: d.aspect_ratio.clone(),
        quality: d.quality.clone(),
        style: d.style.clone(),
        n: d.n,
        format: d.format.clone(),
        duration_seconds: d.duration_seconds,
        fps: d.fps,
        voice: d.voice.clone(),
        speed: d.speed,
        language: d.language.clone(),
        guidance_scale: d.guidance_scale,
        steps: d.steps,
    }
}

fn defaults_from_wire(d: GenerationDefaultsJson) -> GenerationDefaults {
    GenerationDefaults {
        width: d.width,
        height: d.height,
        aspect_ratio: d.aspect_ratio,
        quality: d.quality,
        style: d.style,
        n: d.n,
        format: d.format,
        duration_seconds: d.duration_seconds,
        fps: d.fps,
        voice: d.voice,
        speed: d.speed,
        language: d.language,
        guidance_scale: d.guidance_scale,
        steps: d.steps,
    }
}

/// The client-visible view of a stored provider entry.
///
/// `api_key` is always `None`: the stored config's own `#[serde(skip_serializing)]`
/// used to be what kept it off the wire, and stating it here means the
/// guarantee survives a change to that attribute.
pub(crate) fn config_to_wire(c: &GenerationProviderConfig) -> GenerationProviderConfigJson {
    GenerationProviderConfigJson {
        provider_type: c.provider_type.clone(),
        api_key: None,
        base_url: c.base_url.clone(),
        models: c.models.clone(),
        enabled: c.enabled,
        color: c.color.clone(),
        capabilities: c
            .capabilities
            .iter()
            .map(|t| modality_str(*t).to_string())
            .collect(),
        timeout_seconds: c.timeout_seconds,
        defaults: defaults_to_wire(&c.defaults),
        verified: c.verified,
        edit_url: c.edit_url.clone(),
        voices_url: c.voices_url.clone(),
    }
}

/// The settable half of a provider entry, as a client sent it.
///
/// `model_aliases` is empty here because the wire type cannot express it. That
/// is not a default the caller may keep — see the module docs: persisting this
/// value without merging it onto the stored entry erases the operator's
/// aliases, which is precisely the defect this module was written to end.
pub(crate) fn config_from_wire(c: GenerationProviderConfigJson) -> GenerationProviderConfig {
    GenerationProviderConfig {
        provider_type: c.provider_type,
        api_key: c.api_key,
        base_url: c.base_url,
        models: c.models,
        enabled: c.enabled,
        color: c.color,
        capabilities: c
            .capabilities
            .iter()
            .map(|s| super::helpers::parse_generation_type(s))
            .collect(),
        timeout_seconds: c.timeout_seconds,
        defaults: defaults_from_wire(c.defaults),
        model_aliases: std::collections::HashMap::new(),
        verified: c.verified,
        edit_url: c.edit_url,
        voices_url: c.voices_url,
    }
}

/// Build one `generation_providers.list` row.
///
/// `generation_type` and `has_api_key` are fields of the row type rather than
/// two `serde_json::Map::insert` calls onto an already-serialized entry, which
/// is what they used to be.
pub(crate) fn provider_row(
    name: String,
    config: &GenerationProviderConfig,
    is_default_for: &[GenerationType],
    gen_type: GenerationType,
    has_api_key: bool,
) -> GenerationProviderRow {
    GenerationProviderRow {
        name,
        config: config_to_wire(config),
        is_default_for: is_default_for
            .iter()
            .map(|t| modality_str(*t).to_string())
            .collect(),
        generation_type: Some(modality_str(gen_type).to_string()),
        has_api_key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::handlers::generation_providers::helpers::parse_generation_type;
    use std::collections::HashMap;

    /// Every variant, written out — a new one stops this compiling.
    const ALL_TYPES: [GenerationType; 5] = [
        GenerationType::Image,
        GenerationType::Video,
        GenerationType::Audio,
        GenerationType::Speech,
        GenerationType::Transcription,
    ];

    #[test]
    fn the_modality_string_and_its_parser_are_inverses() {
        for t in ALL_TYPES {
            assert_eq!(
                parse_generation_type(modality_str(t)),
                t,
                "{t:?} does not survive a wire round trip"
            );
        }
    }

    /// A fully-populated stored config, built with an exhaustive literal.
    ///
    /// No `..Default::default()`: a field added to `GenerationProviderConfig`
    /// has to be added here, which is the moment to decide whether the wire
    /// carries it or `prior` protects it.
    fn stored() -> GenerationProviderConfig {
        GenerationProviderConfig {
            provider_type: "openai".into(),
            api_key: Some("sk-secret".into()),
            base_url: Some("https://example.invalid".into()),
            models: vec!["dall-e-3".into(), "dall-e-2".into()],
            enabled: true,
            color: "#10a37f".into(),
            capabilities: vec![GenerationType::Image],
            timeout_seconds: Some(45),
            defaults: GenerationDefaults {
                width: Some(1024),
                height: Some(768),
                aspect_ratio: Some("4:3".into()),
                quality: Some("hd".into()),
                style: Some("vivid".into()),
                n: Some(2),
                format: Some("png".into()),
                duration_seconds: Some(3.5),
                fps: Some(24),
                voice: Some("alloy".into()),
                speed: Some(1.25),
                language: Some("en".into()),
                guidance_scale: Some(7.5),
                steps: Some(30),
            },
            model_aliases: HashMap::from([("fast".to_string(), "dall-e-2".to_string())]),
            verified: true,
            edit_url: Some("https://example.invalid/edit".into()),
            voices_url: Some("https://example.invalid/voices".into()),
        }
    }

    /// Everything the wire CAN carry survives a round trip.
    ///
    /// The two documented exceptions are asserted separately below rather than
    /// left as "well, those are different" — an exception nobody states is
    /// indistinguishable from a field someone forgot.
    #[test]
    fn every_wire_expressible_field_survives_a_round_trip() {
        let before = stored();
        let after = config_from_wire(config_to_wire(&before));

        assert_eq!(after.provider_type, before.provider_type);
        assert_eq!(after.base_url, before.base_url);
        assert_eq!(after.models, before.models);
        assert_eq!(after.enabled, before.enabled);
        assert_eq!(after.color, before.color);
        assert_eq!(after.capabilities, before.capabilities);
        assert_eq!(after.timeout_seconds, before.timeout_seconds);
        assert_eq!(after.verified, before.verified);
        assert_eq!(after.edit_url, before.edit_url);
        assert_eq!(after.voices_url, before.voices_url);

        let d = &after.defaults;
        let b = &before.defaults;
        assert_eq!(d.width, b.width);
        assert_eq!(d.height, b.height);
        assert_eq!(d.aspect_ratio, b.aspect_ratio);
        assert_eq!(d.quality, b.quality);
        assert_eq!(d.style, b.style);
        assert_eq!(d.n, b.n);
        assert_eq!(d.format, b.format);
        assert_eq!(d.duration_seconds, b.duration_seconds);
        assert_eq!(d.fps, b.fps);
        assert_eq!(d.voice, b.voice);
        assert_eq!(d.speed, b.speed);
        assert_eq!(d.language, b.language);
        assert_eq!(d.guidance_scale, b.guidance_scale);
        assert_eq!(d.steps, b.steps);
    }

    /// The API key never leaves, whatever the stored entry holds.
    #[test]
    fn the_api_key_is_not_part_of_the_outbound_view() {
        assert_eq!(config_to_wire(&stored()).api_key, None);
    }

    /// The one field the wire cannot express, pinned as a known loss.
    ///
    /// This assertion is the reason `build_generation_provider_for_persistence`
    /// takes a `prior`: on its own, the inbound conversion drops the aliases.
    #[test]
    fn model_aliases_do_not_survive_the_inbound_conversion_alone() {
        let before = stored();
        assert!(!before.model_aliases.is_empty());
        let after = config_from_wire(config_to_wire(&before));
        assert!(
            after.model_aliases.is_empty(),
            "the wire type gained a model_aliases field -- if that was deliberate, \
             this test should now assert it survives"
        );
    }

    #[test]
    fn a_row_names_its_modality_and_its_defaults_as_strings() {
        let row = provider_row(
            "my-dalle".into(),
            &stored(),
            &[GenerationType::Image],
            GenerationType::Image,
            true,
        );
        assert_eq!(row.generation_type.as_deref(), Some("image"));
        assert_eq!(row.is_default_for, vec!["image".to_string()]);
        assert_eq!(row.effective_modality(), Some("image"));
        assert!(row.has_api_key);
        assert_eq!(row.config.api_key, None);
    }
}
