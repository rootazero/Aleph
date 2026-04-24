//! macOS screen capability: delegates OCR to the Swift helper; all other
//! methods forward to `NativeScreen`.

use std::sync::Arc;

use aleph_desktop::traits::ScreenCapability;
use aleph_desktop::{
    DesktopError, DisplayInfo, MouseButton, NativeScreen, OcrLine, OcrResult, PressAction, Result,
    ScreenRegion, Screenshot, SwiftBridge, WindowInfo,
};
use async_trait::async_trait;
use base64::Engine as _;

/// macOS-specific screen capability that routes `ocr()` through the Swift
/// helper (`screen.ocr` RPC) while delegating everything else to `NativeScreen`.
pub struct MacOSScreen {
    inner: NativeScreen,
    bridge: Arc<SwiftBridge>,
}

impl MacOSScreen {
    pub fn new(bridge: Arc<SwiftBridge>) -> Self {
        Self {
            inner: NativeScreen::new(),
            bridge,
        }
    }
}

#[async_trait]
impl ScreenCapability for MacOSScreen {
    async fn screenshot(&self, region: Option<ScreenRegion>) -> Result<Screenshot> {
        self.inner.screenshot(region).await
    }

    async fn ocr(&self, image_png: Option<&[u8]>) -> Result<OcrResult> {
        use aleph_desktop::perception::capture_screen_png;
        use aleph_protocol::desktop_bridge::methods::screen::{
            OcrParams, OcrResult as RpcOcrResult, METHOD_OCR,
        };

        // Capture screen if no image provided.
        let png_bytes: Vec<u8> = match image_png {
            Some(bytes) => bytes.to_vec(),
            None => tokio::task::spawn_blocking(capture_screen_png)
                .await
                .map_err(|e| DesktopError::OcrFailed(format!("screen capture join error: {e}")))?
                .map_err(|e| DesktopError::OcrFailed(format!("screen capture failed: {e}")))?,
        };

        // Parse dimensions for coordinate conversion.
        let (img_width, img_height) = png_dimensions(&png_bytes).unwrap_or((1.0, 1.0));

        // Encode PNG as base64 for the RPC call.
        let image_base64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);

        // Call the Swift helper.
        let rpc: RpcOcrResult = self
            .bridge
            .call(
                METHOD_OCR,
                OcrParams {
                    image_base64,
                    languages: Vec::new(),
                    fast_mode: false,
                },
            )
            .await
            .map_err(|e| DesktopError::OcrFailed(format!("bridge screen.ocr: {e}")))?;

        // Convert protocol OcrResult → aleph_desktop OcrResult.
        // Vision boundingBox is normalized, bottom-left origin.
        // We convert to pixel-space, top-left origin.
        let lines: Vec<OcrLine> = rpc
            .blocks
            .into_iter()
            .map(|block| {
                use aleph_desktop::BoundingBox;
                let pixel_x = block.bbox.x * img_width;
                let pixel_y = (1.0 - block.bbox.y - block.bbox.height) * img_height;
                let pixel_w = block.bbox.width * img_width;
                let pixel_h = block.bbox.height * img_height;
                OcrLine {
                    text: block.text,
                    bounding_box: Some(BoundingBox {
                        x: pixel_x,
                        y: pixel_y,
                        w: pixel_w,
                        h: pixel_h,
                    }),
                    confidence: Some(block.confidence as f64),
                }
            })
            .collect();

        Ok(OcrResult {
            full_text: rpc.full_text,
            lines,
        })
    }

    async fn click(&self, x: f64, y: f64, button: MouseButton) -> Result<()> {
        self.inner.click(x, y, button).await
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        self.inner.type_text(text).await
    }

    async fn key_combo(&self, modifiers: &[String], key: &str) -> Result<()> {
        self.inner.key_combo(modifiers, key).await
    }

    async fn scroll(&self, direction: &str, amount: i32) -> Result<()> {
        self.inner.scroll(direction, amount).await
    }

    async fn window_list(&self) -> Result<Vec<WindowInfo>> {
        self.inner.window_list().await
    }

    async fn focus_window(&self, window_id: u64) -> Result<()> {
        self.inner.focus_window(window_id).await
    }

    async fn launch_app(&self, app_name: &str) -> Result<()> {
        self.inner.launch_app(app_name).await
    }

    async fn screen_record(
        &self,
        config: aleph_desktop::screen_types::ScreenRecordConfig,
    ) -> Result<aleph_desktop::screen_types::ScreenRecordResult> {
        self.inner.screen_record(config).await
    }

    async fn double_click(&self, x: f64, y: f64, button: MouseButton) -> Result<()> {
        self.inner.double_click(x, y, button).await
    }

    async fn drag(
        &self,
        start_x: f64,
        start_y: f64,
        end_x: f64,
        end_y: f64,
        duration_ms: Option<u64>,
    ) -> Result<()> {
        self.inner
            .drag(start_x, start_y, end_x, end_y, duration_ms)
            .await
    }

    async fn hover(&self, x: f64, y: f64) -> Result<()> {
        self.inner.hover(x, y).await
    }

    async fn cursor_position(&self) -> Result<(f64, f64)> {
        self.inner.cursor_position().await
    }

    async fn mouse_button(
        &self,
        x: f64,
        y: f64,
        button: MouseButton,
        action: PressAction,
    ) -> Result<()> {
        self.inner.mouse_button(x, y, button, action).await
    }

    async fn quit_app(&self, app_name: &str) -> Result<()> {
        self.inner.quit_app(app_name).await
    }

    async fn clipboard_read(&self) -> Result<String> {
        self.inner.clipboard_read().await
    }

    async fn clipboard_write(&self, text: &str) -> Result<()> {
        self.inner.clipboard_write(text).await
    }

    async fn display_list(&self) -> Result<Vec<DisplayInfo>> {
        self.inner.display_list().await
    }
}

/// Extract width/height from PNG IHDR chunk header.
/// Returns None if the data is too short or doesn't start with the PNG signature.
fn png_dimensions(png_bytes: &[u8]) -> Option<(f64, f64)> {
    // PNG: 8 bytes signature, then IHDR chunk: 4 len + 4 "IHDR" + 4 width + 4 height
    if png_bytes.len() < 24 {
        return None;
    }
    if &png_bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let width = u32::from_be_bytes([png_bytes[16], png_bytes[17], png_bytes[18], png_bytes[19]]);
    let height = u32::from_be_bytes([png_bytes[20], png_bytes[21], png_bytes[22], png_bytes[23]]);
    Some((width as f64, height as f64))
}
