//! Claude Vision provider — delegates image understanding and OCR to Claude's
//! multimodal API.

use async_trait::async_trait;
use base64::Engine;
use reqwest::Client;
use std::time::Duration;

use crate::providers::anthropic::{
    ContentBlock, ImageSource, Message, MessageContent, MessagesRequest, MessagesResponse,
    SystemBlock,
};
use crate::vision::error::VisionError;
use crate::vision::provider::VisionProvider;
use crate::vision::types::{
    ImageFormat, ImageInput, OcrResult, VisionCapabilities, VisionResult, MAX_IMAGE_FILE_SIZE,
};

const API_TIMEOUT: Duration = Duration::from_secs(60);

/// Vision provider backed by Anthropic's Claude multimodal model.
///
/// Supports both image understanding (describe, answer questions) and OCR
/// (text extraction). Object detection is not supported.
#[derive(Clone)]
pub struct ClaudeVisionProvider {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
    max_tokens: u32,
    default_confidence: f64,
}

impl std::fmt::Debug for ClaudeVisionProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeVisionProvider")
            .field("client", &self.client)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("max_tokens", &self.max_tokens)
            .field("default_confidence", &self.default_confidence)
            .finish()
    }
}

impl ClaudeVisionProvider {
    fn build_client() -> Result<Client, VisionError> {
        Client::builder()
            .timeout(API_TIMEOUT)
            .build()
            .map_err(|e| VisionError::ProviderError(format!("failed to build HTTP client: {e}")))
    }

    /// Create a new Claude Vision provider.
    ///
    /// # Arguments
    /// * `api_key` — Anthropic API key
    /// * `model` — Model identifier (e.g. "claude-sonnet-4-20250514")
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Result<Self, VisionError> {
        Ok(Self {
            client: Self::build_client()?,
            api_key: api_key.into(),
            model: model.into(),
            base_url: "https://api.anthropic.com".to_string(),
            max_tokens: 1024,
            default_confidence: 0.9,
        })
    }

    /// Create a new provider with a custom base URL (for testing).
    pub fn with_base_url(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Result<Self, VisionError> {
        Ok(Self {
            client: Self::build_client()?,
            api_key: api_key.into(),
            model: model.into(),
            base_url: base_url.into(),
            max_tokens: 1024,
            default_confidence: 0.9,
        })
    }

    /// Set the maximum tokens for API calls (default: 1024).
    #[must_use]
    pub const fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Set the default confidence score for responses (default: 0.9).
    #[must_use]
    pub const fn with_default_confidence(mut self, confidence: f64) -> Self {
        self.default_confidence = if confidence.is_nan() {
            0.0
        } else {
            confidence.clamp(0.0, 1.0)
        };
        self
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }

    fn detect_mime_from_extension(ext: Option<&str>) -> Result<&'static str, VisionError> {
        match ext.map(|e| e.to_lowercase()).as_deref() {
            Some("png") => Ok("image/png"),
            Some("jpg") | Some("jpeg") => Ok("image/jpeg"),
            Some("webp") => Ok("image/webp"),
            Some("gif") => Ok("image/gif"),
            Some(other) => Err(VisionError::ImageError(format!(
                "unsupported image extension: {other}"
            ))),
            None => Err(VisionError::ImageError(
                "image file has no extension; rename the file or use a Base64 \
                 ImageInput with an explicit format"
                    .to_string(),
            )),
        }
    }

    fn to_content_block(&self, image: &ImageInput) -> Result<ContentBlock, VisionError> {
        match image {
            ImageInput::Base64 { data, format } => {
                // Enforce the same size ceiling as the FilePath branch so an
                // oversized payload is rejected locally with a clean error
                // instead of being forwarded over the wire. base64 decodes to
                // ~3/4 of its encoded length; estimate without allocating.
                let estimated_decoded = (data.len() / 4) * 3;
                if estimated_decoded as u64 > MAX_IMAGE_FILE_SIZE {
                    return Err(VisionError::ImageError(format!(
                        "image data exceeds maximum size of {} MB",
                        MAX_IMAGE_FILE_SIZE / (1024 * 1024)
                    )));
                }
                let mime = match format {
                    ImageFormat::Png => "image/png",
                    ImageFormat::Jpeg => "image/jpeg",
                    ImageFormat::WebP => "image/webp",
                };
                Ok(ContentBlock::Image {
                    source: ImageSource {
                        source_type: "base64".to_string(),
                        media_type: mime.to_string(),
                        data: data.clone(),
                    },
                })
            }
            ImageInput::FilePath { path } => {
                let bytes =
                    std::fs::read(path).map_err(|e| VisionError::ImageError(e.to_string()))?;
                if bytes.len() as u64 > MAX_IMAGE_FILE_SIZE {
                    return Err(VisionError::ImageError(format!(
                        "image file exceeds maximum size of {} MB",
                        MAX_IMAGE_FILE_SIZE / (1024 * 1024)
                    )));
                }
                let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let mime = Self::detect_mime_from_extension(
                    path.extension().and_then(|e| e.to_str()),
                )?;
                Ok(ContentBlock::Image {
                    source: ImageSource {
                        source_type: "base64".to_string(),
                        media_type: mime.to_string(),
                        data,
                    },
                })
            }
            ImageInput::Url { url } => {
                // The Anthropic Messages API only supports base64 image sources
                // via our ContentBlock::Image shape. URL images are not supported
                // in this provider — use a different provider or download the image
                // to a FilePath / Base64 input instead.
                Err(VisionError::UnsupportedFormat(format!(
                    "Claude vision provider does not support URL images: {url}"
                )))
            }
        }
    }

