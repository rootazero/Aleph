//! Shared `ScreenCapability` implementation using OS-level APIs.
//!
//! This module provides [`NativeScreen`], which implements the
//! [`ScreenCapability`] trait by delegating to the `perception` and `action`
//! modules via `tokio::task::spawn_blocking`.
//!
//! All three platform crates (`desktop-macos`, `desktop-linux`,
//! `desktop-windows`) embed a `NativeScreen` and return it from
//! `DesktopPlatform::screen()`.

use async_trait::async_trait;

use crate::traits::ScreenCapability;
use crate::{
    action, perception, DesktopError, DisplayInfo, MouseButton, OcrResult, PressAction, Result,
    ScreenRegion, Screenshot, WindowInfo,
};

/// Cross-platform `ScreenCapability` implementation.
///
/// Wraps the synchronous `perception` and `action` module functions with
/// `tokio::task::spawn_blocking` so they can be called from async contexts.
pub struct NativeScreen {
    _private: (),
}

impl NativeScreen {
    /// Create a new `NativeScreen` instance.
    ///
    /// Settles this process's DPI awareness on Windows before returning. That
    /// used to live only in `WindowsPlatform::new`, on the reasoning that it was
    /// the single per-OS construction point — but it is not the only door into
    /// this capability: `src/vision/providers/platform_ocr.rs` builds a
    /// `NativeScreen` directly, so a vision request arriving before any desktop
    /// tool captured the screen while the process was still DPI-unaware. The
    /// symptom is not a crash but a contradiction: [`crate::perception`]'s
    /// `coordinate_scale` reads the *live* awareness level, so the same display
    /// reported one `scale_factor` to the OCR path and a different one to the
    /// desktop tools a moment later.
    ///
    /// The opt-in is latched by a `OnceLock`, so putting it on the shared
    /// constructor costs one atomic load per platform object and closes the
    /// door. See [`crate::win_dpi`].
    #[must_use]
    pub fn new() -> Self {
        crate::win_dpi::ensure_process_dpi_aware();
        Self { _private: () }
    }
}

impl Default for NativeScreen {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ScreenCapability for NativeScreen {
    async fn screenshot(&self, region: Option<ScreenRegion>) -> Result<Screenshot> {
        tokio::task::spawn_blocking(move || perception::take_screenshot(region.as_ref()))
            .await
            .map_err(|e| DesktopError::ScreenCapture(format!("task join error: {e}")))?
    }

    async fn ocr(&self, image_png: Option<&[u8]>) -> Result<OcrResult> {
        let png_bytes = match image_png {
            Some(bytes) => bytes.to_vec(),
            None => tokio::task::spawn_blocking(perception::capture_screen_png)
                .await
                .map_err(|e| DesktopError::OcrFailed(format!("task join error: {e}")))??,
        };
        tokio::task::spawn_blocking(move || perception::perform_ocr(&png_bytes))
            .await
            .map_err(|e| DesktopError::OcrFailed(format!("task join error: {e}")))?
    }

