//! Desktop Bridge protocol types
//!
//! Shared type definitions for the Desktop Bridge JSON-RPC 2.0 protocol.
//! Used by:
//! - Tauri Bridge (UDS server, direct Rust import)
//! - Core (UDS client, direct Rust import)
//! - Swift Bridge (manual alignment)

use serde::{Deserialize, Serialize};

// ============================================================================
// JSON-RPC 2.0 Types
// ============================================================================

/// JSON-RPC 2.0 request for Desktop Bridge
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeRequest {
    pub jsonrpc: String,
    pub id: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 success response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeSuccessResponse {
    pub jsonrpc: String,
    pub id: String,
    pub result: serde_json::Value,
}

/// JSON-RPC 2.0 error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeErrorResponse {
    pub jsonrpc: String,
    pub id: String,
    pub error: BridgeRpcError,
}

/// JSON-RPC 2.0 error object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeRpcError {
    pub code: i32,
    pub message: String,
}

// ============================================================================
// Shared Value Types
// ============================================================================

/// Screen region for screenshot/OCR
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenRegion {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Canvas overlay position
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasPosition {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

// ============================================================================
// Method Constants
// ============================================================================

pub const METHOD_PING: &str = "desktop.ping";
pub const METHOD_SCREENSHOT: &str = "desktop.screenshot";
pub const METHOD_OCR: &str = "desktop.ocr";
pub const METHOD_AX_TREE: &str = "desktop.ax_tree";
pub const METHOD_CLICK: &str = "desktop.click";
pub const METHOD_TYPE_TEXT: &str = "desktop.type_text";
pub const METHOD_KEY_COMBO: &str = "desktop.key_combo";
pub const METHOD_LAUNCH_APP: &str = "desktop.launch_app";
pub const METHOD_WINDOW_LIST: &str = "desktop.window_list";
pub const METHOD_FOCUS_WINDOW: &str = "desktop.focus_window";
pub const METHOD_CANVAS_SHOW: &str = "desktop.canvas_show";
pub const METHOD_CANVAS_HIDE: &str = "desktop.canvas_hide";
pub const METHOD_CANVAS_UPDATE: &str = "desktop.canvas_update";

// ============================================================================
// Error Codes
// ============================================================================

pub const ERR_PARSE: i32 = -32700;
pub const ERR_METHOD_NOT_FOUND: i32 = -32601;
pub const ERR_INTERNAL: i32 = -32603;
pub const ERR_NOT_IMPLEMENTED: i32 = -32000;

// ============================================================================
// Socket Path
// ============================================================================

/// Get the default Desktop Bridge socket path (~/.aleph/desktop.sock)
pub fn default_socket_path() -> std::path::PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    home.join(".aleph").join("desktop.sock")
}
