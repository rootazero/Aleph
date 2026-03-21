//! Screen perception and input automation capability.

use async_trait::async_trait;

use crate::{MouseButton, OcrResult, Result, ScreenRegion, Screenshot, WindowInfo};

/// Screen perception and input automation.
///
/// This trait covers everything that involves *seeing* and *interacting with*
/// the physical display: capturing screenshots, running OCR, moving the mouse,
/// typing text, pressing key combos, scrolling, and managing windows.
#[async_trait]
pub trait ScreenCapability: Send + Sync {
    /// Capture a screenshot, optionally cropped to a region.
    async fn screenshot(&self, region: Option<ScreenRegion>) -> Result<Screenshot>;

    /// Perform OCR on provided PNG bytes, or on a fresh screenshot if `None`.
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
