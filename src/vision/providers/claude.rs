//! Claude Vision provider — delegates image understanding and OCR to Claude's
//! multimodal API.

use async_trait::async_trait;
use base64::Engine;
use reqwest::Client;

use crate::providers::anthropic::{
    ContentBlock, ImageSource, Message, MessageContent, MessagesRequest,
    MessagesResponse, SystemBlock,
};
use crate::vision::error::VisionError;
use crate::vision::provider::VisionProvider;
use crate::vision::types::{ImageFormat, ImageInput, OcrResult, VisionCapabilities, VisionResult};

/// Vision provider backed by Anthropic's Claude multimodal model.
///
/// Supports both image understanding (describe, answer questions) and OCR
/// (text extraction). Object detection is not supported.
#[derive(Debug, Clone)]
pub struct ClaudeVisionProvider {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
}

impl ClaudeVisionProvider {
    /// Create a new Claude Vision provider.
    ///
    /// # Arguments
    /// * `api_key` — Anthropic API key
    /// * `model` — Model identifier (e.g. "claude-sonnet-4-20250514")
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.into(),
            model: model.into(),
            base_url: "https://api.anthropic.com".to_string(),
        }
    }

    /// Create a new provider with a custom base URL (for testing).
    #[allow(dead_code)]
    pub fn with_base_url(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.into(),
            model: model.into(),
            base_url: base_url.into(),
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }

    fn to_content_block(&self, image: &ImageInput) -> Result<ContentBlock, VisionError> {
        match image {
            ImageInput::Base64 { data, format } => {
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
                let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png");
                let mime = match ext.to_lowercase().as_str() {
                    "png" => "image/png",
                    "jpg" | "jpeg" => "image/jpeg",
                    "webp" => "image/webp",
                    _ => "image/png",
                };
                Ok(ContentBlock::Image {
                    source: ImageSource {
                        source_type: "base64".to_string(),
                        media_type: mime.to_string(),
                        data,
                    },
                })
            }
            ImageInput::Url { .. } => Err(VisionError::ImageError(
                "URL images not yet supported".into(),
            )),
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
            max_tokens: 1024,
            system: None,
            temperature: None,
            stream: None,
            thinking: None,
            tools: None,
            service_tier: None,
            output_config: None,
        };

        if let Some(s) = system {
            request.system = Some(vec![SystemBlock::text(s)]);
        }

        let resp = self
            .client
            .post(&self.endpoint())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&request)
            .send()
            .await
            .map_err(|e| VisionError::ProviderError(format!("request failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(VisionError::ProviderError(format!("API {}: {}", status, body)));
        }

        let response = resp
            .json::<MessagesResponse>()
            .await
            .map_err(|e| VisionError::ProviderError(format!("parse failed: {}", e)))?;

        Ok(response
            .content
            .into_iter()
            .find_map(|b| match b {
                crate::providers::anthropic::AnthropicContentBlock::Text { text } => Some(text),
                _ => None,
            })
            .unwrap_or_default())
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
        };
        let description = self
            .call_api(vec![image_block, text_block], None)
            .await?;
        Ok(VisionResult {
            description,
            elements: Vec::new(),
            confidence: 0.9,
        })
    }

    async fn ocr(&self, image: &ImageInput) -> Result<OcrResult, VisionError> {
        let image_block = self.to_content_block(image)?;
        let text_block = ContentBlock::Text {
            text: "Extract all text from this image.".to_string(),
        };
        let text = self.call_api(vec![image_block, text_block], None).await?;
        let lines: Vec<_> = text
            .lines()
            .map(|t| crate::vision::types::OcrLine {
                text: t.to_string(),
                bounding_box: None,
                confidence: 0.9,
            })
            .collect();
        Ok(OcrResult { full_text: text, lines })
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
}