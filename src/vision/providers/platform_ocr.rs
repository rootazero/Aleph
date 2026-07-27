//! Platform OCR provider — delegates OCR to the platform-native desktop layer.
//!
//! This provider only supports OCR. Image understanding and object detection
//! are not available — use [`ClaudeVisionProvider`](super::ClaudeVisionProvider)
//! or another multimodal provider for those capabilities.

use async_trait::async_trait;

use crate::sync_primitives::Arc;

use crate::vision::error::VisionError;
use crate::vision::provider::VisionProvider;
use crate::vision::types::{
    validate_confidence, ImageInput, OcrLine, OcrResult, Rect, VisionCapabilities, VisionResult,
};

/// Vision provider backed by the platform-native OCR engine.
///
/// On macOS this delegates to the Vision framework through `NativeScreen`.
/// On other platforms, all calls return [`VisionError::OcrNotAvailable`].
///
/// # Capabilities
///
/// - Image understanding: **no**
/// - OCR: **yes**
/// - Object detection: **no**
#[derive(Clone)]
pub struct PlatformOcrProvider {
    source: ScreenSource,
}

/// Where the provider obtains its OCR-capable screen capability.
#[derive(Clone)]
enum ScreenSource {
    /// A directly-injected screen capability (tests and the bare `new()` default).
    Direct(Arc<dyn aleph_desktop::ScreenCapability>),
    /// The full desktop platform; the screen is resolved per call via
    /// `platform.screen()`. Production uses this so OCR reuses the injected,
    /// bridge-backed platform screen (macOS routes OCR through the Swift helper)
    /// instead of a bare `NativeScreen` whose OCR is `NotImplemented` on macOS.
    Platform(Arc<dyn aleph_desktop::DesktopPlatform>),
}

impl PlatformOcrProvider {
    /// Create a provider over the bare default native screen capability.
    ///
    /// NOTE: `NativeScreen`'s OCR is `NotImplemented` on macOS (macOS OCR is
    /// routed through the Swift bridge), so the production registry should use
    /// [`with_platform`](Self::with_platform) to reuse the injected,
    /// bridge-backed platform screen instead.
    #[must_use]
    pub fn new() -> Self {
        Self {
            source: ScreenSource::Direct(Arc::new(aleph_desktop::NativeScreen::new())),
        }
    }

    /// Create a provider with a custom screen capability (for testing).
    pub fn with_screen(screen: Arc<dyn aleph_desktop::ScreenCapability>) -> Self {
        Self {
            source: ScreenSource::Direct(screen),
        }
    }

    /// Create a provider that resolves OCR through the injected desktop
    /// platform's screen capability at call time. This is the production path:
    /// on macOS `platform.screen().ocr()` routes through the Swift bridge, so
    /// the `screenshot {describe:true}` OCR text layer works (a bare
    /// `NativeScreen` returns `NotImplemented` there).
    pub fn with_platform(platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        Self {
            source: ScreenSource::Platform(platform),
        }
    }

