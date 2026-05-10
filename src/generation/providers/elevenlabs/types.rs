//! Request/response types for ElevenLabs TTS API.

use serde::{Deserialize, Serialize};

/// Request body for ElevenLabs TTS API
#[derive(Debug, Clone, Serialize)]
pub struct TtsRequest {
    /// The text to synthesize
    pub text: String,
    /// Model ID to use
    pub model_id: String,
    /// Voice settings
    pub voice_settings: VoiceSettings,
}

/// Voice settings for ElevenLabs TTS API
#[derive(Debug, Clone, Serialize)]
pub struct VoiceSettings {
    /// How stable the voice should be (0.0 to 1.0)
    pub stability: f32,
    /// How much to boost similarity to the original voice (0.0 to 1.0)
    pub similarity_boost: f32,
    /// Style exaggeration (0.0 to 1.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<f32>,
    /// Whether to use speaker boost
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_speaker_boost: Option<bool>,
}

/// ElevenLabs API error response format
#[derive(Debug, Clone, Deserialize)]
pub struct ElevenLabsErrorResponse {
    pub detail: Option<ElevenLabsErrorDetail>,
}

/// ElevenLabs API error detail
#[derive(Debug, Clone, Deserialize)]
pub struct ElevenLabsErrorDetail {
    pub message: String,
    #[serde(default)]
    #[allow(dead_code)] // Deserialized from API response
    pub status: Option<String>,
}
