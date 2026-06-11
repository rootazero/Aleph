//! Request/response shapes for MiniMax's native `/v1/audio_to_text` endpoint.
//!
//! MiniMax's STT diverges from OpenAI Whisper in two ways:
//!
//! * Auth requires a per-account `GroupId` query parameter alongside the
//!   bearer token — there's no global account scope.
//! * Responses wrap every payload in a `base_resp` envelope whose
//!   `status_code` (not the HTTP status) signals failure. A 200 OK with
//!   `status_code != 0` is still a logical failure and we map it onto a
//!   `GenerationError::Provider`.
//!
//! The endpoint is synchronous multipart-upload; there is no long-poll
//! variant on this surface (the long-form `voice_recognition` async API is
//! a separate product and out of scope here).

use serde::Deserialize;

/// Top-level MiniMax response envelope.
#[derive(Debug, Clone, Deserialize)]
pub struct MinimaxSttResponse {
    /// Either the transcript text (success) or empty/missing on failure.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub duration: Option<f32>,
    /// Status envelope — present on every response, success or failure.
    pub base_resp: MinimaxBaseResp,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MinimaxBaseResp {
    pub status_code: i32,
    #[serde(default)]
    pub status_msg: Option<String>,
}

impl MinimaxBaseResp {
    /// True for the canonical "everything is fine" code (0).
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.status_code == 0
    }

    /// Best-effort message — falls back to a stringified status code when
    /// the upstream omits a status_msg.
    #[must_use]
    pub fn best_message(&self) -> String {
        self.status_msg
            .clone()
            .unwrap_or_else(|| format!("MiniMax status_code={}", self.status_code))
    }
}
