//! Perception capabilities — screen capture, OCR, accessibility tree.
//!
//! This module provides platform-specific implementations for:
//! - Screenshot capture via `xcap`
//! - OCR via platform APIs (WinRT on Windows; not available on macOS/Linux)
//! - Raw PNG capture for use as OCR input
//!
//! All functions are synchronous and should be called via
//! `tokio::task::spawn_blocking` from async contexts.

use base64::{engine::general_purpose, Engine as _};
use std::io::Cursor;
use tracing::debug;

use crate::error::{DesktopError, Result};
use crate::{OcrResult, ScreenRegion, Screenshot};

/// Capture a screenshot of the primary monitor, optionally cropped to a region.
///
/// Uses `xcap::Monitor` to enumerate displays and capture the primary one.
/// The image is encoded as PNG and returned as a base64-encoded [`Screenshot`].
///
/// # Errors
///
/// - [`DesktopError::ScreenCapture`] if no monitors are found, no primary
///   monitor exists, or the capture/encoding fails.
pub fn take_screenshot(region: Option<&ScreenRegion>) -> Result<Screenshot> {
    debug!("Taking screenshot, region: {:?}", region);

    let monitors = xcap::Monitor::all()
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to enumerate monitors: {e}")))?;

    let monitor = monitors
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .ok_or_else(|| DesktopError::ScreenCapture("No primary monitor found".into()))?;

    let image = match region {
        Some(r) => monitor.capture_region(r.x, r.y, r.width, r.height),
        None => monitor.capture_image(),
    }
    .map_err(|e| DesktopError::ScreenCapture(format!("Screen capture failed: {e}")))?;

    let (width, height) = (image.width(), image.height());

    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| DesktopError::ScreenCapture(format!("PNG encoding failed: {e}")))?;

    let image_base64 = general_purpose::STANDARD.encode(buf.into_inner());

    debug!("Screenshot captured: {}x{}", width, height);

    Ok(Screenshot {
        image_base64,
        width,
        height,
        format: "png".to_string(),
    })
}

/// Capture the primary monitor as raw PNG bytes.
///
/// This is a convenience function for OCR input — it captures the full
/// primary monitor and returns the PNG-encoded bytes without base64 encoding.
///
/// # Errors
///
/// Same as [`take_screenshot`].
pub fn capture_screen_png() -> Result<Vec<u8>> {
    debug!("Capturing screen as raw PNG bytes");

    let monitors = xcap::Monitor::all()
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to enumerate monitors: {e}")))?;

    let monitor = monitors
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .ok_or_else(|| DesktopError::ScreenCapture("No primary monitor found".into()))?;

    let image = monitor
        .capture_image()
        .map_err(|e| DesktopError::ScreenCapture(format!("Screen capture failed: {e}")))?;

    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| DesktopError::ScreenCapture(format!("PNG encoding failed: {e}")))?;

    Ok(buf.into_inner())
}

/// Perform OCR on raw PNG image bytes.
///
/// # Platform support
///
/// - **Windows**: Uses WinRT `OcrEngine` API (prefers zh-Hans, fallback to en-US).
/// - **macOS/Linux**: Returns [`DesktopError::NotImplemented`] — macOS OCR is
///   handled by the native Swift app.
///
/// # Errors
///
/// - [`DesktopError::NotImplemented`] on non-Windows platforms.
/// - [`DesktopError::OcrFailed`] if the Windows OCR engine fails.
pub fn perform_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    #[cfg(target_os = "windows")]
    {
        windows_ocr(png_bytes)
    }

    #[cfg(target_os = "macos")]
    {
        macos_ocr(png_bytes)
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = png_bytes;
        Err(DesktopError::NotImplemented(
            "OCR not implemented on this platform".into(),
        ))
    }
}

