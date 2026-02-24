use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenRegion {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasPosition {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum DesktopRequest {
    #[serde(rename = "desktop.screenshot")]
    Screenshot { region: Option<ScreenRegion> },
    #[serde(rename = "desktop.ocr")]
    Ocr { image_base64: Option<String> },
    #[serde(rename = "desktop.ax_tree")]
    AxTree { app_bundle_id: Option<String> },
    #[serde(rename = "desktop.click")]
    Click { x: f64, y: f64, button: MouseButton },
    #[serde(rename = "desktop.type_text")]
    TypeText { text: String },
    #[serde(rename = "desktop.key_combo")]
    KeyCombo { keys: Vec<String> },
    #[serde(rename = "desktop.launch_app")]
    LaunchApp { bundle_id: String },
    #[serde(rename = "desktop.window_list")]
    WindowList {},
    #[serde(rename = "desktop.focus_window")]
    FocusWindow { window_id: u32 },
    #[serde(rename = "desktop.canvas_show")]
    CanvasShow { html: String, position: CanvasPosition },
    #[serde(rename = "desktop.canvas_hide")]
    CanvasHide {},
    #[serde(rename = "desktop.canvas_update")]
    CanvasUpdate { patch: serde_json::Value },
    #[serde(rename = "desktop.ping")]
    Ping {},
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DesktopResponse {
    Success { result: serde_json::Value },
    Error { error: DesktopRpcError },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopRpcError {
    pub code: i32,
    pub message: String,
}
