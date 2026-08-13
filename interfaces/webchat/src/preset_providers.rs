//! Preset-provider DTO and UI styling helpers.
//!
//! Authoritative preset data lives in the Rust core (see
//! `src/config/types/generation/presets.rs`). The panel fetches the catalogue
//! at runtime via the `generation_providers.list_presets` RPC; this module
//! only owns the in-process DTO and a small icon/color synthesizer because
//! those are pure UI concerns the core has no business knowing about.
//!
//! The previous incarnation of this file hard-coded nine presets and went
//! stale every time the backend grew (Round-2 / Round-3 added 23 more that
//! never surfaced in the UI). Single source of truth fixes that drift.
//!
//! # Why the wire shape is no longer declared here
//!
//! It used to be, and it was wrong: the server has always sent `signup_url`
//! and the local DTO never declared it, so serde dropped the link on every one
//! of the 44 presets. Nothing failed, nothing logged — the page simply never
//! had the one actionable thing an unconfigured row can offer. The shape now
//! comes from [`aleph_protocol::providers::GenerationPresetRow`], which the
//! server also *builds* its response from, so the same omission is a compile
//! error on both sides.
use crate::generation::GenerationType;
use aleph_protocol::providers::{GenerationPresetRow, Searchable};
use serde::{Deserialize, Serialize};

/// Catalog entry rendered as a card in the Generation Providers settings view.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PresetProvider {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub color: String,
    pub provider_type: String,
    pub capabilities: Vec<GenerationType>,
    pub default_model: String,
    pub description: String,
    pub base_url: Option<String>,
    pub homepage: Option<String>,
    /// Where to get an API key. Rendered on the unconfigured setup panel, which
    /// is the only place it can be acted on.
    pub signup_url: Option<String>,
}

/// The preset grid ranks through the same matcher as the chat catalogue.
///
/// `search_aliases` is deliberately left at its default: generation ids carry
/// no vendor nicknames, and claiming an empty alias set is the honest answer
/// rather than a placeholder for one.
impl Searchable for PresetProvider {
    fn search_id(&self) -> &str {
        &self.id
    }
    fn search_display_name(&self) -> &str {
        &self.name
    }
}

/// Convert a wire row into a UI-ready preset, synthesising icon/color from
/// `provider_type` (curated brand colors) with modality fallback.
///
/// The synthesis stays here on purpose: which emoji and which hex a vendor gets
/// is a pure presentation choice the core has no business knowing.
#[must_use]
pub fn into_preset(row: GenerationPresetRow) -> PresetProvider {
    let capabilities = row
        .modalities
        .iter()
        .filter_map(|m| modality_to_generation_type(m))
        .collect::<Vec<_>>();
    let primary = capabilities.first().copied();
    PresetProvider {
        icon: icon_for(&row.provider_type, primary),
        color: color_for(&row.provider_type, primary),
        description: row.notes.unwrap_or_default(),
        id: row.id,
        name: row.display_name,
        provider_type: row.provider_type,
        capabilities,
        default_model: row.default_model,
        base_url: row.base_url,
        homepage: row.homepage,
        signup_url: row.signup_url,
    }
}

/// Catalog snapshot used by the settings view — drop-in replacement for the
/// previous static `PresetProviders` registry.
#[derive(Debug, Clone, Default)]
pub struct PresetCatalog {
    presets: Vec<PresetProvider>,
}

impl PresetCatalog {
    pub fn from_rows(rows: Vec<GenerationPresetRow>) -> Self {
        Self {
            presets: rows.into_iter().map(into_preset).collect(),
        }
    }

    #[must_use]
    pub fn all(&self) -> &[PresetProvider] {
        &self.presets
    }

    #[must_use]
    pub fn by_category(&self, category: GenerationType) -> Vec<PresetProvider> {
        self.presets
            .iter()
            .filter(|p| p.capabilities.contains(&category))
            .cloned()
            .collect()
    }

    /// One category's presets, narrowed to a search query.
    ///
    /// Category first, then the shared ranker — so a query never pulls a video
    /// preset into the image tab, and the rows that survive come back best
    /// match first. An empty query is a no-op, which is what makes this safe to
    /// call unconditionally instead of branching on "is the box empty".
    #[must_use]
    pub fn by_category_matching(&self, category: GenerationType, query: &str) -> Vec<PresetProvider> {
        aleph_protocol::providers::filter_rows(&self.by_category(category), query)
    }

    #[must_use]
    pub fn find(&self, id: &str) -> Option<PresetProvider> {
        self.presets.iter().find(|p| p.id == id).cloned()
    }

    #[must_use]
    pub fn is_preset(&self, id: &str) -> bool {
        self.presets.iter().any(|p| p.id == id)
    }
}

/// Map backend `Modality::as_str()` strings to the panel's `GenerationType`.
/// `music` collapses to `Audio` (panel has no Music variant — music providers
/// share the Audio category tab).
fn modality_to_generation_type(s: &str) -> Option<GenerationType> {
    match s {
        "image" => Some(GenerationType::Image),
        "video" => Some(GenerationType::Video),
        "music" | "audio" => Some(GenerationType::Audio),
        "speech" => Some(GenerationType::Speech),
        "transcription" => Some(GenerationType::Transcription),
        _ => None,
    }
}

