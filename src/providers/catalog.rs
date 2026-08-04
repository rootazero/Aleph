//! Unified provider catalog.
//!
//! Aleph keeps chat providers in `src/providers/` and generation providers
//! in `src/generation/` (the two layers have very different traits and
//! request types — keeping them separate is intentional). This module
//! gives the rest of the codebase **one place** to ask "what providers are
//! available, and what modalities can they serve?" without having to know
//! which layer hosts which.
//!
//! The catalog is purely metadata — it doesn't instantiate providers and
//! has no I/O. Use it for:
//!
//! * Panel / RPC listings (`/providers/catalog`).
//! * Modality-aware routing ("pick any video provider").
//! * "What can this Aleph install do?" introspection.

use crate::config::types::generation::presets as gen_presets;
use crate::providers::metadata::{Modality, ProviderMetadata};
use crate::providers::presets as chat_presets;

/// Which side of the provider house an entry lives on.
///
/// Stays as a coarse enum on purpose — finer routing (e.g. "is this a
/// chat-completion or chat-stream") belongs at the protocol-adapter layer,
/// not at the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogKind {
    /// LLM chat / text generation (`src/providers/`).
    Chat,
    /// Image / video / audio generation (`src/generation/`).
    Generation,
}

/// One row in the unified catalog.
#[derive(Debug, Clone)]
pub struct CatalogEntry {
    /// Preset name (the key used to look it up).
    pub name: &'static str,
    /// Which subsystem hosts this preset.
    pub kind: CatalogKind,
    /// Optional rich metadata. `None` means "no entry registered" —
    /// chat presets default to `Modality::Chat` in that case.
    pub metadata: Option<&'static ProviderMetadata>,
}

impl CatalogEntry {
    /// True if this preset can serve the requested modality.
    #[must_use]
    pub fn supports(&self, modality: Modality) -> bool {
        match self.metadata {
            Some(meta) => meta.supports(modality),
            // No metadata recorded: a chat-side entry defaults to Chat
            // (historical behaviour). Generation-side requires explicit
            // metadata, so the absence means "no" — matches
            // `generation_presets_by_modality` semantics.
            None => self.kind == CatalogKind::Chat && modality == Modality::Chat,
        }
    }
}

/// All presets across both subsystems, sorted by (kind, name).
///
/// Chat entries come first, then generation entries; within each group
/// names are sorted alphabetically for deterministic UI ordering.
pub fn all_presets() -> Vec<CatalogEntry> {
    // Canonical profiles only — aliases are resolution keys, not display
    // rows (`kimi` must not render a second row next to `moonshot`).
    let mut chat: Vec<CatalogEntry> = chat_presets::canonical_profiles()
        .iter()
        .map(|(name, _)| CatalogEntry {
            name,
            kind: CatalogKind::Chat,
            metadata: chat_presets::provider_metadata(name),
        })
        .collect();
    chat.sort_unstable_by_key(|e| e.name);

    let mut gen: Vec<CatalogEntry> = gen_presets::PRESETS
        .keys()
        .map(|name| CatalogEntry {
            name,
            kind: CatalogKind::Generation,
            metadata: gen_presets::generation_metadata(name),
        })
        .collect();
    gen.sort_unstable_by_key(|e| e.name);

    chat.extend(gen);
    chat
}

