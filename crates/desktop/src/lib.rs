//! # aleph-desktop
//!
//! Cross-platform desktop capabilities for the Aleph AI assistant.
//!
//! This crate provides the [`DesktopCapability`] trait — the contract between
//! Aleph's core brain and the physical desktop it controls. Implementations
//! use `xcap` for screen capture, `enigo` for input automation, and
//! platform-specific APIs for OCR and window management.
//!
//! ## Architecture
//!
//! ```text
//! Core (trait contract)  →  aleph-desktop (this crate)  →  OS APIs
//!                              ├── perception.rs  (screenshot, OCR)
//!                              └── action.rs      (click, type, scroll)
//! ```
//!
//! The [`NativeDesktop`] struct implements [`DesktopCapability`] using real
//! OS APIs. Perception (screenshot, OCR) and action (click, type, scroll,
//! key combo, app launch, window management) are implemented; only
//! `capabilities()` remains a stub.

pub mod action;
pub mod error;
pub mod perception;

pub use error::{DesktopError, Result};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ── Types ────────────────────────────────────────────────────────

/// A capability that this desktop implementation supports.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Capability {
    /// Capability name (e.g. "screen_capture", "ocr", "keyboard_control").
    pub name: String,
    /// Capability version string.
    pub version: String,
}

/// A rectangular region on the screen, in physical pixels.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ScreenRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A captured screenshot, encoded as PNG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Screenshot {
    /// Base64-encoded PNG image data.
    pub image_base64: String,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Image format (always "png").
    pub format: String,
}

/// Result of an OCR operation on a screenshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrResult {
    /// Full recognized text, concatenated from all lines.
    pub full_text: String,
    /// Individual lines with optional bounding boxes.
    pub lines: Vec<OcrLine>,
}

/// A single line of text recognized by OCR.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrLine {
    /// The recognized text content.
    pub text: String,
    /// Optional bounding box for this line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounding_box: Option<BoundingBox>,
    /// Optional confidence score (0.0 to 1.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

/// A bounding box in screen coordinates.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Mouse button identifiers.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Information about an on-screen window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Platform-specific window identifier (HWND on Windows, XID on Linux, etc.).
    pub id: u64,
    /// Window title.
    pub title: String,
    /// Owning application name (may be empty on some platforms).
    pub owner: String,
    /// Process ID of the owning application.
    pub pid: u64,
}

// ── DesktopCapability trait ──────────────────────────────────────

/// The contract between Aleph's core and the physical desktop.
///
/// This trait defines all desktop automation operations that the AI can
/// perform: perceiving the screen (screenshot, OCR) and acting on it
/// (click, type, scroll, window management).
///
/// All methods are async and the trait requires `Send + Sync` so it can
/// be used across async task boundaries.
#[async_trait]
pub trait DesktopCapability: Send + Sync {
    /// Return the list of capabilities this implementation supports.
    async fn capabilities(&self) -> Result<Vec<Capability>>;

    /// Capture a screenshot of the primary monitor, optionally cropped to a region.
    async fn screenshot(&self, region: Option<ScreenRegion>) -> Result<Screenshot>;

    /// Perform OCR on the provided PNG image data, or on a fresh screenshot if `None`.
    async fn ocr(&self, image_png: Option<&[u8]>) -> Result<OcrResult>;

    /// Move the mouse to (x, y) and click the specified button.
    async fn click(&self, x: f64, y: f64, button: MouseButton) -> Result<()>;

    /// Type a string of text at the current cursor position.
    async fn type_text(&self, text: &str) -> Result<()>;

    /// Press a key combination (e.g., Cmd+C, Ctrl+Shift+Tab).
    ///
    /// `modifiers` contains modifier key names: "meta", "shift", "control", "alt".
    /// `key` is the main key name: single character or named key ("return", "tab", etc.).
    async fn key_combo(&self, modifiers: &[String], key: &str) -> Result<()>;

    /// Scroll the mouse wheel.
    ///
    /// `direction` is "up", "down", "left", or "right".
    /// `amount` is the number of scroll clicks.
    async fn scroll(&self, direction: &str, amount: i32) -> Result<()>;

    /// List all visible on-screen windows.
    async fn window_list(&self) -> Result<Vec<WindowInfo>>;

    /// Bring the specified window to the foreground.
    async fn focus_window(&self, window_id: u64) -> Result<()>;

    /// Launch an application by name or bundle ID.
    async fn launch_app(&self, app_name: &str) -> Result<()>;
}

// ── NativeDesktop ────────────────────────────────────────────────

/// Native desktop implementation using OS APIs.
///
/// Uses `xcap` for screen capture, `enigo` for input automation, and
/// platform-specific APIs for OCR and window management.
///
/// Most methods delegate to the `perception` and `action` modules via
/// `tokio::task::spawn_blocking` since the underlying libraries are
/// synchronous.
pub struct NativeDesktop {
    _private: (),
}

