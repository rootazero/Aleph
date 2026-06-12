//! Endpoint handlers. Model gating: Missing/Error → kick ensure + 503;
//! Downloading/Unpacking → 503 with percent; Ready → serve (slot lazy-loads
//! the engine; concurrent loaders queue behind the slot mutex, 15 s cap).

use std::time::Duration;

use axum::extract::{Multipart, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::AppState;
use crate::models::manifest::ModelSpec;
use crate::models::ModelState;

const LOAD_TIMEOUT: Duration = Duration::from_secs(15);

/// Gate a request on model readiness. Err = ready-made 503 response.
fn gate(state: &AppState, spec: &'static ModelSpec) -> Result<(), Response> {
    match state.models.state(spec) {
        ModelState::Ready => Ok(()),
        ModelState::Downloading { percent } => Err(downloading_503(percent)),
        ModelState::Unpacking => Err(downloading_503(99)),
        ModelState::Missing | ModelState::Error { .. } => {
            let models = state.models.clone();
            tokio::spawn(async move {
                if let Err(e) = models.ensure(spec).await {
                    tracing::warn!(model = spec.id, error = %e, "model ensure failed");
                }
            });
            Err(downloading_503(0))
        }
    }
}

fn downloading_503(percent: u8) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"status": "downloading", "percent": percent})),
    )
        .into_response()
}

fn error_json(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({"error": {"message": msg.into()}}))).into_response()
}

/// POST /v1/audio/transcriptions — OpenAI multipart compatible.
pub async fn transcriptions(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    if let Err(resp) = gate(&state, state.stt_spec) {
        return resp;
    }
    let mut file: Option<(Vec<u8>, String)> = None;
    let mut language: Option<String> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return error_json(StatusCode::BAD_REQUEST, format!("multipart error: {e}")),
        };
        match field.name().unwrap_or("") {
            "file" => {
                let name = field.file_name().unwrap_or("audio.bin").to_string();
                match field.bytes().await {
                    Ok(b) => file = Some((b.to_vec(), name)),
                    Err(e) => return error_json(StatusCode::BAD_REQUEST, format!("read file: {e}")),
                }
            }
            "language" => language = field.text().await.ok().filter(|s| !s.is_empty()),
            _ => {} // model / response_format accepted and ignored
        }
    }
    let Some((bytes, name)) = file else {
        return error_json(StatusCode::BAD_REQUEST, "missing 'file' field");
    };

    let pcm = match tokio::task::spawn_blocking(move || crate::audio::decode_to_pcm_mono_16k(&bytes, &name)).await {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => return error_json(StatusCode::BAD_REQUEST, format!("decode audio: {e}")),
        Err(e) => return error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("decode task: {e}")),
    };

    // Spawn the load so a timeout does NOT drop it: the detached task keeps
    // loading and caches the engine into the slot for the next request.
    let slot = state.stt_slot.clone();
    let factory = state.stt_factory.clone();
    let load = tokio::spawn(async move {
        slot.get_or_load(crate::lifecycle::now_ms(), move || factory()).await
    });
    let engine = match tokio::time::timeout(LOAD_TIMEOUT, load).await {
        Ok(Ok(Ok(e))) => e,
        Ok(Ok(Err(e))) => return error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("load stt: {e}")),
        Ok(Err(e)) => return error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("load stt task: {e}")),
        Err(_) => return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"status": "loading"}))).into_response(),
    };

    let lang = language.clone();
    match tokio::task::spawn_blocking(move || engine.transcribe(&pcm, lang.as_deref())).await {
        Ok(Ok(r)) => Json(json!({"text": r.text, "language": r.language})).into_response(),
        Ok(Err(e)) => error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("transcribe: {e}")),
        Err(e) => error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("transcribe task: {e}")),
    }
}

#[derive(Deserialize)]
pub struct SpeechRequest {
    pub input: String,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub speed: Option<f32>,
    #[serde(default)]
    pub response_format: Option<String>,
    // `model` accepted and ignored — the sidecar runs what it's configured with.
    #[serde(default)]
    #[allow(dead_code)]
    pub model: Option<String>,
}

