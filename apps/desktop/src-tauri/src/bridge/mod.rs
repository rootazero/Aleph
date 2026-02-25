//! Desktop Bridge — UDS JSON-RPC 2.0 server
//!
//! Symmetric with macOS Swift DesktopBridgeServer.
//! Listens on ~/.aleph/bridge.sock, dispatches JSON-RPC requests
//! to perception/action handlers.

mod action;
mod canvas;
mod perception;
pub mod protocol;

use aleph_protocol::desktop_bridge::{
    self, ERR_INTERNAL, ERR_METHOD_NOT_FOUND, ERR_NOT_IMPLEMENTED, METHOD_BRIDGE_SHUTDOWN,
    METHOD_HANDSHAKE, METHOD_SYSTEM_PING, METHOD_WEBVIEW_HIDE, METHOD_WEBVIEW_NAVIGATE,
    METHOD_WEBVIEW_SHOW,
};
use serde_json::json;
use tauri::Manager;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tracing::{error, info, warn};

/// Start the Desktop Bridge UDS server
///
/// Listens on the configured socket path and dispatches JSON-RPC 2.0 requests.
/// In bridge mode, `ALEPH_SOCKET_PATH` overrides the default `~/.aleph/bridge.sock`.
/// This function runs forever; call it from a spawned task.
pub async fn start_bridge_server() {
    // Allow server-provided socket path override (set in bridge-mode startup)
    let socket_path = match std::env::var("ALEPH_SOCKET_PATH") {
        Ok(path) if !path.is_empty() => {
            info!("Using ALEPH_SOCKET_PATH override: {}", path);
            std::path::PathBuf::from(path)
        }
        _ => desktop_bridge::default_socket_path(),
    };

    // Ensure ~/.aleph/ directory exists
    if let Some(parent) = socket_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            error!("Failed to create directory {:?}: {}", parent, e);
            return;
        }
    }

    // Remove stale socket file
    let _ = tokio::fs::remove_file(&socket_path).await;

    // Bind listener
    let listener = match UnixListener::bind(&socket_path) {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind UDS {:?}: {}", socket_path, e);
            return;
        }
    };

    // Restrict socket file to owner-only access
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        if let Err(e) = std::fs::set_permissions(&socket_path, perms) {
            warn!("Failed to set socket permissions: {}", e);
        }
    }

    info!("DesktopBridge listening on {:?}", socket_path);

    // Accept loop
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(async move {
                    handle_connection(stream).await;
                });
            }
            Err(e) => {
                error!("Failed to accept connection: {}", e);
            }
        }
    }
}

/// Handle a single connection: read one line, dispatch, write response
async fn handle_connection(stream: tokio::net::UnixStream) {
    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    match buf_reader.read_line(&mut line).await {
        Ok(0) => return,
        Err(e) => {
            tracing::debug!("Failed to read from client: {}", e);
            return;
        }
        Ok(_) => {}
    }

    let line = line.trim_end();
    if line.is_empty() {
        return;
    }

    let response = match protocol::parse_request(line) {
        Ok(req) => {
            let result = dispatch(&req.method, req.params.unwrap_or(json!({})));
            match result {
                Ok(value) => protocol::success_response(&req.id, value),
                Err((code, msg)) => protocol::error_response(&req.id, code, &msg),
            }
        }
        Err(err_resp) => serde_json::to_string(&err_resp).unwrap_or_default(),
    };

    let response_line = format!("{}\n", response);
    if let Err(e) = writer.write_all(response_line.as_bytes()).await {
        tracing::debug!("Failed to write response to client: {}", e);
    }
}

/// Dispatch a method call to the appropriate handler
fn dispatch(method: &str, params: serde_json::Value) -> Result<serde_json::Value, (i32, String)> {
    match method {
        // Server ↔ Bridge handshake / health
        METHOD_HANDSHAKE => handle_handshake(params),
        METHOD_SYSTEM_PING => Ok(json!({"pong": true})),

        desktop_bridge::METHOD_PING => Ok(json!("pong")),

        desktop_bridge::METHOD_SCREENSHOT => perception::handle_screenshot(params),

        // WebView control
        METHOD_WEBVIEW_SHOW => handle_webview_show(params),
        METHOD_WEBVIEW_HIDE => handle_webview_hide(params),
        METHOD_WEBVIEW_NAVIGATE => handle_webview_navigate(params),

        // Bridge lifecycle
        METHOD_BRIDGE_SHUTDOWN => {
            info!("Bridge shutdown requested");
            // Trigger Tauri's graceful exit flow instead of hard process::exit
            if let Some(app) = crate::get_app_handle() {
                app.exit(0);
            }
            Ok(json!({"shutdown": true}))
        }

        // Action handlers — mouse, keyboard, app launch
        desktop_bridge::METHOD_CLICK => action::handle_click(params),
        desktop_bridge::METHOD_TYPE_TEXT => action::handle_type_text(params),
        desktop_bridge::METHOD_KEY_COMBO => action::handle_key_combo(params),
        desktop_bridge::METHOD_LAUNCH_APP => action::handle_launch_app(params),

        // Window management
        desktop_bridge::METHOD_WINDOW_LIST => action::handle_window_list(params),
        desktop_bridge::METHOD_FOCUS_WINDOW => action::handle_focus_window(params),

        // Canvas overlay
        desktop_bridge::METHOD_CANVAS_SHOW => canvas::handle_canvas_show(params),
        desktop_bridge::METHOD_CANVAS_HIDE => canvas::handle_canvas_hide(params),
        desktop_bridge::METHOD_CANVAS_UPDATE => canvas::handle_canvas_update(params),

        // Tray control
        desktop_bridge::METHOD_TRAY_UPDATE_STATUS => handle_tray_update_status(params),

        // Perception — OCR
        desktop_bridge::METHOD_OCR => perception::handle_ocr(params),

        // Remaining unimplemented methods
        desktop_bridge::METHOD_AX_TREE => Err((
            ERR_NOT_IMPLEMENTED,
            format!("{} not implemented on this platform", method),
        )),

        _ => Err((
            ERR_METHOD_NOT_FOUND,
            format!("Method not found: {}", method),
        )),
    }
}

