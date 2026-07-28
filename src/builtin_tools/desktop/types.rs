//! Types for the desktop tool — arguments and output structs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A rectangular region on screen (pixels).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScreenRegion {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Mouse button used by desktop input actions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Arguments for the desktop tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DesktopArgs {
    /// The desktop operation to perform. See the tool DESCRIPTION for the full
    /// per-action reference; the complete verb set is:
    ///
    /// Perception: "screenshot", "ocr", "screen_record", "wait_visual",
    /// "`display_list`". Pointer: "click", "`double_click`", "drag", "hover",
    /// "`cursor_position`", "`mouse_button`", "scroll". Keyboard/clipboard:
    /// "`type_text`", "`key_combo`", "`key_button`", "paste", "`clipboard_read`",
    /// "`clipboard_write`". Window/app: "`window_list`", "`focus_window`",
    /// "`move_window`", "`resize_window`", "`launch_app`", "`quit_app`",
    /// "`restart_app`". Semantic (macOS AX / Windows UIA / Linux AT-SPI):
    /// "`set_value`",
    /// "`ax_action`". Meta: "batch", "script".
    pub action: String,

    /// Screen region for screenshot {"x":0,"y":0,"width":1920,"height":1080}. Optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<ScreenRegion>,

    /// Base64 image for OCR. If absent, captures current screen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_base64: Option<String>,

    /// X coordinate for click (pixels).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,

    /// Y coordinate for click (pixels).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,

    /// Mouse button: "left", "right", "middle". Default: "left".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button: Option<MouseButton>,

    /// Text to type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// Key combination. Example: ["cmd","c"] for Copy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keys: Option<Vec<String>>,

    /// App bundle ID to launch or quit. Example: "com.apple.safari"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,

    /// Target application for this action — app name, executable, or bundle id
    /// (e.g. "Safari", "com.apple.safari"). Resolved against the running-app
    /// list into a live pid, which is what [`DesktopArgs::pid`] carries; pass
    /// whichever of the two you already have.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,

    /// Window ID (from `window_list` results). Focuses / moves / resizes that
    /// window; on `screenshot` it captures that window alone; on a pointer or
    /// keyboard action it identifies the target process when neither `pid` nor
    /// `app` was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<u32>,

    /// Start X for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_x: Option<f64>,

    /// Start Y for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_y: Option<f64>,

    /// End X for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_x: Option<f64>,

    /// End Y for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_y: Option<f64>,

    /// Horizontal scroll distance in pixels (negative=left). Quantized to whole
    /// wheel clicks (~100px each) at dispatch; a distance under one click still
    /// moves one click, and the result says so.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_x: Option<f64>,

    /// Vertical scroll distance in pixels (negative=up). Quantized to whole
    /// wheel clicks (~100px each) at dispatch; a distance under one click still
    /// moves one click, and the result says so.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_y: Option<f64>,

    /// Target width in pixels for `resize_window`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,

    /// Target height in pixels for `resize_window`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,

    /// Drag duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    /// Press action for `mouse_button`: "press", "release", "click".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub press_action: Option<String>,

    /// Recording duration in seconds (for `screen_record`, default: 5.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,

    /// Frames per second (for `screen_record`, default: 30).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,

    /// Include system audio (for `screen_record`, default: false).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub with_audio: Option<bool>,

    /// Target display ID for screenshot (from `display_list`). If absent, uses primary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_id: Option<u32>,

    /// Screenshot format: "png" (default) or "jpeg".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    /// JPEG quality 0.0-1.0 (only when format="jpeg", default 0.9).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<f64>,

    /// Max screenshot width in pixels (scale down if wider).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_width: Option<u32>,

    /// Max screenshot height in pixels (scale down if taller).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_height: Option<u32>,

    /// Timeout in milliseconds for `wait_visual` (default 5000, max 60000).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,

    /// Batch action list (only for action="batch").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<DesktopBatchAction>,

    // ── Coordinate space (UI-TARS parity) ─────────────────────────
    //
    // When omitted, all `x`/`y`/`start_x`/`start_y`/`end_x`/`end_y`/`region`
    // fields are taken as pixel coordinates (Aleph's historical default).
    // When `coord_space="normalized"`, those fields live in `[0, factor_w] ×
    // [0, factor_h]` (default factor 1000×1000, matching UI-TARS V1.0/V1.5)
    // and are rescaled to pixels against the primary display at dispatch.
    //
    // Pythonic action script — set when the model emits text in UI-TARS
    // format like `click(start_box='(500,500)')` instead of JSON.
    //
    /// Coordinate space: "pixel" (default), "normalized", or "window".
    ///
    /// "window" means the coordinates are pixels **of the window-cropped
    /// screenshot** identified by `window_id`; they are mapped back to the
    /// global point space (`global = window_bounds.origin + pixel / scale`)
    /// before dispatch. Without it, a pixel read off a window capture would be
    /// resolved against the whole display and land in the wrong place.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coord_space: Option<String>,

    /// Normalized factor `[factor_w, factor_h]`. Default `[1000, 1000]`.
    /// Ignored when `coord_space != "normalized"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coord_factors: Option<[u32; 2]>,

    /// UI-TARS-style action script, e.g.
    /// `Thought: ...\nAction: click(start_box='(500,500)')`. When present,
    /// the parser expands into a sequential batch. Other action fields are
    /// ignored; `action` may be set to "script" or left as any string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,

    /// Screenshot vision bridge (text-only-model support). When `true` on a
    /// `screenshot` action, the captured image is augmented with an OCR text
    /// layer (`ocr_text`) and, when a multimodal provider is wired, a scene
    /// `description` — so a model that cannot see the image can still act.
    /// Defaults to off, keeping the screenshot output byte-identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub describe: Option<bool>,

    /// AX role filter for `set_value` / `ax_action`, e.g. "AXTextField".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,

    /// Element title/label to match for `set_value` / `ax_action`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element_title: Option<String>,

    /// Native AX action name for `ax_action`, e.g. "AXPress".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ax_action_name: Option<String>,

    /// Target process for this action.
    ///
    /// For `set_value` / `ax_action` it scopes the element locator, as before.
    /// For every coordinate-space and keyboard action (click, `double_click`,
    /// drag, hover, scroll, `mouse_button`, `type_text`, `key_combo`, paste) it
    /// additionally selects the **delivery rail**: with a pid the event is
    /// posted straight into that process's event queue — the user's cursor never
    /// moves and the app need not be frontmost. Without one the event goes to
    /// the global input tap, which drags the user's physical cursor and is
    /// refused unless `[desktop] allow_global_pointer = true`.
    ///
    /// Omit for the frontmost app on `set_value` / `ax_action`. See also
    /// [`DesktopArgs::app`], which resolves a name to the same pid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,

    /// Post-action observation for mutating actions: "state" appends a
    /// lightweight `post_state` (frontmost app, focused element) to the
    /// result after a short settle delay; "screenshot" additionally captures
    /// a budget-bounded screenshot. Omit for the historical fire-and-forget
    /// behavior.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observe: Option<String>,

    /// Override the `type_text` focus pre-flight: type even when nothing holds
    /// keyboard focus, or when the focused element reports it takes no typed
    /// value. The escape hatch for apps whose accessibility tree lies (canvas,
    /// terminal, some Electron shells). It does **not** override the refusal to
    /// type into a secure/password field. Ignored by every other action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
}