// ── macOS Vision OCR ────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn macos_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    use crate::{BoundingBox, OcrLine};
    use objc2::AnyThread;
    use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
    use objc2_vision::{
        VNImageRequestHandler, VNRecognizeTextRequest, VNRequest,
        VNRequestTextRecognitionLevel,
    };

    // 1. Create NSData from PNG bytes
    let ns_data = NSData::with_bytes(png_bytes);

    // Decode image dimensions from PNG header for bounding box conversion
    let (img_width, img_height) = png_dimensions(png_bytes).unwrap_or((1.0, 1.0));

    // 2. Create and configure text recognition request
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(true);

    let languages = NSArray::from_retained_slice(&[
        NSString::from_str("zh-Hans"),
        NSString::from_str("en-US"),
    ]);
    request.setRecognitionLanguages(&languages);

    // 3. Create image handler from data and perform request
    let empty_opts: objc2::rc::Retained<NSDictionary<objc2_vision::VNImageOption, objc2::runtime::AnyObject>> =
        NSDictionary::new();
    let handler = VNImageRequestHandler::initWithData_options(
        VNImageRequestHandler::alloc(),
        &ns_data,
        &empty_opts,
    );

    // VNRecognizeTextRequest inherits from VNRequest — use ProtocolObject or direct cast
    let requests: objc2::rc::Retained<NSArray<VNRequest>> = unsafe {
        let ptr = objc2::rc::Retained::into_raw(objc2::rc::Retained::clone(&request));
        let vn_req = objc2::rc::Retained::from_raw(ptr as *mut VNRequest).unwrap();
        NSArray::from_retained_slice(&[vn_req])
    };

    handler
        .performRequests_error(&requests)
        .map_err(|e| DesktopError::OcrFailed(format!("Vision performRequests failed: {e}")))?;

    // 4. Extract results
    let mut lines = Vec::new();
    let mut full_text = String::new();

    if let Some(observations) = request.results() {
        for obs in observations.iter() {
            let candidates = obs.topCandidates(1);
            if candidates.count() == 0 {
                continue;
            }
            let candidate = candidates.objectAtIndex(0);

            let text = candidate.string().to_string();
            let confidence = candidate.confidence() as f64;

            // Get bounding box (normalized 0-1, origin bottom-left)
            let bbox = unsafe { obs.boundingBox() };

            // Convert from Vision coordinates (bottom-left origin) to
            // screen coordinates (top-left origin)
            let bounding_box = BoundingBox {
                x: bbox.origin.x * img_width,
                y: (1.0 - bbox.origin.y - bbox.size.height) * img_height,
                w: bbox.size.width * img_width,
                h: bbox.size.height * img_height,
            };

            if !full_text.is_empty() {
                full_text.push('\n');
            }
            full_text.push_str(&text);

            lines.push(OcrLine {
                text,
                bounding_box: Some(bounding_box),
                confidence: Some(confidence),
            });
        }
    }

    Ok(OcrResult { full_text, lines })
}

/// Extract width/height from PNG header (IHDR chunk).
#[cfg(target_os = "macos")]
fn png_dimensions(png_bytes: &[u8]) -> Option<(f64, f64)> {
    // PNG: 8 bytes signature, then IHDR chunk: 4 len + 4 "IHDR" + 4 width + 4 height
    if png_bytes.len() < 24 {
        return None;
    }
    // Verify PNG signature
    if &png_bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let width = u32::from_be_bytes([png_bytes[16], png_bytes[17], png_bytes[18], png_bytes[19]]);
    let height = u32::from_be_bytes([png_bytes[20], png_bytes[21], png_bytes[22], png_bytes[23]]);
    Some((width as f64, height as f64))
}

// ── Windows WinRT OCR ───────────────────────────────────────────

