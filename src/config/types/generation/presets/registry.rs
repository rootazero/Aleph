//! Static registry of built-in generation provider presets.
//!
//! Single source of truth is `PROFILES` — one entry per preset, with
//! display/modalities/homepage carried inline on each `GenerationPreset`.
//! `PRESETS` and `GENERATION_METADATA` are both derived `Lazy<HashMap>`
//! views so historical callers keep working without code changes.

use once_cell::sync::Lazy;
use std::collections::HashMap;

use super::GenerationPreset;
use crate::providers::metadata::{Modality, ProviderMetadata};

// Modality shorthand slices — match the chat-side convention.
const IMAGE: &[Modality] = &[Modality::Image];
const VIDEO: &[Modality] = &[Modality::Video];
const MUSIC: &[Modality] = &[Modality::Music];
const SPEECH: &[Modality] = &[Modality::Speech];
const TRANSCRIPTION: &[Modality] = &[Modality::Transcription];
const IMAGE_VIDEO_MUSIC: &[Modality] = &[Modality::Image, Modality::Video, Modality::Music];

/// Canonical generation profiles. Each row carries everything needed to
/// list, instantiate and link to the preset — the factory uses the first
/// three fields; the panel/RPC catalog uses the rest.
const PROFILES: &[(&str, GenerationPreset)] = &[
    // ─── Image ────────────────────────────────────────────────────────────────
    (
        "openai-dalle",
        GenerationPreset::new("openai", "dall-e-3", Some("https://api.openai.com"))
            .with_modalities(IMAGE)
            .with_display("OpenAI DALL-E")
            .with_homepage("https://platform.openai.com/docs/guides/images")
            .with_signup("https://platform.openai.com/api-keys"),
    ),
    (
        "stability-ai",
        GenerationPreset::new(
            "stability",
            "stable-diffusion-xl-1024-v1-0",
            Some("https://api.stability.ai"),
        )
        .with_modalities(IMAGE)
        .with_display("Stability AI")
        .with_description("Stable Diffusion XL family")
        .with_homepage("https://platform.stability.ai")
        .with_signup("https://platform.stability.ai/account/keys"),
    ),
    (
        "google-imagen",
        GenerationPreset::new("google", "imagen-3.0-generate-002", None)
            .with_modalities(IMAGE)
            .with_display("Google Imagen")
            .with_homepage("https://ai.google.dev/gemini-api/docs/imagen")
            .with_signup("https://aistudio.google.com/app/apikey"),
    ),
    (
        "ideogram",
        GenerationPreset::new(
            "openai_compat",
            "ideogram-v2-turbo",
            Some("https://api.ideogram.ai/openai/v1/images/generations"),
        )
        .with_modalities(IMAGE)
        .with_display("Ideogram")
        .with_description("Strong on text-in-image")
        .with_homepage("https://developer.ideogram.ai")
        .with_signup("https://ideogram.ai/manage-api"),
    ),
    (
        "recraft",
        GenerationPreset::new(
            "openai_compat",
            "recraftv3",
            Some("https://external.api.recraft.ai/v1/images/generations"),
        )
        .with_modalities(IMAGE)
        .with_display("Recraft v3")
        .with_description("Vector + raster, brand-aware")
        .with_homepage("https://www.recraft.ai/docs")
        .with_signup("https://www.recraft.ai/profile/api"),
    ),
    (
        "bytedance-seedream",
        GenerationPreset::new(
            "openai_compat",
            "doubao-seedream-3-0-t2i-250415",
            Some("https://ark.cn-beijing.volces.com/api/v3/images/generations"),
        )
        .with_modalities(IMAGE)
        .with_display("ByteDance Seedream / 即梦图像")
        .with_description("Volcengine Ark image endpoint")
        .with_homepage("https://www.volcengine.com/docs/82379")
        .with_signup("https://console.volcengine.com/ark"),
    ),
    (
        "xai-image",
        GenerationPreset::new(
            "openai_compat",
            "grok-2-image-1212",
            Some("https://api.x.ai/v1/images/generations"),
        )
        .with_modalities(IMAGE)
        .with_display("xAI Grok Image")
        .with_homepage("https://docs.x.ai/docs/guides/image-generation")
        .with_signup("https://console.x.ai"),
    ),
    (
        "bfl-flux",
        GenerationPreset::new("bfl", "flux-pro-1.1", Some("https://api.bfl.ai"))
            .with_modalities(IMAGE)
            .with_display("Black Forest Labs FLUX")
            .with_description("Direct BFL API; supports flux-pro-1.1, flux-dev, ultra")
            .with_homepage("https://docs.bfl.ai")
            .with_signup("https://docs.bfl.ai"),
    ),
    // ─── Video ────────────────────────────────────────────────────────────────
    (
        "google-veo",
        GenerationPreset::new("google_veo", "veo-2.0-generate-001", None)
            .with_modalities(VIDEO)
            .with_display("Google Veo")
            .with_homepage("https://deepmind.google/technologies/veo")
            .with_signup("https://aistudio.google.com/app/apikey"),
    ),
    (
        "runway",
        GenerationPreset::new(
            "fal",
            "fal-ai/runway-gen3/turbo/image-to-video",
            Some("https://queue.fal.run"),
        )
        .with_modalities(VIDEO)
        .with_display("Runway Gen-3")
        .with_homepage("https://docs.dev.runwayml.com")
        .with_signup("https://fal.ai/dashboard/keys"),
    ),
    (
        "pika",
        GenerationPreset::new(
            "fal",
            "fal-ai/pika/v2.2/text-to-video",
            Some("https://queue.fal.run"),
        )
        .with_modalities(VIDEO)
        .with_display("Pika Labs")
        .with_homepage("https://pika.art")
        .with_signup("https://fal.ai/dashboard/keys"),
    ),
    // ─── Replicate (multi-modal hub) ──────────────────────────────────────────
    (
        "replicate",
        GenerationPreset::new(
            "replicate",
            "black-forest-labs/flux-schnell",
            Some("https://api.replicate.com"),
        )
        .with_modalities(IMAGE_VIDEO_MUSIC)
        .with_display("Replicate")
        .with_description("OSS model hosting (Flux, SDXL, MusicGen, …)")
        .with_homepage("https://replicate.com")
        .with_signup("https://replicate.com/account/api-tokens"),
    ),
    // ─── Fal.ai aggregator family ─────────────────────────────────────────────
    (
        "fal",
        GenerationPreset::new("fal", "fal-ai/flux/dev", Some("https://queue.fal.run"))
            .with_modalities(IMAGE_VIDEO_MUSIC)
            .with_display("Fal.ai")
            .with_description("Aggregator — image/video/music via queue API")
            .with_homepage("https://fal.ai")
            .with_signup("https://fal.ai/dashboard/keys"),
    ),
    (
        "fal-flux",
        GenerationPreset::new("fal", "fal-ai/flux/dev", Some("https://queue.fal.run"))
            .with_modalities(IMAGE)
            .with_display("Flux (via Fal)")
            .with_homepage("https://fal.ai/models/fal-ai/flux/dev")
            .with_signup("https://fal.ai/dashboard/keys"),
    ),
    (
        "fal-recraft",
        GenerationPreset::new("fal", "fal-ai/recraft-v3", Some("https://queue.fal.run"))
            .with_modalities(IMAGE)
            .with_display("Recraft v3 (via Fal)")
            .with_homepage("https://fal.ai/models/fal-ai/recraft-v3")
            .with_signup("https://fal.ai/dashboard/keys"),
    ),
    (
        "fal-ideogram",
        GenerationPreset::new("fal", "fal-ai/ideogram/v2", Some("https://queue.fal.run"))
            .with_modalities(IMAGE)
            .with_display("Ideogram v2 (via Fal)")
            .with_homepage("https://fal.ai/models/fal-ai/ideogram/v2")
            .with_signup("https://fal.ai/dashboard/keys"),
    ),
    (
        "fal-kling-video",
        GenerationPreset::new(
            "fal",
            "fal-ai/kling-video/v1/pro/text-to-video",
            Some("https://queue.fal.run"),
        )
        .with_modalities(VIDEO)
        .with_display("Kling Video (via Fal) / 可灵")
        .with_homepage("https://fal.ai/models/fal-ai/kling-video")
        .with_signup("https://fal.ai/dashboard/keys"),
    ),
    (
        "fal-luma",
        GenerationPreset::new(
            "fal",
            "fal-ai/luma-dream-machine",
            Some("https://queue.fal.run"),
        )
        .with_modalities(VIDEO)
        .with_display("Luma Dream Machine (via Fal)")
        .with_homepage("https://fal.ai/models/fal-ai/luma-dream-machine")
        .with_signup("https://fal.ai/dashboard/keys"),
    ),
    (
        "fal-runway",
        GenerationPreset::new(
            "fal",
            "fal-ai/runway-gen3/turbo/image-to-video",
            Some("https://queue.fal.run"),
        )
        .with_modalities(VIDEO)
        .with_display("Runway Gen-3 (via Fal)")
        .with_homepage("https://fal.ai/models/fal-ai/runway-gen3")
        .with_signup("https://fal.ai/dashboard/keys"),
    ),
    (
        "fal-pika",
        GenerationPreset::new(
            "fal",
            "fal-ai/pika/v2.2/text-to-video",
            Some("https://queue.fal.run"),
        )
        .with_modalities(VIDEO)
        .with_display("Pika (via Fal)")
        .with_homepage("https://fal.ai/models/fal-ai/pika")
        .with_signup("https://fal.ai/dashboard/keys"),
    ),
    (
        "fal-hailuo",
        GenerationPreset::new(
            "fal",
            "fal-ai/minimax/video-01",
            Some("https://queue.fal.run"),
        )
        .with_modalities(VIDEO)
        .with_display("Hailuo (via Fal) / 海螺")
        .with_description("MiniMax video model")
        .with_homepage("https://fal.ai/models/fal-ai/minimax/video-01")
        .with_signup("https://fal.ai/dashboard/keys"),
    ),
    (
        "fal-jimeng",
        GenerationPreset::new(
            "fal",
            "fal-ai/bytedance/seedance-1-pro",
            Some("https://queue.fal.run"),
        )
        .with_modalities(VIDEO)
        .with_display("Jimeng / Seedance (via Fal) / 即梦")
        .with_description("ByteDance Seedance")
        .with_homepage("https://fal.ai/models/fal-ai/bytedance/seedance-1-pro")
        .with_signup("https://fal.ai/dashboard/keys"),
    ),
    (
        "fal-veo",
        GenerationPreset::new("fal", "fal-ai/veo3", Some("https://queue.fal.run"))
            .with_modalities(VIDEO)
            .with_display("Google Veo 3 (via Fal)")
            .with_homepage("https://fal.ai/models/fal-ai/veo3")
            .with_signup("https://fal.ai/dashboard/keys"),
    ),
    (
        "fal-wan",
        GenerationPreset::new(
            "fal",
            "fal-ai/wan-pro/text-to-video",
            Some("https://queue.fal.run"),
        )
        .with_modalities(VIDEO)
        .with_display("Wan 2 (via Fal) / 通义万相")
        .with_description("Alibaba Wan-Pro text-to-video")
        .with_homepage("https://fal.ai/models/fal-ai/wan-pro")
        .with_signup("https://fal.ai/dashboard/keys"),
    ),
    // ─── Music ────────────────────────────────────────────────────────────────
    (
        "fal-musicgen",
        GenerationPreset::new("fal", "fal-ai/musicgen", Some("https://queue.fal.run"))
            .with_modalities(MUSIC)
            .with_display("MusicGen (via Fal)")
            .with_homepage("https://fal.ai/models/fal-ai/musicgen")
            .with_signup("https://fal.ai/dashboard/keys"),
    ),
    (
        "fal-stable-audio",
        GenerationPreset::new("fal", "fal-ai/stable-audio", Some("https://queue.fal.run"))
            .with_modalities(MUSIC)
            .with_display("Stable Audio (via Fal)")
            .with_homepage("https://fal.ai/models/fal-ai/stable-audio")
            .with_signup("https://fal.ai/dashboard/keys"),
    ),
    (
        "fal-cassette",
        GenerationPreset::new(
            "fal",
            "cassetteai/music-generator",
            Some("https://queue.fal.run"),
        )
        .with_modalities(MUSIC)
        .with_display("CassetteAI Music (via Fal)")
        .with_homepage("https://fal.ai/models/cassetteai/music-generator")
        .with_signup("https://fal.ai/dashboard/keys"),
    ),
    (
        "suno",
        GenerationPreset::new("suno", "chirp-v3-5", Some("https://api.sunoaiapi.com"))
            .with_modalities(MUSIC)
            .with_display("Suno")
            .with_description("Targets paid wrappers (sunoapi.com / sunoaiapi.com)")
            .with_homepage("https://suno.ai")
            .with_signup("https://sunoapi.com"),
    ),
    // ─── Speech (TTS) ─────────────────────────────────────────────────────────
    (
        "openai-tts",
        GenerationPreset::new("openai_tts", "tts-1-hd", Some("https://api.openai.com"))
            .with_modalities(SPEECH)
            .with_display("OpenAI TTS")
            .with_homepage("https://platform.openai.com/docs/guides/text-to-speech")
            .with_signup("https://platform.openai.com/api-keys"),
    ),
    (
        "elevenlabs",
        GenerationPreset::new(
            "elevenlabs",
            "eleven_multilingual_v2",
            Some("https://api.elevenlabs.io"),
        )
        .with_modalities(SPEECH)
        .with_display("ElevenLabs")
        .with_homepage("https://elevenlabs.io")
        .with_signup("https://elevenlabs.io/app/settings/api-keys"),
    ),
    (
        "minimax-tts",
        GenerationPreset::new(
            "openai_compat",
            "speech-01-turbo",
            Some("https://api.minimax.chat/v1/t2a_v2"),
        )
        .with_modalities(SPEECH)
        .with_display("MiniMax T2A")
        .with_description("Chinese + multilingual TTS")
        .with_homepage("https://platform.minimaxi.com")
        .with_signup("https://platform.minimaxi.com"),
    ),
    (
        "deepgram-tts",
        GenerationPreset::new(
            "deepgram_tts",
            "aura-asteria-en",
            Some("https://api.deepgram.com"),
        )
        .with_modalities(SPEECH)
        .with_display("Deepgram Aura")
        .with_description("Aura voice family (English; voice = model name)")
        .with_homepage("https://developers.deepgram.com/docs/tts-rest")
        .with_signup("https://console.deepgram.com/signup"),
    ),
    (
        "azure-speech",
        GenerationPreset::new("azure_speech", "en-US-JennyNeural", None)
            .with_modalities(SPEECH)
            .with_display("Azure Speech")
            .with_description("400+ Neural voices, SSML; region required (e.g. eastus)")
            .with_homepage("https://learn.microsoft.com/azure/ai-services/speech-service/")
            .with_signup("https://portal.azure.com"),
    ),
    (
        "cartesia",
        GenerationPreset::new("cartesia", "sonic-2", Some("https://api.cartesia.ai"))
            .with_modalities(SPEECH)
            .with_display("Cartesia Sonic")
            .with_description("Sonic-2 multilingual TTS; voice IDs are UUIDs")
            .with_homepage("https://docs.cartesia.ai")
            .with_signup("https://play.cartesia.ai"),
    ),
    // ─── Transcription (STT) ──────────────────────────────────────────────────
    (
        "openai-whisper",
        GenerationPreset::new("openai_whisper", "whisper-1", Some("https://api.openai.com"))
            .with_modalities(TRANSCRIPTION)
            .with_display("OpenAI Whisper")
            .with_description("whisper-1 multilingual STT")
            .with_homepage("https://platform.openai.com/docs/guides/speech-to-text")
            .with_signup("https://platform.openai.com/api-keys"),
    ),
    (
        "deepgram-stt",
        GenerationPreset::new("deepgram_stt", "nova-3", Some("https://api.deepgram.com"))
            .with_modalities(TRANSCRIPTION)
            .with_display("Deepgram Nova")
            .with_description("Nova-3 streaming STT")
            .with_homepage("https://developers.deepgram.com")
            .with_signup("https://console.deepgram.com/signup"),
    ),
    (
        "minimax-stt",
        GenerationPreset::new("minimax_stt", "whisper-1", Some("https://api.minimaxi.com"))
            .with_modalities(TRANSCRIPTION)
            .with_display("MiniMax 语音转文本")
            .with_description("Native /v1/audio_to_text; requires GroupId in params.extra")
            .with_homepage("https://platform.minimaxi.com")
            .with_signup("https://platform.minimaxi.com"),
    ),
];

/// Registry of known generation provider presets, keyed by preset ID.
pub static PRESETS: Lazy<HashMap<&'static str, GenerationPreset>> = Lazy::new(|| {
    let mut m = HashMap::with_capacity(PROFILES.len());
    for (name, preset) in PROFILES {
        m.insert(*name, *preset);
    }
    m
});

/// Backwards-compatible `ProviderMetadata` view derived from `PROFILES`.
///
/// Same shape as the chat-side `PRESET_METADATA`: every field reads off
/// the preset, so the parallel `metadata.rs` map is no longer maintained
/// by hand (Phase 2 entropy reduction).
pub static GENERATION_METADATA: Lazy<HashMap<&'static str, ProviderMetadata>> = Lazy::new(|| {
    let mut m: HashMap<&'static str, ProviderMetadata> = HashMap::with_capacity(PROFILES.len());
    for (name, preset) in PROFILES {
        m.insert(
            *name,
            ProviderMetadata {
                display_name: preset.display_name.unwrap_or(name),
                modalities: preset.modalities,
                homepage: preset.homepage,
                notes: preset.description,
            },
        );
    }
    m
});
