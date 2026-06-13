//! Unit tests for the Cartesia TTS provider. Wire-level integration is
//! exercised manually against the real API.

use super::*;
use crate::generation::{GenerationParams, GenerationRequest, GenerationType};

#[test]
fn rejects_empty_api_key() {
    let err = CartesiaProvider::new("   ", None, None, None).unwrap_err();
    assert!(matches!(err, GenerationError::AuthenticationError { .. }));
}

#[test]
fn endpoint_appends_tts_bytes_path() {
    let p = CartesiaProvider::new("k", None, None, None).unwrap();
    assert!(p.endpoint.ends_with("/tts/bytes"));
}

#[test]
fn endpoint_preserves_full_path() {
    let p = CartesiaProvider::new(
        "k",
        Some("https://api.cartesia.ai/tts/bytes".into()),
        None,
        None,
    )
    .unwrap();
    assert_eq!(p.endpoint, "https://api.cartesia.ai/tts/bytes");
}

#[test]
fn default_model_is_sonic_2() {
    let p = CartesiaProvider::new("k", None, None, None).unwrap();
    assert_eq!(p.default_model(), Some("sonic-2"));
}

#[test]
fn default_voice_is_set() {
    let p = CartesiaProvider::new("k", None, None, None).unwrap();
    assert_eq!(p.default_voice, DEFAULT_VOICE_ID);
}

#[test]
fn supports_speech_only() {
    let p = CartesiaProvider::new("k", None, None, None).unwrap();
    assert!(p.supports(GenerationType::Speech));
    assert!(!p.supports(GenerationType::Transcription));
    assert!(!p.supports(GenerationType::Image));
}

#[tokio::test]
async fn rejects_wrong_generation_type() {
    let p = CartesiaProvider::new("k", None, None, None).unwrap();
    let request = GenerationRequest::image("a cat");
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::UnsupportedGenerationTypeError { .. }
    ));
}

#[tokio::test]
async fn rejects_empty_input() {
    let p = CartesiaProvider::new("k", None, None, None).unwrap();
    let request = GenerationRequest::speech("   ");
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::InvalidParametersError { .. }
    ));
}

#[tokio::test]
async fn rejects_oversized_input() {
    let text = "a".repeat(MAX_INPUT_CHARS + 1);
    let p = CartesiaProvider::new("k", None, None, None).unwrap();
    let request = GenerationRequest::speech(text);
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::InvalidParametersError { .. }
    ));
}

#[tokio::test]
async fn rejects_unknown_format() {
    let p = CartesiaProvider::new("k", None, None, None).unwrap();
    let params = GenerationParams::builder().format("flac").build();
    let request = GenerationRequest::speech("hi").with_params(params);
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::UnsupportedFormatError { .. }
    ));
}

#[test]
fn voice_selector_serializes_with_mode_tag() {
    let sel = CartesiaVoiceSelector::Id {
        id: DEFAULT_VOICE_ID.to_string(),
    };
    let json = serde_json::to_string(&sel).unwrap();
    assert!(json.contains("\"mode\":\"id\""));
    assert!(json.contains(DEFAULT_VOICE_ID));
}

#[test]
fn output_format_serializes_with_three_fields() {
    let f = CartesiaOutputFormat {
        container: "mp3".into(),
        encoding: "mp3".into(),
        sample_rate: 44_100,
    };
    let json = serde_json::to_string(&f).unwrap();
    assert!(json.contains("\"container\":\"mp3\""));
    assert!(json.contains("\"encoding\":\"mp3\""));
    assert!(json.contains("\"sample_rate\":44100"));
}

#[test]
fn error_envelope_picks_nested_detail() {
    let e = CartesiaError {
        error: Some(CartesiaErrorDetail {
            message: Some("invalid voice".into()),
            r#type: Some("validation".into()),
        }),
        message: None,
    };
    assert_eq!(e.best_message().as_deref(), Some("invalid voice"));
}

#[test]
fn error_envelope_falls_back_to_message() {
    let e = CartesiaError {
        error: None,
        message: Some("forbidden".into()),
    };
    assert_eq!(e.best_message().as_deref(), Some("forbidden"));
}

#[test]
fn static_voice_list_includes_default_voice() {
    let voices = CartesiaProvider::static_voice_list();
    assert!(voices.iter().any(|v| v.id == DEFAULT_VOICE_ID));
}

#[test]
fn speed_multiplier_maps_onto_cartesia_offset() {
    // 1.0 multiplier (normal) → 0.0 offset (normal), not "fastest".
    assert_eq!(map_speed_to_cartesia(1.0), 0.0);
    // Faster/slower map to positive/negative offsets.
    assert_eq!(map_speed_to_cartesia(1.5), 0.5);
    assert_eq!(map_speed_to_cartesia(0.5), -0.5);
    // Out-of-window multipliers clamp into Cartesia's -1.0..=1.0.
    assert_eq!(map_speed_to_cartesia(2.5), 1.0);
    assert_eq!(map_speed_to_cartesia(-3.0), -1.0);
}
