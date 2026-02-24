use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A rectangular region on screen (pixels).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScreenRegion {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Position and size of a canvas overlay window (pixels).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CanvasPosition {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<ScreenRegion> for CanvasPosition {
    fn from(r: ScreenRegion) -> Self {
        CanvasPosition { x: r.x, y: r.y, width: r.width, height: r.height }
    }
}

impl From<CanvasPosition> for ScreenRegion {
    fn from(p: CanvasPosition) -> Self {
        ScreenRegion { x: p.x, y: p.y, width: p.width, height: p.height }
    }
}

/// Desktop Bridge request variants.
///
/// Wire serialization is performed manually in `client::request_to_jsonrpc()`.
/// These types are NOT serialized via serde directly — they exist for type-safe
/// request construction on the Rust side.
#[derive(Debug, Clone)]
pub enum DesktopRequest {
    // Perception
    Screenshot { region: Option<ScreenRegion> },
    Ocr { image_base64: Option<String> },
    AxTree { app_bundle_id: Option<String> },
    // Action
    Click { x: f64, y: f64, button: MouseButton },
    TypeText { text: String },
    KeyCombo { keys: Vec<String> },
    LaunchApp { bundle_id: String },
    WindowList,
    FocusWindow { window_id: u32 },
    // Canvas
    CanvasShow { html: String, position: CanvasPosition },
    CanvasHide,
    CanvasUpdate { patch: serde_json::Value },
    // Internal
    Ping,
}

/// Desktop Bridge response (parsed manually in client, not via serde).
#[derive(Debug, Clone)]
pub enum DesktopResponse {
    Success(serde_json::Value),
    Error { code: i32, message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopRpcError {
    pub code: i32,
    pub message: String,
}
