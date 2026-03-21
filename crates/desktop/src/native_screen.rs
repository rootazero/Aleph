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
use crate::{action, perception, DesktopError, MouseButton, OcrResult, Result, ScreenRegion, Screenshot, WindowInfo};

/// Cross-platform `ScreenCapability` implementation.
///
/// Wraps the synchronous `perception` and `action` module functions with
/// `tokio::task::spawn_blocking` so they can be called from async contexts.
pub struct NativeScreen {
    _private: (),
}

impl NativeScreen {
    /// Create a new `NativeScreen` instance.
    pub fn new() -> Self {
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
            None => {
                tokio::task::spawn_blocking(perception::capture_screen_png)
                    .await
                    .map_err(|e| DesktopError::OcrFailed(format!("task join error: {e}")))??
            }
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

    async fn launch_app(&self, app_name: &str) -> Result<()> {
        let app_name = app_name.to_string();
        tokio::task::spawn_blocking(move || action::launch_app(&app_name))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
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
