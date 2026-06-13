//! Unit tests for the MiniMax T2A (text-to-speech) provider. Network
//! integration is exercised manually with a real API key; these tests cover
//! construction, URL normalization, request shaping, and response decoding —
//! everything that doesn't require a live endpoint.

use super::*;
use crate::generation::{GenerationParams, GenerationRequest, GenerationType};

// === Construction ===

#[test]
fn rejects_empty_api_key() {
    let err = MinimaxTtsProvider::new("   ", None, None, None).unwrap_err();
    assert!(matches!(err, GenerationError::AuthenticationError { .. }));
}

#[test]
fn applies_defaults() {
    let p = MinimaxTtsProvider::new("k", None, None, None).unwrap();
    assert_eq!(p.default_model(), Some("speech-2.6-turbo"));
    assert_eq!(p.voice, "Chinese (Mandarin)_Gentle_Boy");
    assert_eq!(p.endpoint, "https://api.minimaxi.com/v1/t2a_v2");
}

#[test]
fn honours_overrides() {
    let p = MinimaxTtsProvider::new(
        "k",
        Some("https://api.minimax.io".into()),
        Some("speech-02-hd".into()),
        Some("Wise_Woman".into()),
    )
    .unwrap();
    assert_eq!(p.endpoint, "https://api.minimax.io/v1/t2a_v2");
    assert_eq!(p.default_model(), Some("speech-02-hd"));
    assert_eq!(p.voice, "Wise_Woman");
}

#[test]
fn blank_overrides_fall_back_to_defaults() {
    let p = MinimaxTtsProvider::new("k", Some("  ".into()), Some("  ".into()), Some("  ".into()))
        .unwrap();
    assert_eq!(p.default_model(), Some("speech-2.6-turbo"));
    assert_eq!(p.voice, "Chinese (Mandarin)_Gentle_Boy");
    assert_eq!(p.endpoint, "https://api.minimaxi.com/v1/t2a_v2");
}

#[test]
fn supports_speech_only() {
    let p = MinimaxTtsProvider::new("k", None, None, None).unwrap();
    assert!(p.supports(GenerationType::Speech));
    assert!(!p.supports(GenerationType::Transcription));
    assert!(!p.supports(GenerationType::Image));
}

// === Endpoint normalization ===

#[test]
fn resolve_endpoint_appends_path_to_bare_host() {
    assert_eq!(
        resolve_endpoint("https://api.minimaxi.com"),
        "https://api.minimaxi.com/v1/t2a_v2"
    );
}

#[test]
fn resolve_endpoint_strips_trailing_slash() {
    assert_eq!(
        resolve_endpoint("https://api.minimaxi.com/"),
        "https://api.minimaxi.com/v1/t2a_v2"
    );
}

#[test]
fn resolve_endpoint_does_not_double_v1() {
    // A pasted LLM-style base URL ending in /v1 must not become /v1/v1/t2a_v2.
    assert_eq!(
        resolve_endpoint("https://api.minimaxi.com/v1"),
        "https://api.minimaxi.com/v1/t2a_v2"
    );
}

#[test]
fn resolve_endpoint_strips_anthropic_suffix() {
    assert_eq!(
        resolve_endpoint("https://proxy.example.com/anthropic"),
        "https://proxy.example.com/v1/t2a_v2"
    );
}

#[test]
fn resolve_endpoint_preserves_complete_url() {
    assert_eq!(
        resolve_endpoint("https://api.minimaxi.com/v1/t2a_v2"),
        "https://api.minimaxi.com/v1/t2a_v2"
    );
}

#[test]
fn resolve_endpoint_preserves_embedded_group_id() {
    assert_eq!(
        resolve_endpoint("https://api.minimaxi.com/v1/t2a_v2?GroupId=g1"),
        "https://api.minimaxi.com/v1/t2a_v2?GroupId=g1"
    );
}

// === GroupId (optional, TTS does not require it) ===

#[test]
fn append_group_id_adds_query_when_supplied() {
    assert_eq!(
        append_group_id("https://h/v1/t2a_v2", Some("g1")),
        "https://h/v1/t2a_v2?GroupId=g1"
    );
}

#[test]
fn append_group_id_noop_when_absent_or_blank() {
    assert_eq!(append_group_id("https://h/v1/t2a_v2", None), "https://h/v1/t2a_v2");
    assert_eq!(
        append_group_id("https://h/v1/t2a_v2", Some("   ")),
        "https://h/v1/t2a_v2"
    );
}

#[test]
fn append_group_id_does_not_duplicate() {
    assert_eq!(
        append_group_id("https://h/v1/t2a_v2?GroupId=x", Some("g1")),
        "https://h/v1/t2a_v2?GroupId=x"
    );
}

#[test]
fn append_group_id_uses_ampersand_when_query_exists() {
    assert_eq!(
        append_group_id("https://h/v1/t2a_v2?foo=bar", Some("g1")),
        "https://h/v1/t2a_v2?foo=bar&GroupId=g1"
    );
}

// === Request shaping ===

#[test]
fn build_request_produces_expected_shape() {
    let req = build_request("speech-2.6-turbo", "hello", "Wise_Woman", Some(1.2), "mp3");
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v["model"].as_str(), Some("speech-2.6-turbo"));
    assert_eq!(v["text"].as_str(), Some("hello"));
    assert_eq!(v["stream"].as_bool(), Some(false));
    assert_eq!(v["voice_setting"]["voice_id"].as_str(), Some("Wise_Woman"));
    assert_eq!(v["audio_setting"]["format"].as_str(), Some("mp3"));
    assert_eq!(v["audio_setting"]["sample_rate"].as_u64(), Some(32_000));
}

