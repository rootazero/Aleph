//! Unix Domain Socket client for the macOS App Desktop Bridge.
//!
//! Uses JSON-RPC 2.0 over a newline-delimited stream. Each request opens a
//! fresh connection, sends one JSON-RPC request, reads one response, and closes.

use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;
use tracing::debug;
use uuid::Uuid;

use super::error::DesktopError;
use super::types::{DesktopRequest, MouseButton};

/// Client that sends requests to the macOS App's UDS Desktop Bridge server.
#[derive(Clone)]
pub struct DesktopBridgeClient {
    socket_path: PathBuf,
}

impl DesktopBridgeClient {
    /// Create a new client pointing at `~/.aleph/desktop.sock`.
    pub fn new() -> Self {
        let socket_path = dirs::home_dir()
            .expect("cannot resolve home directory")
            .join(".aleph/desktop.sock");
        Self { socket_path }
    }

    /// Returns `true` if the socket file exists (App appears to be running).
    pub fn is_available(&self) -> bool {
        self.socket_path.exists()
    }

    /// Send a `DesktopRequest` and return the JSON `result` value on success.
    pub async fn send(
        &self,
        request: DesktopRequest,
    ) -> Result<serde_json::Value, DesktopError> {
        if !self.socket_path.exists() {
            return Err(DesktopError::AppNotRunning);
        }

        let stream = UnixStream::connect(&self.socket_path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::ConnectionRefused {
                DesktopError::AppNotRunning
            } else {
                DesktopError::ConnectionFailed(e)
            }
        })?;

        let id = Uuid::new_v4().to_string();
        let (method, params) = request_to_jsonrpc(&request);
        let envelope = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let mut wire = serde_json::to_string(&envelope)
            .map_err(|e| DesktopError::Protocol(e.to_string()))?;
        wire.push('\n');

        debug!("desktop → {}", wire.trim_end());

        let (reader, mut writer) = stream.into_split();
        writer
            .write_all(wire.as_bytes())
            .await
            .map_err(DesktopError::ConnectionFailed)?;

        let mut lines = BufReader::new(reader).lines();
        let line = timeout(Duration::from_secs(30), lines.next_line())
            .await
            .map_err(|_| DesktopError::Protocol("desktop bridge request timed out after 30s".into()))?
            .map_err(DesktopError::ConnectionFailed)?
            .ok_or_else(|| DesktopError::Protocol("connection closed without response".into()))?;

        debug!("desktop ← {}", line.trim_end());

        let response: serde_json::Value = serde_json::from_str(&line)
            .map_err(|e| DesktopError::Protocol(e.to_string()))?;

        if let Some(error) = response.get("error") {
            let msg = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(DesktopError::Operation(msg.to_string()));
        }

        Ok(response["result"].clone())
    }
}

impl Default for DesktopBridgeClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Converts a `DesktopRequest` to a `(method, params)` pair for JSON-RPC 2.0.
///
/// All params are `serde_json::Value::Object` so the Swift side always receives
/// `{"method": "...", "params": {...}}` — even for zero-argument methods
/// (where `params` is `{}`).
fn request_to_jsonrpc(request: &DesktopRequest) -> (&'static str, serde_json::Value) {
    use serde_json::json;
    match request {
        DesktopRequest::Ping => ("desktop.ping", json!({})),
        DesktopRequest::Screenshot { region } => {
            let region_val = region
                .as_ref()
                .map(|r| json!({"x": r.x, "y": r.y, "width": r.width, "height": r.height}))
                .unwrap_or(serde_json::Value::Null);
            ("desktop.screenshot", json!({ "region": region_val }))
        }
        DesktopRequest::Ocr { image_base64 } => {
            ("desktop.ocr", json!({ "image_base64": image_base64 }))
        }
        DesktopRequest::AxTree { app_bundle_id } => {
            ("desktop.ax_tree", json!({ "app_bundle_id": app_bundle_id }))
        }
        DesktopRequest::Click { x, y, button } => {
            let btn = match button {
                MouseButton::Left => "left",
                MouseButton::Right => "right",
                MouseButton::Middle => "middle",
            };
            ("desktop.click", json!({ "x": x, "y": y, "button": btn }))
        }
        DesktopRequest::TypeText { text } => ("desktop.type_text", json!({ "text": text })),
        DesktopRequest::KeyCombo { keys } => ("desktop.key_combo", json!({ "keys": keys })),
        DesktopRequest::LaunchApp { bundle_id } => {
            ("desktop.launch_app", json!({ "bundle_id": bundle_id }))
        }
        DesktopRequest::WindowList => ("desktop.window_list", json!({})),
        DesktopRequest::FocusWindow { window_id } => {
            ("desktop.focus_window", json!({ "window_id": window_id }))
        }
        DesktopRequest::CanvasShow { html, position } => (
            "desktop.canvas_show",
            json!({
                "html": html,
                "position": {
                    "x": position.x,
                    "y": position.y,
                    "width": position.width,
                    "height": position.height,
                }
            }),
        ),
        DesktopRequest::CanvasHide => ("desktop.canvas_hide", json!({})),
        DesktopRequest::CanvasUpdate { patch } => {
            ("desktop.canvas_update", json!({ "patch": patch }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::ScreenRegion;

    #[test]
    fn test_request_to_jsonrpc_ping() {
        let (method, params) = request_to_jsonrpc(&DesktopRequest::Ping);
        assert_eq!(method, "desktop.ping");
        assert_eq!(params, serde_json::json!({}));
    }

    #[test]
    fn test_request_to_jsonrpc_screenshot_no_region() {
        let (method, params) = request_to_jsonrpc(&DesktopRequest::Screenshot { region: None });
        assert_eq!(method, "desktop.screenshot");
        assert_eq!(params["region"], serde_json::Value::Null);
    }

    #[test]
    fn test_request_to_jsonrpc_screenshot_with_region() {
        let region = ScreenRegion { x: 10.0, y: 20.0, width: 100.0, height: 200.0 };
        let (method, params) =
            request_to_jsonrpc(&DesktopRequest::Screenshot { region: Some(region) });
        assert_eq!(method, "desktop.screenshot");
        assert_eq!(params["region"]["x"], 10.0);
        assert_eq!(params["region"]["width"], 100.0);
    }

    #[test]
    fn test_request_to_jsonrpc_click() {
        let (method, params) = request_to_jsonrpc(&DesktopRequest::Click {
            x: 100.0,
            y: 200.0,
            button: MouseButton::Left,
        });
        assert_eq!(method, "desktop.click");
        assert_eq!(params["button"], "left");
    }

    #[test]
    fn test_request_to_jsonrpc_window_list() {
        let (method, params) = request_to_jsonrpc(&DesktopRequest::WindowList);
        assert_eq!(method, "desktop.window_list");
        assert_eq!(params, serde_json::json!({}));
    }

    #[tokio::test]
    #[ignore = "requires macOS App running"]
    async fn test_ping() {
        let client = DesktopBridgeClient::new();
        if !client.is_available() {
            eprintln!("Skipping: macOS App not running");
            return;
        }
        let result = client.send(DesktopRequest::Ping).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), serde_json::json!("pong"));
    }
}
