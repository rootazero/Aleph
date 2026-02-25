//! Platform OCR provider — delegates OCR to the macOS Vision framework via the
//! Desktop Bridge (`desktop.ocr` JSON-RPC method).
//!
//! This provider only supports OCR. Image understanding and object detection
//! are not available — use [`ClaudeVisionProvider`](super::ClaudeVisionProvider)
//! or another multimodal provider for those capabilities.

use async_trait::async_trait;

use crate::desktop::client::DesktopBridgeClient;
use crate::desktop::types::DesktopRequest;
use crate::vision::error::VisionError;
use crate::vision::provider::VisionProvider;
use crate::vision::types::{
    ImageInput, OcrLine, OcrResult, Rect, VisionCapabilities, VisionResult,
};

/// Vision provider backed by the platform-native OCR engine.
///
/// On macOS this delegates to the Vision framework through the Desktop Bridge.
/// On other platforms, all calls return [`VisionError::OcrNotAvailable`].
///
/// # Capabilities
///
/// - Image understanding: **no**
/// - OCR: **yes**
/// - Object detection: **no**
#[derive(Clone)]
pub struct PlatformOcrProvider {
    client: DesktopBridgeClient,
}

impl PlatformOcrProvider {
    /// Create a new platform OCR provider using the default Desktop Bridge socket.
    pub fn new() -> Self {
        Self {
            client: DesktopBridgeClient::new(),
        }
    }

    /// Create a new provider with a custom Desktop Bridge client (for testing).
    pub fn with_client(client: DesktopBridgeClient) -> Self {
        Self { client }
    }

    /// Resolve an [`ImageInput`] to a base64 string suitable for the bridge.
    ///
    /// - `Base64` variant: returned directly.
    /// - `FilePath` variant: read from disk and base64-encoded.
    /// - `Url` variant: not supported for platform OCR (would need HTTP fetch).
    fn resolve_base64(image: &ImageInput) -> Result<Option<String>, VisionError> {
        match image {
            ImageInput::Base64 { data, .. } => Ok(Some(data.clone())),
            ImageInput::FilePath { path } => {
                let bytes = std::fs::read(path).map_err(|e| {
                    VisionError::ImageError(format!(
                        "Failed to read image file {}: {}",
                        path.display(),
                        e
                    ))
                })?;
                Ok(Some(base64_encode(&bytes)))
            }
            ImageInput::Url { url } => Err(VisionError::ProviderError(format!(
                "Platform OCR does not support URL images directly: {url}"
            ))),
        }
    }
}

impl Default for PlatformOcrProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VisionProvider for PlatformOcrProvider {
    async fn understand_image(
        &self,
        _image: &ImageInput,
        _prompt: &str,
    ) -> Result<VisionResult, VisionError> {
        Err(VisionError::ProviderError(
            "Platform OCR does not support image understanding — \
             use a multimodal LLM provider instead"
                .into(),
        ))
    }

    async fn ocr(&self, image: &ImageInput) -> Result<OcrResult, VisionError> {
        let image_base64 = Self::resolve_base64(image)?;

        let request = DesktopRequest::Ocr { image_base64 };

        let result = self.client.send(request).await.map_err(|e| {
            VisionError::ProviderError(format!("Desktop Bridge OCR failed: {e}"))
        })?;

        // Parse the bridge response into OcrResult.
        //
        // The bridge returns a JSON object with:
        //   { "text": "full text", "lines": [ { "text": "...", "bounding_box": {...}, "confidence": 0.99 } ] }
        //
        // We normalize this into our VisionResult types.
        parse_ocr_response(&result)
    }

    fn capabilities(&self) -> VisionCapabilities {
        VisionCapabilities {
            image_understanding: false,
            ocr: true,
            object_detection: false,
        }
    }

