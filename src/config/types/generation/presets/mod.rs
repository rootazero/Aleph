//! Generation provider presets registry
//!
//! Contains default configurations for known generation providers (image,
//! video, audio, speech). Follows the same hermes-parity shape as the chat
//! preset registry in `src/providers/presets/`:
//! * `PROFILES` is the single source of truth (const slice).
//! * `PRESETS` is a `Lazy<HashMap>` for backwards-compatible lookups.
//! * `GENERATION_METADATA` is a derived `Lazy<HashMap>` over the same
//!   `PROFILES` — folding the parallel metadata map (Phase 2 entropy).

use crate::providers::metadata::{Modality, ProviderMetadata};

mod registry;

#[cfg(test)]
mod tests;

pub use registry::{GENERATION_METADATA, PRESETS};

/// Generation provider preset configuration.
///
/// The first three fields (`provider_type` / `default_model` / `base_url`) are
/// required and used by the factory to instantiate the right provider.
/// The remaining fields are hermes-parity opt-in metadata; defaults are
/// safe (no aliases, chat-modality-less, no homepage) and entries opt in
/// via the `with_*` const builders.
#[derive(Debug, Clone, Copy, Default)]
pub struct GenerationPreset {
    // ── Required identity / routing ──────────────────────────────────────────
    /// Provider type identifier (e.g., "openai", "stability")
    pub provider_type: &'static str,
    /// Default model for the provider
    pub default_model: &'static str,
    /// Default base URL (None means provider-specific SDK default)
    pub base_url: Option<&'static str>,

    // ── hermes-parity extensions (opt-in, zero-cost defaults) ────────────────
    /// Modalities this preset can serve. Defaults to empty (callers must
    /// opt in via `.with_modalities(&[..])`) — unlike chat presets,
    /// generation presets cover heterogeneous modalities (image / video /
    /// music / speech / transcription) with no single sensible default.
    pub modalities: &'static [Modality],
    /// Human-friendly display name; falls back to canonical preset name.
    pub display_name: Option<&'static str>,
    /// One-line description shown alongside `display_name`.
    pub description: Option<&'static str>,
    /// Vendor docs / landing URL.
    pub homepage: Option<&'static str>,
    /// Vendor signup / API-key flow URL.
    pub signup_url: Option<&'static str>,
}

impl GenerationPreset {
    /// Const factory for the required three-tuple; extensions stay defaulted.
    pub const fn new(
        provider_type: &'static str,
        default_model: &'static str,
        base_url: Option<&'static str>,
    ) -> Self {
        Self {
            provider_type,
            default_model,
            base_url,
            modalities: &[],
            display_name: None,
            description: None,
            homepage: None,
            signup_url: None,
        }
    }

    pub const fn with_modalities(mut self, modalities: &'static [Modality]) -> Self {
        self.modalities = modalities;
        self
    }

    pub const fn with_display(mut self, name: &'static str) -> Self {
        self.display_name = Some(name);
        self
    }

    pub const fn with_description(mut self, desc: &'static str) -> Self {
        self.description = Some(desc);
        self
    }

    pub const fn with_homepage(mut self, url: &'static str) -> Self {
        self.homepage = Some(url);
        self
    }

    pub const fn with_signup(mut self, url: &'static str) -> Self {
        self.signup_url = Some(url);
        self
    }
}

/// Look up rich metadata for a generation preset by name.
pub fn generation_metadata(name: &str) -> Option<&'static ProviderMetadata> {
    GENERATION_METADATA.get(name)
}

/// All generation preset names (sorted) that serve the requested modality.
///
/// Reads directly from `preset.modalities`. Presets that don't declare any
/// modality (default state) are excluded — generation presets must opt in
/// explicitly (unlike chat presets, which default to `Modality::Chat`).
pub fn generation_presets_by_modality(modality: Modality) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = PRESETS
        .iter()
        .filter(|(_, preset)| preset.modalities.contains(&modality))
        .map(|(name, _)| *name)
        .collect();
    out.sort_unstable();
    out
}

/// Get a generation preset by name (exact match)
pub fn get_preset(name: &str) -> Option<&'static GenerationPreset> {
    PRESETS.get(name)
}

/// Find a generation preset by `provider_type`.
///
/// `PRESETS` is a `HashMap`, so a plain `.find()` would return an arbitrary
/// preset when several share the same `provider_type` (e.g. 17 `fal-*`
/// presets), making the fallback default model non-deterministic across runs.
/// Select the lexicographically-smallest preset name for a stable result.
pub fn get_preset_by_type(provider_type: &str) -> Option<&'static GenerationPreset> {
    let mut matches: Vec<(&'static str, &'static GenerationPreset)> = PRESETS
        .iter()
        .filter(|(_, p)| p.provider_type == provider_type)
        .map(|(name, p)| (*name, p))
        .collect();
    matches.sort_by_key(|(name, _)| *name);
    matches.first().map(|(_, p)| *p)
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