/// Perform OCR using the Windows WinRT `OcrEngine` API.
///
/// Steps:
/// 1. Decode PNG bytes into a `SoftwareBitmap` via `BitmapDecoder`.
/// 2. Create an `OcrEngine` (prefer zh-Hans, fallback to en, then user default).
/// 3. Call `RecognizeAsync` to extract text and line bounding boxes.
#[cfg(target_os = "windows")]
fn windows_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    use crate::{BoundingBox, OcrLine};
    use windows::core::Interface;
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::BitmapDecoder;
    use windows::Media::Ocr as WinOcr;
    use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream, IRandomAccessStream};

    // 1. Write PNG bytes into an IRandomAccessStream via DataWriter.
    let stream = InMemoryRandomAccessStream::new()
        .map_err(|e| DesktopError::OcrFailed(format!("Failed to create memory stream: {e}")))?;

    let writer = DataWriter::CreateDataWriter(
        &stream
            .cast::<windows::Storage::Streams::IOutputStream>()
            .map_err(|e| DesktopError::OcrFailed(format!("Stream cast failed: {e}")))?,
    )
    .map_err(|e| DesktopError::OcrFailed(format!("Failed to create DataWriter: {e}")))?;

    writer
        .WriteBytes(png_bytes)
        .map_err(|e| DesktopError::OcrFailed(format!("WriteBytes failed: {e}")))?;
    writer
        .StoreAsync()
        .map_err(|e| DesktopError::OcrFailed(format!("StoreAsync failed: {e}")))?
        .get()
        .map_err(|e| DesktopError::OcrFailed(format!("StoreAsync.get failed: {e}")))?;
    writer
        .FlushAsync()
        .map_err(|e| DesktopError::OcrFailed(format!("FlushAsync failed: {e}")))?
        .get()
        .map_err(|e| DesktopError::OcrFailed(format!("FlushAsync.get failed: {e}")))?;

    // Seek to beginning before decoding.
    stream
        .Seek(0)
        .map_err(|e| DesktopError::OcrFailed(format!("Seek failed: {e}")))?;

    // 2. Decode the PNG into a SoftwareBitmap.
    let decoder = BitmapDecoder::CreateAsync(
        &stream
            .cast::<IRandomAccessStream>()
            .map_err(|e| {
                DesktopError::OcrFailed(format!(
                    "Stream cast to IRandomAccessStream failed: {e}"
                ))
            })?,
    )
    .map_err(|e| DesktopError::OcrFailed(format!("BitmapDecoder::CreateAsync failed: {e}")))?
    .get()
    .map_err(|e| DesktopError::OcrFailed(format!("BitmapDecoder async get failed: {e}")))?;

    let bitmap = decoder
        .GetSoftwareBitmapAsync()
        .map_err(|e| DesktopError::OcrFailed(format!("GetSoftwareBitmapAsync failed: {e}")))?
        .get()
        .map_err(|e| DesktopError::OcrFailed(format!("SoftwareBitmap async get failed: {e}")))?;

    // 3. Create OcrEngine — prefer zh-Hans, fallback to en-US, then user default.
    let engine = {
        let zh = Language::CreateLanguage(&windows::core::HSTRING::from("zh-Hans")).ok();
        let en = Language::CreateLanguage(&windows::core::HSTRING::from("en-US")).ok();

        let try_create = |lang: &Language| -> Option<WinOcr::OcrEngine> {
            if WinOcr::OcrEngine::IsLanguageSupported(lang).unwrap_or(false) {
                WinOcr::OcrEngine::TryCreateFromLanguage(lang).ok()
            } else {
                None
            }
        };

        zh.as_ref()
            .and_then(try_create)
            .or_else(|| en.as_ref().and_then(try_create))
            .or_else(|| WinOcr::OcrEngine::TryCreateFromUserProfileLanguages().ok())
            .ok_or_else(|| {
                DesktopError::OcrFailed("No OCR language available on this system".into())
            })?
    };

    // 4. Recognize text.
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| DesktopError::OcrFailed(format!("RecognizeAsync failed: {e}")))?
        .get()
        .map_err(|e| DesktopError::OcrFailed(format!("OCR result async get failed: {e}")))?;

    let full_text = result
        .Text()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();

    // 5. Build lines array with bounding boxes.
    let ocr_lines: windows::Foundation::Collections::IVectorView<WinOcr::OcrLine> = result
        .Lines()
        .map_err(|e| DesktopError::OcrFailed(format!("Failed to get OCR lines: {e}")))?;

    let mut lines: Vec<OcrLine> = Vec::new();
    for line in &ocr_lines {
        let line: WinOcr::OcrLine = line;
        let text = line
            .Text()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();

        // Merge bounding boxes of all words in this line.
        let words: windows::Foundation::Collections::IVectorView<WinOcr::OcrWord> = line
            .Words()
            .map_err(|e| DesktopError::OcrFailed(format!("Failed to get words: {e}")))?;

        let mut min_x: f64 = f64::MAX;
        let mut min_y: f64 = f64::MAX;
        let mut max_x: f64 = f64::MIN;
        let mut max_y: f64 = f64::MIN;
        let mut has_bounds = false;

        for word in &words {
            let word: WinOcr::OcrWord = word;
            if let Ok(rect) = word.BoundingRect() {
                has_bounds = true;
                min_x = min_x.min(rect.X as f64);
                min_y = min_y.min(rect.Y as f64);
                max_x = max_x.max((rect.X + rect.Width) as f64);
                max_y = max_y.max((rect.Y + rect.Height) as f64);
            }
        }

        let bounding_box = if has_bounds {
            Some(BoundingBox {
                x: min_x,
                y: min_y,
                w: max_x - min_x,
                h: max_y - min_y,
            })
        } else {
            None
        };

        lines.push(OcrLine {
            text,
            bounding_box,
            confidence: None,
        });
    }

    Ok(OcrResult { full_text, lines })
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// On non-Windows/macOS platforms, `perform_ocr` should return `NotImplemented`.
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    #[test]
    fn test_ocr_not_implemented_on_non_windows() {
        let dummy_png = b"fake png data";
        let result = perform_ocr(dummy_png);
        assert!(result.is_err());
        match result.unwrap_err() {
            DesktopError::NotImplemented(msg) => {
                assert!(
                    msg.contains("OCR not implemented"),
                    "Expected NotImplemented message about OCR, got: {msg}"
                );
            }
            other => panic!("Expected NotImplemented, got: {other:?}"),
        }
    }

    /// Verify that `take_screenshot` returns a proper error type when
    /// it fails (e.g., no display in CI).
    ///
    /// This test doesn't require a display — it just validates that the
    /// function exists and returns a meaningful `DesktopError` variant
    /// (either `ScreenCapture` on failure, or `Ok` if a display is present).
    #[test]
    fn test_take_screenshot_returns_correct_types() {
        let result = take_screenshot(None);
        match result {
            Ok(screenshot) => {
                // If we have a display, verify the screenshot is well-formed.
                assert!(!screenshot.image_base64.is_empty());
                assert!(screenshot.width > 0);
                assert!(screenshot.height > 0);
                assert_eq!(screenshot.format, "png");
            }
            Err(DesktopError::ScreenCapture(msg)) => {
                // No display available (CI) — that's fine, just verify
                // we got the correct error variant.
                assert!(
                    !msg.is_empty(),
                    "ScreenCapture error should have a message"
                );
            }
            Err(other) => {
                panic!("Expected ScreenCapture error or Ok, got: {other:?}");
            }
        }
    }

    /// Verify that `capture_screen_png` returns raw PNG bytes or a proper error.
    #[test]
    fn test_capture_screen_png_returns_correct_types() {
        let result = capture_screen_png();
        match result {
            Ok(bytes) => {
                // PNG files start with the magic bytes: 0x89 P N G
                assert!(bytes.len() > 8, "PNG should be more than 8 bytes");
                assert_eq!(&bytes[..4], b"\x89PNG", "Should start with PNG magic");
            }
            Err(DesktopError::ScreenCapture(_)) => {
                // No display available (CI) — acceptable.
            }
            Err(other) => {
                panic!("Expected ScreenCapture error or Ok, got: {other:?}");
            }
        }
    }
}