// ── Handshake handler ────────────────────────────────────────────

/// Handle `aleph.handshake` — respond with bridge capabilities so the
/// server knows what operations this bridge supports.
fn handle_handshake(params: serde_json::Value) -> Result<serde_json::Value, (i32, String)> {
    let protocol_version = params
        .get("protocol_version")
        .and_then(|v| v.as_str())
        .unwrap_or("1.0");

    tracing::info!(
        protocol_version,
        "Handshake received from server"
    );

    // Return capability registration
    Ok(json!({
        "protocol_version": protocol_version,
        "bridge_type": "desktop",
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "capabilities": [
            {"name": "screen_capture", "version": "1.0"},
            {"name": "webview", "version": "1.0"},
            {"name": "tray", "version": "1.0"},
            {"name": "global_hotkey", "version": "1.0"},
            {"name": "notification", "version": "1.0"}
        ]
    }))
}

// ── WebView handlers ──────────────────────────────────────────────

/// Show a WebView window, optionally navigating to a URL first.
fn handle_webview_show(params: serde_json::Value) -> Result<serde_json::Value, (i32, String)> {
    let label = params
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or("halo");
    let url = params.get("url").and_then(|v| v.as_str());

    let app = crate::get_app_handle()
        .ok_or_else(|| (ERR_INTERNAL, "App handle not available".into()))?;

    let window = app
        .get_webview_window(label)
        .ok_or_else(|| (ERR_INTERNAL, format!("Window '{}' not found", label)))?;

    if let Some(url_str) = url {
        let parsed = url_str
            .parse()
            .map_err(|e| (ERR_INTERNAL, format!("Invalid URL: {e}")))?;
        let _ = window.navigate(parsed);
    }

    let _ = window.show();
    let _ = window.set_focus();

    Ok(json!({"shown": true, "label": label}))
}

/// Hide a WebView window.
fn handle_webview_hide(params: serde_json::Value) -> Result<serde_json::Value, (i32, String)> {
    let label = params
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or("halo");

    let app = crate::get_app_handle()
        .ok_or_else(|| (ERR_INTERNAL, "App handle not available".into()))?;

    let window = app
        .get_webview_window(label)
        .ok_or_else(|| (ERR_INTERNAL, format!("Window '{}' not found", label)))?;

    let _ = window.hide();

    Ok(json!({"hidden": true, "label": label}))
}

/// Navigate a WebView window to a URL.
fn handle_webview_navigate(
    params: serde_json::Value,
) -> Result<serde_json::Value, (i32, String)> {
    let label = params
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or("halo");
    let url = params
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (ERR_INTERNAL, "Missing 'url' parameter".to_string()))?;

    let app = crate::get_app_handle()
        .ok_or_else(|| (ERR_INTERNAL, "App handle not available".into()))?;

    let window = app
        .get_webview_window(label)
        .ok_or_else(|| (ERR_INTERNAL, format!("Window '{}' not found", label)))?;

    let parsed = url
        .parse()
        .map_err(|e| (ERR_INTERNAL, format!("Invalid URL: {e}")))?;
    window
        .navigate(parsed)
        .map_err(|e| (ERR_INTERNAL, format!("Navigation failed: {e}")))?;

    Ok(json!({"navigated": true, "label": label, "url": url}))
}

// ── Tray handlers ────────────────────────────────────────────────

/// Handle `tray.update_status` — update tray icon tooltip.
///
/// Params: `{ "status": "idle"|"thinking"|"acting"|"error", "tooltip": "optional text" }`
/// Returns: `{ "updated": true, "status": "..." }`
fn handle_tray_update_status(
    params: serde_json::Value,
) -> Result<serde_json::Value, (i32, String)> {
    let status = params
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("idle");
    let explicit_tooltip = params.get("tooltip").and_then(|v| v.as_str());

    let app = crate::get_app_handle()
        .ok_or_else(|| (ERR_INTERNAL, "App handle not available".into()))?;

    let tray = app
        .tray_by_id("main")
        .ok_or_else(|| (ERR_INTERNAL, "Tray icon 'main' not found".into()))?;

    // Use explicit tooltip if provided, otherwise derive from status
    let tooltip_text = match explicit_tooltip {
        Some(text) => text.to_string(),
        None => match status {
            "thinking" => "Aleph - Thinking...".to_string(),
            "acting" => "Aleph - Acting...".to_string(),
            "error" => "Aleph - Error".to_string(),
            _ => "Aleph - AI Assistant".to_string(),
        },
    };

    tray.set_tooltip(Some(&tooltip_text))
        .map_err(|e| (ERR_INTERNAL, format!("Failed to set tooltip: {e}")))?;

    info!(status, tooltip = %tooltip_text, "Tray status updated");

    Ok(json!({"updated": true, "status": status}))
}