    fn name(&self) -> &str {
        "platform-ocr"
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Base64-encode raw bytes (standard, no padding stripping).
fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Parse the Desktop Bridge OCR response JSON into an [`OcrResult`].
fn parse_ocr_response(value: &serde_json::Value) -> Result<OcrResult, VisionError> {
    let full_text = value
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut lines = Vec::new();
    if let Some(arr) = value.get("lines").and_then(|v| v.as_array()) {
        for item in arr {
            let text = item
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let bounding_box = item.get("bounding_box").and_then(|bb| {
                Some(Rect {
                    x: bb.get("x")?.as_f64()?,
                    y: bb.get("y")?.as_f64()?,
                    width: bb.get("width")?.as_f64()?,
                    height: bb.get("height")?.as_f64()?,
                })
            });

            let confidence = item
                .get("confidence")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            lines.push(OcrLine {
                text,
                bounding_box,
                confidence,
            });
        }
    }

    Ok(OcrResult { full_text, lines })
}

// =============================================================================
// Tests
// =============================================================================

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
        let provider = PlatformOcrProvider::new();
        let caps = provider.capabilities();
        assert!(!caps.image_understanding);
        assert!(caps.ocr);
        assert!(!caps.object_detection);
    }

    #[test]
    fn name_is_platform_ocr() {
        let provider = PlatformOcrProvider::new();
        assert_eq!(provider.name(), "platform-ocr");
    }

    #[tokio::test]
    async fn understand_image_returns_error() {
        let provider = PlatformOcrProvider::new();
        let err = provider
            .understand_image(&sample_image(), "describe this")
            .await
            .unwrap_err();
        assert!(matches!(err, VisionError::ProviderError(_)));
        assert!(err.to_string().contains("does not support image understanding"));
    }

    #[test]
    fn resolve_base64_from_base64_input() {
        let image = ImageInput::Base64 {
            data: "abc123".to_string(),
            format: ImageFormat::Png,
        };
        let result = PlatformOcrProvider::resolve_base64(&image).unwrap();
        assert_eq!(result, Some("abc123".to_string()));
    }

    #[test]
    fn resolve_base64_from_url_returns_error() {
        let image = ImageInput::Url {
            url: "https://example.com/img.png".to_string(),
        };
        let err = PlatformOcrProvider::resolve_base64(&image).unwrap_err();
        assert!(matches!(err, VisionError::ProviderError(_)));
    }

    #[test]
    fn parse_ocr_response_full() {
        let json = serde_json::json!({
            "text": "Hello World\nLine 2",
            "lines": [
                {
                    "text": "Hello World",
                    "bounding_box": { "x": 10.0, "y": 20.0, "width": 200.0, "height": 30.0 },
                    "confidence": 0.98
                },
                {
                    "text": "Line 2",
                    "confidence": 0.95
                }
            ]
        });

        let result = parse_ocr_response(&json).unwrap();
        assert_eq!(result.full_text, "Hello World\nLine 2");
        assert_eq!(result.lines.len(), 2);
        assert_eq!(result.lines[0].text, "Hello World");
        assert!(result.lines[0].bounding_box.is_some());
        let bb = result.lines[0].bounding_box.unwrap();
        assert!((bb.x - 10.0).abs() < f64::EPSILON);
        assert!((bb.width - 200.0).abs() < f64::EPSILON);
        assert!((result.lines[0].confidence - 0.98).abs() < f64::EPSILON);

        assert_eq!(result.lines[1].text, "Line 2");
        assert!(result.lines[1].bounding_box.is_none());
    }

    #[test]
    fn parse_ocr_response_empty() {
        let json = serde_json::json!({});
        let result = parse_ocr_response(&json).unwrap();
        assert_eq!(result.full_text, "");
        assert!(result.lines.is_empty());
    }

    #[test]
    fn parse_ocr_response_text_only() {
        let json = serde_json::json!({ "text": "Just text" });
        let result = parse_ocr_response(&json).unwrap();
        assert_eq!(result.full_text, "Just text");
        assert!(result.lines.is_empty());
    }
}
