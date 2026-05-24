//! Unit tests for the Deepgram TTS provider. Network integration is exercised
//! manually with a real API key — we cover plumbing and error paths that
//! don't depend on hitting the wire.

use super::*;
use crate::generation::{GenerationParams, GenerationRequest, GenerationType};

#[test]
fn rejects_empty_api_key() {
    let err = DeepgramTtsProvider::new("   ", None, None).unwrap_err();
    assert!(matches!(err, GenerationError::AuthenticationError { .. }));
}

#[test]
fn endpoint_appends_speak_path() {
    let p = DeepgramTtsProvider::new("k", None, None).unwrap();
    assert!(p.endpoint.ends_with("/v1/speak"), "got {}", p.endpoint);
}

#[test]
fn endpoint_preserves_full_path() {
    let p = DeepgramTtsProvider::new("k", Some("https://api.deepgram.com/v1/speak".into()), None).unwrap();
    assert_eq!(p.endpoint, "https://api.deepgram.com/v1/speak");
}

#[test]
fn default_model_is_aura_asteria_en() {
    let p = DeepgramTtsProvider::new("k", None, None).unwrap();
    assert_eq!(p.default_model(), Some("aura-asteria-en"));
}

#[test]
fn supports_speech_only() {
    let p = DeepgramTtsProvider::new("k", None, None).unwrap();
    assert!(p.supports(GenerationType::Speech));
    assert!(!p.supports(GenerationType::Transcription));
    assert!(!p.supports(GenerationType::Image));
}

#[tokio::test]
async fn rejects_wrong_generation_type() {
    let p = DeepgramTtsProvider::new("k", None, None).unwrap();
    let request = GenerationRequest::image("a cat");
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::UnsupportedGenerationTypeError { .. }
    ));
}

#[tokio::test]
async fn rejects_empty_input() {
    let p = DeepgramTtsProvider::new("k", None, None).unwrap();
    let request = GenerationRequest::speech("   ");
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(err, GenerationError::InvalidParametersError { .. }));
}

#[tokio::test]
async fn rejects_oversized_input() {
    // 2001 chars — one above the cap.
    let text = "a".repeat(MAX_INPUT_CHARS + 1);
    let p = DeepgramTtsProvider::new("k", None, None).unwrap();
    let request = GenerationRequest::speech(text);
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(err, GenerationError::InvalidParametersError { .. }));
}

#[tokio::test]
async fn rejects_unknown_format() {
    let p = DeepgramTtsProvider::new("k", None, None).unwrap();
    let params = GenerationParams::builder().format("ogg-vorbis").build();
    let request = GenerationRequest::speech("hi").with_params(params);
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(err, GenerationError::UnsupportedFormatError { .. }));
}

#[test]
fn static_voice_list_is_non_empty_and_covers_known_models() {
    let voices = DeepgramTtsProvider::static_voice_list();
    assert_eq!(voices.len(), AVAILABLE_VOICES.len());
    assert!(voices.iter().any(|v| v.id == "aura-asteria-en"));
    assert!(voices.iter().any(|v| v.id == "aura-zeus-en"));
}
