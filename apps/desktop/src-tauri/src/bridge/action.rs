//! Desktop action handlers — click, type_text, key_combo, launch_app
//!
//! Uses `enigo` for cross-platform mouse/keyboard automation and
//! platform-specific commands for app launching.

use aleph_protocol::desktop_bridge::ERR_INTERNAL;
use enigo::{Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use serde_json::{json, Value};
use tracing::info;

/// Handle `desktop.click` — move mouse to (x, y) and click
///
/// Params:
/// - `x`: f64 — screen X coordinate
/// - `y`: f64 — screen Y coordinate
/// - `button`: string (optional) — "left" (default), "right", or "middle"
pub fn handle_click(params: Value) -> Result<Value, (i32, String)> {
    let x = params
        .get("x")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| (ERR_INTERNAL, "Missing or invalid 'x' parameter".to_string()))?;
    let y = params
        .get("y")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| (ERR_INTERNAL, "Missing or invalid 'y' parameter".to_string()))?;
    let button_str = params
        .get("button")
        .and_then(|v| v.as_str())
        .unwrap_or("left");

    let button = match button_str {
        "left" => Button::Left,
        "right" => Button::Right,
        "middle" => Button::Middle,
        other => {
            return Err((
                ERR_INTERNAL,
                format!("Unknown button: '{}'. Expected left, right, or middle", other),
            ));
        }
    };

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| (ERR_INTERNAL, format!("Failed to create Enigo instance: {e}")))?;

    enigo
        .move_mouse(x as i32, y as i32, Coordinate::Abs)
        .map_err(|e| (ERR_INTERNAL, format!("Failed to move mouse: {e}")))?;

    enigo
        .button(button, Direction::Click)
        .map_err(|e| (ERR_INTERNAL, format!("Failed to click: {e}")))?;

    info!(x, y, button = button_str, "Click performed");
    Ok(json!({"clicked": true, "x": x, "y": y, "button": button_str}))
}

/// Handle `desktop.type_text` — type a string of text
///
/// Params:
/// - `text`: string — the text to type
pub fn handle_type_text(params: Value) -> Result<Value, (i32, String)> {
    let text = params
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (ERR_INTERNAL, "Missing or invalid 'text' parameter".to_string()))?;

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| (ERR_INTERNAL, format!("Failed to create Enigo instance: {e}")))?;

    enigo
        .text(text)
        .map_err(|e| (ERR_INTERNAL, format!("Failed to type text: {e}")))?;

    info!(chars = text.len(), "Text typed");
    Ok(json!({"typed": true, "length": text.len()}))
}

/// Handle `desktop.key_combo` — press a key combination
///
/// Params:
/// - `modifiers`: array of strings — e.g. ["meta", "shift"]
/// - `key`: string — the main key, e.g. "c", "v", "space", "return", "tab", "escape"
///
/// Modifier names: "meta"/"command"/"super", "shift", "control"/"ctrl", "alt"/"option"
pub fn handle_key_combo(params: Value) -> Result<Value, (i32, String)> {
    let modifiers = params
        .get("modifiers")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let key_str = params
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (ERR_INTERNAL, "Missing or invalid 'key' parameter".to_string()))?;

    let main_key = parse_key(key_str)?;
    let modifier_keys: Vec<Key> = modifiers
        .iter()
        .filter_map(|v| v.as_str())
        .map(parse_modifier)
        .collect::<Result<Vec<_>, _>>()?;

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| (ERR_INTERNAL, format!("Failed to create Enigo instance: {e}")))?;

    // Press all modifiers
    for m in &modifier_keys {
        enigo
            .key(*m, Direction::Press)
            .map_err(|e| (ERR_INTERNAL, format!("Failed to press modifier: {e}")))?;
    }

    // Click the main key
    enigo
        .key(main_key, Direction::Click)
        .map_err(|e| (ERR_INTERNAL, format!("Failed to click key: {e}")))?;

    // Release modifiers in reverse order
    for m in modifier_keys.iter().rev() {
        enigo
            .key(*m, Direction::Release)
            .map_err(|e| (ERR_INTERNAL, format!("Failed to release modifier: {e}")))?;
    }

    let mod_names: Vec<&str> = modifiers.iter().filter_map(|v| v.as_str()).collect();
    info!(modifiers = ?mod_names, key = key_str, "Key combo performed");
    Ok(json!({"pressed": true, "modifiers": mod_names, "key": key_str}))
}

