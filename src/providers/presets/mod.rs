//! Provider presets registry
//!
//! Contains default configurations for known AI providers.

use crate::providers::metadata::{Modality, ProviderMetadata};

mod metadata;
mod registry;

#[cfg(test)]
mod tests;

pub use metadata::PRESET_METADATA;
pub use registry::PRESETS;

/// Provider preset configuration
#[derive(Debug, Clone)]
pub struct ProviderPreset {
    /// Default base URL for the provider
    pub base_url: &'static str,
    /// Protocol to use (e.g., "openai", "anthropic")
    pub protocol: &'static str,
    /// Default color for UI
    pub color: &'static str,
    /// Default model for the provider
    pub default_model: &'static str,
}

/// Look up rich metadata for a preset by name (case-insensitive).
///
/// Returns `None` if the preset has no metadata entry — callers can still
/// treat it as a chat-only provider for routing purposes.
pub fn provider_metadata(name: &str) -> Option<&'static ProviderMetadata> {
    PRESET_METADATA.get(name.to_lowercase().as_str())
}

/// All preset names (sorted) that serve the requested modality.
///
/// Presets without an explicit metadata entry are treated as `Chat`-only,
/// matching the historical default — this keeps backward compatibility
/// while letting new multimodal entries opt in via [`PRESET_METADATA`].
pub fn presets_by_modality(modality: Modality) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = PRESETS
        .keys()
        .copied()
        .filter(|name| match PRESET_METADATA.get(*name) {
            Some(meta) => meta.supports(modality),
            // Default assumption for entries without metadata: chat-only.
            None => modality == Modality::Chat,
        })
        .collect();
    out.sort_unstable();
    out
}

/// Get a preset by name (case-insensitive)
pub fn get_preset(name: &str) -> Option<&'static ProviderPreset> {
    PRESETS.get(name.to_lowercase().as_str())
}

/// Get a preset with override support.
///
/// Resolution order:
/// 1. If a user override exists for the name (or via alias), merge it onto the built-in preset.
/// 2. If only a built-in preset exists, convert it to owned form.
/// 3. If only a user override exists (new provider), create from partial.
/// 4. Returns `None` if disabled or not found.
pub fn get_merged_preset(
    name: &str,
    overrides: &crate::config::presets_override::PresetsOverride,
) -> Option<crate::config::presets_override::OwnedProviderPreset> {
    let lower = name.to_lowercase();
    let builtin = PRESETS.get(lower.as_str());
    let partial = overrides.providers.get(&lower).or_else(|| {
        // Check aliases in user overrides
        overrides
            .providers
            .values()
            .find(|p| p.aliases.iter().any(|a| a.to_lowercase() == lower))
    });

    match (builtin, partial) {
        (Some(b), Some(p)) => {
            if !p.enabled {
                return None;
            }
            Some(crate::config::presets_override::merge_provider_preset(b, p))
        }
        (Some(b), None) => Some(crate::config::presets_override::OwnedProviderPreset {
            base_url: b.base_url.to_string(),
            protocol: b.protocol.to_string(),
            color: b.color.to_string(),
            default_model: b.default_model.to_string(),
        }),
        (None, Some(p)) => {
            if !p.enabled {
                return None;
            }
            crate::config::presets_override::partial_to_provider_preset(p)
        }
        (None, None) => None,
    }
}

/// Resolve provider name from model name using known prefix patterns.
pub fn resolve_provider_from_model(model: &str) -> Option<String> {
    let m = model.to_lowercase();
    if m.starts_with("gpt-") || m.starts_with("o1-") || m.starts_with("o3-") || m.starts_with("o4-")
    {
        Some("openai".into())
    } else if m.starts_with("claude-") {
        Some("anthropic".into())
    } else if m.starts_with("gemini-") {
        Some("google".into())
    } else if m.starts_with("deepseek-") {
        Some("deepseek".into())
    } else {
        None
    }
}
