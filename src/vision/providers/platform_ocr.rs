//! Platform OCR provider — delegates OCR to the platform-native desktop layer.
//!
//! This provider only supports OCR. Image understanding is not available —
//! use a multimodal provider for that capability.

use async_trait::async_trait;

use crate::sync_primitives::Arc;

use crate::vision::error::VisionError;
use crate::vision::provider::VisionProvider;
use crate::vision::types::{ImageInput, OcrResult, VisionCapabilities, VisionResult};

/// Vision provider backed by the platform-native OCR engine.
///
/// On macOS this delegates to the Vision framework through `NativeScreen`.
///
/// # Capabilities
///
/// - Image understanding: **no**
/// - OCR: **yes**
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
            ImageInput::Base64 { data, format } => {
                use base64::Engine;
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|e| VisionError::ImageError(format!("Invalid base64 image: {e}")))?;
                if decoded.len() as u64 > crate::vision::types::MAX_IMAGE_FILE_SIZE {
                    return Err(VisionError::ImageError(format!(
                        "decoded image exceeds maximum size of {} MB",
                        crate::vision::types::MAX_IMAGE_FILE_SIZE / (1024 * 1024)
                    )));
                }
                // The platform OCR backend only accepts PNG. Fail loudly
                // instead of silently handing it a mislabeled payload — the
                // upstream Vision framework may detect format from magic
                // bytes, but other platform providers will not.
                match format {
                    ImageFormat::Png => Ok(decoded),
                    other => Err(VisionError::ImageError(format!(
                        "Platform OCR currently accepts only PNG; got {other:?}. \
                         Route the request through a transcoding pipeline or \
                         add a transcode step."
                    ))),
                }
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
///
/// TODO(vision): expose `lines` (bounding box, confidence, recognition
/// candidates) once a consumer asks for them — per CLAUDE.md R10 YAGNI, defer
/// until a real caller. Until then, retain the field on the input type so
/// future migration is cheap; the explicit `let _lines =` binding makes the
/// omission greppable so future readers don't 'fix' it by mistake.
fn convert_platform_ocr_result(result: aleph_desktop::OcrResult) -> OcrResult {
    let _lines = result.lines; // tracked above; intentional discard
    OcrResult {
        full_text: result.full_text,
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
    fn convert_platform_ocr_result_preserves_full_text() {
        let result = convert_platform_ocr_result(aleph_desktop::OcrResult {
            full_text: "Hello World\nLine 2".to_string(),
            lines: vec![],
        });
        assert_eq!(result.full_text, "Hello World\nLine 2");
    }

    #[test]
    fn convert_platform_ocr_result_empty() {
        let result = convert_platform_ocr_result(aleph_desktop::OcrResult {
            full_text: String::new(),
            lines: vec![],
        });
        assert_eq!(result.full_text, "");
    }
}