#[test]
fn build_request_clamps_speed() {
    let fast = build_request("m", "t", "v", Some(3.0), "mp3");
    let slow = build_request("m", "t", "v", Some(0.1), "mp3");
    assert_eq!(fast.voice_setting.speed, Some(2.0));
    assert_eq!(slow.voice_setting.speed, Some(0.5));
}

#[test]
fn build_request_omits_speed_when_none() {
    let req = build_request("m", "t", "v", None, "mp3");
    let v = serde_json::to_value(&req).unwrap();
    assert!(v["voice_setting"].get("speed").is_none());
}

#[test]
fn clamp_speed_bounds() {
    assert!((clamp_speed(3.0) - 2.0).abs() < f32::EPSILON);
    assert!((clamp_speed(0.1) - 0.5).abs() < f32::EPSILON);
    assert!((clamp_speed(1.0) - 1.0).abs() < f32::EPSILON);
}

// === Format handling ===

#[test]
fn resolve_format_defaults_to_mp3() {
    assert_eq!(resolve_format(None).unwrap(), "mp3");
}

#[test]
fn resolve_format_accepts_supported() {
    assert_eq!(resolve_format(Some("mp3")).unwrap(), "mp3");
    assert_eq!(resolve_format(Some("flac")).unwrap(), "flac");
    assert_eq!(resolve_format(Some("pcm")).unwrap(), "pcm");
}

#[test]
fn resolve_format_rejects_unknown() {
    let err = resolve_format(Some("ogg")).unwrap_err();
    assert!(matches!(err, GenerationError::UnsupportedFormatError { .. }));
}

#[test]
fn content_type_mapping() {
    assert_eq!(content_type_for_format("mp3"), "audio/mpeg");
    assert_eq!(content_type_for_format("flac"), "audio/flac");
    assert_eq!(content_type_for_format("pcm"), "audio/pcm");
}

// === Response decoding ===

#[test]
fn parses_success_response_and_hex_decodes_audio() {
    // "48656c6c6f" is "Hello" hex-encoded — the encoding MiniMax actually uses.
    let json = r#"{
        "data": {"audio": "48656c6c6f", "status": 2},
        "extra_info": {"audio_length": 1200, "audio_format": "mp3"},
        "trace_id": "trace-123",
        "base_resp": {"status_code": 0, "status_msg": "success"}
    }"#;
    let parsed: T2aResponse = serde_json::from_str(json).unwrap();
    assert!(parsed.base_resp.is_success());
    let audio_hex = parsed.data.unwrap().audio.unwrap();
    assert_eq!(hex::decode(&audio_hex).unwrap(), b"Hello");
    assert_eq!(parsed.extra_info.unwrap().audio_length, Some(1200));
}

#[test]
fn parses_logical_failure_envelope() {
    // A 200 OK with status_code != 0 is still a failure on MiniMax.
    let json = r#"{"base_resp": {"status_code": 1004, "status_msg": "invalid api key"}}"#;
    let parsed: T2aResponse = serde_json::from_str(json).unwrap();
    assert!(!parsed.base_resp.is_success());
    assert_eq!(parsed.base_resp.best_message(), "invalid api key");
    assert!(parsed.data.is_none());
}

#[test]
fn base_resp_best_message_falls_back_to_code() {
    let json = r#"{"base_resp": {"status_code": 2049}}"#;
    let parsed: T2aResponse = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.base_resp.best_message(), "MiniMax status_code=2049");
}

#[test]
fn status_code_mapping() {
    assert!(matches!(
        map_status_code(1004, "x".into()),
        GenerationError::AuthenticationError { .. }
    ));
    assert!(matches!(
        map_status_code(1008, "x".into()),
        GenerationError::RateLimitError { .. }
    ));
    assert!(matches!(
        map_status_code(1500, "x".into()),
        GenerationError::InvalidParametersError { .. }
    ));
    assert!(matches!(
        map_status_code(9999, "x".into()),
        GenerationError::ProviderError { .. }
    ));
}

// === Voice catalogue ===

#[test]
fn voice_list_uses_modern_taxonomy() {
    let voices = MinimaxTtsProvider::static_voice_list();
    assert!(!voices.is_empty());
    assert!(voices.iter().any(|v| v.id == "Chinese (Mandarin)_Gentle_Boy"));
    assert!(voices.iter().any(|v| v.id == "English_expressive_narrator"));
    // Classic speech-01-only ids must not appear (they break the default model).
    assert!(voices.iter().all(|v| !v.id.starts_with("male-qn-")));
}

// === Async guards (return before any network call) ===

#[tokio::test]
async fn rejects_wrong_generation_type() {
    let p = MinimaxTtsProvider::new("k", None, None, None).unwrap();
    let err = p.generate(GenerationRequest::image("a cat")).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::UnsupportedGenerationTypeError { .. }
    ));
}

#[tokio::test]
async fn rejects_empty_input() {
    let p = MinimaxTtsProvider::new("k", None, None, None).unwrap();
    let err = p.generate(GenerationRequest::speech("")).await.unwrap_err();
    assert!(matches!(err, GenerationError::InvalidParametersError { .. }));
}

#[tokio::test]
async fn rejects_oversized_input() {
    let text = "a".repeat(MAX_INPUT_CHARS + 1);
    let p = MinimaxTtsProvider::new("k", None, None, None).unwrap();
    let err = p.generate(GenerationRequest::speech(text)).await.unwrap_err();
    assert!(matches!(err, GenerationError::InvalidParametersError { .. }));
}

#[tokio::test]
async fn rejects_unknown_format() {
    let p = MinimaxTtsProvider::new("k", None, None, None).unwrap();
    let params = GenerationParams::builder().format("ogg").build();
    let request = GenerationRequest::speech("hi").with_params(params);
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(err, GenerationError::UnsupportedFormatError { .. }));
}
