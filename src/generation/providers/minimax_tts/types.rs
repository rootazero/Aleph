//! Request/response shapes for `MiniMax`'s native `POST /v1/t2a_v2` endpoint.
//!
//! `MiniMax` TTS diverges from the `OpenAI` `/v1/audio/speech` shape in two
//! ways that make the generic `openai_compat` provider unusable for it:
//!
//! * The request body nests voice and audio parameters under `voice_setting`
//!   and `audio_setting` objects, instead of `OpenAI`'s flat
//!   `{ input, voice, response_format, speed }`.
//! * The response is a JSON envelope — **not** raw audio bytes. The audio
//!   arrives **hex-encoded** in `data.audio` and must be hex-decoded. Success
//!   is signalled by `base_resp.status_code == 0` (a 200 OK with a non-zero
//!   `status_code` is still a logical failure), mirroring every other
//!   `MiniMax` surface (see the STT provider's `base_resp` handling).

use serde::{Deserialize, Serialize};

/// Request body for `POST /v1/t2a_v2` (synchronous, non-streaming).
#[derive(Debug, Clone, Serialize)]
pub struct T2aRequest {
    pub model: String,
    pub text: String,
    /// Always `false` here — streaming (SSE / WebSocket) is out of scope for
    /// the single-shot `generate()` contract.
    pub stream: bool,
    pub voice_setting: VoiceSetting,
    pub audio_setting: AudioSetting,
}

#[derive(Debug, Clone, Serialize)]
pub struct VoiceSetting {
    pub voice_id: String,
    /// Speaking rate. `MiniMax` accepts `[0.5, 2.0]`; omitted (defaulting to
    /// `1.0` upstream) when the caller doesn't specify one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioSetting {
    pub format: String,
    pub sample_rate: u32,
}

/// Top-level `MiniMax` T2A response envelope.
#[derive(Debug, Clone, Deserialize)]
pub struct T2aResponse {
    /// Present on success; carries the hex-encoded audio payload.
    #[serde(default)]
    pub data: Option<T2aData>,
    /// Optional generation metadata (audio length, format, size).
    #[serde(default)]
    pub extra_info: Option<ExtraInfo>,
    #[serde(default)]
    pub trace_id: Option<String>,
    /// Status envelope — present on every response, success or failure.
    pub base_resp: MinimaxBaseResp,
}

#[derive(Debug, Clone, Deserialize)]
pub struct T2aData {
    /// Hex-encoded audio bytes (NOT base64). Empty/absent on failure.
    #[serde(default)]
    pub audio: Option<String>,
    /// Generation status; `2` means the audio is complete.
    #[serde(default)]
    pub status: Option<i32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExtraInfo {
    /// Audio duration in milliseconds.
    #[serde(default)]
    pub audio_length: Option<u64>,
    #[serde(default)]
    pub audio_format: Option<String>,
    #[serde(default)]
    pub audio_sample_rate: Option<u32>,
    #[serde(default)]
    pub audio_size: Option<u64>,
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

    /// Best-effort message — falls back to a stringified status code when the
    /// upstream omits a `status_msg`.
    #[must_use]
    pub fn best_message(&self) -> String {
        self.status_msg
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("MiniMax status_code={}", self.status_code))
    }
}
