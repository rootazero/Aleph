//! Per-preset rich metadata — display name, modalities, homepage.

use once_cell::sync::Lazy;
use std::collections::HashMap;

use crate::providers::metadata::{Modality, ProviderMetadata};

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

    // -------------------------------------------------------------------------
    // Phase R2 metadata for the new presets above.
    // -------------------------------------------------------------------------

    // Image — direct vendors
    m.insert(
        "ideogram",
        ProviderMetadata {
            display_name: "Ideogram",
            modalities: IMAGE_ONLY,
            homepage: Some("https://developer.ideogram.ai"),
            notes: Some("Strong on text-in-image"),
        },
    );
    m.insert(
        "recraft",
        ProviderMetadata {
            display_name: "Recraft v3",
            modalities: IMAGE_ONLY,
            homepage: Some("https://www.recraft.ai/docs"),
            notes: Some("Vector + raster, brand-aware"),
        },
    );
    m.insert(
        "bytedance-seedream",
        ProviderMetadata {
            display_name: "ByteDance Seedream / 即梦图像",
            modalities: IMAGE_ONLY,
            homepage: Some("https://www.volcengine.com/docs/82379"),
            notes: Some("Volcengine Ark image endpoint"),
        },
    );
    m.insert(
        "xai-image",
        ProviderMetadata {
            display_name: "xAI Grok Image",
            modalities: IMAGE_ONLY,
            homepage: Some("https://docs.x.ai/docs/guides/image-generation"),
            notes: None,
        },
    );
    // Image — fal-hosted alternates
    m.insert(
        "fal-recraft",
        ProviderMetadata {
            display_name: "Recraft v3 (via Fal)",
            modalities: IMAGE_ONLY,
            homepage: Some("https://fal.ai/models/fal-ai/recraft-v3"),
            notes: None,
        },
    );
    m.insert(
        "fal-ideogram",
        ProviderMetadata {
            display_name: "Ideogram v2 (via Fal)",
            modalities: IMAGE_ONLY,
            homepage: Some("https://fal.ai/models/fal-ai/ideogram/v2"),
            notes: None,
        },
    );

    // Video extras
    m.insert(
        "fal-veo",
        ProviderMetadata {
            display_name: "Google Veo 3 (via Fal)",
            modalities: VIDEO_ONLY,
            homepage: Some("https://fal.ai/models/fal-ai/veo3"),
            notes: None,
        },
    );
    m.insert(
        "fal-wan",
        ProviderMetadata {
            display_name: "Wan 2 (via Fal) / 通义万相",
            modalities: VIDEO_ONLY,
            homepage: Some("https://fal.ai/models/fal-ai/wan-pro"),
            notes: Some("Alibaba Wan-Pro text-to-video"),
        },
    );

    // Music
    m.insert(
        "fal-stable-audio",
        ProviderMetadata {
            display_name: "Stable Audio (via Fal)",
            modalities: &[Modality::Music],
            homepage: Some("https://fal.ai/models/fal-ai/stable-audio"),
            notes: None,
        },
    );
    m.insert(
        "fal-cassette",
        ProviderMetadata {
            display_name: "CassetteAI Music (via Fal)",
            modalities: &[Modality::Music],
            homepage: Some("https://fal.ai/models/cassetteai/music-generator"),
            notes: None,
        },
    );

    // Speech (TTS)
    m.insert(
        "minimax-tts",
        ProviderMetadata {
            display_name: "MiniMax T2A",
            modalities: SPEECH_ONLY,
            homepage: Some("https://platform.minimaxi.com"),
            notes: Some("Chinese + multilingual TTS"),
        },
    );
    m.insert(
        "deepgram-tts",
        ProviderMetadata {
            display_name: "Deepgram Aura",
            modalities: SPEECH_ONLY,
            homepage: Some("https://developers.deepgram.com/docs/tts-rest"),
            notes: Some("Aura voice family (English; voice = model name)"),
        },
    );
    m.insert(
        "azure-speech",
        ProviderMetadata {
            display_name: "Azure Speech",
            modalities: SPEECH_ONLY,
            homepage: Some("https://learn.microsoft.com/azure/ai-services/speech-service/"),
            notes: Some("400+ Neural voices, SSML; region required (e.g. eastus)"),
        },
    );

    // Phase R3-B2: Suno (music) + BFL FLUX (image) direct providers.
    m.insert(
        "suno",
        ProviderMetadata {
            display_name: "Suno",
            modalities: &[Modality::Music],
            homepage: Some("https://suno.ai"),
            notes: Some("Targets paid wrappers (sunoapi.com / sunoaiapi.com)"),
        },
    );
    m.insert(
        "bfl-flux",
        ProviderMetadata {
            display_name: "Black Forest Labs FLUX",
            modalities: IMAGE_ONLY,
            homepage: Some("https://docs.bfl.ai"),
            notes: Some("Direct BFL API; supports flux-pro-1.1, flux-dev, ultra"),
        },
    );
    // Phase R3-B3 metadata: Cartesia (TTS) + MiniMax native STT.
    m.insert(
        "cartesia",
        ProviderMetadata {
            display_name: "Cartesia Sonic",
            modalities: SPEECH_ONLY,
            homepage: Some("https://docs.cartesia.ai"),
            notes: Some("Sonic-2 multilingual TTS; voice IDs are UUIDs"),
        },
    );
    m.insert(
        "minimax-stt",
        ProviderMetadata {
            display_name: "MiniMax 语音转文本",
            modalities: &[Modality::Transcription],
            homepage: Some("https://platform.minimaxi.com"),
            notes: Some("Native /v1/audio_to_text; requires GroupId in params.extra"),
        },
    );

    // Transcription (STT) — closes the empty Transcription modality
    m.insert(
        "openai-whisper",
        ProviderMetadata {
            display_name: "OpenAI Whisper",
            modalities: &[Modality::Transcription],
            homepage: Some("https://platform.openai.com/docs/guides/speech-to-text"),
            notes: Some("whisper-1 multilingual STT"),
        },
    );
    m.insert(
        "deepgram-stt",
        ProviderMetadata {
            display_name: "Deepgram Nova",
            modalities: &[Modality::Transcription],
            homepage: Some("https://developers.deepgram.com"),
            notes: Some("Nova-3 streaming STT"),
        },
    );

    m
});