/// A single sub-action inside a `batch` operation.
///
/// Mirrors every field of `DesktopArgs` except the recursive `actions`
/// field — nested batches are unrepresentable by construction.
///
/// This type exists so the JSON schema for `DesktopArgs.actions` is a clean
/// `{"type": "array", "items": <object>}` instead of a multi-type
/// `["array", "null"]` with a `serde_json::Value` (any-type) item — which
/// `OpenAI`'s tool schema validator rejects even in non-strict mode.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DesktopBatchAction {
    /// The desktop operation to perform. See the tool DESCRIPTION for the full
    /// per-action reference; the complete verb set is:
    ///
    /// Perception: "screenshot", "ocr", "screen_record", "wait_visual",
    /// "`display_list`". Pointer: "click", "`double_click`", "drag", "hover",
    /// "`cursor_position`", "`mouse_button`", "scroll". Keyboard/clipboard:
    /// "`type_text`", "`key_combo`", "`key_button`", "paste", "`clipboard_read`",
    /// "`clipboard_write`". Window/app: "`window_list`", "`focus_window`",
    /// "`move_window`", "`resize_window`", "`launch_app`", "`quit_app`",
    /// "`restart_app`". Semantic (macOS AX / Windows UIA / Linux AT-SPI):
    /// "`set_value`",
    /// "`ax_action`". Meta: "script".
    pub action: String,

    /// Screen region for screenshot {"x":0,"y":0,"width":1920,"height":1080}. Optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<ScreenRegion>,

    /// Base64 image for OCR. If absent, captures current screen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_base64: Option<String>,

    /// X coordinate for click (pixels).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,

    /// Y coordinate for click (pixels).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,

    /// Mouse button: "left", "right", "middle". Default: "left".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button: Option<MouseButton>,

    /// Text to type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// Key combination. Example: ["cmd","c"] for Copy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keys: Option<Vec<String>>,

    /// App bundle ID to launch or quit. Example: "com.apple.safari"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,

    /// Target application for this action — app name, executable, or bundle id
    /// (e.g. "Safari", "com.apple.safari"). Resolved against the running-app
    /// list into a live pid, which is what [`DesktopArgs::pid`] carries; pass
    /// whichever of the two you already have.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,

    /// Window ID (from `window_list` results). Focuses / moves / resizes that
    /// window; on `screenshot` it captures that window alone; on a pointer or
    /// keyboard action it identifies the target process when neither `pid` nor
    /// `app` was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<u32>,

    /// Start X for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_x: Option<f64>,

    /// Start Y for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_y: Option<f64>,

    /// End X for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_x: Option<f64>,

    /// End Y for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_y: Option<f64>,

    /// Horizontal scroll distance in pixels (negative=left). See
    /// [`DesktopArgs::delta_x`] for the wheel-click quantization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_x: Option<f64>,

    /// Vertical scroll distance in pixels (negative=up). See
    /// [`DesktopArgs::delta_y`] for the wheel-click quantization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_y: Option<f64>,

    /// Target width in pixels for `resize_window`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,

    /// Target height in pixels for `resize_window`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,

    /// Drag duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    /// Press action for `mouse_button`: "press", "release", "click".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub press_action: Option<String>,

    /// Recording duration in seconds (for `screen_record`, default: 5.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,

    /// Frames per second (for `screen_record`, default: 30).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,

    /// Include system audio (for `screen_record`, default: false).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub with_audio: Option<bool>,

    /// Target display ID for screenshot (from `display_list`). If absent, uses primary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_id: Option<u32>,

    /// Screenshot format: "png" (default) or "jpeg".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    /// JPEG quality 0.0-1.0 (only when format="jpeg", default 0.9).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<f64>,

    /// Max screenshot width in pixels (scale down if wider).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_width: Option<u32>,

    /// Max screenshot height in pixels (scale down if taller).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_height: Option<u32>,

    /// Timeout in milliseconds for `wait_visual` (default 5000, max 60000).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,

    /// Coordinate space for this sub-action ("pixel" / "normalized" /
    /// "window"; see [`DesktopArgs::coord_space`]). Falls back to the enclosing
    /// `DesktopArgs.coord_space` when omitted (set by the dispatcher before
    /// invocation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coord_space: Option<String>,

    /// Normalized factor `[factor_w, factor_h]`. Falls back to the enclosing
    /// batch's `coord_factors` when omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coord_factors: Option<[u32; 2]>,

    /// Augment a `screenshot` sub-action with OCR / description for text-only
    /// models. See [`DesktopArgs::describe`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub describe: Option<bool>,

    /// AX role filter for `set_value` / `ax_action`, e.g. "AXTextField".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,

    /// Element title/label to match for `set_value` / `ax_action`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element_title: Option<String>,

    /// Native AX action name for `ax_action`, e.g. "AXPress".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ax_action_name: Option<String>,

    /// Target process for this action.
    ///
    /// For `set_value` / `ax_action` it scopes the element locator, as before.
    /// For every coordinate-space and keyboard action (click, `double_click`,
    /// drag, hover, scroll, `mouse_button`, `type_text`, `key_combo`, paste) it
    /// additionally selects the **delivery rail**: with a pid the event is
    /// posted straight into that process's event queue — the user's cursor never
    /// moves and the app need not be frontmost. Without one the event goes to
    /// the global input tap, which drags the user's physical cursor and is
    /// refused unless `[desktop] allow_global_pointer = true`.
    ///
    /// Omit for the frontmost app on `set_value` / `ax_action`. See also
    /// [`DesktopArgs::app`], which resolves a name to the same pid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,

    /// Post-action observation for this sub-action. Falls back to the
    /// enclosing batch's `observe` when omitted (set by the dispatcher
    /// before invocation, mirroring `coord_space`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observe: Option<String>,

    /// Override the `type_text` focus pre-flight for this sub-action. See
    /// [`DesktopArgs::force`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
}

