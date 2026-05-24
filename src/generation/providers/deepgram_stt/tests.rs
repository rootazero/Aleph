//! Unit tests for the Deepgram STT provider.

use super::*;
use crate::generation::{GenerationParams, GenerationRequest, GenerationType};

#[test]
fn rejects_empty_api_key() {
    let err = DeepgramSttProvider::new("   ", None, None).unwrap_err();
    assert!(matches!(err, GenerationError::AuthenticationError { .. }));
}

#[test]
fn endpoint_appends_listen_path() {
    let provider = DeepgramSttProvider::new("k", None, None).unwrap();
    assert!(provider.endpoint.ends_with("/v1/listen"));
}

#[test]
fn endpoint_preserves_full_path() {
    let provider = DeepgramSttProvider::new(
        "k",
        Some("https://api.deepgram.com/v1/listen".into()),
        None,
    )
    .unwrap();
    assert_eq!(provider.endpoint, "https://api.deepgram.com/v1/listen");
}

#[test]
fn default_model_is_nova_3() {
    let provider = DeepgramSttProvider::new("k", None, None).unwrap();
    assert_eq!(provider.default_model(), Some("nova-3"));
}

#[test]
fn supports_transcription_only() {
    let provider = DeepgramSttProvider::new("k", None, None).unwrap();
    assert!(provider.supports(GenerationType::Transcription));
    assert!(!provider.supports(GenerationType::Speech));
}

#[tokio::test]
async fn rejects_wrong_generation_type() {
    let provider = DeepgramSttProvider::new("k", None, None).unwrap();
    let request = GenerationRequest::speech("hello");
    let err = provider.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::UnsupportedGenerationTypeError { .. }
    ));
}

#[tokio::test]
async fn rejects_missing_audio_source() {
    let provider = DeepgramSttProvider::new("k", None, None).unwrap();
    let request = GenerationRequest::new(GenerationType::Transcription, "");
    let err = provider.generate(request).await.unwrap_err();
    assert!(matches!(err, GenerationError::InvalidParametersError { .. }));
}

#[test]
fn mime_from_extension_known_types() {
    assert_eq!(mime_from_extension(Some("mp3")), "audio/mpeg");
    assert_eq!(mime_from_extension(Some("FLAC")), "audio/flac");
    assert_eq!(mime_from_extension(Some("unknown")), "application/octet-stream");
}

#[test]
fn data_url_base64_round_trip() {
    let (bytes, mime) = decode_data_url("audio/mpeg;base64,aGVsbG8=").unwrap();
    assert_eq!(bytes, b"hello");
    assert_eq!(mime, "audio/mpeg");
}

#[tokio::test]
async fn rejects_oversized_inline_audio() {
    // 251 MB synthetic file via data URL; we never make a network call.
    let big = vec![0u8; (MAX_AUDIO_BYTES + 1) as usize];
    let url = format!(
        "data:audio/mpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&big)
    );
    let provider = DeepgramSttProvider::new("k", None, None).unwrap();
    let params = GenerationParams::builder().reference_audio(url).build();
    let request =
        GenerationRequest::new(GenerationType::Transcription, "").with_params(params);
    let err = provider.generate(request).await.unwrap_err();
    assert!(matches!(err, GenerationError::InvalidParametersError { .. }));
}
