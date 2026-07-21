//! macOS screen capability.
//!
//! Routes `ocr()` and `screenshot()` / `display_list()` through the Swift
//! helper (`screen.*` RPCs — backed by Vision + `ScreenCaptureKit`); falls
//! back to `NativeScreen` (xcap) for screenshots when the bridge call fails
//! (e.g. macOS 13 lacks `SCScreenshotManager`) *or* when it succeeds with a
//! degenerate frame.
//!
//! # Two input rails
//!
//! The plain input methods (`click`, `type_text`, …) forward to `NativeScreen`
//! (enigo), which posts to the **global** HID event tap: the user's physical
//! cursor is dragged across the screen and the target app must be frontmost.
//!
//! The `*_targeted` methods route through the Swift helper's `input.*` RPCs,
//! which build each event against a session-state event source and post it
//! straight into **one process's** event queue (`postToPid`). The user's cursor
//! never moves and the app need not be frontmost — the same property the AX
//! write rail (`ax.set_value` / `ax.perform_action`) has always had.
//!
//! There is deliberately **no fallback** from the targeted rail to the global
//! one here: a caller that asked for "do not disturb the user" must be told it
//! could not be done, not silently be given the intrusive path. Choosing what
//! to do next is the model's call (A2), not this layer's.

use std::sync::Arc;

use aleph_desktop::perception::is_degenerate;
use aleph_desktop::traits::ScreenCapability;
use aleph_desktop::{
    BoundingBox, DesktopError, DisplayInfo, MouseButton, NativeScreen, OcrLine, OcrResult,
    PressAction, Result, ScreenRegion, Screenshot, SwiftBridge, WindowInfo, WindowShot,
};
use aleph_protocol::desktop_bridge::methods::input as rpc_input;
use async_trait::async_trait;
use base64::Engine as _;
use tracing::warn;

/// Translate the capability-level mouse button into its wire form.
const fn to_rpc_button(button: MouseButton) -> rpc_input::MouseButton {
    match button {
        MouseButton::Left => rpc_input::MouseButton::Left,
        MouseButton::Right => rpc_input::MouseButton::Right,
        MouseButton::Middle => rpc_input::MouseButton::Middle,
    }
}

/// Translate the capability-level press action into its wire form.
const fn to_rpc_press(action: PressAction) -> rpc_input::PressAction {
    match action {
        PressAction::Press => rpc_input::PressAction::Press,
        PressAction::Release => rpc_input::PressAction::Release,
        PressAction::Click => rpc_input::PressAction::Click,
    }
}

/// Hold the helper to the promise the `*_targeted` contract makes.
///
/// `ok: false` is a plain failure. `delivery != "targeted"` is the subtler one:
/// the helper already moved the user's cursor on the global tap. It cannot be
/// undone, but it must not be reported as a background action — the caller
/// promised the user's cursor would stay put, and silence here would make that
/// promise a lie. Surface it as an error so the model sees what happened and
/// picks its own way forward.
fn ensure_targeted(method: &str, ok: bool, delivery: &str) -> Result<()> {
    if !ok {
        return Err(DesktopError::InputFailed(format!(
            "bridge {method}: the helper reported the event was not delivered"
        )));
    }
    if delivery != rpc_input::DELIVERY_TARGETED {
        return Err(DesktopError::InputFailed(format!(
            "bridge {method}: targeted delivery dropped — the helper delivered via the \
             '{delivery}' rail instead of into the target process"
        )));
    }
    Ok(())
}

/// macOS-specific screen capability that routes `ocr()` through the Swift
/// helper (`screen.ocr` RPC) while delegating everything else to `NativeScreen`.
pub struct MacOSScreen {
    inner: NativeScreen,
    bridge: Arc<SwiftBridge>,
}

impl MacOSScreen {
    pub const fn new(bridge: Arc<SwiftBridge>) -> Self {
        Self {
            inner: NativeScreen::new(),
            bridge,
        }
    }
}

