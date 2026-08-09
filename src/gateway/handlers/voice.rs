//! `voice.transcribe` — browser-recorded audio → text for the panel mic button.
//!
//! The Leptos panel captures microphone audio via `MediaRecorder`, base64-encodes
//! the resulting blob and posts it here. We decode it and reuse the exact same
//! Whisper-compatible STT path as the channel inbound voice middleware
//! ([`crate::gateway::voice::inbound`]) — no second transcription implementation.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use tokio::sync::RwLock;

use crate::config::Config;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::SharedTokenManager;
use crate::gateway::voice::inbound::{resolve_stt_source, transcribe_with_source};
use crate::gateway::voice::streaming::{self, StreamConfig, StreamRegistry, StreamingTarget};
use crate::sync_primitives::Arc;

/// `OpenAI` Whisper rejects payloads larger than 25 MB; reject early so we never
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

    let stt_source = {
        let cfg = config.read().await;
        resolve_stt_source(
            &cfg.generation,
            &vault,
            cfg.voice_local.vocabulary_hint().as_deref(),
        )
    };
    let Some(stt_source) = stt_source else {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "No transcription provider configured. Add one in Settings → Generation Providers.",
        );
    };
    match transcribe_with_source(
        bytes::Bytes::from(bytes),
        &filename,
        &mime,
        params.language.as_deref(),
        &stt_source,
    )
    .await
    {
        Ok(text) => {
            JsonRpcResponse::success(request.id, serde_json::json!({ "text": text.trim() }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Transcription failed: {e}"),
        ),
    }
}

/// `voice.format` — AI speech-formatting pass: turn a raw disfluent transcript
/// into clean written text for display. Display-level polish only — it does NOT
/// gate the Agent (the raw text was already sent earlier in the pipeline).
///
/// Params: `{ text: String }`. Success: `{ "formatted": String }`. (A per-call
/// `prompt` override existed but had zero callers — the system head comes from
/// `[voice.format] prompt`.)
///
/// Pure I/O glue (R4/R10): parse → `format_text` → respond. When
/// `[voice.format] enabled = false`, returns the text unchanged. The formatting
/// pass itself degrades gracefully — a provider/network failure yields the raw
/// text, never an RPC error — so this handler effectively always succeeds.
pub async fn handle_format(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(serde::Deserialize)]
    struct Params {
        text: String,
    }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Snapshot the format config; disabled → unchanged passthrough.
    let format = { config.read().await.voice_local.format.clone() };
    if !format.enabled {
        return JsonRpcResponse::success(
            request.id,
            serde_json::json!({ "formatted": params.text }),
        );
    }

    let formatted =
        crate::gateway::voice::format::format_text(&params.text, &format, &config, &vault)
            .await
            // `format_text` degrades to the raw text on any failure, so this is
            // belt-and-suspenders: never surface an error for display polish.
            .unwrap_or(params.text);

    JsonRpcResponse::success(request.id, serde_json::json!({ "formatted": formatted }))
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

// ---------------------------------------------------------------------------
// Native push-to-talk capture (macOS bridge) + TTS playback
//
// The Panel mic button is a full voice channel: capture (endpoint) → STT (core)
// → LLM (core) → TTS (core) → playback (endpoint). These three handlers are the
// core's I/O surface for the endpoint:
//   - `voice.record_start` / `voice.record_stop` proxy the native AVFoundation
//     recorder over the desktop bridge, the macOS path that works when the
//     unsigned WKWebView cannot reach `getUserMedia`. Capture only — the bytes
//     come back to the Panel which then posts them to `voice.transcribe`, so
//     capture and transcription stay two separate steps (mirrors file upload).
//   - `voice.synthesize` reuses the channel TTS path ([`generate_tts`]) so the
//     Panel can play back the agent's reply as speech.
// ---------------------------------------------------------------------------

/// Sentinel surfaced in the JSON-RPC error message when the platform has no
/// native audio helper (non-macOS, or the bridge returned `NotImplemented`).
/// The Panel matches this exact token to fall back to browser `getUserMedia`.
const NATIVE_AUDIO_UNAVAILABLE: &str = "NATIVE_AUDIO_UNAVAILABLE";

/// Token the macOS bridge puts in its error when the host has no audio input
/// device at all (matched here, not shown to the user verbatim).
const NO_AUDIO_INPUT_DEVICE: &str = "NO_AUDIO_INPUT_DEVICE";

/// User-facing message when there is no microphone on the host. Distinct from
/// the browser-fallback sentinel: there is nothing to fall back to.
const NO_MICROPHONE_MESSAGE: &str =
    "No microphone found. Connect a microphone to this computer and try again.";

/// Begin an open-ended native recording via the desktop bridge.
///
/// Returns `{}` on success. When native capture is unavailable, the error
/// message is the [`NATIVE_AUDIO_UNAVAILABLE`] sentinel so the Panel falls back
/// to browser capture.
pub async fn handle_record_start(
    request: JsonRpcRequest,
    platform: Arc<dyn aleph_desktop::DesktopPlatform>,
) -> JsonRpcResponse {
    let Some(media) = platform.media() else {
        return JsonRpcResponse::error(request.id, INTERNAL_ERROR, NATIVE_AUDIO_UNAVAILABLE);
    };
    match media.record_audio_start().await {
        Ok(()) => JsonRpcResponse::success(request.id, serde_json::json!({})),
        // No native helper (default trait impl) or the desktop bridge isn't
        // running (headless / remote daemon) → tell the Panel to fall back to
        // browser capture.
        Err(
            aleph_desktop::DesktopError::NotImplemented(_)
            | aleph_desktop::DesktopError::BridgeDisabled(_),
        ) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, NATIVE_AUDIO_UNAVAILABLE),
        Err(e) => {
            // The bridge has no input device (e.g. a Mac mini with no built-in
            // mic). Surface a clear, actionable message instead of a cryptic
            // `record() failed` — and do NOT fall back to browser capture
            // (getUserMedia on the same host has no device either).
            if e.to_string().contains(NO_AUDIO_INPUT_DEVICE) {
                return JsonRpcResponse::error(request.id, INTERNAL_ERROR, NO_MICROPHONE_MESSAGE);
            }
            JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to start recording: {e}"),
            )
        }
    }
}

