//! Generation provider presets registry
//!
//! Contains default configurations for known generation providers (image, video, audio, speech).

use crate::providers::metadata::{Modality, ProviderMetadata};

mod metadata;
mod registry;

#[cfg(test)]
mod tests;

pub use metadata::GENERATION_METADATA;
pub use registry::PRESETS;

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
