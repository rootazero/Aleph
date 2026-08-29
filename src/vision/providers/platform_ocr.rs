//! Platform OCR provider — delegates OCR to the platform-native desktop layer.
//!
//! This provider only supports OCR. Image understanding is not available —
//! use a multimodal provider for that capability.

use async_trait::async_trait;

use crate::sync_primitives::Arc;

use crate::vision::error::VisionError;
use crate::vision::provider::VisionProvider;
use crate::vision::types::{ImageFormat, ImageInput, OcrResult, VisionCapabilities, VisionResult};

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
                // Pre-decode bound check. Reject oversized encoded payloads
                // *before* allocating the decoded buffer — otherwise a 1 GiB
                // base64 string costs 1 GiB of `String` plus ~750 MiB of
                // `Vec<u8>` before the existing post-decode size cap fires.
                if data.len() > crate::vision::types::MAX_BASE64_ENCODED_SIZE {
                    return Err(VisionError::ImageError(format!(
                        "base64 image exceeds maximum encoded size of {} KB \
                         (decoded limit: {} MB)",
                        crate::vision::types::MAX_BASE64_ENCODED_SIZE / 1024,
                        crate::vision::types::MAX_IMAGE_FILE_SIZE / (1024 * 1024)
                    )));
                }
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|e| VisionError::ImageError(format!("Invalid base64 image: {e}")))?;
                if decoded.len() as u64 > crate::vision::types::MAX_IMAGE_FILE_SIZE {
                    return Err(VisionError::ImageError(format!(
                        "decoded image exceeds maximum size of {} MB",
                        crate::vision::types::MAX_IMAGE_FILE_SIZE / (1024 * 1024)
                    )));
                }
                // The platform OCR backend only accepts PNG. Verify the
                // PNG magic bytes so a mislabeled JPEG-as-PNG payload is
                // rejected at the boundary — the upstream Vision framework
                // detects format from magic bytes, but other platform
                // providers will not (and silently producing wrong OCR text
                // is worse than failing fast).
                const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
                match format {
                    ImageFormat::Png => {
                        if !decoded.starts_with(PNG_MAGIC) {
                            return Err(VisionError::ImageError(
                                "declared PNG but bytes do not start with PNG magic \
                                 signature; refusing mislabeled payload"
                                    .to_string(),
                            ));
                        }
                        Ok(decoded)
                    }
                    other => Err(VisionError::ImageError(format!(
                        "Platform OCR currently accepts only PNG; got {other:?}. \
                         Route the request through a transcoding pipeline or \
                         add a transcode step."
                    ))),
                }
            }
            ImageInput::FilePath { path } => {
                // Metadata-first size check. `tokio::fs::read` would
                // allocate the entire file into memory before the post-read
                // cap fires; `tokio::fs::metadata` is a single `stat(2)` and
                // short-circuits oversized files before any allocation.
                let meta = tokio::fs::metadata(path).await.map_err(|e| {
                    VisionError::ImageError(format!(
                        "Failed to stat image file {}: {}",
                        path.display(),
                        e
                    ))
                })?;
                if meta.len() > crate::vision::types::MAX_IMAGE_FILE_SIZE {
                    return Err(VisionError::ImageError(format!(
                        "image file exceeds maximum size of {} MB",
                        crate::vision::types::MAX_IMAGE_FILE_SIZE / (1024 * 1024)
                    )));
                }
                let bytes = tokio::fs::read(path).await.map_err(|e| {
                    VisionError::ImageError(format!(
                        "Failed to read image file {}: {}",
                        path.display(),
                        e
                    ))
                })?;
                // Defence-in-depth: re-check post-read in case the file was
                // extended between `metadata` and `read` (TOCTOU).
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
    use base64::Engine;

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
        // Well-known 1×1 transparent PNG (96 base64 chars → 72 bytes).
        // The post-decode magic-byte check needs a payload that actually
        // starts with the PNG signature (`89 50 4E 47 0D 0A 1A 0A`).
        let image = ImageInput::Base64 {
            data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAfbLI3wAAAABJRU5ErkJggg==".to_string(),
            format: ImageFormat::Png,
        };
        let result = PlatformOcrProvider::resolve_png_bytes(&image)
            .await
            .unwrap();
        assert_eq!(
            &result[..8],
            &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']
        );
        assert_eq!(result.len(), 72);
    }

    #[tokio::test]
    async fn resolve_png_bytes_rejects_oversized_base64_pre_decode() {
        // A long string of 'A' is valid base64 (it decodes to zero bytes)
        // so it would pass the base64 decoder. The pre-decode bound check
        // must reject it on encoded length alone, before the decoder
        // allocates the decoded buffer.
        use crate::vision::types::MAX_BASE64_ENCODED_SIZE;
        let huge = "A".repeat(MAX_BASE64_ENCODED_SIZE + 4);
        let image = ImageInput::Base64 {
            data: huge,
            format: ImageFormat::Png,
        };
        let err = PlatformOcrProvider::resolve_png_bytes(&image)
            .await
            .unwrap_err();
        assert!(matches!(err, VisionError::ImageError(_)));
        assert!(err.to_string().contains("encoded size"));
    }

    #[tokio::test]
    async fn resolve_png_bytes_rejects_mislabeled_png_payload() {
        // JPEG magic bytes (FF D8 FF E0) labeled as PNG. The magic-byte
        // check must refuse the mislabeled payload at the boundary rather
        // than handing JPEG bytes to a PNG-only OCR backend.
        let jpeg_bytes = b"\xFF\xD8\xFF\xE0\x00\x10JFIF\x00\x01";
        let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg_bytes);
        let image = ImageInput::Base64 {
            data: b64,
            format: ImageFormat::Png,
        };
        let err = PlatformOcrProvider::resolve_png_bytes(&image)
            .await
            .unwrap_err();
        assert!(matches!(err, VisionError::ImageError(_)));
        assert!(err.to_string().contains("PNG magic"));
    }

    #[tokio::test]
    async fn resolve_png_bytes_rejects_non_png_declared_format() {
        // A valid PNG labeled as JPEG must be rejected by the format arm
        // (before the magic-byte check fires), so the error message stays
        // the platform-OCR-only-accepts-PNG wording.
        let image = ImageInput::Base64 {
            data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAfbLI3wAAAABJRU5ErkJggg==".to_string(),
            format: ImageFormat::Jpeg,
        };
        let err = PlatformOcrProvider::resolve_png_bytes(&image)
            .await
            .unwrap_err();
        assert!(matches!(err, VisionError::ImageError(_)));
        assert!(err.to_string().contains("only PNG"));
    }

    #[tokio::test]
    async fn resolve_png_bytes_file_path_uses_metadata_size_check() {
        use crate::vision::types::MAX_IMAGE_FILE_SIZE;
        // Create a file just over the cap. The metadata-first check must
        // reject it without reading the body into memory.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oversized.png");
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(MAX_IMAGE_FILE_SIZE + 1).unwrap();
        drop(f);
        let image = ImageInput::FilePath { path: path.clone() };
        let err = PlatformOcrProvider::resolve_png_bytes(&image)
            .await
            .unwrap_err();
        assert!(matches!(err, VisionError::ImageError(_)));
        assert!(err.to_string().contains("exceeds maximum size"));
    }

    #[tokio::test]
    async fn resolve_png_bytes_file_path_small_file_passes() {
        // Positive path: a small PNG file is read and returned as-is.
        // Bytes are not validated against the PNG magic (only Base64 inputs
        // are magic-checked; FilePath inputs trust the caller's filesystem).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.png");
        std::fs::write(&path, b"not-a-real-png-but-the-file-is-tiny").unwrap();
        let image = ImageInput::FilePath { path };
        let result = PlatformOcrProvider::resolve_png_bytes(&image)
            .await
            .unwrap();
        assert_eq!(result, b"not-a-real-png-but-the-file-is-tiny");
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
