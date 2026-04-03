//! Screen perception and input automation capability.

use async_trait::async_trait;

use crate::screen_types::{ScreenRecordConfig, ScreenRecordResult};
use crate::{DisplayInfo, MouseButton, OcrResult, PressAction, Result, ScreenRegion, Screenshot, WindowInfo};

/// Screen perception and input automation.
#[async_trait]
pub trait ScreenCapability: Send + Sync {
    // ── Existing methods (unchanged) ────────────────────────────
    async fn screenshot(&self, region: Option<ScreenRegion>) -> Result<Screenshot>;
    async fn ocr(&self, image_png: Option<&[u8]>) -> Result<OcrResult>;
    async fn click(&self, x: f64, y: f64, button: MouseButton) -> Result<()>;
    async fn type_text(&self, text: &str) -> Result<()>;
    async fn key_combo(&self, modifiers: &[String], key: &str) -> Result<()>;
    async fn scroll(&self, direction: &str, amount: i32) -> Result<()>;
    async fn window_list(&self) -> Result<Vec<WindowInfo>>;
    async fn focus_window(&self, window_id: u64) -> Result<()>;
    async fn launch_app(&self, app_name: &str) -> Result<()>;

    async fn screen_record(&self, config: ScreenRecordConfig) -> Result<ScreenRecordResult> {
        let _ = config;
        Err(crate::DesktopError::NotImplemented(
            "screen recording not available on this platform".into(),
        ))
    }

    // ── New methods (Phase 1) ───────────────────────────────────

    /// Double-click at the specified coordinates.
    async fn double_click(&self, x: f64, y: f64, button: MouseButton) -> Result<()> {
        let _ = (x, y, button);
        Err(crate::DesktopError::NotImplemented("double_click".into()))
    }

    /// Drag from (start_x, start_y) to (end_x, end_y).
    /// duration_ms controls drag animation; None for instant.
    async fn drag(
        &self,
        start_x: f64,
        start_y: f64,
        end_x: f64,
        end_y: f64,
        duration_ms: Option<u64>,
    ) -> Result<()> {
        let _ = (start_x, start_y, end_x, end_y, duration_ms);
        Err(crate::DesktopError::NotImplemented("drag".into()))
    }

    /// Move mouse to (x, y) without clicking.
    async fn hover(&self, x: f64, y: f64) -> Result<()> {
        let _ = (x, y);
        Err(crate::DesktopError::NotImplemented("hover".into()))
    }

    /// Get current mouse cursor position.
    async fn cursor_position(&self) -> Result<(f64, f64)> {
        Err(crate::DesktopError::NotImplemented("cursor_position".into()))
    }

    /// Press/release mouse button independently.
    async fn mouse_button(
        &self,
        x: f64,
        y: f64,
        button: MouseButton,
        action: PressAction,
    ) -> Result<()> {
        let _ = (x, y, button, action);
        Err(crate::DesktopError::NotImplemented("mouse_button".into()))
    }

    /// Quit/close an application by name or bundle ID.
    async fn quit_app(&self, app_name: &str) -> Result<()> {
        let _ = app_name;
        Err(crate::DesktopError::NotImplemented("quit_app".into()))
    }

    /// Read text content from system clipboard.
    async fn clipboard_read(&self) -> Result<String> {
        Err(crate::DesktopError::NotImplemented("clipboard_read".into()))
    }

    /// Write text to system clipboard.
    async fn clipboard_write(&self, text: &str) -> Result<()> {
        let _ = text;
        Err(crate::DesktopError::NotImplemented("clipboard_write".into()))
    }

    /// List all connected displays.
    async fn display_list(&self) -> Result<Vec<DisplayInfo>> {
        Err(crate::DesktopError::NotImplemented("display_list".into()))
    }
}