/// Stop the active native recording and return the captured audio as base64.
///
/// The core reads the file the bridge wrote and hands the bytes back to the
/// Panel — it does NOT transcribe here (capture and transcription are separate
/// steps, the same split as a file upload). Success:
/// `{ audio_base64, mime_type, duration_secs }`.
pub async fn handle_record_stop(
    request: JsonRpcRequest,
    platform: Arc<dyn aleph_desktop::DesktopPlatform>,
) -> JsonRpcResponse {
    let Some(media) = platform.media() else {
        return JsonRpcResponse::error(request.id, INTERNAL_ERROR, NATIVE_AUDIO_UNAVAILABLE);
    };
    let result = match media.record_audio_stop().await {
        Ok(r) => r,
        Err(
            aleph_desktop::DesktopError::NotImplemented(_)
            | aleph_desktop::DesktopError::BridgeDisabled(_),
        ) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, NATIVE_AUDIO_UNAVAILABLE),
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to stop recording: {e}"),
            )
        }
    };
    let bytes = match tokio::fs::read(&result.file_path).await {
        Ok(b) => b,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to read recording: {e}"),
            )
        }
    };
    JsonRpcResponse::success(
        request.id,
        serde_json::json!({
            "audio_base64": BASE64.encode(&bytes),
            "mime_type": mime_for_format(&result.format),
            "duration_secs": result.duration_secs,
        }),
    )
}

/// Synthesize speech for the agent's reply so the Panel can play it back.
///
/// Reuses the channel TTS path ([`crate::gateway::voice::outbound::generate_tts`]).
/// Params: `{ text, voice?, provider? }`. Success carries either inline bytes
/// (`{ audio_base64, mime_type }`) or a remote URL (`{ audio_url, mime_type }`).
pub async fn handle_synthesize(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    generation_registry: Arc<
        crate::sync_primitives::RwLock<crate::generation::GenerationProviderRegistry>,
    >,
) -> JsonRpcResponse {
    #[derive(serde::Deserialize)]
    struct Params {
        text: String,
        #[serde(default)]
        voice: Option<String>,
        #[serde(default)]
        provider: Option<String>,
    }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let text = params.text.trim();
    if text.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Empty text");
    }

    let gen_cfg = { config.read().await.generation.clone() };
    // A throwaway VoiceState carries the optional per-call provider/voice
    // overrides; `enabled` is irrelevant here (we're synthesizing on demand).
    let voice_state = crate::gateway::voice::state::VoiceState {
        enabled: true,
        provider: params.provider,
        voice: params.voice,
        consecutive_failures: 0,
    };

    // Snapshot the registry under its sync lock (no guard held across `.await`),
    // mirroring how the inbound router wires voice TTS output. Picks up
    // providers added since boot; provider handles are cheap Arc clones.
    let registry = {
        let guard = generation_registry
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let mut snapshot = crate::generation::GenerationProviderRegistry::new();
        for name in guard.names() {
            if let Some(provider) = guard.get(&name) {
                let _ = snapshot.register(name, provider);
            }
        }
        snapshot
    };
    let attachment =
        crate::gateway::voice::outbound::generate_tts(text, &voice_state, &registry, &gen_cfg)
            .await;
    let Some(attachment) = attachment else {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "TTS failed or no speech provider configured. Add one in Settings → Generation Providers.",
        );
    };

    let mime = attachment.mime_type.clone();
    if let Some(data) = attachment.data {
        JsonRpcResponse::success(
            request.id,
            serde_json::json!({ "audio_base64": BASE64.encode(&data), "mime_type": mime }),
        )
    } else if let Some(url) = attachment.url {
        JsonRpcResponse::success(
            request.id,
            serde_json::json!({ "audio_url": url, "mime_type": mime }),
        )
    } else if let Some(path) = attachment.path {
        match tokio::fs::read(&path).await {
            Ok(b) => JsonRpcResponse::success(
                request.id,
                serde_json::json!({ "audio_base64": BASE64.encode(&b), "mime_type": mime }),
            ),
            Err(e) => JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to read TTS output: {e}"),
            ),
        }
    } else {
        JsonRpcResponse::error(request.id, INTERNAL_ERROR, "TTS produced no audio")
    }
}