    /// Resolve an [`ImageInput`] to PNG bytes suitable for the native OCR API.
    ///
    /// - `Base64` variant: decoded into bytes.
    /// - `FilePath` variant: read from disk as-is.
    /// - `Url` variant: not supported for platform OCR (would need HTTP fetch).
    async fn resolve_png_bytes(image: &ImageInput) -> Result<Vec<u8>, VisionError> {
        match image {
            ImageInput::Base64 { data, .. } => {
                use base64::Engine;
                let estimated_decoded = (data.len() / 4) * 3;
                if estimated_decoded as u64 > crate::vision::types::MAX_IMAGE_FILE_SIZE {
                    return Err(VisionError::ImageError(format!(
                        "base64 image exceeds maximum size of {} MB",
                        crate::vision::types::MAX_IMAGE_FILE_SIZE / (1024 * 1024)
                    )));
                }
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|e| VisionError::ImageError(format!("Invalid base64 image: {e}")))?;
                if decoded.len() as u64 > crate::vision::types::MAX_IMAGE_FILE_SIZE {
                    return Err(VisionError::ImageError(format!(
                        "decoded base64 image exceeds maximum size of {} MB",
                        crate::vision::types::MAX_IMAGE_FILE_SIZE / (1024 * 1024)
                    )));
                }
                Ok(decoded)
            }
            ImageInput::FilePath { path } => {
                let bytes = tokio::fs::read(path).await.map_err(|e| {
                    VisionError::ImageError(format!(
                        "Failed to read image file {}: {}",
                        path.display(),
                        e
                    ))
                })?;
                if bytes.len() as u64 > crate::vision::types::MAX_IMAGE_FILE_SIZE {
                    return Err(VisionError::ImageError(format!(
                        "image file exceeds maximum size of {} MB",
                        crate::vision::types::MAX_IMAGE_FILE_SIZE / (1024 * 1024)
                    )));
                }
                Ok(bytes)
            }
            ImageInput::Url { url } => Err(VisionError::ImageError(format!(
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
        let png_bytes = Self::resolve_png_bytes(image).await?;
        let result = match &self.source {
            ScreenSource::Direct(screen) => screen.ocr(Some(&png_bytes)).await,
            ScreenSource::Platform(platform) => {
                let screen = platform.screen().ok_or_else(|| {
                    VisionError::ProviderError(
                        "desktop screen capability unavailable for OCR".into(),
                    )
                })?;
                screen.ocr(Some(&png_bytes)).await
            }
        }
        .map_err(|e| VisionError::ProviderError(format!("Platform OCR failed: {e}")))?;

        Ok(convert_platform_ocr_result(result))
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

/// Convert the desktop-layer OCR result into the vision-layer OCR shape.
fn convert_platform_ocr_result(result: aleph_desktop::OcrResult) -> OcrResult {
    let lines = result
        .lines
        .into_iter()
        .map(|line| OcrLine {
            text: line.text,
            // The desktop bridge crosses the R1 brain-limb boundary via IPC, so
            // its geometry/scores are untrusted: drop non-finite or negative
            // rects to `None` and clamp out-of-range confidences to 0.0 rather
            // than propagating poisoned values downstream.
            bounding_box: line
                .bounding_box
                .and_then(|bb| Rect::new(bb.x, bb.y, bb.w, bb.h).ok()),
            confidence: line
                .confidence
                .and_then(|c| validate_confidence(c).ok())
                .unwrap_or(0.0),
        })
        .collect();

    OcrResult {
        full_text: result.full_text,
        lines,
    }
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
        assert!(err
            .to_string()
            .contains("does not support image understanding"));
    }

    #[tokio::test]
    async fn resolve_png_bytes_from_base64_input() {
        let image = ImageInput::Base64 {
            data: "aGVsbG8=".to_string(),
            format: ImageFormat::Png,
        };
        let result = PlatformOcrProvider::resolve_png_bytes(&image)
            .await
            .unwrap();
        assert_eq!(result, b"hello");
    }

    #[tokio::test]
    async fn resolve_png_bytes_from_url_returns_error() {
        let image = ImageInput::Url {
            url: "https://example.com/img.png".to_string(),
        };
        let err = PlatformOcrProvider::resolve_png_bytes(&image)
            .await
            .unwrap_err();
        assert!(matches!(err, VisionError::ImageError(_)));
    }

    #[test]
    fn convert_platform_ocr_result_full() {
        let result = convert_platform_ocr_result(aleph_desktop::OcrResult {
            full_text: "Hello World\nLine 2".to_string(),
            lines: vec![
                aleph_desktop::OcrLine {
                    text: "Hello World".to_string(),
                    bounding_box: Some(aleph_desktop::BoundingBox {
                        x: 10.0,
                        y: 20.0,
                        w: 200.0,
                        h: 30.0,
                    }),
                    confidence: Some(0.98),
                },
                aleph_desktop::OcrLine {
                    text: "Line 2".to_string(),
                    bounding_box: None,
                    confidence: Some(0.95),
                },
            ],
        });
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
    fn convert_platform_ocr_result_sanitizes_untrusted_geometry_and_confidence() {
        // Bridge data crossing the R1 IPC boundary may carry invalid geometry
        // (negative / NaN dimensions) and out-of-range confidence scores.
        let result = convert_platform_ocr_result(aleph_desktop::OcrResult {
            full_text: "x".to_string(),
            lines: vec![
                aleph_desktop::OcrLine {
                    text: "negative-width".to_string(),
                    bounding_box: Some(aleph_desktop::BoundingBox {
                        x: 0.0,
                        y: 0.0,
                        w: -5.0,
                        h: 30.0,
                    }),
                    confidence: Some(2.5),
                },
                aleph_desktop::OcrLine {
                    text: "nan-height".to_string(),
                    bounding_box: Some(aleph_desktop::BoundingBox {
                        x: 0.0,
                        y: 0.0,
                        w: 10.0,
                        h: f64::NAN,
                    }),
                    confidence: Some(f64::NAN),
                },
            ],
        });
        assert_eq!(result.lines.len(), 2);
        // Invalid rects are dropped rather than propagated as poisoned values.
        assert!(result.lines[0].bounding_box.is_none());
        assert!(result.lines[1].bounding_box.is_none());
        // Out-of-range / NaN confidences clamp to 0.0.
        assert_eq!(result.lines[0].confidence, 0.0);
        assert_eq!(result.lines[1].confidence, 0.0);
    }

    #[test]
    fn convert_platform_ocr_result_empty() {
        let result = convert_platform_ocr_result(aleph_desktop::OcrResult {
            full_text: String::new(),
            lines: vec![],
        });
        assert_eq!(result.full_text, "");
        assert!(result.lines.is_empty());
    }

    #[test]
    fn convert_platform_ocr_result_text_only() {
        let result = convert_platform_ocr_result(aleph_desktop::OcrResult {
            full_text: "Just text".to_string(),
            lines: vec![],
        });
        assert_eq!(result.full_text, "Just text");
        assert!(result.lines.is_empty());
    }
}
