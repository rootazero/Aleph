//! Request/response shapes for Volcengine's legacy speech HTTP API
//! (`POST https://openspeech.bytedance.com/api/v1/tts`).
//!
//! This is **not** OpenAI-compatible — like `minimax_tts`, routing it through
//! the generic `openai_compat` provider produces a silent failure:
//!
//! * The request body nests credentials/voice/audio under `app` / `audio` /
//!   `request` objects rather than OpenAI's flat
//!   `{ input, voice, response_format }`.
//! * The response is a JSON envelope — **not** raw audio bytes. The audio
//!   arrives **base64-encoded** in `data` (MiniMax uses hex; Volcengine uses
//!   base64). Success is signalled by `code == 3000`, not the HTTP status.
//! * Auth is `Authorization: Bearer;<token>` (a semicolon, not a space) plus
//!   an `appid` carried in the request body — the China-hosted
//!   `openspeech.bytedance.com` endpoint, chosen over the international
//!   BytePlus Seed endpoint to keep the round-trip on a domestic edge.

use serde::{Deserialize, Serialize};

/// Request body for `POST /api/v1/tts` (synchronous, non-streaming).
#[derive(Debug, Clone, Serialize)]
pub struct TtsRequest {
    pub app: AppConfig,
    pub user: UserConfig,
    pub audio: AudioConfig,
    pub request: RequestConfig,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppConfig {
    pub appid: String,
    pub token: String,
    pub cluster: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserConfig {
    pub uid: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioConfig {
    pub voice_type: String,
    pub encoding: String,
    /// Speaking rate. Volcengine accepts `[0.2, 3.0]`; omitted (defaulting to
    /// `1.0` upstream) when the caller doesn't specify one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_ratio: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestConfig {
    pub reqid: String,
    pub text: String,
    pub text_type: String,
    pub operation: String,
}

/// Top-level Volcengine TTS response envelope.
///
/// Success is `code == 3000` with a non-empty base64 `data` payload. A 200 OK
/// carrying any other `code` is still a logical failure.
#[derive(Debug, Clone, Deserialize)]
pub struct TtsResponse {
    #[serde(default)]
    pub code: Option<i32>,
    #[serde(default)]
    pub message: Option<String>,
    /// Base64-encoded audio bytes. Empty/absent on failure.
    #[serde(default)]
    pub data: Option<String>,
}

impl TtsResponse {
    /// Volcengine's canonical success code for a completed synthesis.
    pub const SUCCESS_CODE: i32 = 3000;

    /// True when the envelope reports a completed synthesis.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.code == Some(Self::SUCCESS_CODE)
    }

    /// Best-effort error message — falls back to a stringified code.
    #[must_use]
    pub fn best_message(&self) -> String {
        self.message
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| match self.code {
                Some(c) => format!("Volcengine code={c}"),
                None => "Volcengine: missing response code".to_string(),
            })
    }
}
