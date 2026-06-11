//! Deepgram `/v1/listen` response types — we only deserialize the fields we
//! actually surface up the stack, so unknown ones are silently dropped.

use serde::Deserialize;

/// Top-level Deepgram response envelope.
#[derive(Debug, Clone, Deserialize)]
pub struct DeepgramResponse {
    pub results: DeepgramResults,
    #[serde(default)]
    #[allow(dead_code)] // surfaced via JSON dump in metadata.extra
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeepgramResults {
    pub channels: Vec<DeepgramChannel>,
    #[serde(default)]
    pub utterances: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeepgramChannel {
    pub alternatives: Vec<DeepgramAlternative>,
    /// Detected language code; only present when `detect_language=true`.
    #[serde(default)]
    pub detected_language: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeepgramAlternative {
    pub transcript: String,
    #[serde(default)]
    pub confidence: Option<f32>,
}

/// Deepgram error envelope — distinct from `OpenAI`'s shape.
#[derive(Debug, Clone, Deserialize)]
pub struct DeepgramError {
    #[serde(rename = "err_code", default)]
    #[allow(dead_code)] // useful in logs
    pub err_code: Option<String>,
    #[serde(rename = "err_msg", default)]
    pub err_msg: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}
