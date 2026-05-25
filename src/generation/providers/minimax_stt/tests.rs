//! Unit tests for the MiniMax STT provider.

use super::*;
use crate::generation::{GenerationParams, GenerationRequest, GenerationType};

#[test]
fn rejects_empty_api_key() {
    let err = MinimaxSttProvider::new("   ", None, None).unwrap_err();
    assert!(matches!(err, GenerationError::AuthenticationError { .. }));
}

#[test]
fn endpoint_appends_audio_to_text_path() {
    let p = MinimaxSttProvider::new("k", None, None).unwrap();
    assert!(
        p.endpoint.ends_with("/v1/audio_to_text"),
        "got {}",
        p.endpoint
    );
}

#[test]
fn endpoint_preserves_full_path() {
    let p = MinimaxSttProvider::new(
        "k",
        Some("https://api.minimaxi.com/v1/audio_to_text".into()),
        None,
    )
    .unwrap();
    assert_eq!(p.endpoint, "https://api.minimaxi.com/v1/audio_to_text");
}

#[test]
fn default_model_is_whisper_1() {
    let p = MinimaxSttProvider::new("k", None, None).unwrap();
    assert_eq!(p.default_model(), Some("whisper-1"));
}

#[test]
fn supports_transcription_only() {
    let p = MinimaxSttProvider::new("k", None, None).unwrap();
    assert!(p.supports(GenerationType::Transcription));
    assert!(!p.supports(GenerationType::Speech));
}

#[tokio::test]
async fn rejects_wrong_generation_type() {
    let p = MinimaxSttProvider::new("k", None, None).unwrap();
    let request = GenerationRequest::speech("hello");
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::UnsupportedGenerationTypeError { .. }
    ));
}

#[tokio::test]
async fn rejects_missing_group_id() {
    let p = MinimaxSttProvider::new("k", None, None).unwrap();
    let request = GenerationRequest::new(GenerationType::Transcription, "/tmp/missing.mp3");
    let err = p.generate(request).await.unwrap_err();
    match err {
        GenerationError::InvalidParametersError { parameter, .. } => {
            assert_eq!(parameter, Some("group_id".to_string()));
        }
        other => panic!("expected InvalidParametersError(group_id), got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_missing_audio_source() {
    let mut extra = std::collections::HashMap::new();
    extra.insert("group_id".to_string(), serde_json::json!("g-123"));
    let params = GenerationParams {
        extra,
        ..Default::default()
    };
    let p = MinimaxSttProvider::new("k", None, None).unwrap();
    let request = GenerationRequest::new(GenerationType::Transcription, "").with_params(params);
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(err, GenerationError::InvalidParametersError { .. }));
}

#[tokio::test]
async fn rejects_http_audio_url() {
    let mut extra = std::collections::HashMap::new();
    extra.insert("group_id".to_string(), serde_json::json!("g-123"));
    let params = GenerationParams {
        extra,
        reference_audio: Some("https://example.com/audio.mp3".into()),
        ..Default::default()
    };
    let p = MinimaxSttProvider::new("k", None, None).unwrap();
    let request = GenerationRequest::new(GenerationType::Transcription, "").with_params(params);
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(err, GenerationError::InvalidParametersError { .. }));
}

#[tokio::test]
async fn rejects_oversized_inline_audio() {
    let big = vec![0u8; (MAX_AUDIO_BYTES + 1) as usize];
    let url = format!(
        "data:audio/mpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&big)
    );
    let mut extra = std::collections::HashMap::new();
    extra.insert("group_id".to_string(), serde_json::json!("g-123"));
    let params = GenerationParams {
        extra,
        reference_audio: Some(url),
        ..Default::default()
    };
    let p = MinimaxSttProvider::new("k", None, None).unwrap();
    let request = GenerationRequest::new(GenerationType::Transcription, "").with_params(params);
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(err, GenerationError::InvalidParametersError { .. }));
}

#[test]
fn base_resp_zero_is_success() {
    let r = MinimaxBaseResp {
        status_code: 0,
        status_msg: Some("success".into()),
    };
    assert!(r.is_success());
}

#[test]
fn base_resp_non_zero_is_failure() {
    let r = MinimaxBaseResp {
        status_code: 1004,
        status_msg: Some("invalid api key".into()),
    };
    assert!(!r.is_success());
    assert_eq!(r.best_message(), "invalid api key");
}

#[test]
fn base_resp_fallback_message_when_missing() {
    let r = MinimaxBaseResp {
        status_code: 2042,
        status_msg: None,
    };
    assert_eq!(r.best_message(), "MiniMax status_code=2042");
}

#[test]
fn decode_data_url_base64_round_trip() {
    let (bytes, name, mime) = decode_data_url("audio/mpeg;base64,aGVsbG8=").unwrap();
    assert_eq!(bytes, b"hello");
    assert_eq!(mime, "audio/mpeg");
    assert_eq!(name, "audio.mpeg");
}

#[test]
fn mime_from_extension_covers_common_audio() {
    assert_eq!(mime_from_extension(Some("mp3")), "audio/mpeg");
    assert_eq!(mime_from_extension(Some("WAV")), "audio/wav");
    assert_eq!(mime_from_extension(Some("flac")), "audio/flac");
    assert_eq!(mime_from_extension(None), "application/octet-stream");
}
