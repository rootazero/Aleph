//! Unit tests for the OpenAI Whisper provider.
//!
//! Network integration is exercised by manual smoke tests with a real API
//! key — here we only cover input plumbing and error paths that don't
//! depend on hitting the wire.

use super::*;
use crate::generation::{GenerationParams, GenerationRequest, GenerationType};

#[test]
fn rejects_empty_api_key() {
    let err = OpenAiWhisperProvider::new("   ", None, None, None).unwrap_err();
    assert!(matches!(err, GenerationError::AuthenticationError { .. }));
}

#[test]
fn endpoint_includes_transcription_path() {
    let provider = OpenAiWhisperProvider::new("sk-test", None, None, None).unwrap();
    assert!(
        provider.endpoint.ends_with("/v1/audio/transcriptions"),
        "expected resolved transcription endpoint, got: {}",
        provider.endpoint
    );
}

#[test]
fn default_model_is_whisper_1() {
    let provider = OpenAiWhisperProvider::new("sk-test", None, None, None).unwrap();
    assert_eq!(provider.default_model(), Some("whisper-1"));
}

#[test]
fn supports_transcription_only() {
    let provider = OpenAiWhisperProvider::new("sk-test", None, None, None).unwrap();
    assert!(provider.supports(GenerationType::Transcription));
    assert!(!provider.supports(GenerationType::Speech));
    assert!(!provider.supports(GenerationType::Image));
}

#[tokio::test]
async fn rejects_wrong_generation_type() -> Result<(), GenerationError> {
    let provider = OpenAiWhisperProvider::new("sk-test", None, None, None)?;
    let request = GenerationRequest::image("a cat");
    let err = provider.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::UnsupportedGenerationTypeError { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn rejects_missing_audio_source() -> Result<(), GenerationError> {
    let provider = OpenAiWhisperProvider::new("sk-test", None, None, None)?;
    let request = GenerationRequest::new(GenerationType::Transcription, "");
    let err = provider.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::InvalidParametersError { .. }
    ));
    Ok(())
}

#[test]
fn decodes_base64_data_url() {
    // "hello" base64-encoded as a fake mp3 payload.
    let payload = "data:audio/mpeg;base64,aGVsbG8=";
    let (bytes, name, mime) = decode_data_url(payload.strip_prefix("data:").unwrap()).unwrap();
    assert_eq!(bytes, b"hello");
    assert_eq!(name, "audio.mpeg");
    assert_eq!(mime, "audio/mpeg");
}

#[test]
fn data_url_without_base64_is_inline() {
    let (bytes, _, _) = decode_data_url("text/plain,raw_payload").unwrap();
    assert_eq!(bytes, b"raw_payload");
}

#[test]
fn mime_from_extension_known_types() {
    assert_eq!(mime_from_extension(Some("mp3")), "audio/mpeg");
    assert_eq!(mime_from_extension(Some("WAV")), "audio/wav");
    assert_eq!(mime_from_extension(Some("flac")), "audio/flac");
    assert_eq!(mime_from_extension(None), "application/octet-stream");
    assert_eq!(mime_from_extension(Some("xyz")), "application/octet-stream");
}

#[tokio::test]
async fn rejects_oversized_file() -> Result<(), GenerationError> {
    // Build a 26 MB in-memory buffer wrapped in a base64 data URL.
    let big_size = 26 * 1024 * 1024;
    let buf = vec![0u8; big_size];
    let encoded = base64::engine::general_purpose::STANDARD.encode(&buf);
    let url = format!("data:audio/mpeg;base64,{}", encoded);

    let provider = OpenAiWhisperProvider::new("sk-test", None, None, None)?;
    let params = GenerationParams::builder().reference_audio(url).build();
    let request = GenerationRequest::new(GenerationType::Transcription, "").with_params(params);
    let err = provider.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::InvalidParametersError { .. }
    ));
    Ok(())
}