// ---------------------------------------------------------------------------
// Real-time streaming STT (Panel audio frames → backend WS → delta events)
//
// Three pure-I/O handlers bridge the Panel to the backend streaming relay
// ([`crate::gateway::voice::streaming::relay`]):
//   - `voice.stream.start` opens a backend session (or reports disabled) and
//     returns a `stream_id`.
//   - `voice.stream.audio` pushes one s16le PCM frame into that session.
//   - `voice.stream.stop` drops the session's audio sender so the backend
//     finishes and the delta pump shuts down.
// Normalized `TranscriptDelta`s are published on the `voice.transcribe.delta`
// topic by the relay's pump task — the Panel subscribes there.
// ---------------------------------------------------------------------------

/// `voice.stream.start` — params `{ language?: String }` → `{ stream_id: String | null }`.
///
/// `stream_id: null` means BYO streaming is disabled in config; the Panel falls
/// back to the batch `voice.transcribe` path. The API key is read directly from
/// `[voice.streaming]` (LAN-trust, user-controlled config) — no vault lookup.
pub async fn handle_stream_start(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    registry: Arc<StreamRegistry>,
) -> JsonRpcResponse {
    #[derive(serde::Deserialize, Default)]
    struct Params {
        #[serde(default)]
        language: Option<String>,
    }
    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let target = {
        let cfg = config.read().await;
        let s = &cfg.voice_local.streaming;
        if !s.enabled {
            return JsonRpcResponse::success(request.id, serde_json::json!({ "stream_id": null }));
        }
        StreamingTarget {
            provider: s.provider.clone(),
            base_url: s.base_url.clone(),
            api_key: s.api_key.clone(),
            language: s.language.clone(),
            model: s.model.clone(),
            vocabulary: cfg.voice_local.vocabulary_hint().unwrap_or_default(),
        }
    };
    // `open()` falls back to `target.language` when the per-call language is None.
    let cfg = StreamConfig::new(params.language);
    match streaming::relay::start_stream(&registry, event_bus, target, cfg).await {
        Ok(id) => JsonRpcResponse::success(request.id, serde_json::json!({ "stream_id": id })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("stream start failed: {e}"),
        ),
    }
}

/// Upper bound on one streaming PCM frame. The Panel emits ~200 ms of 16 kHz
/// mono s16le (≈6.4 KB); 64 KB is over 2 s of audio — well past any legitimate
/// frame, and small enough that a malformed or hostile client cannot push
/// megabytes per call into the relay.
const MAX_PCM_FRAME_BYTES: usize = 64 * 1024;

/// `voice.stream.audio` — params `{ stream_id: String, pcm_base64: String }` → `{}`.
///
/// Pushes one s16le PCM frame into the backend session. An unknown or already
/// closed `stream_id` is a silent no-op (the stream may have just stopped) —
/// never an error.
pub async fn handle_stream_audio(
    request: JsonRpcRequest,
    registry: Arc<StreamRegistry>,
) -> JsonRpcResponse {
    #[derive(serde::Deserialize)]
    struct Params {
        stream_id: String,
        pcm_base64: String,
    }
    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let bytes = match BASE64.decode(params.pcm_base64.as_bytes()) {
        Ok(b) => b,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid base64 pcm: {e}"),
            )
        }
    };
    if bytes.len() > MAX_PCM_FRAME_BYTES {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "PCM frame exceeds the 64KB streaming limit",
        );
    }
    // Whose stream is this? Injecting PCM into someone else's transcription
    // puts words in their mouth — the deltas they and their surfaces read come
    // back carrying it. An unknown id keeps the existing silent no-op, so a
    // refusal is indistinguishable from a stream that just stopped.
    if !registry.caller_may_use(&params.stream_id).await {
        return JsonRpcResponse::success(request.id, serde_json::json!({}));
    }
    if let Some(tx) = registry.audio_sender(&params.stream_id).await {
        // `try_send`, never `send().await`: this is realtime audio. A backend
        // that stops draining (wedged ASR server, TCP backpressure) would
        // otherwise park this RPC — and pile up every later frame behind it —
        // while the channel already holds seconds of stale audio. Dropping the
        // newest frame is the correct realtime degradation: the utterance loses
        // a slice, the session survives. A closed channel stays a silent no-op
        // (the Panel is about to stop the stream anyway).
        if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(bytes) {
            tracing::debug!(
                stream_id = %params.stream_id,
                "voice stream backend is not draining — dropped a PCM frame"
            );
        }
    }
    JsonRpcResponse::success(request.id, serde_json::json!({}))
}

