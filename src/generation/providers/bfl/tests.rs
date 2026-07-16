//! Unit tests for the BFL FLUX provider. Network-bound integration is
//! exercised manually with a real `x-key`.

use super::*;
use crate::generation::{GenerationParams, GenerationRequest, GenerationType};

#[test]
fn rejects_empty_api_key() {
    let err = BflProvider::new("   ", None, None).unwrap_err();
    assert!(matches!(err, GenerationError::AuthenticationError { .. }));
}

#[test]
fn default_model_is_flux_pro_1_1() {
    let p = BflProvider::new("k", None, None).unwrap();
    assert_eq!(p.default_model(), Some("flux-pro-1.1"));
}

#[test]
fn submit_url_uses_model_path() {
    let p = BflProvider::new("k", None, None).unwrap();
    assert_eq!(p.submit_url("flux-dev"), "https://api.bfl.ai/v1/flux-dev");
    assert_eq!(
        p.submit_url("flux-pro-1.1-ultra"),
        "https://api.bfl.ai/v1/flux-pro-1.1-ultra"
    );
}

#[test]
fn get_result_url_appends_query() {
    let p = BflProvider::new("k", None, None).unwrap();
    assert_eq!(
        p.get_result_url("abc-123"),
        "https://api.bfl.ai/v1/get_result?id=abc-123"
    );
}

#[test]
fn base_url_strips_trailing_slash() {
    let p = BflProvider::new("k", Some("https://api.bfl.ai//".into()), None).unwrap();
    assert_eq!(p.base_url, "https://api.bfl.ai");
}

#[test]
fn supports_image_only() {
    let p = BflProvider::new("k", None, None).unwrap();
    assert!(p.supports(GenerationType::Image));
    assert!(!p.supports(GenerationType::Video));
    assert!(!p.supports(GenerationType::Audio));
}

#[tokio::test]
async fn rejects_wrong_generation_type() -> Result<(), GenerationError> {
    let p = BflProvider::new("k", None, None)?;
    let request = GenerationRequest::speech("hello");
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::UnsupportedGenerationTypeError { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn rejects_empty_prompt() -> Result<(), GenerationError> {
    let p = BflProvider::new("k", None, None)?;
    let request = GenerationRequest::image("");
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::InvalidParametersError { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn rejects_oversized_prompt() -> Result<(), GenerationError> {
    let text = "a".repeat(MAX_PROMPT_CHARS + 1);
    let p = BflProvider::new("k", None, None)?;
    let request = GenerationRequest::image(text);
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::InvalidParametersError { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn rejects_unknown_format() -> Result<(), GenerationError> {
    let p = BflProvider::new("k", None, None)?;
    let params = GenerationParams::builder().format("webp").build();
    let request = GenerationRequest::image("a cat").with_params(params);
    let err = p.generate(request).await.unwrap_err();
    assert!(matches!(
        err,
        GenerationError::UnsupportedFormatError { .. }
    ));
    Ok(())
}

#[test]
fn classify_status_terminal_success() {
    assert!(matches!(classify_status("Ready"), StatusClass::Success));
}

#[test]
fn classify_status_pending_paths() {
    assert!(matches!(classify_status("Queued"), StatusClass::Pending));
    assert!(matches!(classify_status("Pending"), StatusClass::Pending));
}

#[test]
fn classify_status_error_paths() {
    assert!(matches!(classify_status("Error"), StatusClass::Error));
    assert!(matches!(
        classify_status("Request Moderated"),
        StatusClass::Moderated
    ));
    assert!(matches!(
        classify_status("Content Moderated"),
        StatusClass::Moderated
    ));
    assert!(matches!(
        classify_status("Task not found"),
        StatusClass::NotFound
    ));
}

#[test]
fn classify_status_unknown_keeps_polling() {
    assert!(matches!(
        classify_status("future-state"),
        StatusClass::Unknown
    ));
}

#[test]
fn error_envelope_picks_best_message() {
    let e = BflError {
        error: Some("invalid prompt".into()),
        detail: None,
        message: None,
    };
    assert_eq!(e.best_message().as_deref(), Some("invalid prompt"));

    let e2 = BflError {
        error: None,
        detail: Some(serde_json::json!({"loc": ["body", "prompt"]})),
        message: None,
    };
    assert!(e2.best_message().is_some());
}