/// POST /v1/audio/speech — OpenAI JSON compatible. Formats: wav | opus.
pub async fn speech(State(state): State<AppState>, Json(req): Json<SpeechRequest>) -> Response {
    if let Err(resp) = gate(&state, state.tts_spec) {
        return resp;
    }
    if req.input.trim().is_empty() {
        return error_json(StatusCode::BAD_REQUEST, "input is empty");
    }
    let format = req.response_format.as_deref().unwrap_or("opus");
    if !matches!(format, "wav" | "opus") {
        return error_json(StatusCode::BAD_REQUEST, format!("unsupported response_format '{format}' (wav|opus)"));
    }

    // Spawn the load so a timeout does NOT drop it: the detached task keeps
    // loading and caches the engine into the slot for the next request.
    let slot = state.tts_slot.clone();
    let factory = state.tts_factory.clone();
    let load = tokio::spawn(async move {
        slot.get_or_load(crate::lifecycle::now_ms(), move || factory()).await
    });
    let engine = match tokio::time::timeout(LOAD_TIMEOUT, load).await {
        Ok(Ok(Ok(e))) => e,
        Ok(Ok(Err(e))) => return error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("load tts: {e}")),
        Ok(Err(e)) => return error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("load tts task: {e}")),
        Err(_) => return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"status": "loading"}))).into_response(),
    };

    let voice = req.voice.clone().unwrap_or_else(|| state.default_voice.clone());
    let speed = req.speed.unwrap_or(1.0).clamp(0.25, 4.0);
    let text = req.input.clone();
    let fmt = format.to_string();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(Vec<u8>, &'static str)> {
        let audio = engine.synthesize(&text, &voice, speed)?;
        match fmt.as_str() {
            "wav" => Ok((crate::audio::encode_wav(&audio.samples, audio.sample_rate)?, "audio/wav")),
            _ => Ok((crate::audio::ogg_opus::encode(&audio.samples, audio.sample_rate)?, "audio/ogg")),
        }
    })
    .await;

    match result {
        Ok(Ok((bytes, content_type))) => ([(header::CONTENT_TYPE, content_type)], bytes).into_response(),
        Ok(Err(e)) => error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("synthesize: {e}")),
        Err(e) => error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("synthesize task: {e}")),
    }
}

/// GET /v1/voice/status — engine + model states for the supervisor/tool.
pub async fn status(State(state): State<AppState>) -> Response {
    let now = crate::lifecycle::now_ms();
    Json(json!({
        "stt": {
            "model": state.stt_spec.id,
            "model_state": state.models.state(state.stt_spec),
            "engine_loaded": state.stt_slot.is_loaded().await,
        },
        "tts": {
            "model": state.tts_spec.id,
            "model_state": state.models.state(state.tts_spec),
            "engine_loaded": state.tts_slot.is_loaded().await,
        },
        "uptime_secs": now.saturating_sub(state.started_ms) / 1000,
    }))
    .into_response()
}

/// POST /v1/voice/warmup — fire-and-forget: ensure models then load engines.
pub async fn warmup(State(state): State<AppState>) -> Response {
    let s = state.clone();
    tokio::spawn(async move {
        for (spec, which) in [(s.stt_spec, "stt"), (s.tts_spec, "tts")] {
            if let Err(e) = s.models.ensure(spec).await {
                tracing::warn!(model = spec.id, error = %e, "warmup ensure failed");
                continue; // warm the other model independently
            }
            let now = crate::lifecycle::now_ms();
            let res = match which {
                "stt" => {
                    let f = s.stt_factory.clone();
                    s.stt_slot.get_or_load(now, move || f()).await.map(|_| ())
                }
                _ => {
                    let f = s.tts_factory.clone();
                    s.tts_slot.get_or_load(now, move || f()).await.map(|_| ())
                }
            };
            if let Err(e) = res {
                tracing::warn!(which, error = %e, "warmup engine load failed");
            }
        }
        tracing::info!("warmup complete");
    });
    (StatusCode::ACCEPTED, Json(json!({"started": true}))).into_response()
}