/// Brand-color lookup with modality fallback. Add new providers here as they
/// land in the backend catalogue; unknown `provider_types` fall back to a
/// modality-tinted neutral.
fn color_for(provider_type: &str, modality: Option<GenerationType>) -> String {
    let curated = match provider_type {
        "openai" | "openai_image" | "dalle" | "openai_tts" | "tts" | "openai_whisper"
        | "whisper" => "#10a37f",
        "stability" | "stability_image" | "sdxl" => "#8B5CF6",
        "google" | "google_imagen" | "imagen" | "google_veo" | "veo" => "#4285F4",
        "replicate" => "#F97316",
        "elevenlabs" => "#000000",
        "midjourney" | "mj" => "#5865F2",
        "fal" => "#FF5C00",
        "deepgram_stt" | "deepgram" | "deepgram_tts" => "#13EF93",
        "azure_speech" | "azure_tts" => "#0078D4",
        "cartesia" => "#5B21B6",
        "minimax_stt" => "#FF6B6B",
        "suno" => "#FF6B35",
        "bfl" | "bfl_flux" | "flux" => "#1F1F1F",
        _ => return modality_color(modality).to_string(),
    };
    curated.to_string()
}

const fn modality_color(modality: Option<GenerationType>) -> &'static str {
    match modality {
        Some(GenerationType::Image) => "#6366F1",
        Some(GenerationType::Video) => "#EC4899",
        Some(GenerationType::Audio) => "#FB923C",
        Some(GenerationType::Speech) => "#0EA5E9",
        Some(GenerationType::Transcription) => "#14B8A6",
        None => "#64748B",
    }
}

/// Icon lookup: prefer brand-specific where it's distinct, else fall back
/// to the modality emoji (matches the existing `GenerationType::icon()`).
fn icon_for(provider_type: &str, modality: Option<GenerationType>) -> String {
    let curated = match provider_type {
        "openai" | "openai_image" | "dalle" => "🎨",
        "stability" | "stability_image" | "sdxl" => "✨",
        "google" | "google_imagen" | "imagen" => "📷",
        "google_veo" | "veo" => "🎬",
        "replicate" => "🔄",
        "elevenlabs" => "🔊",
        "openai_tts" | "tts" => "🗣️",
        "openai_whisper" | "whisper" => "🎤",
        "midjourney" | "mj" => "🖌️",
        "fal" => "⚡",
        "deepgram_stt" | "deepgram" => "📝",
        "deepgram_tts" => "📢",
        "azure_speech" | "azure_tts" => "☁️",
        "cartesia" => "🎙️",
        "minimax_stt" => "📜",
        "suno" => "🎵",
        "bfl" | "bfl_flux" | "flux" => "🌀",
        _ => return modality_icon(modality).to_string(),
    };
    curated.to_string()
}

fn modality_icon(modality: Option<GenerationType>) -> &'static str {
    match modality {
        Some(g) => g.icon(),
        None => "🧩",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dto(id: &str, ptype: &str, modalities: &[&str]) -> PresetProviderDto {
        PresetProviderDto {
            id: id.to_string(),
            provider_type: ptype.to_string(),
            default_model: "m".to_string(),
            base_url: None,
            display_name: id.to_string(),
            modalities: modalities.iter().map(|s| s.to_string()).collect(),
            homepage: None,
            notes: None,
        }
    }

    #[test]
    fn music_modality_maps_to_audio_category() {
        let p = dto("suno", "suno", &["music"]).into_preset();
        assert_eq!(p.capabilities, vec![GenerationType::Audio]);
    }

    #[test]
    fn unknown_provider_type_falls_back_to_modality_color() {
        let p = dto("custom", "weirdo", &["image"]).into_preset();
        assert_eq!(p.color, "#6366F1");
        assert_eq!(p.icon, "🖼️");
    }

    #[test]
    fn catalog_by_category_filters() {
        let cat = PresetCatalog::from_dtos(vec![
            dto("a", "openai", &["image"]),
            dto("b", "elevenlabs", &["speech"]),
            dto("c", "suno", &["music"]),
        ]);
        assert_eq!(cat.by_category(GenerationType::Image).len(), 1);
        assert_eq!(cat.by_category(GenerationType::Audio).len(), 1);
        assert_eq!(cat.by_category(GenerationType::Video).len(), 0);
    }

    #[test]
    fn catalog_find_and_is_preset() {
        let cat = PresetCatalog::from_dtos(vec![dto("openai-dalle", "openai", &["image"])]);
        assert!(cat.is_preset("openai-dalle"));
        assert!(!cat.is_preset("missing"));
        assert!(cat.find("openai-dalle").is_some());
    }

    #[test]
    fn curated_color_for_known_provider() {
        let p = dto("o", "openai", &["image"]).into_preset();
        assert_eq!(p.color, "#10a37f");
    }

    #[test]
    fn homepage_propagates_from_dto() {
        let mut d = dto("openai-dalle", "openai", &["image"]);
        d.homepage = Some("https://platform.openai.com".to_string());
        let p = d.into_preset();
        assert_eq!(p.homepage.as_deref(), Some("https://platform.openai.com"));
    }
}
