//! Claude Vision provider — delegates image understanding and OCR to Claude's
//! multimodal API.
//!
//! This is currently a **stub**. The actual API integration will be wired up
//! when connecting to the existing providers module (`crate::providers`).

use async_trait::async_trait;

use crate::vision::error::VisionError;
use crate::vision::provider::VisionProvider;
use crate::vision::types::{ImageInput, OcrResult, VisionCapabilities, VisionResult};

/// Vision provider backed by Anthropic's Claude multimodal model.
///
/// Supports both image understanding (describe, answer questions) and OCR
/// (text extraction). Object detection is not supported.
///
/// # Stub
///
/// This provider currently returns [`VisionError::ProviderError`] for all
/// operations. The actual API wiring will be completed when integrating with
/// `crate::providers`.
#[derive(Debug, Clone)]
pub struct ClaudeVisionProvider {
    /// API key for authentication (will be used once wired up).
    #[allow(dead_code)]
    api_key: String,
    /// Model identifier (e.g. "claude-sonnet-4-20250514").
    #[allow(dead_code)]
    model: String,
}

impl ClaudeVisionProvider {
    /// Create a new Claude Vision provider.
    ///
    /// # Arguments
    /// * `api_key` — Anthropic API key
    /// * `model` — Model identifier (e.g. "claude-sonnet-4-20250514")
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
        }
    }
}

#[async_trait]
impl VisionProvider for ClaudeVisionProvider {
    async fn understand_image(
        &self,
        _image: &ImageInput,
        _prompt: &str,
    ) -> Result<VisionResult, VisionError> {
        // TODO: Wire up to crate::providers for actual Claude API calls
        Err(VisionError::ProviderError(
            "Claude Vision not yet connected — API integration pending".into(),
        ))
    }

    async fn ocr(&self, _image: &ImageInput) -> Result<OcrResult, VisionError> {
        // TODO: Wire up to crate::providers for actual Claude API calls
        Err(VisionError::ProviderError(
            "Claude Vision not yet connected — API integration pending".into(),
        ))
    }

    fn capabilities(&self) -> VisionCapabilities {
        VisionCapabilities {
            image_understanding: true,
            ocr: true,
            object_detection: false,
        }
    }

    fn name(&self) -> &str {
        "claude-vision"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vision::types::ImageFormat;

    fn sample_image() -> ImageInput {
        ImageInput::Base64 {
            data: "iVBORw0KGgo=".to_string(),
            format: ImageFormat::Png,
        }
    }

    #[test]
    fn capabilities_correct() {
        let provider = ClaudeVisionProvider::new("sk-test", "claude-sonnet-4-20250514");
        let caps = provider.capabilities();
        assert!(caps.image_understanding);
        assert!(caps.ocr);
        assert!(!caps.object_detection);
    }

    #[test]
    fn name_is_claude_vision() {
        let provider = ClaudeVisionProvider::new("sk-test", "claude-sonnet-4-20250514");
        assert_eq!(provider.name(), "claude-vision");
    }

    #[tokio::test]
    async fn understand_image_returns_stub_error() {
        let provider = ClaudeVisionProvider::new("sk-test", "claude-sonnet-4-20250514");
        let err = provider
            .understand_image(&sample_image(), "describe this")
            .await
            .unwrap_err();
        assert!(matches!(err, VisionError::ProviderError(_)));
        assert!(err.to_string().contains("not yet connected"));
    }

    #[tokio::test]
    async fn ocr_returns_stub_error() {
        let provider = ClaudeVisionProvider::new("sk-test", "claude-sonnet-4-20250514");
        let err = provider.ocr(&sample_image()).await.unwrap_err();
        assert!(matches!(err, VisionError::ProviderError(_)));
        assert!(err.to_string().contains("not yet connected"));
    }
}
