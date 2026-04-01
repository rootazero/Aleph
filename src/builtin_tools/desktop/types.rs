//! Types for the desktop tool — arguments and output structs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::desktop::types::{CanvasPosition, MouseButton, ScreenRegion};

/// Arguments for the desktop tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DesktopArgs {
    /// The desktop operation to perform.
    ///
    /// Perception: "screenshot", "ocr", "ax_tree"
    /// Action:     "click", "type_text", "key_combo", "launch_app", "window_list", "focus_window"
    /// Canvas:     "canvas_show", "canvas_hide", "canvas_update"
    pub action: String,

    /// Screen region for screenshot {"x":0,"y":0,"width":1920,"height":1080}. Optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<ScreenRegion>,

    /// Base64 image for OCR. If absent, captures current screen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_base64: Option<String>,

    /// App bundle ID for ax_tree. Example: "com.apple.Safari"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_bundle_id: Option<String>,

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

    /// App bundle ID to launch. Example: "com.apple.safari"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,

    /// Window ID to focus (from window_list results).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<u32>,

    /// HTML content for canvas_show.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,

    /// Canvas overlay position {"x":100,"y":100,"width":800,"height":600}.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<CanvasPosition>,

    /// A2UI patch for canvas_update (JSON array of patch operations).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<Value>,

    /// Element ref from snapshot (alternative to x/y coordinates).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ref")]
    pub ref_id: Option<String>,

    /// Start element ref for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_ref: Option<String>,

    /// Start X for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_x: Option<f64>,

    /// Start Y for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_y: Option<f64>,

    /// End element ref for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_ref: Option<String>,

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

    /// Max AX tree depth for snapshot (default: 5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,

    /// Include non-interactive elements in snapshot refs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_non_interactive: Option<bool>,

    /// Recording duration in seconds (for screen_record, default: 5.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,

    /// Frames per second (for screen_record, default: 30).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,

    /// Include system audio (for screen_record, default: false).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub with_audio: Option<bool>,
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