impl NativeDesktop {
    /// Create a new `NativeDesktop` instance.
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for NativeDesktop {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DesktopCapability for NativeDesktop {
    async fn capabilities(&self) -> Result<Vec<Capability>> {
        Err(DesktopError::NotImplemented(
            "capabilities() not yet implemented".into(),
        ))
    }

    async fn screenshot(&self, region: Option<ScreenRegion>) -> Result<Screenshot> {
        tokio::task::spawn_blocking(move || perception::take_screenshot(region.as_ref()))
            .await
            .map_err(|e| DesktopError::ScreenCapture(format!("Task join error: {e}")))?
    }

    async fn ocr(&self, image_png: Option<&[u8]>) -> Result<OcrResult> {
        let png_bytes = match image_png {
            Some(bytes) => bytes.to_vec(),
            None => {
                tokio::task::spawn_blocking(perception::capture_screen_png)
                    .await
                    .map_err(|e| DesktopError::OcrFailed(format!("Task join error: {e}")))??
            }
        };
        tokio::task::spawn_blocking(move || perception::perform_ocr(&png_bytes))
            .await
            .map_err(|e| DesktopError::OcrFailed(format!("Task join error: {e}")))?
    }

    async fn click(&self, x: f64, y: f64, button: MouseButton) -> Result<()> {
        tokio::task::spawn_blocking(move || action::click(x, y, button))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("Task join error: {e}")))?
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        let text = text.to_string();
        tokio::task::spawn_blocking(move || action::type_text(&text))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("Task join error: {e}")))?
    }

    async fn key_combo(&self, modifiers: &[String], key: &str) -> Result<()> {
        let modifiers = modifiers.to_vec();
        let key = key.to_string();
        tokio::task::spawn_blocking(move || action::key_combo(&modifiers, &key))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("Task join error: {e}")))?
    }

    async fn scroll(&self, direction: &str, amount: i32) -> Result<()> {
        let direction = direction.to_string();
        tokio::task::spawn_blocking(move || action::scroll(&direction, amount))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("Task join error: {e}")))?
    }

    async fn window_list(&self) -> Result<Vec<WindowInfo>> {
        tokio::task::spawn_blocking(action::window_list)
            .await
            .map_err(|e| DesktopError::WindowFailed(format!("Task join error: {e}")))?
    }

    async fn focus_window(&self, window_id: u64) -> Result<()> {
        tokio::task::spawn_blocking(move || action::focus_window(window_id))
            .await
            .map_err(|e| DesktopError::WindowFailed(format!("Task join error: {e}")))?
    }

    async fn launch_app(&self, app_name: &str) -> Result<()> {
        let app_name = app_name.to_string();
        tokio::task::spawn_blocking(move || action::launch_app(&app_name))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("Task join error: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_desktop_default() {
        let _desktop = NativeDesktop::default();
    }

    #[tokio::test]
    async fn remaining_stubs_return_not_implemented() {
        let desktop = NativeDesktop::new();

        // capabilities() is still a stub.
        assert!(matches!(
            desktop.capabilities().await,
            Err(DesktopError::NotImplemented(_))
        ));

        // screenshot() is implemented — see perception::tests.
        // ocr() is implemented — on non-Windows returns NotImplemented (from perform_ocr).

        // Input actions (click, type_text, key_combo, scroll) are now implemented
        // via enigo — they attempt real OS calls, so we don't test them in unit
        // tests (they'd move the mouse / press keys).

        // Window management: on macOS, window_list and focus_window return NotImplemented.
        #[cfg(target_os = "macos")]
        {
            assert!(matches!(
                desktop.window_list().await,
                Err(DesktopError::NotImplemented(_))
            ));

            assert!(matches!(
                desktop.focus_window(1).await,
                Err(DesktopError::NotImplemented(_))
            ));
        }

        // key_combo with invalid input should return InputFailed, not NotImplemented.
        let result = desktop.key_combo(&[], "").await;
        assert!(
            matches!(result, Err(DesktopError::InputFailed(_))),
            "Expected InputFailed for empty key, got: {result:?}"
        );

        // scroll with invalid direction should return InputFailed.
        let result = desktop.scroll("diagonal", 3).await;
        assert!(
            matches!(result, Err(DesktopError::InputFailed(_))),
            "Expected InputFailed for invalid direction, got: {result:?}"
        );
    }

    /// Verify that screenshot() via NativeDesktop returns correct types.
    #[tokio::test]
    async fn screenshot_returns_correct_types() {
        let desktop = NativeDesktop::new();
        let result = desktop.screenshot(None).await;
        match result {
            Ok(screenshot) => {
                assert!(!screenshot.image_base64.is_empty());
                assert!(screenshot.width > 0);
                assert!(screenshot.height > 0);
                assert_eq!(screenshot.format, "png");
            }
            Err(DesktopError::ScreenCapture(_)) => {
                // No display (CI) — acceptable.
            }
            Err(other) => {
                panic!("Expected Ok or ScreenCapture error, got: {other:?}");
            }
        }
    }

    /// Verify that ocr() with provided image bytes works on non-Windows.
    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn ocr_with_bytes_returns_not_implemented() {
        let desktop = NativeDesktop::new();
        let dummy_png = b"fake png data";
        let result = desktop.ocr(Some(dummy_png)).await;
        assert!(
            matches!(result, Err(DesktopError::NotImplemented(_))),
            "Expected NotImplemented on non-Windows, got: {result:?}"
        );
    }
}
