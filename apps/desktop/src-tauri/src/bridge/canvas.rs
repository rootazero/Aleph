//! Canvas overlay — transparent WebView window for AI-generated UI.
//!
//! The canvas is a frameless, transparent, always-on-top window that the
//! server can control via three RPC methods:
//!
//! - `desktop.canvas_show`   — create or reuse, load HTML, show
//! - `desktop.canvas_hide`   — hide without destroying
//! - `desktop.canvas_update` — apply A2UI surface patches via JS eval

use aleph_protocol::desktop_bridge::ERR_INTERNAL;
use base64::{engine::general_purpose, Engine as _};
use serde_json::{json, Value};
use tauri::Manager;

/// Label used for the canvas WebView window.
const CANVAS_LABEL: &str = "canvas";

// ── Show ────────────────────────────────────────────────────────────

/// Handle `desktop.canvas_show` — create or reuse a canvas overlay.
///
/// Params:
/// ```json
/// { "html": "<h1>Hello</h1>",
///   "position": { "x": 100, "y": 100, "width": 400, "height": 300 } }
/// ```
///
/// Returns: `{ "visible": true, "position": { "x", "y", "width", "height" } }`
pub fn handle_canvas_show(params: Value) -> Result<Value, (i32, String)> {
    let html = params
        .get("html")
        .and_then(|v| v.as_str())
        .unwrap_or("<html><body></body></html>");

    let pos = params.get("position");
    let x = pos
        .and_then(|p| p.get("x"))
        .and_then(|v| v.as_f64())
        .unwrap_or(100.0);
    let y = pos
        .and_then(|p| p.get("y"))
        .and_then(|v| v.as_f64())
        .unwrap_or(100.0);
    let width = pos
        .and_then(|p| p.get("width"))
        .and_then(|v| v.as_f64())
        .unwrap_or(400.0);
    let height = pos
        .and_then(|p| p.get("height"))
        .and_then(|v| v.as_f64())
        .unwrap_or(600.0);

    let app = crate::get_app_handle()
        .ok_or_else(|| (ERR_INTERNAL, "App handle not available".into()))?;

    // Reuse existing canvas window or create a new one
    if let Some(window) = app.get_webview_window(CANVAS_LABEL) {
        // Update position and size on the existing window
        let _ = window.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
        let _ = window.set_size(tauri::PhysicalSize::new(width as u32, height as u32));

        // Load new HTML content via data URI
        let encoded = general_purpose::STANDARD.encode(html.as_bytes());
        let data_url = format!("data:text/html;base64,{}", encoded);
        if let Ok(parsed) = data_url.parse() {
            let _ = window.navigate(parsed);
        }

        let _ = window.show();

        // Inject A2UI patch handler after page load
        inject_a2ui_handler(&window);
    } else {
        // Create new canvas window — transparent, no decorations, always on top
        let encoded = general_purpose::STANDARD.encode(html.as_bytes());
        let data_url = format!("data:text/html;base64,{}", encoded);

        let builder = tauri::WebviewWindowBuilder::new(
            app,
            CANVAS_LABEL,
            tauri::WebviewUrl::External(
                data_url
                    .parse()
                    .map_err(|e| (ERR_INTERNAL, format!("Invalid data URL: {e}")))?,
            ),
        )
        .title("Aleph Canvas")
        .decorations(false)
        .always_on_top(true)
        .inner_size(width, height)
        .position(x, y)
        .visible(true);

        let window = builder
            .build()
            .map_err(|e| (ERR_INTERNAL, format!("Failed to create canvas window: {e}")))?;

        // Inject A2UI patch handler
        inject_a2ui_handler(&window);
    }

    Ok(json!({
        "visible": true,
        "position": { "x": x, "y": y, "width": width, "height": height }
    }))
}

// ── Hide ────────────────────────────────────────────────────────────

/// Handle `desktop.canvas_hide` — hide the canvas overlay.
///
/// Returns: `{ "visible": false }`
pub fn handle_canvas_hide(_params: Value) -> Result<Value, (i32, String)> {
    let app = crate::get_app_handle()
        .ok_or_else(|| (ERR_INTERNAL, "App handle not available".into()))?;

    if let Some(window) = app.get_webview_window(CANVAS_LABEL) {
        let _ = window.hide();
    }

    Ok(json!({ "visible": false }))
}

// ── Update (A2UI patch) ─────────────────────────────────────────────

/// Handle `desktop.canvas_update` — apply an A2UI patch to the canvas.
///
/// Params: `{ "patch": [{"type": "surfaceUpdate", "content": "<p>Updated</p>"}] }`
/// Returns: `{ "patched": true }`
pub fn handle_canvas_update(params: Value) -> Result<Value, (i32, String)> {
    let patch = params
        .get("patch")
        .ok_or((ERR_INTERNAL, "Missing 'patch' parameter".into()))?;

    let app = crate::get_app_handle()
        .ok_or_else(|| (ERR_INTERNAL, "App handle not available".into()))?;

    let window = app
        .get_webview_window(CANVAS_LABEL)
        .ok_or_else(|| (ERR_INTERNAL, "Canvas not shown -- call canvas_show first".into()))?;

    let patch_json = serde_json::to_string(patch)
        .map_err(|e| (ERR_INTERNAL, format!("Invalid patch JSON: {e}")))?;

    let script = format!(
        "if (typeof window.alephApplyPatch === 'function') {{ window.alephApplyPatch({}); }}",
        patch_json
    );

    // Evaluate JS — fire and forget; eval errors are non-fatal
    let _ = window.eval(&script);

    Ok(json!({ "patched": true }))
}

// ── A2UI handler injection ──────────────────────────────────────────

/// Inject the A2UI patch handler JavaScript into a window.
///
/// This installs `window.alephApplyPatch(patch)` which the server can later
/// invoke via `desktop.canvas_update` to apply surface mutations.
fn inject_a2ui_handler(window: &tauri::WebviewWindow) {
    let script = r#"
        window.alephApplyPatch = function(patch) {
            if (!Array.isArray(patch)) return;
            patch.forEach(function(op) {
                if (op.type === 'surfaceUpdate' && op.content) {
                    document.body.innerHTML = op.content;
                }
            });
        };
    "#;
    let _ = window.eval(script);
}