impl DesktopBatchAction {
    /// An action with every optional field unset — the one obvious way to
    /// build a sub-action programmatically.
    pub(super) fn empty(action: &str) -> Self {
        Self {
            action: action.to_string(),
            region: None,
            image_base64: None,
            x: None,
            y: None,
            button: None,
            text: None,
            keys: None,
            bundle_id: None,
            app: None,
            window_id: None,
            start_x: None,
            start_y: None,
            end_x: None,
            end_y: None,
            delta_x: None,
            delta_y: None,
            width: None,
            height: None,
            duration_ms: None,
            press_action: None,
            duration: None,
            fps: None,
            with_audio: None,
            display_id: None,
            format: None,
            quality: None,
            max_width: None,
            max_height: None,
            timeout_ms: None,
            coord_space: None,
            coord_factors: None,
            describe: None,
            role: None,
            element_title: None,
            ax_action_name: None,
            pid: None,
            observe: None,
            force: None,
        }
    }
}

impl From<&DesktopBatchAction> for DesktopArgs {
    fn from(b: &DesktopBatchAction) -> Self {
        Self {
            action: b.action.clone(),
            region: b.region.clone(),
            image_base64: b.image_base64.clone(),
            x: b.x,
            y: b.y,
            button: b.button.clone(),
            text: b.text.clone(),
            keys: b.keys.clone(),
            bundle_id: b.bundle_id.clone(),
            app: b.app.clone(),
            window_id: b.window_id,
            start_x: b.start_x,
            start_y: b.start_y,
            end_x: b.end_x,
            end_y: b.end_y,
            delta_x: b.delta_x,
            delta_y: b.delta_y,
            width: b.width,
            height: b.height,
            duration_ms: b.duration_ms,
            press_action: b.press_action.clone(),
            duration: b.duration,
            fps: b.fps,
            with_audio: b.with_audio,
            display_id: b.display_id,
            format: b.format.clone(),
            quality: b.quality,
            max_width: b.max_width,
            max_height: b.max_height,
            timeout_ms: b.timeout_ms,
            actions: Vec::new(),
            coord_space: b.coord_space.clone(),
            coord_factors: b.coord_factors,
            script: None,
            describe: b.describe,
            role: b.role.clone(),
            element_title: b.element_title.clone(),
            ax_action_name: b.ax_action_name.clone(),
            pid: b.pid,
            observe: b.observe.clone(),
            force: b.force,
        }
    }
}

/// Output from desktop operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
