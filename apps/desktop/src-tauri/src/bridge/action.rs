//! Desktop action handlers — click, type_text, key_combo, launch_app
//!
//! Uses `enigo` for cross-platform mouse/keyboard automation and
//! platform-specific commands for app launching.

use aleph_protocol::desktop_bridge::ERR_INTERNAL;
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
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

    let char_count = text.chars().count();
    info!(chars = char_count, "Text typed");
    Ok(json!({"typed": true, "length": char_count}))
}

/// Handle `desktop.key_combo` — press a key combination
///
/// Accepts two formats:
/// 1. New format: `{ "modifiers": ["meta", "shift"], "key": "c" }`
/// 2. Legacy format: `{ "keys": ["cmd", "c"] }` — last non-modifier element is the main key
///
/// Modifier names: "meta"/"command"/"cmd"/"super"/"win", "shift", "control"/"ctrl", "alt"/"option"
pub fn handle_key_combo(params: Value) -> Result<Value, (i32, String)> {
    let (modifier_strs, key_str) = if let Some(keys_arr) = params.get("keys").and_then(|v| v.as_array()) {
        // Legacy format: flat array like ["cmd", "c"]
        // Split into modifiers and main key — last non-modifier element is the main key
        parse_legacy_keys(keys_arr)?
    } else {
        // New format: separate modifiers + key
        let modifiers: Vec<String> = params
            .get("modifiers")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let key = params
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| (ERR_INTERNAL, "Missing 'key' parameter (or use legacy 'keys' array)".to_string()))?
            .to_string();
        (modifiers, key)
    };

    let main_key = parse_key(&key_str)?;
    let modifier_keys: Vec<Key> = modifier_strs
        .iter()
        .map(|s| parse_modifier(s))
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

    let mod_names: Vec<&str> = modifier_strs.iter().map(|s| s.as_str()).collect();
    info!(modifiers = ?mod_names, key = %key_str, "Key combo performed");
    Ok(json!({"pressed": true, "modifiers": mod_names, "key": key_str}))
}

/// Parse the legacy `keys` flat array into (modifiers, main_key).
///
/// The last element that is NOT a known modifier name is treated as the main key.
/// All preceding modifier-like elements become modifiers.
/// Example: `["cmd", "shift", "c"]` -> (["cmd", "shift"], "c")
fn parse_legacy_keys(keys: &[Value]) -> Result<(Vec<String>, String), (i32, String)> {
    let strs: Vec<&str> = keys
        .iter()
        .filter_map(|v| v.as_str())
        .collect();

    if strs.is_empty() {
        return Err((ERR_INTERNAL, "Empty 'keys' array".to_string()));
    }

    // The last element is always the main key in the legacy format
    // (even if it happens to also be a modifier name, the convention is last = main key).
    // All preceding elements are treated as modifiers.
    let (mod_strs, main_str) = strs.split_at(strs.len() - 1);
    let modifiers: Vec<String> = mod_strs.iter().map(|s| s.to_string()).collect();
    let main_key = main_str[0].to_string();

    // Validate that all prefix elements are actually modifiers
    for m in &modifiers {
        parse_modifier(m)?;
    }

    Ok((modifiers, main_key))
}

/// Handle `desktop.scroll` — scroll the mouse wheel
///
/// Params:
/// - `direction`: string (optional) — "up", "down" (default), "left", or "right"
/// - `amount`: integer (optional) — number of scroll clicks (default: 3)
///
/// Enigo convention: positive length = down/right, negative = up/left
pub fn handle_scroll(params: Value) -> Result<Value, (i32, String)> {
    let direction = params
        .get("direction")
        .and_then(|v| v.as_str())
        .unwrap_or("down");
    let amount = params
        .get("amount")
        .and_then(|v| v.as_i64())
        .unwrap_or(3) as i32;

    let (axis, length) = match direction {
        "down" => (Axis::Vertical, amount),
        "up" => (Axis::Vertical, -amount),
        "right" => (Axis::Horizontal, amount),
        "left" => (Axis::Horizontal, -amount),
        other => {
            return Err((
                ERR_INTERNAL,
                format!(
                    "Unknown scroll direction: '{}'. Expected up, down, left, or right",
                    other
                ),
            ));
        }
    };

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| (ERR_INTERNAL, format!("Failed to create Enigo instance: {e}")))?;

    enigo
        .scroll(length, axis)
        .map_err(|e| (ERR_INTERNAL, format!("Failed to scroll: {e}")))?;

    info!(direction, amount, "Scroll performed");
    Ok(json!({"scrolled": true, "direction": direction, "amount": amount}))
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

// ── Window management handlers ───────────────────────────────────

/// Handle `desktop.window_list` — list visible on-screen windows.
///
/// Returns: `{ "windows": [{ "id", "title", "owner", "pid" }] }`
pub fn handle_window_list(_params: Value) -> Result<Value, (i32, String)> {
    #[cfg(target_os = "macos")]
    {
        macos_window_list()
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err((
            aleph_protocol::desktop_bridge::ERR_NOT_IMPLEMENTED,
            "window_list not implemented on this platform".into(),
        ))
    }
}

