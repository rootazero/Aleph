//! Static registry of built-in generation provider presets.

use once_cell::sync::Lazy;
use std::collections::HashMap;

use super::GenerationPreset;

/// Registry of known generation provider presets, keyed by preset ID
pub static PRESETS: Lazy<HashMap<&'static str, GenerationPreset>> = Lazy::new(|| {
    let mut m = HashMap::new();

    // Image providers
    m.insert(
        "openai-dalle",
        GenerationPreset {
            provider_type: "openai",
            default_model: "dall-e-3",
            base_url: Some("https://api.openai.com"),
        },
    );
    m.insert(
        "stability-ai",
        GenerationPreset {
            provider_type: "stability",
            default_model: "stable-diffusion-xl-1024-v1-0",
            base_url: Some("https://api.stability.ai"),
        },
    );
    m.insert(
        "google-imagen",
        GenerationPreset {
            provider_type: "google",
            default_model: "imagen-3.0-generate-002",
            base_url: None,
        },
    );
    m.insert(
        "replicate",
        GenerationPreset {
            provider_type: "replicate",
            default_model: "black-forest-labs/flux-schnell",
            base_url: Some("https://api.replicate.com"),
        },
    );

    // Video providers
    m.insert(
        "google-veo",
        GenerationPreset {
            provider_type: "google_veo",
            default_model: "veo-2.0-generate-001",
            base_url: None,
        },
    );
    // Runway / Pika are reached via the Fal aggregator (no separate native
    // provider_type — factory has no handler for "runway"/"pika"). Preset
    // names preserved for backward compat; Phase F (entropy reduction).
    m.insert(
        "runway",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/runway-gen3/turbo/image-to-video",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "pika",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/pika/v2.2/text-to-video",
            base_url: Some("https://queue.fal.run"),
        },
    );

    // -------------------------------------------------------------------------
    // Phase C — Fal.ai aggregator. One provider, many model endpoints. Each
    // preset below points at a Fal-hosted model and enables the right
    // modality. `capabilities` on the GenerationProviderConfig (set via
    // user config.toml or the factory default) controls which generation
    // type the preset will accept.
    // -------------------------------------------------------------------------
    m.insert(
        "fal",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/flux/dev",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-flux",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/flux/dev",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-kling-video",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/kling-video/v1/pro/text-to-video",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-luma",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/luma-dream-machine",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-runway",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/runway-gen3/turbo/image-to-video",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-pika",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/pika/v2.2/text-to-video",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-hailuo",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/minimax/video-01",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-jimeng",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/bytedance/seedance-1-pro",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-musicgen",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/musicgen",
            base_url: Some("https://queue.fal.run"),
        },
    );
    // Phase R3-B2 — Suno direct (paid wrapper), Music modality.
    m.insert(
        "suno",
        GenerationPreset {
            provider_type: "suno",
            default_model: "chirp-v3-5",
            base_url: Some("https://api.sunoaiapi.com"),
        },
    );
    // Phase R3-B2 — BFL FLUX direct, Image modality.
    m.insert(
        "bfl-flux",
        GenerationPreset {
            provider_type: "bfl",
            default_model: "flux-pro-1.1",
            base_url: Some("https://api.bfl.ai"),
        },
    );
    // Phase R3-B3 — Cartesia Sonic TTS (Speech) + MiniMax native STT
    // (Transcription). Both have non-OpenAI wire shapes (nested voice
    // selector / base_resp envelope) and so live in dedicated modules.
    m.insert(
        "cartesia",
        GenerationPreset {
            provider_type: "cartesia",
            default_model: "sonic-2",
            base_url: Some("https://api.cartesia.ai"),
        },
    );
    m.insert(
        "minimax-stt",
        GenerationPreset {
            provider_type: "minimax_stt",
            default_model: "whisper-1",
            base_url: Some("https://api.minimaxi.com"),
        },
    );

    // Audio/Speech providers
    m.insert(
        "openai-tts",
        GenerationPreset {
            provider_type: "openai_tts",
            default_model: "tts-1-hd",
            base_url: Some("https://api.openai.com"),
        },
    );
    m.insert(
        "elevenlabs",
        GenerationPreset {
            provider_type: "elevenlabs",
            default_model: "eleven_multilingual_v2",
            base_url: Some("https://api.elevenlabs.io"),
        },
    );

    // -------------------------------------------------------------------------
    // Phase R2 — additional generation presets. Each entry pins:
    // 1. provider_type pointing to an existing factory branch (openai_compat
    //    for OpenAI-shape APIs, fal for fal.ai-hosted models, etc.)
    // 2. default_model — the upstream model identifier
    // 3. base_url — fully-formed endpoint URL (path included for openai_compat)
    //
    // No new provider modules are added here — these presets re-use the
    // existing openai_compat and fal aggregator infrastructure (R3 core
    // minimalism). New vendors that don't fit either pattern (Azure Speech,
    // Cartesia, Deepgram TTS, etc.) need dedicated provider modules and
    // are out of scope for round-2 presets.
    // -------------------------------------------------------------------------

    // Image — OpenAI-compatible vendors
    m.insert(
        "ideogram",
        GenerationPreset {
            provider_type: "openai_compat",
            default_model: "ideogram-v2-turbo",
            base_url: Some("https://api.ideogram.ai/openai/v1/images/generations"),
        },
    );
    m.insert(
        "recraft",
        GenerationPreset {
            provider_type: "openai_compat",
            default_model: "recraftv3",
            base_url: Some("https://external.api.recraft.ai/v1/images/generations"),
        },
    );
    m.insert(
        "bytedance-seedream",
        GenerationPreset {
            provider_type: "openai_compat",
            default_model: "doubao-seedream-3-0-t2i-250415",
            base_url: Some("https://ark.cn-beijing.volces.com/api/v3/images/generations"),
        },
    );
    m.insert(
        "xai-image",
        GenerationPreset {
            provider_type: "openai_compat",
            default_model: "grok-2-image-1212",
            base_url: Some("https://api.x.ai/v1/images/generations"),
        },
    );
    // Note: OpenAI's `gpt-image-1` is reachable via the existing `openai-dalle`
    // preset by overriding `model = "gpt-image-1"` at the user config layer —
    // no separate preset is needed and adding one would break
    // `get_preset_by_type("openai")`'s single-canonical-preset contract.

    // Image — Fal-hosted alternates (some vendors are available both direct
    // and via Fal; the fal-* alias is for users who want unified billing).
    m.insert(
        "fal-recraft",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/recraft-v3",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-ideogram",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/ideogram/v2",
            base_url: Some("https://queue.fal.run"),
        },
    );

    // Video — Fal-hosted extras
    m.insert(
        "fal-veo",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/veo3",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-wan",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/wan-pro/text-to-video",
            base_url: Some("https://queue.fal.run"),
        },
    );

    // Music — Fal hosts Stable Audio + other music models
    m.insert(
        "fal-stable-audio",
        GenerationPreset {
            provider_type: "fal",
            default_model: "fal-ai/stable-audio",
            base_url: Some("https://queue.fal.run"),
        },
    );
    m.insert(
        "fal-cassette",
        GenerationPreset {
            provider_type: "fal",
            default_model: "cassetteai/music-generator",
            base_url: Some("https://queue.fal.run"),
        },
    );

    // Speech — MiniMax T2A (OpenAI-style speech endpoint).
    m.insert(
        "minimax-tts",
        GenerationPreset {
            provider_type: "openai_compat",
            default_model: "speech-01-turbo",
            base_url: Some("https://api.minimax.chat/v1/t2a_v2"),
        },
    );
    // Speech — Deepgram Aura TTS (Phase R3-B1, dedicated provider).
    m.insert(
        "deepgram-tts",
        GenerationPreset {
            provider_type: "deepgram_tts",
            default_model: "aura-asteria-en",
            base_url: Some("https://api.deepgram.com"),
        },
    );
    // Speech — Azure Cognitive Services (Phase R3-B1, dedicated provider).
    // `base_url` is intentionally None — Azure has no global endpoint; the
    // user must supply a region (e.g. `eastus`) or a full regional URL.
    m.insert(
        "azure-speech",
        GenerationPreset {
            provider_type: "azure_speech",
            default_model: "en-US-JennyNeural",
            base_url: None,
        },
    );

    // Transcription — endpoints filled in by the new whisper / deepgram-stt
    // providers (Phase R2-B).
    m.insert(
        "openai-whisper",
        GenerationPreset {
            provider_type: "openai_whisper",
            default_model: "whisper-1",
            base_url: Some("https://api.openai.com"),
        },
    );
    m.insert(
        "deepgram-stt",
        GenerationPreset {
            provider_type: "deepgram_stt",
            default_model: "nova-3",
            base_url: Some("https://api.deepgram.com"),
        },
    );

    m
});

