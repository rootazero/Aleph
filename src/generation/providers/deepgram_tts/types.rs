//! Request/response shapes for the Deepgram `/v1/speak` endpoint.
//!
//! The request body is a tiny JSON envelope (`{"text": "..."}`) — encoding,
//! sample rate, container and model are all query parameters. The response
//! is raw audio bytes, so no response struct is needed for the success path.
//! Only the error envelope is deserialized.
//!
//! Deepgram's error shape is shared with `/v1/listen`, but we keep a dedicated
//! type here to avoid coupling the two providers' lifetimes.

use serde::{Deserialize, Serialize};

/// `POST /v1/speak` request body.
#[derive(Debug, Clone, Serialize)]
pub struct SpeakRequest {
    /// Text to synthesize. Deepgram caps a single request at ~2000 characters.
    pub text: String,
}

/// Deepgram error envelope, mirrored from the STT side.
#[derive(Debug, Clone, Deserialize)]
pub struct DeepgramError {
    #[serde(rename = "err_code", default)]
    #[allow(dead_code)] // surfaced via logs only
    pub err_code: Option<String>,
    #[serde(rename = "err_msg", default)]
    pub err_msg: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}