/// Handle `desktop.launch_app` — launch an application
///
/// Params:
/// - `bundle_id`: string (macOS) — e.g. "com.apple.Safari"
/// - `app_name`: string (Windows/Linux) — e.g. "notepad" or "firefox"
///
/// On macOS: uses `open -b <bundle_id>`
/// On Windows: uses `cmd /C start "" "<app_name>"`
/// On Linux: uses `xdg-open <app_name>` or direct exec
pub fn handle_launch_app(params: Value) -> Result<Value, (i32, String)> {
    #[cfg(target_os = "macos")]
    {
        let bundle_id = params
            .get("bundle_id")
            .and_then(|v| v.as_str())
            .or_else(|| params.get("app_name").and_then(|v| v.as_str()))
            .ok_or_else(|| {
                (
                    ERR_INTERNAL,
                    "Missing 'bundle_id' or 'app_name' parameter".to_string(),
                )
            })?;

        let output = std::process::Command::new("open")
            .arg("-b")
            .arg(bundle_id)
            .output()
            .map_err(|e| (ERR_INTERNAL, format!("Failed to launch app: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err((
                ERR_INTERNAL,
                format!("Failed to launch '{}': {}", bundle_id, stderr.trim()),
            ));
        }

        info!(bundle_id, "App launched (macOS)");
        Ok(json!({"launched": true, "bundle_id": bundle_id}))
    }

    #[cfg(target_os = "windows")]
    {
        let app_name = params
            .get("app_name")
            .and_then(|v| v.as_str())
            .or_else(|| params.get("bundle_id").and_then(|v| v.as_str()))
            .ok_or_else(|| {
                (
                    ERR_INTERNAL,
                    "Missing 'app_name' parameter".to_string(),
                )
            })?;

        let output = std::process::Command::new("cmd")
            .args(["/C", "start", "", app_name])
            .output()
            .map_err(|e| (ERR_INTERNAL, format!("Failed to launch app: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err((
                ERR_INTERNAL,
                format!("Failed to launch '{}': {}", app_name, stderr.trim()),
            ));
        }

        info!(app_name, "App launched (Windows)");
        Ok(json!({"launched": true, "app_name": app_name}))
    }

    #[cfg(target_os = "linux")]
    {
        let app_name = params
            .get("app_name")
            .and_then(|v| v.as_str())
            .or_else(|| params.get("bundle_id").and_then(|v| v.as_str()))
            .ok_or_else(|| {
                (
                    ERR_INTERNAL,
                    "Missing 'app_name' parameter".to_string(),
                )
            })?;

        let output = std::process::Command::new("xdg-open")
            .arg(app_name)
            .output()
            .map_err(|e| (ERR_INTERNAL, format!("Failed to launch app: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err((
                ERR_INTERNAL,
                format!("Failed to launch '{}': {}", app_name, stderr.trim()),
            ));
        }

        info!(app_name, "App launched (Linux)");
        Ok(json!({"launched": true, "app_name": app_name}))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Err((
            ERR_INTERNAL,
            "launch_app not supported on this platform".to_string(),
        ))
    }
}

// ── Key parsing helpers ──────────────────────────────────────────

/// Parse a modifier name to an enigo Key
fn parse_modifier(name: &str) -> Result<Key, (i32, String)> {
    match name.to_lowercase().as_str() {
        "meta" | "command" | "cmd" | "super" | "win" => Ok(Key::Meta),
        "shift" => Ok(Key::Shift),
        "control" | "ctrl" => Ok(Key::Control),
        "alt" | "option" => Ok(Key::Alt),
        other => Err((
            ERR_INTERNAL,
            format!(
                "Unknown modifier: '{}'. Expected meta/command, shift, control/ctrl, alt/option",
                other
            ),
        )),
    }
}

/// Parse a key name to an enigo Key
fn parse_key(name: &str) -> Result<Key, (i32, String)> {
    // Single character keys
    if name.len() == 1 {
        let ch = name.chars().next().unwrap();
        return Ok(Key::Unicode(ch));
    }

    // Named keys
    match name.to_lowercase().as_str() {
        "space" => Ok(Key::Unicode(' ')),
        "return" | "enter" => Ok(Key::Return),
        "tab" => Ok(Key::Tab),
        "escape" | "esc" => Ok(Key::Escape),
        "backspace" | "delete" => Ok(Key::Backspace),
        "up" | "uparrow" => Ok(Key::UpArrow),
        "down" | "downarrow" => Ok(Key::DownArrow),
        "left" | "leftarrow" => Ok(Key::LeftArrow),
        "right" | "rightarrow" => Ok(Key::RightArrow),
        "home" => Ok(Key::Home),
        "end" => Ok(Key::End),
        "pageup" => Ok(Key::PageUp),
        "pagedown" => Ok(Key::PageDown),
        "f1" => Ok(Key::F1),
        "f2" => Ok(Key::F2),
        "f3" => Ok(Key::F3),
        "f4" => Ok(Key::F4),
        "f5" => Ok(Key::F5),
        "f6" => Ok(Key::F6),
        "f7" => Ok(Key::F7),
        "f8" => Ok(Key::F8),
        "f9" => Ok(Key::F9),
        "f10" => Ok(Key::F10),
        "f11" => Ok(Key::F11),
        "f12" => Ok(Key::F12),
        other => Err((
            ERR_INTERNAL,
            format!("Unknown key: '{}'. Use single char or named key (space, return, tab, escape, etc.)", other),
        )),
    }
}
