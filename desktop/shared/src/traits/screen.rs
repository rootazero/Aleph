//! Screen perception and input automation capability.

use async_trait::async_trait;

use crate::screen_types::{ScreenRecordConfig, ScreenRecordResult};
use crate::{
    DisplayInfo, MouseButton, OcrResult, PressAction, Result, ScreenRegion, Screenshot, WindowInfo,
    WindowShot,
};

/// Screen perception and input automation.
///
/// # Two input rails
///
/// The plain input methods (`click`, `type_text`, …) post to the platform's
/// **global** HID event tap. That physically drags the user's cursor across the
/// screen and only lands correctly when the target app is already frontmost.
///
/// The `*_targeted` methods deliver the same event straight into a **single
/// process's** event queue, addressed by pid. The user's real cursor never
/// moves and the app need not be frontmost — which is the only way a
/// coordinate-space action can run without disturbing the person at the machine
/// (R5).
///
/// The catch is that "background delivery" is not uniform across event kinds.
/// Keyboard events route to a process's focused element without a location, so
/// `type_text_targeted` / `key_combo_targeted` land, and a click that resolves to
/// an AX press (`ax.perform_action`) is pid-scoped and cursor-free by
/// construction. But a synthesized *mouse* event carries a screen point that the
/// window server must bind to a window, and posting it to a pid bypasses exactly
/// that — verified on macOS 27, where such an event reaches the process but is
/// never acted on. So a coordinate mouse act with no AX rung (a scroll, a drag, a
/// click on a plain view) has no background delivery at all on macOS, and those
/// `*_targeted` methods surface an error rather than pretend. The bridge refuses
/// it; whether to retry on the visible global rail is the caller's policy.
///
/// Every `*_targeted` method defaults to `NotImplemented`, so a platform that
/// cannot address a single process (currently Windows and Linux) inherits the
/// default and its behavior stays byte-identical. Callers must ask
/// [`supports_targeted_input`](ScreenCapability::supports_targeted_input) — or
/// handle `NotImplemented` (platform has no targeted rail) and the
/// coordinate-mouse-undeliverable error (platform has one, but not for this
/// event) — and fall back to the global rail themselves; the trait never
/// silently downgrades, because the caller has to be able to tell the model which
/// rail actually ran.
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

    /// Move a window so its top-left corner sits at `(x, y)` in global
    /// screen coordinates. `window_id` is the id returned by [`window_list`].
    ///
    /// Default stub returns [`DesktopError::NotImplemented`]; platform
    /// implementations override it.
    async fn move_window(&self, window_id: u64, x: i32, y: i32) -> Result<()> {
        let _ = (window_id, x, y);
        Err(crate::DesktopError::NotImplemented("move_window".into()))
    }

    /// Resize a window to `width` × `height` pixels. `window_id` is the id
    /// returned by [`window_list`].
    ///
    /// Default stub returns [`DesktopError::NotImplemented`]; platform
    /// implementations override it.
    async fn resize_window(&self, window_id: u64, width: u32, height: u32) -> Result<()> {
        let _ = (window_id, width, height);
        Err(crate::DesktopError::NotImplemented("resize_window".into()))
    }

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

    /// Drag from (`start_x`, `start_y`) to (`end_x`, `end_y`).
    /// `duration_ms` controls drag animation; None for instant.
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
        Err(crate::DesktopError::NotImplemented(
            "cursor_position".into(),
        ))
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

    /// Press or release one or more keys without the paired counterpart, so a
    /// key (or chord) can be held down across actions and released later
    /// (UI-TARS `press` / `release`). `Click` performs a full press-and-release.
    async fn key_button(&self, keys: &[String], action: PressAction) -> Result<()> {
        let _ = (keys, action);
        Err(crate::DesktopError::NotImplemented("key_button".into()))
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
        Err(crate::DesktopError::NotImplemented(
            "clipboard_write".into(),
        ))
    }

    /// List all connected displays.
    async fn display_list(&self) -> Result<Vec<DisplayInfo>> {
        Err(crate::DesktopError::NotImplemented("display_list".into()))
    }

    // ── Targeted input rail (pid-scoped, cursor-free) ────────────
    //
    // Every method below is addressed to one process and must NOT touch the
    // user's cursor or the frontmost-app state. A platform that cannot honour
    // that contract must keep the `NotImplemented` default rather than quietly
    // fall back to the global tap: a caller that asked for "don't disturb the
    // user" has to learn that it could not be done, not be told it was.

    /// Whether this platform can deliver input into a single process's event
    /// queue without moving the user's cursor.
    ///
    /// Static capability of the platform — not a judgement about any particular
    /// request.
    fn supports_targeted_input(&self) -> bool {
        false
    }

    /// Click at global point (`x`, `y`), delivered to `pid` only.
    async fn click_targeted(&self, pid: i32, x: f64, y: f64, button: MouseButton) -> Result<()> {
        let _ = (pid, x, y, button);
        Err(crate::DesktopError::NotImplemented("click_targeted".into()))
    }

    /// Double-click at global point (`x`, `y`), delivered to `pid` only.
    ///
    /// Must carry a native click count of 2 on the event itself. Two
    /// independent single clicks are not a double-click: the app reads the
    /// count off the event.
    async fn double_click_targeted(
        &self,
        pid: i32,
        x: f64,
        y: f64,
        button: MouseButton,
    ) -> Result<()> {
        let _ = (pid, x, y, button);
        Err(crate::DesktopError::NotImplemented(
            "double_click_targeted".into(),
        ))
    }

    /// Drag from (`start_x`, `start_y`) to (`end_x`, `end_y`) in global points,
    /// delivered to `pid` only. `duration_ms` controls the drag animation;
    /// `None` for instant.
    async fn drag_targeted(
        &self,
        pid: i32,
        start_x: f64,
        start_y: f64,
        end_x: f64,
        end_y: f64,
        duration_ms: Option<u64>,
    ) -> Result<()> {
        let _ = (pid, start_x, start_y, end_x, end_y, duration_ms);
        Err(crate::DesktopError::NotImplemented("drag_targeted".into()))
    }

    /// Deliver a mouse-moved event at global point (`x`, `y`) to `pid` only, so
    /// the app updates its hover state. The user's cursor stays where it is.
    async fn hover_targeted(&self, pid: i32, x: f64, y: f64) -> Result<()> {
        let _ = (pid, x, y);
        Err(crate::DesktopError::NotImplemented("hover_targeted".into()))
    }

    /// Scroll at global point (`x`, `y`), delivered to `pid` only.
    ///
    /// The point is mandatory on this rail: the cursor never moves, so the event
    /// carries the only location the app can route the scroll by.
    async fn scroll_targeted(
        &self,
        pid: i32,
        x: f64,
        y: f64,
        direction: &str,
        amount: i32,
    ) -> Result<()> {
        let _ = (pid, x, y, direction, amount);
        Err(crate::DesktopError::NotImplemented(
            "scroll_targeted".into(),
        ))
    }

    /// Type `text` into `pid`'s focused element without stealing focus.
    async fn type_text_targeted(&self, pid: i32, text: &str) -> Result<()> {
        let _ = (pid, text);
        Err(crate::DesktopError::NotImplemented(
            "type_text_targeted".into(),
        ))
    }

    /// Send a modifier + key chord to `pid` without stealing focus.
    async fn key_combo_targeted(&self, pid: i32, modifiers: &[String], key: &str) -> Result<()> {
        let _ = (pid, modifiers, key);
        Err(crate::DesktopError::NotImplemented(
            "key_combo_targeted".into(),
        ))
    }

    /// Hold or release a key/chord in `pid` only, without stealing focus.
    ///
    /// The held-input counterpart of [`key_combo_targeted`]. It matters more
    /// than the atomic chord does: a press outlives the call, so the release
    /// that pairs with it happens in some *later* call — and it must ride the
    /// same rail. Releasing a targeted press on the global tap leaves the target
    /// process's key permanently down while banging a stray release into
    /// whatever the user has in front of them.
    ///
    /// [`key_combo_targeted`]: ScreenCapability::key_combo_targeted
    async fn key_button_targeted(
        &self,
        pid: i32,
        keys: &[String],
        action: PressAction,
    ) -> Result<()> {
        let _ = (pid, keys, action);
        Err(crate::DesktopError::NotImplemented(
            "key_button_targeted".into(),
        ))
    }

    /// Press/release a mouse button independently at global point (`x`, `y`),
    /// delivered to `pid` only.
    async fn mouse_button_targeted(
        &self,
        pid: i32,
        x: f64,
        y: f64,
        button: MouseButton,
        action: PressAction,
    ) -> Result<()> {
        let _ = (pid, x, y, button, action);
        Err(crate::DesktopError::NotImplemented(
            "mouse_button_targeted".into(),
        ))
    }

    /// Capture a single window (`window_id` from [`window_list`]), cropped to
    /// its frame, without raising it or moving the user's focus.
    ///
    /// The returned [`WindowShot`] carries the window's global-point origin and
    /// the backing scale alongside the pixels, because a crop whose pixels
    /// cannot be mapped back to global click coordinates is a targeting
    /// regression, not a feature.
    ///
    /// [`window_list`]: ScreenCapability::window_list
    async fn screenshot_window(&self, window_id: u64, show_cursor: bool) -> Result<WindowShot> {
        let _ = (window_id, show_cursor);
        Err(crate::DesktopError::NotImplemented(
            "screenshot_window".into(),
        ))
    }
}