#[async_trait]
impl ScreenCapability for MacOSScreen {
    async fn screenshot(&self, region: Option<ScreenRegion>) -> Result<Screenshot> {
        match self.screenshot_via_bridge(region.as_ref()).await {
            Ok(shot) if !is_degenerate(&shot) => Ok(shot),
            // A blank frame is a wedged helper reporting success. xcap is a
            // genuinely different transport, so it can still see the screen —
            // but it is the *only* retry: whatever it returns is returned as-is,
            // degenerate or not, and the tool layer decides what to tell the
            // model. Two dead transports are a diagnosis, not a reason to loop.
            Ok(_) => {
                warn!("screen.capture bridge returned a degenerate frame; falling back to xcap");
                self.inner.screenshot(region).await
            }
            Err(err) => {
                warn!(
                    error = %err,
                    "screen.capture bridge call failed; falling back to xcap"
                );
                self.inner.screenshot(region).await
            }
        }
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
        let (img_width, img_height) = png_dimensions(&png_bytes).ok_or_else(|| {
            DesktopError::OcrFailed("invalid PNG: cannot read image dimensions".to_string())
        })?;

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
            .map_err(|e| match e {
                DesktopError::BridgeFailed(m) => {
                    DesktopError::OcrFailed(format!("bridge screen.ocr: {m}"))
                }
                other => other,
            })?;

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
                    confidence: Some(f64::from(block.confidence)),
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

    async fn move_window(&self, window_id: u64, x: i32, y: i32) -> Result<()> {
        self.inner.move_window(window_id, x, y).await
    }

    async fn resize_window(&self, window_id: u64, width: u32, height: u32) -> Result<()> {
        self.inner.resize_window(window_id, width, height).await
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

    async fn key_button(&self, keys: &[String], action: PressAction) -> Result<()> {
        self.inner.key_button(keys, action).await
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
        match self.display_list_via_bridge().await {
            Ok(list) => Ok(list),
            Err(err) => {
                warn!(
                    error = %err,
                    "screen.list_displays bridge call failed; falling back to xcap"
                );
                self.inner.display_list().await
            }
        }
    }

    // ── Targeted rail (pid-scoped, cursor-free) ──────────────────────
    //
    // Each of these posts into one process's event queue via the Swift helper.
    // None of them touches the user's cursor, and none of them falls back to
    // the global tap: an error here means the background delivery did not
    // happen, and the *caller* decides whether the intrusive path is worth it.

    fn supports_targeted_input(&self) -> bool {
        true
    }

    async fn click_targeted(&self, pid: i32, x: f64, y: f64, button: MouseButton) -> Result<()> {
        let r: rpc_input::ClickResult = self
            .call_input(
                rpc_input::METHOD_CLICK,
                rpc_input::ClickParams {
                    x,
                    y,
                    button: to_rpc_button(button),
                    pid: Some(pid),
                    click_count: None,
                },
            )
            .await?;
        ensure_targeted(rpc_input::METHOD_CLICK, r.ok, &r.delivery)
    }

    async fn double_click_targeted(
        &self,
        pid: i32,
        x: f64,
        y: f64,
        button: MouseButton,
    ) -> Result<()> {
        // `input.double_click` reuses ClickParams and forces click count 2 on
        // the event itself — two independent single clicks are not a
        // double-click, the app reads the count off the event.
        let r: rpc_input::ClickResult = self
            .call_input(
                rpc_input::METHOD_DOUBLE_CLICK,
                rpc_input::ClickParams {
                    x,
                    y,
                    button: to_rpc_button(button),
                    pid: Some(pid),
                    click_count: Some(2),
                },
            )
            .await?;
        ensure_targeted(rpc_input::METHOD_DOUBLE_CLICK, r.ok, &r.delivery)
    }

    async fn drag_targeted(
        &self,
        pid: i32,
        start_x: f64,
        start_y: f64,
        end_x: f64,
        end_y: f64,
        duration_ms: Option<u64>,
    ) -> Result<()> {
        let r: rpc_input::DragResult = self
            .call_input(
                rpc_input::METHOD_DRAG,
                rpc_input::DragParams {
                    start_x,
                    start_y,
                    end_x,
                    end_y,
                    duration_ms,
                    pid: Some(pid),
                },
            )
            .await?;
        ensure_targeted(rpc_input::METHOD_DRAG, r.ok, &r.delivery)
    }

    async fn hover_targeted(&self, pid: i32, x: f64, y: f64) -> Result<()> {
        let r: rpc_input::HoverResult = self
            .call_input(
                rpc_input::METHOD_HOVER,
                rpc_input::HoverParams {
                    x,
                    y,
                    pid: Some(pid),
                },
            )
            .await?;
        ensure_targeted(rpc_input::METHOD_HOVER, r.ok, &r.delivery)
    }

    async fn scroll_targeted(
        &self,
        pid: i32,
        x: f64,
        y: f64,
        direction: &str,
        amount: i32,
    ) -> Result<()> {
        // The point is mandatory on this rail: the cursor never moves, so the
        // event carries the only location the app can route the scroll by.
        let r: rpc_input::ScrollResult = self
            .call_input(
                rpc_input::METHOD_SCROLL,
                rpc_input::ScrollParams {
                    direction: direction.to_string(),
                    amount,
                    pid: Some(pid),
                    x: Some(x),
                    y: Some(y),
                },
            )
            .await?;
        ensure_targeted(rpc_input::METHOD_SCROLL, r.ok, &r.delivery)
    }

    async fn type_text_targeted(&self, pid: i32, text: &str) -> Result<()> {
        let r: rpc_input::TypeTextResult = self
            .call_input(
                rpc_input::METHOD_TYPE_TEXT,
                rpc_input::TypeTextParams {
                    text: text.to_string(),
                    pid: Some(pid),
                },
            )
            .await?;
        ensure_targeted(rpc_input::METHOD_TYPE_TEXT, r.ok, &r.delivery)
    }

    async fn key_combo_targeted(&self, pid: i32, modifiers: &[String], key: &str) -> Result<()> {
        let r: rpc_input::KeyComboResult = self
            .call_input(
                rpc_input::METHOD_KEY_COMBO,
                rpc_input::KeyComboParams {
                    modifiers: modifiers.to_vec(),
                    key: key.to_string(),
                    pid: Some(pid),
                },
            )
            .await?;
        ensure_targeted(rpc_input::METHOD_KEY_COMBO, r.ok, &r.delivery)
    }

    async fn mouse_button_targeted(
        &self,
        pid: i32,
        x: f64,
        y: f64,
        button: MouseButton,
        action: PressAction,
    ) -> Result<()> {
        let r: rpc_input::MouseButtonResult = self
            .call_input(
                rpc_input::METHOD_MOUSE_BUTTON,
                rpc_input::MouseButtonParams {
                    x,
                    y,
                    button: to_rpc_button(button),
                    action: to_rpc_press(action),
                    pid: Some(pid),
                },
            )
            .await?;
        ensure_targeted(rpc_input::METHOD_MOUSE_BUTTON, r.ok, &r.delivery)
    }

    async fn screenshot_window(&self, window_id: u64, show_cursor: bool) -> Result<WindowShot> {
        use aleph_protocol::desktop_bridge::methods::screen::{
            CaptureParams, CaptureResult, METHOD_CAPTURE,
        };

        // No xcap fallback: NativeScreen cannot capture a specific window, and
        // silently widening the shot to the whole display would hand the model
        // pixels it would then mis-map back onto the window's coordinate space.
        let rpc: CaptureResult = self
            .bridge
            .call(
                METHOD_CAPTURE,
                CaptureParams {
                    display_id: None,
                    region: None,
                    window_id: Some(window_id),
                    show_cursor: Some(show_cursor),
                },
            )
            .await
            .map_err(|e| match e {
                DesktopError::BridgeFailed(m) => {
                    DesktopError::ScreenCapture(format!("bridge screen.capture (window): {m}"))
                }
                other => other,
            })?;

        Ok(WindowShot {
            image: Screenshot {
                image_base64: rpc.png_base64,
                width: rpc.width,
                height: rpc.height,
                format: "png".to_string(),
                scale_factor: rpc.scale,
            },
            // Region {x,y,width,height} → BoundingBox {x,y,w,h}. Both are the
            // window's frame in global screen POINTS, top-left origin.
            window_bounds: rpc.window_bounds.map(|b| BoundingBox {
                x: b.x,
                y: b.y,
                w: b.width,
                h: b.height,
            }),
        })
    }
}

// ── Bridge-backed helpers ────────────────────────────────────────────

impl MacOSScreen {
    /// Issue one `input.*` RPC. Every targeted-rail method funnels through
    /// here so the failure text names the method uniformly and no call site can
    /// forget to turn a transport error into an `InputFailed`.
    async fn call_input<P, R>(&self, method: &str, params: P) -> Result<R>
    where
        P: serde::Serialize + Send,
        R: serde::de::DeserializeOwned + Send,
    {
        self.bridge.call(method, params).await.map_err(|e| match e {
            // Keep typed recovery variants (permission / timeout / …); only an
            // opaque BridgeFailed becomes an InputFailed with rail context.
            DesktopError::BridgeFailed(m) => {
                DesktopError::InputFailed(format!("bridge {method}: {m}"))
            }
            other => other,
        })
    }

    /// SCK-backed screenshot via the Swift helper. Caller is expected to
    /// fall back to `NativeScreen` when this returns `Err` (e.g. on macOS
    /// 13 which lacks `SCScreenshotManager`).
    async fn screenshot_via_bridge(&self, region: Option<&ScreenRegion>) -> Result<Screenshot> {
        use aleph_protocol::desktop_bridge::methods::screen::{
            CaptureParams, CaptureResult, Region, METHOD_CAPTURE,
        };
        let params = CaptureParams {
            display_id: None,
            region: region.map(|r| Region {
                x: f64::from(r.x),
                y: f64::from(r.y),
                width: f64::from(r.width),
                height: f64::from(r.height),
            }),
            window_id: None,
            // Historical behavior of this rail was a cursor-in-frame capture;
            // the helper now defaults to omitting it, which is what a model
            // reading pixel coordinates wants (the cursor is not UI).
            show_cursor: None,
        };
        let rpc: CaptureResult =
            self.bridge
                .call(METHOD_CAPTURE, params)
                .await
                .map_err(|e| match e {
                    DesktopError::BridgeFailed(m) => {
                        DesktopError::ScreenCapture(format!("bridge screen.capture: {m}"))
                    }
                    other => other,
                })?;
        Ok(Screenshot {
            image_base64: rpc.png_base64,
            width: rpc.width,
            height: rpc.height,
            format: "png".to_string(),
            // The helper now reports the backing scale of the captured surface.
            // An older helper omits it and this stays `None` — callers already
            // resolve the scale via `display_list()` in that case.
            scale_factor: rpc.scale,
        })
    }

    /// SCK-backed display enumeration via the Swift helper.
    async fn display_list_via_bridge(&self) -> Result<Vec<DisplayInfo>> {
        use aleph_protocol::desktop_bridge::methods::screen::{
            ListDisplaysResult, METHOD_LIST_DISPLAYS,
        };
        let rpc: ListDisplaysResult = self
            .bridge
            .call(METHOD_LIST_DISPLAYS, serde_json::Value::Null)
            .await
            .map_err(|e| match e {
                DesktopError::BridgeFailed(m) => {
                    DesktopError::ScreenCapture(format!("bridge screen.list_displays: {m}"))
                }
                other => other,
            })?;
        Ok(rpc
            .displays
            .into_iter()
            .map(|d| DisplayInfo {
                id: d.id,
                name: String::new(),
                width: d.bounds.width as u32,
                height: d.bounds.height as u32,
                scale_factor: d.scale,
                is_primary: d.primary,
                origin_x: d.bounds.x as i32,
                origin_y: d.bounds.y as i32,
            })
            .collect())
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
    Some((f64::from(width), f64::from(height)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_png_dimensions_valid_1x1() {
        // Minimal valid 1x1 PNG (signature + IHDR chunk, no IDAT/IEND)
        let valid_png: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, // PNG signature
            0x00, 0x00, 0x00, 0x0d, // IHDR length = 13
            0x49, 0x48, 0x44, 0x52, // "IHDR"
            0x00, 0x00, 0x00, 0x01, // width = 1
            0x00, 0x00, 0x00, 0x01, // height = 1
            0x08, 0x02, // bit depth = 8, color type = 2 (RGB)
            0x00, 0x00, 0x00, // compression, filter, interlace
            0x90, 0x77, 0x53, 0xde, // CRC
        ];
        assert_eq!(png_dimensions(valid_png), Some((1.0, 1.0)));
    }

    #[test]
    fn test_png_dimensions_too_short() {
        let short: &[u8] = &[0x89, 0x50, 0x4e, 0x47]; // only 4 bytes
        assert_eq!(png_dimensions(short), None);
    }

    #[test]
    fn test_png_dimensions_wrong_signature() {
        let not_png: &[u8] = &[
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // wrong sig
            0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
            0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00,
        ];
        assert_eq!(png_dimensions(not_png), None);
    }

    #[test]
    fn targeted_delivery_is_accepted_only_when_the_helper_says_targeted() {
        assert!(ensure_targeted("input.click", true, rpc_input::DELIVERY_TARGETED).is_ok());
    }

    #[test]
    fn a_global_delivery_on_the_targeted_rail_is_an_error() {
        // The helper already moved the user's cursor. It cannot be undone, but
        // it must never be reported as a background action.
        let err = ensure_targeted("input.click", true, rpc_input::DELIVERY_GLOBAL)
            .expect_err("global delivery must not pass as targeted");
        let msg = err.to_string();
        assert!(msg.contains("targeted delivery dropped"), "{msg}");
    }

    #[test]
    fn a_failed_event_is_an_error_even_if_delivery_claims_targeted() {
        assert!(ensure_targeted("input.scroll", false, rpc_input::DELIVERY_TARGETED).is_err());
    }

    #[test]
    fn test_png_dimensions_200x150() {
        // PNG with width=200, height=150
        let mut png = vec![0u8; 24 + 12]; // header + some padding
        png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        png[16..20].copy_from_slice(&200u32.to_be_bytes());
        png[20..24].copy_from_slice(&150u32.to_be_bytes());
        assert_eq!(png_dimensions(&png), Some((200.0, 150.0)));
    }
}
