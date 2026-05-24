//! Request/response shapes for the OpenAI `/v1/audio/transcriptions` endpoint.
//!
//! The request is `multipart/form-data` (the audio bytes plus optional text
//! fields), so there is no JSON request body to serialize. The response is a
//! flat JSON object whose shape depends on `response_format` — we always ask
//! for `json` so we get the minimal `{ text, language?, duration? }` form.

use serde::Deserialize;

/// Whisper API JSON response (response_format=json or verbose_json).
///
/// `text` is always present; `language` and `duration` only appear when
/// `verbose_json` is requested. Both are tolerated as optional so the same
/// struct works regardless of which format the upstream chose.
#[derive(Debug, Clone, Deserialize)]
pub struct WhisperResponse {
    pub text: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub duration: Option<f32>,
}

/// Standard OpenAI error envelope reused by Whisper.
#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiErrorResponse {
    pub error: OpenAiError,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiError {
    pub message: String,
    #[serde(rename = "type")]
    #[allow(dead_code)] // Deserialized from API response; useful in logs only.
    pub error_type: String,
    #[serde(default)]
    #[allow(dead_code)] // Deserialized from API response; useful in logs only.
    pub param: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // Deserialized from API response; useful in logs only.
    pub code: Option<String>,
}
