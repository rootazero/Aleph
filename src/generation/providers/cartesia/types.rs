//! Request/response shapes for Cartesia's `/tts/bytes` endpoint.
//!
//! Cartesia's wire format diverges from OpenAI TTS in two material ways:
//!
//! * Voice selection is a nested `{mode, id}` object (not a string), which
//!   carries an `embedding`/`id`/`voice_design_id` discriminator we'd lose
//!   if shoved through openai_compat.
//! * `output_format` is a 3-tuple (`container`, `encoding`, `sample_rate`)
//!   rather than a free-form string.
//!
//! Cartesia returns audio bytes directly for the `/bytes` endpoint, so there
//! is no JSON response struct on the success path.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct CartesiaTtsRequest {
    /// Text to synthesize.
    pub transcript: String,
    /// Cartesia model id — e.g. `"sonic-2"`, `"sonic"`.
    pub model_id: String,
    /// Nested voice selector.
    pub voice: CartesiaVoiceSelector,
    /// ISO-639 language code; Cartesia auto-detects when omitted, but
    /// pinning helps multilingual voices.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Container/encoding/sample-rate tuple.
    pub output_format: CartesiaOutputFormat,
    /// Optional speed knob, range -1.0..=1.0 (negative slower, positive faster).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum CartesiaVoiceSelector {
    /// Refer to a Cartesia-hosted voice by its public UUID.
    Id { id: String },
    /// Inline embedding vector (advanced — not exposed via our API yet).
    Embedding { embedding: Vec<f32> },
}

#[derive(Debug, Clone, Serialize)]
pub struct CartesiaOutputFormat {
    pub container: String,
    pub encoding: String,
    pub sample_rate: u32,
}

/// Best-effort error envelope. Cartesia normally returns
/// `{ "error": { "message": "...", "type": "..." } }`.
#[derive(Debug, Clone, Deserialize)]
pub struct CartesiaError {
    #[serde(default)]
    pub error: Option<CartesiaErrorDetail>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CartesiaErrorDetail {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub r#type: Option<String>,
}

impl CartesiaError {
    pub fn best_message(&self) -> Option<String> {
        if let Some(detail) = &self.error {
            if let Some(m) = &detail.message {
                return Some(m.clone());
            }
        }
        self.message.clone()
    }
}