    async fn click(&self, x: f64, y: f64, button: MouseButton) -> Result<()> {
        tokio::task::spawn_blocking(move || action::click(x, y, button))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        let text = text.to_string();
        tokio::task::spawn_blocking(move || action::type_text(&text))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn key_combo(&self, modifiers: &[String], key: &str) -> Result<()> {
        let modifiers = modifiers.to_vec();
        let key = key.to_string();
        tokio::task::spawn_blocking(move || action::key_combo(&modifiers, &key))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn scroll(&self, direction: &str, amount: i32) -> Result<()> {
        let direction = direction.to_string();
        tokio::task::spawn_blocking(move || action::scroll(&direction, amount))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn window_list(&self) -> Result<Vec<WindowInfo>> {
        tokio::task::spawn_blocking(action::window_list)
            .await
            .map_err(|e| DesktopError::WindowFailed(format!("task join error: {e}")))?
    }

    async fn focus_window(&self, window_id: u64) -> Result<()> {
        tokio::task::spawn_blocking(move || action::focus_window(window_id))
            .await
            .map_err(|e| DesktopError::WindowFailed(format!("task join error: {e}")))?
    }

    async fn move_window(&self, window_id: u64, x: i32, y: i32) -> Result<()> {
        tokio::task::spawn_blocking(move || action::move_window(window_id, x, y))
            .await
            .map_err(|e| DesktopError::WindowFailed(format!("task join error: {e}")))?
    }

    async fn resize_window(&self, window_id: u64, width: u32, height: u32) -> Result<()> {
        tokio::task::spawn_blocking(move || action::resize_window(window_id, width, height))
            .await
            .map_err(|e| DesktopError::WindowFailed(format!("task join error: {e}")))?
    }

    async fn launch_app(&self, app_name: &str) -> Result<()> {
        let app_name = app_name.to_string();
        tokio::task::spawn_blocking(move || action::launch_app(&app_name))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn screen_record(
        &self,
        config: crate::screen_types::ScreenRecordConfig,
    ) -> Result<crate::screen_types::ScreenRecordResult> {
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        {
            tokio::task::spawn_blocking(move || perception::screen_record(&config))
                .await
                .map_err(|e| DesktopError::ScreenCapture(format!("task join error: {e}")))?
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            let _ = config;
            Err(DesktopError::NotImplemented(
                "screen recording not available on this platform".into(),
            ))
        }
    }

    async fn double_click(&self, x: f64, y: f64, button: MouseButton) -> Result<()> {
        tokio::task::spawn_blocking(move || action::double_click(x, y, button))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn drag(
        &self,
        start_x: f64,
        start_y: f64,
        end_x: f64,
        end_y: f64,
        duration_ms: Option<u64>,
    ) -> Result<()> {
        tokio::task::spawn_blocking(move || {
            action::drag(start_x, start_y, end_x, end_y, duration_ms)
        })
        .await
        .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn hover(&self, x: f64, y: f64) -> Result<()> {
        tokio::task::spawn_blocking(move || action::hover(x, y))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn cursor_position(&self) -> Result<(f64, f64)> {
        tokio::task::spawn_blocking(action::cursor_position)
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn mouse_button(
        &self,
        x: f64,
        y: f64,
        button: MouseButton,
        press_action: PressAction,
    ) -> Result<()> {
        tokio::task::spawn_blocking(move || action::mouse_button(x, y, button, press_action))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn key_button(&self, keys: &[String], action: PressAction) -> Result<()> {
        let keys = keys.to_vec();
        tokio::task::spawn_blocking(move || action::key_button(&keys, action))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn quit_app(&self, app_name: &str) -> Result<()> {
        let app_name = app_name.to_string();
        tokio::task::spawn_blocking(move || action::quit_app(&app_name))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn clipboard_read(&self) -> Result<String> {
        tokio::task::spawn_blocking(action::clipboard_read)
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn clipboard_write(&self, text: &str) -> Result<()> {
        let text = text.to_string();
        tokio::task::spawn_blocking(move || action::clipboard_write(&text))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    /// Single-window capture, wired on Linux.
    ///
    /// `xcap` can enumerate and capture windows on Windows too, but the arm
    /// stays Linux-only until it is exercised on a Windows host: the trait
    /// default is an explicit `NotImplemented`, and turning that into a wrong
    /// capture would be worse than leaving it unimplemented. Widening the `cfg`
    /// is the whole change when someone can verify it there.
    ///
    /// `show_cursor` is ignored: `xcap`'s window capture never includes the
    /// pointer, so honouring `true` is not something this backend can do — and
    /// a cursor-free shot of the right window is the useful half anyway.
    #[cfg(target_os = "linux")]
    async fn screenshot_window(
        &self,
        window_id: u64,
        _show_cursor: bool,
    ) -> Result<crate::WindowShot> {
        tokio::task::spawn_blocking(move || perception::take_screenshot_window(window_id))
            .await
            .map_err(|e| DesktopError::ScreenCapture(format!("task join error: {e}")))?
    }

    async fn display_list(&self) -> Result<Vec<DisplayInfo>> {
        tokio::task::spawn_blocking(perception::list_displays)
            .await
            .map_err(|e| DesktopError::ScreenCapture(format!("task join error: {e}")))?
    }

    /// Capture one window, cropped to its visible frame.
    ///
    /// Windows renders the window itself (`PrintWindow`), so an occluded or
    /// background window comes back intact and the user's foreground app is
    /// never raised or disturbed. Platforms without an implementation keep the
    /// trait's `NotImplemented` default rather than silently degrading to a
    /// full-screen capture — a caller that asked for one window and received the
    /// whole desktop would map its coordinates through the wrong origin.
    #[cfg(windows)]
    async fn screenshot_window(
        &self,
        window_id: u64,
        show_cursor: bool,
    ) -> Result<crate::WindowShot> {
        tokio::task::spawn_blocking(move || perception::capture_window(window_id, show_cursor))
            .await
            .map_err(|e| DesktopError::ScreenCapture(format!("task join error: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_native_screen_creation() {
        let _screen = NativeScreen::new();
        let _screen_default = NativeScreen::default();
    }

    #[tokio::test]
    async fn test_screenshot_returns_correct_types() {
        let screen = NativeScreen::new();
        let result = screen.screenshot(None).await;
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
}