/// All presets across both subsystems that can serve a given modality.
///
/// Equivalent to `all_presets().into_iter().filter(|e| e.supports(m))` but
/// implemented directly so callers don't have to materialise the full
/// catalog first.
#[must_use]
pub fn presets_for_modality(modality: Modality) -> Vec<CatalogEntry> {
    let mut out = Vec::new();

    // Chat side — uses the chat default-to-Chat convention.
    let mut chat: Vec<CatalogEntry> = chat_presets::presets_by_modality(modality)
        .into_iter()
        .map(|name| CatalogEntry {
            name,
            kind: CatalogKind::Chat,
            metadata: chat_presets::provider_metadata(name),
        })
        .collect();
    chat.sort_unstable_by_key(|e| e.name);
    out.append(&mut chat);

    // Generation side — explicit metadata only.
    let mut gen: Vec<CatalogEntry> = gen_presets::generation_presets_by_modality(modality)
        .into_iter()
        .map(|name| CatalogEntry {
            name,
            kind: CatalogKind::Generation,
            metadata: gen_presets::generation_metadata(name),
        })
        .collect();
    gen.sort_unstable_by_key(|e| e.name);
    out.append(&mut gen);

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_presets_contains_both_sides() {
        let entries = all_presets();
        // Chat side
        assert!(entries
            .iter()
            .any(|e| e.name == "openai" && e.kind == CatalogKind::Chat));
        // Phase B chat additions
        assert!(entries
            .iter()
            .any(|e| e.name == "qwen" && e.kind == CatalogKind::Chat));
        // Generation side
        assert!(entries
            .iter()
            .any(|e| e.name == "openai-dalle" && e.kind == CatalogKind::Generation));
        // Phase C Fal family
        assert!(entries
            .iter()
            .any(|e| e.name == "fal-kling-video" && e.kind == CatalogKind::Generation));
    }

    #[test]
    fn presets_for_modality_video_only_returns_generation_video_presets() {
        let videos = presets_for_modality(Modality::Video);
        // Pure video presets we shipped in Phase C
        for needed in [
            "fal-kling-video",
            "fal-luma",
            "fal-runway",
            "fal-pika",
            "fal-hailuo",
            "fal-jimeng",
            "google-veo",
        ] {
            assert!(
                videos.iter().any(|e| e.name == needed),
                "modality Video should include {needed}, got {:?}",
                videos.iter().map(|e| e.name).collect::<Vec<_>>()
            );
        }
        // No chat presets advertise video
        assert!(videos.iter().all(|e| e.kind == CatalogKind::Generation));
    }

    #[test]
    fn presets_for_modality_chat_is_chat_only() {
        let chats = presets_for_modality(Modality::Chat);
        assert!(!chats.is_empty());
        assert!(chats.iter().all(|e| e.kind == CatalogKind::Chat));
    }

    #[test]
    fn presets_for_modality_music_includes_fal_musicgen() {
        let music = presets_for_modality(Modality::Music);
        assert!(music.iter().any(|e| e.name == "fal-musicgen"));
        // replicate is multimodality (image+video+music)
        assert!(music.iter().any(|e| e.name == "replicate"));
    }

    #[test]
    fn catalog_entry_supports_uses_metadata_when_present() {
        let e = presets_for_modality(Modality::Music)
            .into_iter()
            .find(|e| e.name == "fal-musicgen")
            .unwrap();
        assert!(e.supports(Modality::Music));
        assert!(!e.supports(Modality::Video));
    }

    #[test]
    fn catalog_entry_supports_default_chat_for_chat_without_metadata() {
        // Synthesise an entry to test the no-metadata branch.
        let e = CatalogEntry {
            name: "synthetic",
            kind: CatalogKind::Chat,
            metadata: None,
        };
        assert!(e.supports(Modality::Chat));
        assert!(!e.supports(Modality::Image));
    }

    #[test]
    fn presets_for_modality_speech_includes_round3_tts() {
        // Round-3 (Phase B1) adds dedicated Deepgram Aura TTS + Azure Speech.
        let tts = presets_for_modality(Modality::Speech);
        for needed in ["deepgram-tts", "azure-speech"] {
            assert!(
                tts.iter().any(|e| e.name == needed),
                "Speech modality should include {needed}, got {:?}",
                tts.iter().map(|e| e.name).collect::<Vec<_>>()
            );
        }
        // All Speech presets live on the generation side.
        assert!(tts.iter().all(|e| e.kind == CatalogKind::Generation));
    }

    #[test]
    fn presets_for_modality_music_includes_round3_suno() {
        // Round-3 (Phase B2) adds Suno music generation.
        let music = presets_for_modality(Modality::Music);
        assert!(
            music.iter().any(|e| e.name == "suno"),
            "Music modality should expose suno, got {:?}",
            music.iter().map(|e| e.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn presets_for_modality_image_includes_round3_bfl() {
        // Round-3 (Phase B2) adds direct BFL FLUX image generation.
        let imgs = presets_for_modality(Modality::Image);
        assert!(
            imgs.iter().any(|e| e.name == "bfl-flux"),
            "Image modality should expose bfl-flux, got {:?}",
            imgs.iter().map(|e| e.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn presets_for_modality_speech_includes_round3_b3_cartesia() {
        // Round-3 (Phase B3) adds Cartesia Sonic TTS.
        let tts = presets_for_modality(Modality::Speech);
        assert!(
            tts.iter().any(|e| e.name == "cartesia"),
            "Speech modality should expose cartesia, got {:?}",
            tts.iter().map(|e| e.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn presets_for_modality_transcription_includes_round2_stt() {
        // Round-2 closes the previously empty Transcription modality.
        let stt = presets_for_modality(Modality::Transcription);
        assert!(
            stt.iter().any(|e| e.name == "openai-whisper"),
            "Transcription modality should expose openai-whisper, got {:?}",
            stt.iter().map(|e| e.name).collect::<Vec<_>>()
        );
        assert!(
            stt.iter().any(|e| e.name == "deepgram-stt"),
            "Transcription modality should expose deepgram-stt, got {:?}",
            stt.iter().map(|e| e.name).collect::<Vec<_>>()
        );
        // Pure STT presets — no chat side leaks through.
        assert!(stt.iter().all(|e| e.kind == CatalogKind::Generation));
    }

    #[test]
    fn presets_for_modality_image_includes_round2_vendors() {
        let imgs = presets_for_modality(Modality::Image);
        for needed in ["ideogram", "recraft", "bytedance-seedream", "xai-image"] {
            assert!(
                imgs.iter().any(|e| e.name == needed),
                "Image modality should include {needed}, got {:?}",
                imgs.iter().map(|e| e.name).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn presets_for_modality_chat_includes_round2_vendors() {
        let chats = presets_for_modality(Modality::Chat);
        for needed in [
            "amazon-bedrock",
            "cloudflare-ai",
            "deepinfra",
            "github-copilot",
            "lmstudio",
            "litellm",
            "nvidia-nim",
            "inflection",
            "novita",
            "chutes",
        ] {
            assert!(
                chats.iter().any(|e| e.name == needed),
                "Chat modality should include {needed}",
            );
        }
    }
}
