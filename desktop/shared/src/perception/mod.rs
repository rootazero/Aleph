//! Perception capabilities — screen capture, OCR, accessibility tree.
//!
//! This module provides platform-specific implementations for:
//! - Screenshot capture via `xcap`
//! - OCR via platform APIs (`WinRT` on Windows; macOS OCR routes through the
//!   Swift helper via `screen.ocr` RPC in `desktop/macos/src/screen.rs`)
//! - Raw PNG capture for use as OCR input
//!
//! All functions are synchronous and should be called via
//! `tokio::task::spawn_blocking` from async contexts.

// Compiled on all hosts (like `ocr_windows`) so the pure TSV parser stays
// unit-testable cross-platform; `linux_ocr` is only *called* under
// `#[cfg(target_os = "linux")]` in `perform_ocr`.
mod ocr_linux;
mod ocr_windows;
mod screen_record;
mod screenshot;
#[cfg(windows)]
mod window_capture;

pub use screenshot::{
    capture_screen_png, is_degenerate, list_displays, process_screenshot, take_screenshot,
    take_screenshot_display, DEFAULT_SCREENSHOT_MAX_BYTES,
};

#[cfg(windows)]
pub use window_capture::capture_window;

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub use screen_record::screen_record;

#[allow(unused_imports)]
use crate::error::{DesktopError, Result};
use crate::OcrResult;

/// Perform OCR on raw PNG image bytes.
///
/// # Platform support
///
/// - **Windows**: Uses `WinRT` `OcrEngine` API (prefers zh-Hans, fallback to en-US).
/// - **Linux**: Uses Tesseract CLI (requires `tesseract-ocr` package).
/// - **macOS**: Returns [`DesktopError::NotImplemented`] — macOS OCR is routed
///   through the Swift helper via `screen.ocr` RPC in `desktop/macos/src/screen.rs`.
///
/// # Errors
///
/// - [`DesktopError::NotImplemented`] on non-Windows platforms.
/// - [`DesktopError::OcrFailed`] if the Windows OCR engine fails.
pub fn perform_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    #[cfg(target_os = "windows")]
    {
        ocr_windows::windows_ocr(png_bytes)
    }

    #[cfg(target_os = "linux")]
    {
        ocr_linux::linux_ocr(png_bytes)
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = png_bytes;
        Err(DesktopError::NotImplemented(
            "OCR not implemented on this platform".into(),
        ))
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// On non-Windows platforms, `perform_ocr` should return `NotImplemented`.
    #[cfg(not(target_os = "windows"))]
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
                assert!(!msg.is_empty(), "ScreenCapture error should have a message");
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
