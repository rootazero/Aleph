//! Desktop Bridge — UDS JSON-RPC 2.0 server
//!
//! Symmetric with macOS Swift DesktopBridgeServer.
//! Listens on ~/.aleph/bridge.sock, dispatches JSON-RPC requests
//! to perception/action handlers.

mod perception;
pub mod protocol;

use aleph_protocol::desktop_bridge::{self, ERR_METHOD_NOT_FOUND, ERR_NOT_IMPLEMENTED};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tracing::{error, info, warn};

/// Start the Desktop Bridge UDS server
///
/// Listens on ~/.aleph/bridge.sock and dispatches JSON-RPC 2.0 requests.
/// This function runs forever; call it from a spawned task.
pub async fn start_bridge_server() {
    let socket_path = desktop_bridge::default_socket_path();

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
        desktop_bridge::METHOD_PING => Ok(json!("pong")),

        desktop_bridge::METHOD_SCREENSHOT => perception::handle_screenshot(params),

        // All other methods return "not implemented" for MVP
        desktop_bridge::METHOD_OCR
        | desktop_bridge::METHOD_AX_TREE
        | desktop_bridge::METHOD_CLICK
        | desktop_bridge::METHOD_TYPE_TEXT
        | desktop_bridge::METHOD_KEY_COMBO
        | desktop_bridge::METHOD_LAUNCH_APP
        | desktop_bridge::METHOD_WINDOW_LIST
        | desktop_bridge::METHOD_FOCUS_WINDOW
        | desktop_bridge::METHOD_CANVAS_SHOW
        | desktop_bridge::METHOD_CANVAS_HIDE
        | desktop_bridge::METHOD_CANVAS_UPDATE => Err((
            ERR_NOT_IMPLEMENTED,
            format!("{} not implemented on this platform", method),
        )),

        _ => Err((
            ERR_METHOD_NOT_FOUND,
            format!("Method not found: {}", method),
        )),
    }
}
