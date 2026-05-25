//! Unit tests for the Suno music provider. Integration tests against a real
//! paid wrapper require a live API key and are exercised manually.

use super::*;
use crate::generation::{GenerationRequest, GenerationType};

#[test]
fn rejects_empty_api_key() {
    let err = SunoProvider::new("   ", None, None).unwrap_err();
    assert!(matches!(err, GenerationError::AuthenticationError { .. }));
}

#[test]
fn base_url_strips_trailing_slash() {
    let p =
        SunoProvider::new("k", Some("https://api.sunoaiapi.com//".into()), None).unwrap();
    assert_eq!(p.base_url, "https://api.sunoaiapi.com");
    assert_eq!(p.generate_url(), "https://api.sunoaiapi.com/api/generate");
    assert_eq!(
        p.get_url("abc,def"),
        "https://api.sunoaiapi.com/api/get?ids=abc,def"
    );
}

#[test]
fn default_model_is_chirp_v3_5() {
    let p = SunoProvider::new("k", None, None).unwrap();
    assert_eq!(p.default_model(), Some("chirp-v3-5"));
}

#[test]
fn supports_music_only() {
    let p = SunoProvider::new("k", None, None).unwrap();
    assert!(p.supports(GenerationType::Audio));
    assert!(!p.supports(GenerationType::Speech));
    assert!(!p.supports(GenerationType::Image));
}

#[tokio::test]
async fn rejects_wrong_generation_type() {
    let p = SunoProvider::new("k", None, None).unwrap();
    let request = GenerationRequest::image("a cat");
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::UnsupportedGenerationTypeError { .. }
    ));
}

#[tokio::test]
async fn rejects_empty_prompt() {
    let p = SunoProvider::new("k", None, None).unwrap();
    let request = GenerationRequest::new(GenerationType::Audio, "");
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(err, GenerationError::InvalidParametersError { .. }));
}

#[tokio::test]
async fn rejects_oversized_prompt() {
    let text = "a".repeat(MAX_PROMPT_CHARS + 1);
    let p = SunoProvider::new("k", None, None).unwrap();
    let request = GenerationRequest::new(GenerationType::Audio, text);
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(err, GenerationError::InvalidParametersError { .. }));
}

#[test]
fn clip_is_complete_requires_status_and_url() {
    let mut clip = SunoClip {
        id: "x".into(),
        status: Some("complete".into()),
        audio_url: Some("https://example.com/a.mp3".into()),
        video_url: None,
        image_url: None,
        title: None,
        tags: None,
        prompt: None,
        lyric: None,
        duration: Some(120.0),
        clip_type: None,
    };
    assert!(clip.is_complete());

    clip.audio_url = Some(String::new());
    assert!(!clip.is_complete(), "empty audio_url should not be complete");

    clip.audio_url = Some("https://example.com/a.mp3".into());
    clip.status = Some("streaming".into());
    assert!(!clip.is_complete());
}

#[test]
fn clip_is_error_only_when_status_error() {
    let mut clip = SunoClip {
        id: "x".into(),
        status: Some("error".into()),
        audio_url: None,
        video_url: None,
        image_url: None,
        title: None,
        tags: None,
        prompt: None,
        lyric: None,
        duration: None,
        clip_type: None,
    };
    assert!(clip.is_error());
    clip.status = Some("queued".into());
    assert!(!clip.is_error());
}

#[test]
fn error_envelope_picks_best_message() {
    let e = SunoError {
        message: None,
        error: Some("rate limited".into()),
        detail: Some("never reached".into()),
    };
    assert_eq!(e.best_message().as_deref(), Some("rate limited"));
    let empty = SunoError {
        message: None,
        error: None,
        detail: None,
    };
    assert!(empty.best_message().is_none());
}
