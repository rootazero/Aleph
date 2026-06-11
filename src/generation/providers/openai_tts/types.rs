//! Request/response types for `OpenAI` TTS API.

use serde::{Deserialize, Serialize};

/// Request body for `OpenAI` TTS API
#[derive(Debug, Clone, Serialize)]
pub struct TtsRequest {
    /// Model to use (e.g., "tts-1", "tts-1-hd")
    pub model: String,
    /// The text to synthesize
    pub input: String,
    /// Voice to use (alloy, echo, fable, onyx, nova, shimmer)
    pub voice: String,
    /// Output format (mp3, opus, aac, flac)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,
    /// Speaking speed (0.25 to 4.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
}

/// `OpenAI` API error response format
#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiErrorResponse {
    pub error: OpenAiError,
}

/// `OpenAI` API error details
#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiError {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(default)]
    #[allow(dead_code)] // Deserialized from API response
    pub param: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // Deserialized from API response
    pub code: Option<String>,
}
