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
//! OS APIs. All methods are currently stubs returning `NotImplemented` —
//! they will be filled in by subsequent tasks.

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

// ── NativeDesktop (stub implementation) ─────────────────────────

/// Native desktop implementation using OS APIs.
///
/// Uses `xcap` for screen capture, `enigo` for input automation, and
/// platform-specific APIs for OCR and window management.
///
/// Currently all methods return `DesktopError::NotImplemented` — they
/// will be filled in by subsequent tasks.
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

    async fn screenshot(&self, _region: Option<ScreenRegion>) -> Result<Screenshot> {
        Err(DesktopError::NotImplemented(
            "screenshot() not yet implemented".into(),
        ))
    }

    async fn ocr(&self, _image_png: Option<&[u8]>) -> Result<OcrResult> {
        Err(DesktopError::NotImplemented(
            "ocr() not yet implemented".into(),
        ))
    }

    async fn click(&self, _x: f64, _y: f64, _button: MouseButton) -> Result<()> {
        Err(DesktopError::NotImplemented(
            "click() not yet implemented".into(),
        ))
    }

    async fn type_text(&self, _text: &str) -> Result<()> {
        Err(DesktopError::NotImplemented(
            "type_text() not yet implemented".into(),
        ))
    }

    async fn key_combo(&self, _modifiers: &[String], _key: &str) -> Result<()> {
        Err(DesktopError::NotImplemented(
            "key_combo() not yet implemented".into(),
        ))
    }

    async fn scroll(&self, _direction: &str, _amount: i32) -> Result<()> {
        Err(DesktopError::NotImplemented(
            "scroll() not yet implemented".into(),
        ))
    }

    async fn window_list(&self) -> Result<Vec<WindowInfo>> {
        Err(DesktopError::NotImplemented(
            "window_list() not yet implemented".into(),
        ))
    }

    async fn focus_window(&self, _window_id: u64) -> Result<()> {
        Err(DesktopError::NotImplemented(
            "focus_window() not yet implemented".into(),
        ))
    }

    async fn launch_app(&self, _app_name: &str) -> Result<()> {
        Err(DesktopError::NotImplemented(
            "launch_app() not yet implemented".into(),
        ))
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
    async fn stub_methods_return_not_implemented() {
        let desktop = NativeDesktop::new();

        assert!(matches!(
            desktop.capabilities().await,
            Err(DesktopError::NotImplemented(_))
        ));

        assert!(matches!(
            desktop.screenshot(None).await,
            Err(DesktopError::NotImplemented(_))
        ));

        assert!(matches!(
            desktop.click(0.0, 0.0, MouseButton::Left).await,
            Err(DesktopError::NotImplemented(_))
        ));

        assert!(matches!(
            desktop.type_text("hello").await,
            Err(DesktopError::NotImplemented(_))
        ));

        assert!(matches!(
            desktop.key_combo(&[], "a").await,
            Err(DesktopError::NotImplemented(_))
        ));

        assert!(matches!(
            desktop.scroll("down", 3).await,
            Err(DesktopError::NotImplemented(_))
        ));

        assert!(matches!(
            desktop.window_list().await,
            Err(DesktopError::NotImplemented(_))
        ));

        assert!(matches!(
            desktop.focus_window(1).await,
            Err(DesktopError::NotImplemented(_))
        ));

        assert!(matches!(
            desktop.launch_app("test").await,
            Err(DesktopError::NotImplemented(_))
        ));

        assert!(matches!(
            desktop.ocr(None).await,
            Err(DesktopError::NotImplemented(_))
        ));
    }
}