/// Handle `desktop.focus_window` — bring a window's owning application to front.
///
/// Params:
/// - `window_id`: u64 — the CGWindowID to focus
pub fn handle_focus_window(params: Value) -> Result<Value, (i32, String)> {
    let window_id = params
        .get("window_id")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| (ERR_INTERNAL, "Missing or invalid 'window_id' parameter".to_string()))?
        as u32;

    #[cfg(target_os = "macos")]
    {
        macos_focus_window(window_id)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = window_id;
        Err((
            aleph_protocol::desktop_bridge::ERR_NOT_IMPLEMENTED,
            "focus_window not implemented on this platform".into(),
        ))
    }
}

#[cfg(target_os = "macos")]
fn macos_window_list() -> Result<Value, (i32, String)> {
    use core_foundation::array::CFArray;
    use core_foundation::base::TCFType;
    use core_foundation::dictionary::CFDictionary;
    use core_graphics::display::{
        kCGNullWindowID, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
        CGWindowListCopyWindowInfo,
    };
    use core_graphics::window::{
        kCGWindowName, kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID,
    };

    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let window_info = unsafe { CGWindowListCopyWindowInfo(options, kCGNullWindowID) };
    if window_info.is_null() {
        return Err((ERR_INTERNAL, "Failed to list windows".into()));
    }

    let info_array: CFArray = unsafe { TCFType::wrap_under_create_rule(window_info) };
    let mut windows = Vec::new();

    for i in 0..info_array.len() {
        let dict_ptr = unsafe { *info_array.get_unchecked(i) };
        let dict: CFDictionary =
            unsafe { TCFType::wrap_under_get_rule(dict_ptr as *const _ as _) };

        let id = unsafe { get_cf_number(&dict, kCGWindowNumber) }.unwrap_or(0);
        let title = unsafe { get_cf_string(&dict, kCGWindowName) }.unwrap_or_default();
        let owner = unsafe { get_cf_string(&dict, kCGWindowOwnerName) }.unwrap_or_default();
        let pid = unsafe { get_cf_number(&dict, kCGWindowOwnerPID) }.unwrap_or(0);

        windows.push(json!({
            "id": id,
            "title": title,
            "owner": owner,
            "pid": pid,
        }));
    }

    info!(count = windows.len(), "Window list retrieved");
    Ok(json!({ "windows": windows }))
}

#[cfg(target_os = "macos")]
fn macos_focus_window(window_id: u32) -> Result<Value, (i32, String)> {
    use core_foundation::array::CFArray;
    use core_foundation::base::TCFType;
    use core_foundation::dictionary::CFDictionary;
    use core_graphics::display::{kCGWindowListOptionAll, CGWindowListCopyWindowInfo};
    use core_graphics::window::kCGWindowOwnerPID;

    // Find the PID for this window
    let window_info =
        unsafe { CGWindowListCopyWindowInfo(kCGWindowListOptionAll, window_id) };
    if window_info.is_null() {
        return Err((ERR_INTERNAL, format!("Window {} not found", window_id)));
    }

    let info_array: CFArray = unsafe { TCFType::wrap_under_create_rule(window_info) };
    if info_array.len() == 0 {
        return Err((ERR_INTERNAL, format!("Window {} not found", window_id)));
    }

    let dict_ptr = unsafe { *info_array.get_unchecked(0) };
    let dict: CFDictionary =
        unsafe { TCFType::wrap_under_get_rule(dict_ptr as *const _ as _) };
    let pid = unsafe { get_cf_number(&dict, kCGWindowOwnerPID) }
        .ok_or_else(|| (ERR_INTERNAL, format!("Cannot determine PID for window {}", window_id)))?;

    // Activate the application owning this window
    unsafe {
        let cls = objc::runtime::Class::get("NSRunningApplication")
            .ok_or_else(|| (ERR_INTERNAL, "NSRunningApplication class not found".to_string()))?;
        let app: *mut objc::runtime::Object =
            msg_send![cls, runningApplicationWithProcessIdentifier: pid as i32];
        if app.is_null() {
            return Err((ERR_INTERNAL, format!("No running application with PID {}", pid)));
        }
        // NSApplicationActivateAllWindows (1) | NSApplicationActivateIgnoringOtherApps (2) = 3
        let _: bool = msg_send![app, activateWithOptions: 3u64];
    }

    info!(window_id, pid, "Window focused");
    Ok(json!({ "focused": window_id }))
}

/// Extract an integer value from a CFDictionary by CFString key reference.
#[cfg(target_os = "macos")]
fn get_cf_number(
    dict: &core_foundation::dictionary::CFDictionary,
    key: core_foundation::string::CFStringRef,
) -> Option<i64> {
    use core_foundation::base::TCFType;
    use core_foundation::number::CFNumber;
    dict.find(key as *const _)
        .map(|v| unsafe { CFNumber::wrap_under_get_rule(*v as *const _) })
        .and_then(|n| n.to_i64())
}

/// Extract a string value from a CFDictionary by CFString key reference.
#[cfg(target_os = "macos")]
fn get_cf_string(
    dict: &core_foundation::dictionary::CFDictionary,
    key: core_foundation::string::CFStringRef,
) -> Option<String> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    dict.find(key as *const _)
        .map(|v| unsafe { CFString::wrap_under_get_rule(*v as *const _) })
        .map(|s| s.to_string())
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