/// `voice.stream.stop` — params `{ stream_id: String }` → `{}`.
///
/// Removes the registry entry, dropping the sole audio sender so the backend
/// session finishes and the delta pump shuts down cleanly.
pub async fn handle_stream_stop(
    request: JsonRpcRequest,
    registry: Arc<StreamRegistry>,
) -> JsonRpcResponse {
    #[derive(serde::Deserialize)]
    struct Params {
        stream_id: String,
    }
    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // Same ownership question, same silent shape: `stop` on a foreign stream
    // cuts off someone else's dictation mid-sentence.
    if registry.caller_may_use(&params.stream_id).await {
        registry.remove(&params.stream_id).await;
    }
    JsonRpcResponse::success(request.id, serde_json::json!({}))
}

/// Map a bridge-reported audio `format` (e.g. "m4a") to a MIME type the Whisper
/// multipart endpoint accepts. `AVFoundation` records AAC/m4a by default.
fn mime_for_format(format: &str) -> &'static str {
    match format.trim().to_lowercase().as_str() {
        "m4a" | "mp4" | "aac" => "audio/mp4",
        "wav" | "wave" => "audio/wav",
        "mp3" | "mpeg" => "audio/mpeg",
        "ogg" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        _ => "audio/mp4",
    }
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

    #[test]
    fn native_format_maps_to_whisper_mime() {
        // AVFoundation default is AAC/m4a → audio/mp4 (Whisper-accepted).
        assert_eq!(mime_for_format("m4a"), "audio/mp4");
        assert_eq!(mime_for_format("WAV"), "audio/wav");
        assert_eq!(mime_for_format("mp3"), "audio/mpeg");
        // Unknown → safe default the native recorder actually emits.
        assert_eq!(mime_for_format("caf"), "audio/mp4");
    }

    #[test]
    fn no_input_device_token_survives_bridge_wrapping() {
        // The macOS client wraps the bridge error; the no-device token must
        // still be matchable so the user sees the clear message, not a fallback.
        let wrapped = "media.audio.record_start RPC: bridge error -32004: \
                       audio.record_start: NO_AUDIO_INPUT_DEVICE";
        assert!(wrapped.contains(NO_AUDIO_INPUT_DEVICE));
        // The no-device case must NOT be confused with the browser-fallback path.
        assert!(!wrapped.contains(NATIVE_AUDIO_UNAVAILABLE));
        assert!(!NO_MICROPHONE_MESSAGE.is_empty());
    }

    #[tokio::test]
    async fn stream_audio_rejects_an_oversized_frame() {
        // Bound the relay's input: a legitimate frame is ~6.4 KB.
        let oversized = BASE64.encode(vec![0u8; MAX_PCM_FRAME_BYTES + 1]);
        let req = JsonRpcRequest::with_id(
            "voice.stream.audio",
            Some(serde_json::json!({ "stream_id": "s1", "pcm_base64": oversized })),
            serde_json::json!(1),
        );
        let resp = handle_stream_audio(req, Arc::new(StreamRegistry::default())).await;
        let err = resp.error.expect("oversized frame must be rejected");
        assert!(err.message.contains("64KB"), "got: {}", err.message);
    }

    #[tokio::test]
    async fn stream_audio_for_an_unknown_stream_is_a_silent_no_op() {
        // The Panel races stop-then-flush; a late frame must never error.
        let req = JsonRpcRequest::with_id(
            "voice.stream.audio",
            Some(serde_json::json!({
                "stream_id": "gone",
                "pcm_base64": BASE64.encode([0u8; 64]),
            })),
            serde_json::json!(1),
        );
        let resp = handle_stream_audio(req, Arc::new(StreamRegistry::default())).await;
        assert!(resp.error.is_none());
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
        let vault = Arc::new(SharedTokenManager::new(
            store,
            dir.path().join("test.vault"),
        ));
        let config = Arc::new(RwLock::new(Config::default()));
        let resp = handle_transcribe(req, config, vault).await;
        // Decode happens before any provider lookup → invalid base64 errors out.
        assert!(resp.error.is_some());
    }
}
