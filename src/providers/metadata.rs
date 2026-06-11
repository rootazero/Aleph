//! Cross-modality provider metadata.
//!
//! `ProviderPreset` and `GenerationPreset` carry the minimum required to
//! actually instantiate a provider (base URL, protocol, default model). This
//! module adds the **discovery layer** on top — modality coverage, display
//! names and homepage URLs — that lets routing/UI code answer questions like
//! "which providers can do video?" or "what's the human-readable name for
//! `siliconflow`?".
//!
//! Kept deliberately small: only fields we have a concrete consumer for.
//! Pricing / context-window / capability bitmaps are intentionally omitted
//! — they live closer to the protocol adapter (which is the authoritative
//! source) and can be added here later when a routing decision demands it.

/// Coarse-grained modality classes. A provider may declare multiple.
///
/// This is the cross-cutting vocabulary shared by the chat-provider layer
/// (`src/providers/`) and the generation layer (`src/generation/`). Adding a
/// new modality here makes it visible to the catalog and any code that
/// filters providers by capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Modality {
    /// Conversational text completion (the dominant modality).
    Chat,
    /// Text → image generation (DALL-E, Imagen, SDXL, Flux, Ideogram, …).
    Image,
    /// Text/image → video generation (Veo, Sora, Runway, Luma, Kling, …).
    Video,
    /// Text → music / song generation (Suno, Udio, `MusicGen`).
    Music,
    /// Text → speech (TTS — `OpenAI` TTS, `ElevenLabs`, Azure, …).
    Speech,
    /// Audio → text (STT / transcription — Whisper, Deepgram, Scribe).
    Transcription,
    /// Text → vector embedding.
    Embedding,
    /// Document re-ranking against a query.
    Rerank,
}

impl Modality {
    /// Stable, lowercase identifier for serialization.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Image => "image",
            Self::Video => "video",
            Self::Music => "music",
            Self::Speech => "speech",
            Self::Transcription => "transcription",
            Self::Embedding => "embedding",
            Self::Rerank => "rerank",
        }
    }
}

/// Rich, optional metadata attached to a preset name.
///
/// Stored in a parallel `Lazy<HashMap>` rather than embedded in
/// `ProviderPreset` / `GenerationPreset` so the addition is zero-churn —
/// existing presets keep working without any field changes, and new
/// presets opt in by adding an entry to the metadata map.
#[derive(Debug, Clone, Copy)]
pub struct ProviderMetadata {
    /// Human-readable display name (e.g. `"DeepSeek"`, `"Volcengine Doubao"`).
    pub display_name: &'static str,
    /// Modalities this preset can serve. Must contain at least one.
    pub modalities: &'static [Modality],
    /// Vendor homepage / docs URL (for UI links).
    pub homepage: Option<&'static str>,
    /// Free-form short note shown in catalog (e.g. "OpenAI-compatible",
    /// "Anthropic-protocol variant for Vertex AI"). Kept under ~80 chars.
    pub notes: Option<&'static str>,
}

impl ProviderMetadata {
    /// True if this preset can serve the given modality.
    #[must_use]
    pub fn supports(&self, m: Modality) -> bool {
        self.modalities.contains(&m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modality_as_str_is_stable() {
        // Lock these strings — they appear in RPC / TOML output.
        assert_eq!(Modality::Chat.as_str(), "chat");
        assert_eq!(Modality::Image.as_str(), "image");
        assert_eq!(Modality::Video.as_str(), "video");
        assert_eq!(Modality::Music.as_str(), "music");
        assert_eq!(Modality::Speech.as_str(), "speech");
        assert_eq!(Modality::Transcription.as_str(), "transcription");
        assert_eq!(Modality::Embedding.as_str(), "embedding");
        assert_eq!(Modality::Rerank.as_str(), "rerank");
    }

    #[test]
    fn supports_checks_modality_membership() {
        const META: ProviderMetadata = ProviderMetadata {
            display_name: "Test",
            modalities: &[Modality::Chat, Modality::Image],
            homepage: None,
            notes: None,
        };
        assert!(META.supports(Modality::Chat));
        assert!(META.supports(Modality::Image));
        assert!(!META.supports(Modality::Video));
        assert!(!META.supports(Modality::Embedding));
    }
}