    async fn call_api(
        &self,
        blocks: Vec<ContentBlock>,
        system: Option<&str>,
    ) -> Result<String, VisionError> {
        let messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Multimodal { content: blocks },
        }];

        let mut request = MessagesRequest {
            model: self.model.clone(),
            messages,
            max_tokens: self.max_tokens,
            system: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            thinking: None,
            tools: None,
            service_tier: None,
            metadata: None,
            output_config: None,
        };

        if let Some(s) = system {
            request.system = Some(vec![SystemBlock::text(s)]);
        }

        let resp = self
            .client
            .post(self.endpoint())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&request)
            .send()
            .await
            .map_err(|e| VisionError::ProviderError(format!("request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(VisionError::ProviderError(format!("API {status}: {body}")));
        }

        let response = resp
            .json::<MessagesResponse>()
            .await
            .map_err(|e| VisionError::ProviderError(format!("parse failed: {e}")))?;

        let text = response
            .content
            .into_iter()
            .find_map(|b| match b {
                crate::providers::anthropic::AnthropicContentBlock::Text { text } => Some(text),
                _ => None,
            })
            .ok_or_else(|| {
                VisionError::ProviderError(
                    "Claude API returned no text content for vision request".to_string(),
                )
            })?;

        if text.is_empty() {
            tracing::warn!("Claude API returned empty text content for vision request");
        }

        Ok(text)
    }
}

#[async_trait]
impl VisionProvider for ClaudeVisionProvider {
    async fn understand_image(
        &self,
        image: &ImageInput,
        prompt: &str,
    ) -> Result<VisionResult, VisionError> {
        let image_block = self.to_content_block(image)?;
        let text_block = ContentBlock::Text {
            text: prompt.to_string(),
            cache_control: None,
        };
        let description = self.call_api(vec![image_block, text_block], None).await?;
        Ok(VisionResult {
            description,
            elements: Vec::new(),
            confidence: self.default_confidence,
        })
    }

    async fn ocr(&self, image: &ImageInput) -> Result<OcrResult, VisionError> {
        let image_block = self.to_content_block(image)?;
        let text_block = ContentBlock::Text {
            text: "Extract all text from this image.".to_string(),
            cache_control: None,
        };
        let text = self.call_api(vec![image_block, text_block], None).await?;
        let lines: Vec<_> = text
            .lines()
            .map(|t| crate::vision::types::OcrLine {
                text: t.to_string(),
                bounding_box: None,
                confidence: self.default_confidence,
            })
            .collect();
        Ok(OcrResult {
            full_text: text,
            lines,
        })
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

    #[test]
    fn capabilities_correct() -> Result<(), VisionError> {
        let provider = ClaudeVisionProvider::new("sk-test", "claude-sonnet-4-20250514")?;
        let caps = provider.capabilities();
        assert!(caps.image_understanding);
        assert!(caps.ocr);
        assert!(!caps.object_detection);
        Ok(())
    }

    #[test]
    fn name_is_claude_vision() -> Result<(), VisionError> {
        let provider = ClaudeVisionProvider::new("sk-test", "claude-sonnet-4-20250514")?;
        assert_eq!(provider.name(), "claude-vision");
        Ok(())
    }

    #[test]
    fn detect_mime_from_extension_known_types() {
        assert_eq!(
            ClaudeVisionProvider::detect_mime_from_extension(Some("png")),
            Ok("image/png")
        );
        assert_eq!(
            ClaudeVisionProvider::detect_mime_from_extension(Some("jpg")),
            Ok("image/jpeg")
        );
        assert_eq!(
            ClaudeVisionProvider::detect_mime_from_extension(Some("jpeg")),
            Ok("image/jpeg")
        );
        assert_eq!(
            ClaudeVisionProvider::detect_mime_from_extension(Some("webp")),
            Ok("image/webp")
        );
        assert_eq!(
            ClaudeVisionProvider::detect_mime_from_extension(Some("gif")),
            Ok("image/gif")
        );
        assert_eq!(
            ClaudeVisionProvider::detect_mime_from_extension(Some("PNG")),
            Ok("image/png")
        );
    }

    #[test]
    fn detect_mime_from_extension_unknown_errors() {
        match ClaudeVisionProvider::detect_mime_from_extension(Some("bmp")) {
            Err(VisionError::ImageError(msg)) => assert!(msg.contains("bmp")),
            other => panic!("expected ImageError for unknown extension, got {other:?}"),
        }
    }

    #[test]
    fn detect_mime_from_extension_missing_errors() {
        match ClaudeVisionProvider::detect_mime_from_extension(None) {
            Err(VisionError::ImageError(msg)) => assert!(msg.contains("no extension")),
            other => panic!("expected ImageError for missing extension, got {other:?}"),
        }
    }
}
