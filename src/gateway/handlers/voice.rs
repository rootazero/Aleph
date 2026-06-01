//! `voice.transcribe` — browser-recorded audio → text for the panel mic button.
//!
//! The Leptos panel captures microphone audio via `MediaRecorder`, base64-encodes
//! the resulting blob and posts it here. We decode it and reuse the exact same
//! Whisper-compatible STT path as the channel inbound voice middleware
//! ([`crate::gateway::voice::inbound`]) — no second transcription implementation.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use tokio::sync::RwLock;

use crate::config::Config;
use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::SharedTokenManager;
use crate::gateway::voice::inbound::{resolve_stt_config, transcribe_bytes};
use crate::sync_primitives::Arc;

/// OpenAI Whisper rejects payloads larger than 25 MB; reject early so we never
/// stream a doomed request and to bound the base64 we decode.
const MAX_AUDIO_BYTES: usize = 25 * 1024 * 1024;

/// Transcribe a base64-encoded audio blob to text using the configured
/// transcription provider.
///
/// Params: `{ audio_base64: String, mime_type?: String, language?: String }`.
/// Success: `{ "text": String }`.
pub async fn handle_transcribe(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(serde::Deserialize)]
    struct Params {
        audio_base64: String,
        #[serde(default)]
        mime_type: Option<String>,
        #[serde(default)]
        language: Option<String>,
    }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let bytes = match BASE64.decode(params.audio_base64.as_bytes()) {
        Ok(b) => b,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid base64 audio: {e}"),
            )
        }
    };
    if bytes.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Empty audio payload");
    }
    if bytes.len() > MAX_AUDIO_BYTES {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "Audio exceeds the 25MB transcription limit",
        );
    }

    let mime = params
        .mime_type
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "audio/webm".to_string());
    let filename = filename_for_mime(&mime);

    let stt = {
        let cfg = config.read().await;
        resolve_stt_config(&cfg.generation, &vault)
    };
    let Some(stt) = stt else {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "No transcription provider configured. Add one in Settings → Generation Providers.",
        );
    };

    match transcribe_bytes(bytes, &filename, &mime, params.language.as_deref(), &stt).await {
        Ok(text) => {
            JsonRpcResponse::success(request.id, serde_json::json!({ "text": text.trim() }))
        }
        Err(e) => {
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Transcription failed: {e}"))
        }
    }
}

/// Map a MIME type to a filename the Whisper multipart endpoint accepts.
/// The extension is what most servers sniff for the codec, so an accurate
/// one matters more than the part name.
fn filename_for_mime(mime: &str) -> String {
    let ext = match mime.split(';').next().unwrap_or(mime).trim() {
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => "m4a",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/flac" => "flac",
        // MediaRecorder's default on Chromium/Firefox is webm/opus.
        _ => "webm",
    };
    format!("voice.{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_extension_follows_mime() {
        assert_eq!(filename_for_mime("audio/webm;codecs=opus"), "voice.webm");
        assert_eq!(filename_for_mime("audio/ogg"), "voice.ogg");
        assert_eq!(filename_for_mime("audio/mp4"), "voice.m4a");
        assert_eq!(filename_for_mime("audio/mpeg"), "voice.mp3");
        assert_eq!(filename_for_mime("audio/wav"), "voice.wav");
        // Unknown → safe default matching the browser recorder.
        assert_eq!(filename_for_mime("application/octet-stream"), "voice.webm");
    }

    #[tokio::test]
    async fn rejects_invalid_base64() {
        use crate::gateway::security::SecurityStore;

        let req = JsonRpcRequest::with_id(
            "voice.transcribe",
            Some(serde_json::json!({ "audio_base64": "not!base64!" })),
            serde_json::json!(1),
        );
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let vault = Arc::new(SharedTokenManager::new(store, dir.path().join("test.vault")));
        let config = Arc::new(RwLock::new(Config::default()));
        let resp = handle_transcribe(req, config, vault).await;
        // Decode happens before any provider lookup → invalid base64 errors out.
        assert!(resp.error.is_some());
    }
}
