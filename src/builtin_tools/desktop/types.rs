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
    /// The desktop operation to perform.
    ///
    /// Supported actions: "screenshot", "ocr", "click", "double_click", "drag",
    /// "hover", "cursor_position", "mouse_button", "type_text", "key_combo",
    /// "scroll", "launch_app", "quit_app", "window_list", "focus_window",
    /// "clipboard_read", "clipboard_write", "screen_record".
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

    /// Window ID to focus (from window_list results).
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

    /// Horizontal scroll amount in pixels (negative=left).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_x: Option<f64>,

    /// Vertical scroll amount in pixels (negative=up).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_y: Option<f64>,

    /// Drag duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    /// Press action for mouse_button: "press", "release", "click".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub press_action: Option<String>,

    /// Recording duration in seconds (for screen_record, default: 5.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,

    /// Frames per second (for screen_record, default: 30).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,

    /// Include system audio (for screen_record, default: false).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub with_audio: Option<bool>,

    /// Target display ID for screenshot (from display_list). If absent, uses primary.
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

    /// Batch action list (only for action="batch").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<DesktopBatchAction>,
}

/// A single sub-action inside a `batch` operation.
///
/// Mirrors every field of `DesktopArgs` except the recursive `actions`
/// field — nested batches are unrepresentable by construction.
///
/// This type exists so the JSON schema for `DesktopArgs.actions` is a clean
/// `{"type": "array", "items": <object>}` instead of a multi-type
/// `["array", "null"]` with a `serde_json::Value` (any-type) item — which
/// OpenAI's tool schema validator rejects even in non-strict mode.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DesktopBatchAction {
    /// The desktop operation to perform.
    ///
    /// Supported actions: "screenshot", "ocr", "click", "double_click", "drag",
    /// "hover", "cursor_position", "mouse_button", "type_text", "key_combo",
    /// "scroll", "launch_app", "quit_app", "window_list", "focus_window",
    /// "clipboard_read", "clipboard_write", "screen_record".
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

    /// Window ID to focus (from window_list results).
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

    /// Horizontal scroll amount in pixels (negative=left).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_x: Option<f64>,

    /// Vertical scroll amount in pixels (negative=up).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_y: Option<f64>,

    /// Drag duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    /// Press action for mouse_button: "press", "release", "click".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub press_action: Option<String>,

    /// Recording duration in seconds (for screen_record, default: 5.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,

    /// Frames per second (for screen_record, default: 30).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,

    /// Include system audio (for screen_record, default: false).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub with_audio: Option<bool>,

    /// Target display ID for screenshot (from display_list). If absent, uses primary.
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
            window_id: b.window_id,
            start_x: b.start_x,
            start_y: b.start_y,
            end_x: b.end_x,
            end_y: b.end_y,
            delta_x: b.delta_x,
            delta_y: b.delta_y,
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
            actions: Vec::new(),
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
