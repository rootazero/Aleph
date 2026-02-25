//! Unix Domain Socket client for the Desktop Bridge.
//!
//! Uses JSON-RPC 2.0 over a newline-delimited stream. Each request opens a
//! fresh connection, sends one JSON-RPC request, reads one response, and closes.
//!
//! ## Socket path resolution
//!
//! The bridge can run in two modes, each using a different socket path:
//!
//! 1. **Managed mode** — `~/.aleph/run/desktop-bridge.sock` (bridge launched by `BridgeSupervisor`)
//! 2. **Standalone mode** — `~/.aleph/bridge.sock` (bridge launched manually by the user)
//!
//! `DesktopBridgeClient::new()` probes both paths, preferring managed mode.

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
    /// Create a new client.
    ///
    /// Probes two socket paths in order:
    /// 1. `~/.aleph/run/desktop-bridge.sock` (managed by BridgeSupervisor)
    /// 2. `~/.aleph/bridge.sock` (standalone bridge)
    pub fn new() -> Self {
        let home = dirs::home_dir().expect("cannot resolve home directory");
        let managed = home.join(".aleph").join("run").join("desktop-bridge.sock");
        let standalone = home.join(".aleph").join("bridge.sock");

        // Prefer managed socket (from BridgeSupervisor) when it exists
        let socket_path = if managed.exists() { managed } else { standalone };
        Self { socket_path }
    }

    /// Create a client pointing at a specific socket path (for testing).
    pub fn with_path(socket_path: PathBuf) -> Self {
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
        DesktopRequest::Click { ref_id, x, y, button } => {
            let btn = match button {
                MouseButton::Left => "left",
                MouseButton::Right => "right",
                MouseButton::Middle => "middle",
            };
            ("desktop.click", json!({ "ref": ref_id, "x": x, "y": y, "button": btn }))
        }
        DesktopRequest::TypeText { ref_id, text } => {
            ("desktop.type_text", json!({ "ref": ref_id, "text": text }))
        }
        DesktopRequest::KeyCombo { keys } => ("desktop.key_combo", json!({ "keys": keys })),
        DesktopRequest::LaunchApp { bundle_id } => {
            ("desktop.launch_app", json!({ "bundle_id": bundle_id }))
        }
        DesktopRequest::WindowList => ("desktop.window_list", json!({})),
        DesktopRequest::FocusWindow { window_id } => {
            ("desktop.focus_window", json!({ "window_id": window_id }))
        }
        DesktopRequest::Snapshot { app_bundle_id, max_depth, include_non_interactive } => {
            ("desktop.snapshot", json!({
                "app_bundle_id": app_bundle_id,
                "max_depth": max_depth,
                "include_non_interactive": include_non_interactive,
            }))
        }
        DesktopRequest::Scroll { ref_id, x, y, delta_x, delta_y } => {
            ("desktop.scroll", json!({
                "ref": ref_id, "x": x, "y": y,
                "delta_x": delta_x, "delta_y": delta_y,
            }))
        }
        DesktopRequest::DoubleClick { ref_id, x, y, button } => {
            let btn = match button {
                MouseButton::Left => "left",
                MouseButton::Right => "right",
                MouseButton::Middle => "middle",
            };
            ("desktop.double_click", json!({ "ref": ref_id, "x": x, "y": y, "button": btn }))
        }
        DesktopRequest::Drag { start_ref, start_x, start_y, end_ref, end_x, end_y, duration_ms } => {
            ("desktop.drag", json!({
                "start_ref": start_ref, "start_x": start_x, "start_y": start_y,
                "end_ref": end_ref, "end_x": end_x, "end_y": end_y,
                "duration_ms": duration_ms,
            }))
        }
        DesktopRequest::Hover { ref_id, x, y } => {
            ("desktop.hover", json!({ "ref": ref_id, "x": x, "y": y }))
        }
        DesktopRequest::Paste { text } => {
            ("desktop.paste", json!({ "text": text }))
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
            ref_id: None,
            x: Some(100.0),
            y: Some(200.0),
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

    #[test]
    fn test_request_to_jsonrpc_snapshot() {
        let (method, params) = request_to_jsonrpc(&DesktopRequest::Snapshot {
            app_bundle_id: None,
            max_depth: Some(5),
            include_non_interactive: Some(false),
        });
        assert_eq!(method, "desktop.snapshot");
        assert_eq!(params["max_depth"], 5);
    }

    #[test]
    fn test_request_to_jsonrpc_scroll() {
        let (method, params) = request_to_jsonrpc(&DesktopRequest::Scroll {
            ref_id: Some("e3".into()),
            x: None, y: None,
            delta_x: 0.0, delta_y: -200.0,
        });
        assert_eq!(method, "desktop.scroll");
        assert_eq!(params["ref"], "e3");
        assert_eq!(params["delta_y"], -200.0);
    }

    #[test]
    fn test_request_to_jsonrpc_double_click() {
        let (method, params) = request_to_jsonrpc(&DesktopRequest::DoubleClick {
            ref_id: Some("e1".into()),
            x: None, y: None,
            button: MouseButton::Left,
        });
        assert_eq!(method, "desktop.double_click");
        assert_eq!(params["ref"], "e1");
    }

    #[test]
    fn test_request_to_jsonrpc_drag() {
        let (method, params) = request_to_jsonrpc(&DesktopRequest::Drag {
            start_ref: Some("e1".into()), start_x: None, start_y: None,
            end_ref: Some("e5".into()), end_x: None, end_y: None,
            duration_ms: Some(500),
        });
        assert_eq!(method, "desktop.drag");
        assert_eq!(params["start_ref"], "e1");
        assert_eq!(params["end_ref"], "e5");
    }

    #[test]
    fn test_request_to_jsonrpc_hover() {
        let (method, params) = request_to_jsonrpc(&DesktopRequest::Hover {
            ref_id: None,
            x: Some(300.0), y: Some(400.0),
        });
        assert_eq!(method, "desktop.hover");
        assert_eq!(params["x"], 300.0);
    }

    #[test]
    fn test_request_to_jsonrpc_paste() {
        let (method, params) = request_to_jsonrpc(&DesktopRequest::Paste {
            text: "hello".into(),
        });
        assert_eq!(method, "desktop.paste");
        assert_eq!(params["text"], "hello");
    }

    #[test]
    fn test_request_to_jsonrpc_click_with_ref() {
        let (method, params) = request_to_jsonrpc(&DesktopRequest::Click {
            ref_id: Some("e7".into()),
            x: None, y: None,
            button: MouseButton::Left,
        });
        assert_eq!(method, "desktop.click");
        assert_eq!(params["ref"], "e7");
        assert!(params["x"].is_null());
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
