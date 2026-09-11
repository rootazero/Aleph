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
    /// A directly-injected screen capability (tests).
    Direct(Arc<dyn aleph_desktop::ScreenCapability>),
    /// Lazy variant used by [`PlatformOcrProvider::new`]. The first
    /// [`ocr`](PlatformOcrProvider::ocr) call instantiates a
    /// `NativeScreen`; subsequent calls reuse it. Avoids paying the
    /// platform-side setup cost (accessibility permissions, window-server
    /// connection on macOS) at registry build time when the provider may
    /// never actually serve an OCR request.
    LazyDirect(tokio::sync::OnceCell<Arc<dyn aleph_desktop::ScreenCapability>>),
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
    ///
    /// The screen capability is **lazily** instantiated on first [`ocr`](Self::ocr)
    /// call rather than at construction time. Constructing a `NativeScreen`
    /// touches platform state (accessibility permissions, window-server
    /// connections on macOS) that is wasteful to pay at registry build time
    /// when the provider may never actually serve an OCR request (test
    /// scaffolding, capability-only introspection, etc.). The
    /// `with_platform` path was already lazy by virtue of its
    /// `platform.screen()` lookup inside `ocr`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            source: ScreenSource::LazyDirect(tokio::sync::OnceCell::new()),
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
                // Symmetric with the Base64 arm: a mislabeled JPEG-as-PNG
                // payload gets caught at the boundary rather than handed to
                // the platform OCR backend, which on some platforms silently
                // produces wrong text instead of failing fast. The upstream
                // Vision framework detects format from magic bytes too, so
                // this matches its expectations rather than fighting them.
                const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
                if !bytes.starts_with(PNG_MAGIC) {
                    return Err(VisionError::ImageError(format!(
                        "image file {} does not start with PNG magic signature; \
                         Platform OCR currently accepts only PNG. Route the \
                         request through a transcoding pipeline or add a \
                         transcode step.",
                        path.display()
                    )));
                }
                Ok(bytes)
            }
            ImageInput::Url { url } => Err(VisionError::ImageError(format!(
                "Platform OCR does not support URL images directly (no HTTP \
                 fetch in this provider; bytes-only backend). Add an \
                 HTTP-fetching provider upstream or pre-fetch the URL and \
                 re-issue with Base64/FilePath. Rejected URL: {url}"
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
        let image_ctx = format!(
            "variant={} bytes={}",
            match image {
                ImageInput::Base64 { format, .. } => format!("base64({format:?})"),
                ImageInput::FilePath { path } => format!("FilePath({})", path.display()),
                ImageInput::Url { .. } => "Url".to_string(),
            },
            png_bytes.len()
        );
        let result = match &self.source {
            ScreenSource::Direct(screen) => screen.ocr(Some(&png_bytes)).await,
            ScreenSource::LazyDirect(cell) => {
                let screen = cell
                    .get_or_init(|| async {
                        Arc::new(aleph_desktop::NativeScreen::new())
                            as Arc<dyn aleph_desktop::ScreenCapability>
                    })
                    .await;
                screen.ocr(Some(&png_bytes)).await
            }
            ScreenSource::Platform(platform) => {
                let screen = platform.screen().ok_or_else(|| {
                    VisionError::ProviderError(
                        "desktop screen capability unavailable for OCR".into(),
                    )
                })?;
                screen.ocr(Some(&png_bytes)).await
            }
        }
        .map_err(|e| {
            VisionError::ProviderError(format!(
                "Platform OCR failed ({image_ctx}): {e}"
            ))
        })?;

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
/// Forwards the per-line structured output (`text`, `bounding_box`,
/// `confidence`) when the platform-side backend produced it — the
/// platform layer already paid the cost of recognising these on the
/// Vision framework side, and discarding them at the boundary meant a
/// downstream consumer could not debug or post-process OCR output.
fn convert_platform_ocr_result(result: aleph_desktop::OcrResult) -> OcrResult {
    OcrResult {
        full_text: result.full_text,
        lines: result
            .lines
            .into_iter()
            .map(|line| crate::vision::types::OcrLine {
                text: line.text,
                bounding_box: line.bounding_box.map(|bb| {
                    crate::vision::types::BoundingBox {
                        x: bb.x,
                        y: bb.y,
                        w: bb.w,
                        h: bb.h,
                    }
                }),
                confidence: line.confidence,
            })
            .collect(),
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
        // Well-known 1×1 transparent PNG. 96 base64 characters, two of them
        // `=` padding, so 3 * (96 / 4) - 2 = 70 decoded bytes — the padding is
        // what the "→ 72" this comment used to claim left out, and 72 is what
        // the assertion below asked for, so it had never once been green.
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
        assert_eq!(result.len(), 70);
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
        // Positive path: a small file under the size cap is read and
        // returned as-is. The FilePath arm magic-checks the bytes exactly
        // like the Base64 arm does, so the fixture must carry the PNG
        // signature — the body after it is not inspected.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.png");
        let mut bytes = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        bytes.extend_from_slice(b"tiny-body-not-a-decodable-png");
        std::fs::write(&path, &bytes).unwrap();
        let image = ImageInput::FilePath { path };
        let result = PlatformOcrProvider::resolve_png_bytes(&image)
            .await
            .unwrap();
        assert_eq!(result, bytes);
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
