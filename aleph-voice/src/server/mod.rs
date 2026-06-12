//! Loopback HTTP server: OpenAI-compatible STT/TTS endpoints + status/warmup.

pub mod auth;
pub mod handlers;

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use axum::middleware;
use axum::routing::{get, post};
use axum::Router;

use crate::engine::{SttEngine, TtsEngine};
use crate::lifecycle::EngineSlot;
use crate::models::manifest::ModelSpec;
use crate::models::ModelManager;

/// Factory closures let tests inject mocks and main inject sherpa loads.
pub type SttFactory = Arc<dyn Fn() -> anyhow::Result<Arc<dyn SttEngine>> + Send + Sync>;
pub type TtsFactory = Arc<dyn Fn() -> anyhow::Result<Arc<dyn TtsEngine>> + Send + Sync>;

#[derive(Clone)]
pub struct AppState {
    pub token: String,
    pub models: Arc<ModelManager>,
    pub stt_spec: &'static ModelSpec,
    pub tts_spec: &'static ModelSpec,
    pub stt_slot: Arc<EngineSlot<dyn SttEngine>>,
    pub tts_slot: Arc<EngineSlot<dyn TtsEngine>>,
    pub stt_factory: SttFactory,
    pub tts_factory: TtsFactory,
    pub default_voice: String,
    /// Epoch ms of last request — feeds the deep-idle process exit.
    pub last_activity_ms: Arc<AtomicU64>,
    pub started_ms: u64,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/audio/transcriptions", post(handlers::transcriptions))
        .route("/v1/audio/speech", post(handlers::speech))
        .route("/v1/voice/status", get(handlers::status))
        .route("/v1/voice/warmup", post(handlers::warmup))
        // axum's default body cap is 2 MB — voice files routinely exceed it.
        // 25 MB mirrors the existing whisper.rs MAX_AUDIO_BYTES ceiling.
        .layer(axum::extract::DefaultBodyLimit::max(25 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), auth::require_bearer))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::mock::{MockStt, MockTts};
    use crate::models::manifest::ModelSpec;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    static READY_SPEC: ModelSpec = ModelSpec { id: "ready-model", urls: &[], sha256: "0", marker: "marker.onnx" };
    static MISSING_SPEC: ModelSpec = ModelSpec {
        id: "missing-model",
        urls: &["http://127.0.0.1:1/x.tar.bz2"],
        sha256: "0",
        marker: "marker.onnx",
    };

    fn test_state(root: &std::path::Path, stt_spec: &'static ModelSpec, tts_spec: &'static ModelSpec) -> AppState {
        // Mark READY_SPEC present on disk.
        let d = root.join("ready-model");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("marker.onnx"), b"x").unwrap();
        AppState {
            token: "tok".into(),
            models: Arc::new(ModelManager::new(root.to_path_buf())),
            stt_spec,
            tts_spec,
            stt_slot: Arc::new(EngineSlot::new()),
            tts_slot: Arc::new(EngineSlot::new()),
            stt_factory: Arc::new(|| Ok(Arc::new(MockStt) as Arc<dyn SttEngine>)),
            tts_factory: Arc::new(|| Ok(Arc::new(MockTts) as Arc<dyn TtsEngine>)),
            default_voice: "zf_001".into(),
            last_activity_ms: Arc::new(AtomicU64::new(0)),
            started_ms: 0,
        }
    }

    fn authed(req: axum::http::request::Builder) -> axum::http::request::Builder {
        req.header(header::AUTHORIZATION, "Bearer tok")
    }

    #[tokio::test]
    async fn rejects_missing_or_bad_token() {
        let tmp = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp.path(), &READY_SPEC, &READY_SPEC));
        for auth in [None, Some("Bearer wrong")] {
            let mut req = Request::builder().uri("/v1/voice/status").method("GET");
            if let Some(a) = auth {
                req = req.header(header::AUTHORIZATION, a);
            }
            let resp = app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn transcribes_wav_via_mock() {
        let tmp = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp.path(), &READY_SPEC, &READY_SPEC));
        let pcm: Vec<f32> = vec![0.1; 1600];
        let wav = crate::audio::encode_wav(&pcm, 16_000).unwrap();
        let boundary = "XBOUND";
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes());
        body.extend_from_slice(&wav);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let req = authed(Request::builder().uri("/v1/audio/transcriptions").method("POST"))
            .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(v["text"].as_str().unwrap().contains("samples"));
    }

    #[tokio::test]
    async fn speech_emits_wav_and_opus() {
        let tmp = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp.path(), &READY_SPEC, &READY_SPEC));
        for (fmt, ct, magic) in [("wav", "audio/wav", b"RIFF".as_slice()), ("opus", "audio/ogg", b"OggS".as_slice())] {
            let req = authed(Request::builder().uri("/v1/audio/speech").method("POST"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"input":"你好","response_format":"{fmt}"}}"#)))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "fmt={fmt}");
            assert_eq!(resp.headers()[header::CONTENT_TYPE], ct);
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            assert!(bytes.starts_with(magic), "fmt={fmt}");
        }
    }

    #[tokio::test]
    async fn rejects_unsupported_format() {
        let tmp = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp.path(), &READY_SPEC, &READY_SPEC));
        let req = authed(Request::builder().uri("/v1/audio/speech").method("POST"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"input":"hi","response_format":"mp3"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn missing_model_returns_503_downloading() {
        let tmp = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp.path(), &MISSING_SPEC, &READY_SPEC));
        let req = authed(Request::builder().uri("/v1/audio/transcriptions").method("POST"))
            .header(header::CONTENT_TYPE, "multipart/form-data; boundary=B")
            .body(Body::from("--B--\r\n"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["status"], "downloading");
    }

    #[tokio::test]
    async fn status_reports_states() {
        let tmp = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp.path(), &READY_SPEC, &READY_SPEC));
        let req = authed(Request::builder().uri("/v1/voice/status").method("GET"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["stt"]["model_state"]["state"], "ready");
        assert_eq!(v["stt"]["engine_loaded"], false);
    }
}
