# Phase 3: Desktop Capabilities Implementation

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement all 12 stubbed desktop bridge methods so the AI agent has full desktop control — input simulation, window management, canvas overlay, tray status, OCR, and accessibility inspection.

**Architecture:** Extend the Tauri bridge with three new handler modules (`action.rs`, `canvas.rs`, `platform/`). Cross-platform capabilities use `enigo` (input) and `std::process::Command` (app launch). Platform-specific capabilities (OCR, AX tree, window management) use `objc`/`cocoa`/`core-graphics` on macOS with compile-time `#[cfg]` gates. Other platforms return `ERR_NOT_IMPLEMENTED` for platform-specific methods until future work adds Windows/Linux support.

**Tech Stack:** Rust, Tauri v2, enigo (input simulation), cocoa/core-graphics/objc (macOS APIs), xcap (already present)

---

### Task 1: Add Dependencies and Create Action Module (click, type_text, key_combo)

**Files:**
- Modify: `apps/desktop/src-tauri/Cargo.toml`
- Create: `apps/desktop/src-tauri/src/bridge/action.rs`
- Modify: `apps/desktop/src-tauri/src/bridge/mod.rs`

**Context:** `enigo` 0.3 provides cross-platform mouse/keyboard simulation. The dispatch in `mod.rs` currently routes click/type_text/key_combo to `ERR_NOT_IMPLEMENTED`.

**Step 1: Add enigo dependency**

In `Cargo.toml`, after the `image` line (~line 48), add:

```toml
# Input simulation (mouse, keyboard)
enigo = { version = "0.3", features = ["serde"] }
```

**Step 2: Create `action.rs`**

```rust
//! Action handlers — mouse clicks, keyboard input, app launch, window management.

use aleph_protocol::desktop_bridge::ERR_INTERNAL;
use enigo::{Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use serde_json::{json, Value};

/// Handle `desktop.click` — move mouse and click.
///
/// Params: `{ "x": f64, "y": f64, "button": "left"|"right"|"middle" }`
/// Returns: `{ "clicked": true, "x": ..., "y": ..., "button": "..." }`
pub fn handle_click(params: Value) -> Result<Value, (i32, String)> {
    let x = params.get("x").and_then(|v| v.as_f64())
        .ok_or((ERR_INTERNAL, "Missing 'x' coordinate".into()))?;
    let y = params.get("y").and_then(|v| v.as_f64())
        .ok_or((ERR_INTERNAL, "Missing 'y' coordinate".into()))?;
    let button_str = params.get("button").and_then(|v| v.as_str()).unwrap_or("left");

    let button = match button_str {
        "right" => Button::Right,
        "middle" => Button::Middle,
        _ => Button::Left,
    };

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| (ERR_INTERNAL, format!("Failed to create input controller: {e}")))?;

    enigo.move_mouse(x as i32, y as i32, Coordinate::Abs)
        .map_err(|e| (ERR_INTERNAL, format!("Failed to move mouse: {e}")))?;
    enigo.button(button, Direction::Click)
        .map_err(|e| (ERR_INTERNAL, format!("Failed to click: {e}")))?;

    Ok(json!({ "clicked": true, "x": x, "y": y, "button": button_str }))
}

/// Handle `desktop.type_text` — type a string using keyboard events.
///
/// Params: `{ "text": "Hello, world!" }`
/// Returns: `{ "typed": <char_count> }`
pub fn handle_type_text(params: Value) -> Result<Value, (i32, String)> {
    let text = params.get("text").and_then(|v| v.as_str())
        .ok_or((ERR_INTERNAL, "Missing 'text' parameter".into()))?;

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| (ERR_INTERNAL, format!("Failed to create input controller: {e}")))?;

    enigo.text(text)
        .map_err(|e| (ERR_INTERNAL, format!("Failed to type text: {e}")))?;

    Ok(json!({ "typed": text.chars().count() }))
}

/// Handle `desktop.key_combo` — press a key combination.
///
/// Params: `{ "keys": ["cmd", "c"] }`
/// Returns: `{ "keys": [...] }`
pub fn handle_key_combo(params: Value) -> Result<Value, (i32, String)> {
    let keys: Vec<String> = params.get("keys")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .ok_or((ERR_INTERNAL, "Missing 'keys' array".into()))?;

    if keys.is_empty() {
        return Err((ERR_INTERNAL, "Empty 'keys' array".into()));
    }

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| (ERR_INTERNAL, format!("Failed to create input controller: {e}")))?;

    // Separate modifiers from the main key
    let mut modifiers = Vec::new();
    let mut main_key = None;

    for key in &keys {
        match key.to_lowercase().as_str() {
            "cmd" | "command" | "meta" | "super" => modifiers.push(Key::Meta),
            "shift" => modifiers.push(Key::Shift),
            "alt" | "opt" | "option" => modifiers.push(Key::Alt),
            "ctrl" | "control" => modifiers.push(Key::Control),
            other => main_key = Some(key_name_to_enigo(other)),
        }
    }

    // Press modifiers
    for m in &modifiers {
        enigo.key(*m, Direction::Press)
            .map_err(|e| (ERR_INTERNAL, format!("Failed to press modifier: {e}")))?;
    }

    // Press and release main key
    if let Some(k) = main_key {
        enigo.key(k, Direction::Click)
            .map_err(|e| (ERR_INTERNAL, format!("Failed to press key: {e}")))?;
    }

    // Release modifiers in reverse
    for m in modifiers.iter().rev() {
        enigo.key(*m, Direction::Release)
            .map_err(|e| (ERR_INTERNAL, format!("Failed to release modifier: {e}")))?;
    }

    Ok(json!({ "keys": keys }))
}

/// Handle `desktop.launch_app` — launch an application.
///
/// Params: `{ "bundle_id": "com.apple.Safari" }` (macOS)
///       or `{ "app_name": "notepad" }` (Windows/Linux)
/// Returns: `{ "launched": "...", "pid": ... }`
pub fn handle_launch_app(params: Value) -> Result<Value, (i32, String)> {
    let bundle_id = params.get("bundle_id").and_then(|v| v.as_str());
    let app_name = params.get("app_name").and_then(|v| v.as_str());

    #[cfg(target_os = "macos")]
    {
        let id = bundle_id.or(app_name)
            .ok_or((ERR_INTERNAL, "Missing 'bundle_id' parameter".into()))?;
        let output = std::process::Command::new("open")
            .args(["-b", id])
            .output()
            .map_err(|e| (ERR_INTERNAL, format!("Failed to launch app: {e}")))?;
        if output.status.success() {
            Ok(json!({ "launched": id }))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err((ERR_INTERNAL, format!("Failed to launch '{}': {}", id, stderr.trim())))
        }
    }

    #[cfg(target_os = "windows")]
    {
        let name = app_name.or(bundle_id)
            .ok_or((ERR_INTERNAL, "Missing 'app_name' parameter".into()))?;
        let output = std::process::Command::new("cmd")
            .args(["/C", "start", "", name])
            .output()
            .map_err(|e| (ERR_INTERNAL, format!("Failed to launch app: {e}")))?;
        if output.status.success() {
            Ok(json!({ "launched": name }))
        } else {
            Err((ERR_INTERNAL, format!("Failed to launch '{}'", name)))
        }
    }

    #[cfg(target_os = "linux")]
    {
        let name = app_name.or(bundle_id)
            .ok_or((ERR_INTERNAL, "Missing 'app_name' parameter".into()))?;
        let output = std::process::Command::new("xdg-open")
            .arg(name)
            .output()
            .map_err(|e| (ERR_INTERNAL, format!("Failed to launch app: {e}")))?;
        if output.status.success() {
            Ok(json!({ "launched": name }))
        } else {
            Err((ERR_INTERNAL, format!("Failed to launch '{}'", name)))
        }
    }
}

fn key_name_to_enigo(name: &str) -> Key {
    match name.to_lowercase().as_str() {
        "return" | "enter" => Key::Return,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "backspace" | "delete" => Key::Backspace,
        "escape" | "esc" => Key::Escape,
        "up" => Key::UpArrow,
        "down" => Key::DownArrow,
        "left" => Key::LeftArrow,
        "right" => Key::RightArrow,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "f1" => Key::F1, "f2" => Key::F2, "f3" => Key::F3, "f4" => Key::F4,
        "f5" => Key::F5, "f6" => Key::F6, "f7" => Key::F7, "f8" => Key::F8,
        "f9" => Key::F9, "f10" => Key::F10, "f11" => Key::F11, "f12" => Key::F12,
        s if s.len() == 1 => Key::Unicode(s.chars().next().unwrap()),
        _ => Key::Unicode(' '),
    }
}
```

**Step 3: Register `action` module and wire dispatch**

In `bridge/mod.rs`, add `mod action;` after `mod perception;` (line 7).

In the `dispatch()` match, replace the `METHOD_CLICK | METHOD_TYPE_TEXT | METHOD_KEY_COMBO | METHOD_LAUNCH_APP` arm with individual handlers:

```rust
desktop_bridge::METHOD_CLICK => action::handle_click(params),
desktop_bridge::METHOD_TYPE_TEXT => action::handle_type_text(params),
desktop_bridge::METHOD_KEY_COMBO => action::handle_key_combo(params),
desktop_bridge::METHOD_LAUNCH_APP => action::handle_launch_app(params),
```

Remove these four from the `ERR_NOT_IMPLEMENTED` arm.

**Step 4: Verify**

```bash
cargo check -p aleph-tauri
```
Expected: Compiles cleanly

**Step 5: Commit**

```bash
git add apps/desktop/src-tauri/Cargo.toml apps/desktop/src-tauri/src/bridge/action.rs apps/desktop/src-tauri/src/bridge/mod.rs
git commit -m "bridge: implement click, type_text, key_combo, launch_app via enigo"
```

---

### Task 2: Window Management (window_list, focus_window)

**Files:**
- Modify: `apps/desktop/src-tauri/src/bridge/action.rs`
- Modify: `apps/desktop/src-tauri/src/bridge/mod.rs`
- Modify: `apps/desktop/src-tauri/Cargo.toml`

**Context:** Window listing and focusing require platform-specific APIs. On macOS, `CGWindowListCopyWindowInfo` (via `core-graphics` crate) lists windows and `NSRunningApplication.activate()` focuses them. Other platforms return `ERR_NOT_IMPLEMENTED` for now.

**Step 1: Add core-graphics dependency for macOS**

In `Cargo.toml`, update the `[target.'cfg(target_os = "macos")'.dependencies]` section:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
cocoa = "0.26"
objc = "0.2"
core-graphics = "0.24"
```

**Step 2: Add window handlers to `action.rs`**

Add at the end of `action.rs`:

```rust
/// Handle `desktop.window_list` — list visible windows.
///
/// Returns: `{ "windows": [{ "id", "title", "owner", "pid", "bounds" }] }`
pub fn handle_window_list(_params: Value) -> Result<Value, (i32, String)> {
    #[cfg(target_os = "macos")]
    {
        macos_window_list()
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err((aleph_protocol::desktop_bridge::ERR_NOT_IMPLEMENTED,
             "window_list not implemented on this platform".into()))
    }
}

/// Handle `desktop.focus_window` — bring a window to front.
///
/// Params: `{ "window_id": 123 }`
pub fn handle_focus_window(params: Value) -> Result<Value, (i32, String)> {
    let window_id = params.get("window_id").and_then(|v| v.as_u64())
        .ok_or((ERR_INTERNAL, "Missing 'window_id' parameter".into()))? as u32;

    #[cfg(target_os = "macos")]
    {
        macos_focus_window(window_id)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = window_id;
        Err((aleph_protocol::desktop_bridge::ERR_NOT_IMPLEMENTED,
             "focus_window not implemented on this platform".into()))
    }
}

#[cfg(target_os = "macos")]
fn macos_window_list() -> Result<Value, (i32, String)> {
    use core_graphics::display::{
        kCGNullWindowID, kCGWindowListOptionOnScreenOnly, kCGWindowListExcludeDesktopElements,
        CGWindowListCopyWindowInfo,
    };
    use core_graphics::window::{
        kCGWindowNumber, kCGWindowName, kCGWindowOwnerName, kCGWindowOwnerPID, kCGWindowBounds,
    };
    use core_foundation::array::CFArray;
    use core_foundation::base::TCFType;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;

    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let window_info = unsafe { CGWindowListCopyWindowInfo(options, kCGNullWindowID) };
    if window_info.is_null() {
        return Err((ERR_INTERNAL, "Failed to list windows".into()));
    }

    let info_array: CFArray = unsafe { TCFType::wrap_under_create_rule(window_info) };
    let mut windows = Vec::new();

    for i in 0..info_array.len() {
        let dict_ref = unsafe { info_array.get_unchecked(i) };
        let dict: CFDictionary = unsafe { TCFType::wrap_under_get_rule(dict_ref as *const _ as _) };

        let id = get_cf_number(&dict, kCGWindowNumber).unwrap_or(0);
        let title = get_cf_string(&dict, kCGWindowName).unwrap_or_default();
        let owner = get_cf_string(&dict, kCGWindowOwnerName).unwrap_or_default();
        let pid = get_cf_number(&dict, kCGWindowOwnerPID).unwrap_or(0);

        windows.push(json!({
            "id": id,
            "title": title,
            "owner": owner,
            "pid": pid,
        }));
    }

    Ok(json!({ "windows": windows }))
}

#[cfg(target_os = "macos")]
fn macos_focus_window(window_id: u32) -> Result<Value, (i32, String)> {
    use core_graphics::display::{CGWindowListCopyWindowInfo, kCGWindowListOptionAll};
    use core_graphics::window::{kCGWindowOwnerPID};
    use core_foundation::array::CFArray;
    use core_foundation::base::TCFType;
    use core_foundation::dictionary::CFDictionary;

    // Find the PID for this window
    let window_info = unsafe {
        CGWindowListCopyWindowInfo(kCGWindowListOptionAll, window_id)
    };
    if window_info.is_null() {
        return Err((ERR_INTERNAL, format!("Window {} not found", window_id)));
    }

    let info_array: CFArray = unsafe { TCFType::wrap_under_create_rule(window_info) };
    if info_array.len() == 0 {
        return Err((ERR_INTERNAL, format!("Window {} not found", window_id)));
    }

    let dict_ref = unsafe { info_array.get_unchecked(0) };
    let dict: CFDictionary = unsafe { TCFType::wrap_under_get_rule(dict_ref as *const _ as _) };
    let pid = get_cf_number(&dict, kCGWindowOwnerPID)
        .ok_or((ERR_INTERNAL, format!("Cannot determine PID for window {}", window_id)))?;

    // Activate the application owning this window
    unsafe {
        let cls = objc::runtime::Class::get("NSRunningApplication").unwrap();
        let app: *mut objc::runtime::Object =
            objc::msg_send![cls, runningApplicationWithProcessIdentifier: pid as i32];
        if app.is_null() {
            return Err((ERR_INTERNAL, format!("No app with PID {}", pid)));
        }
        let _: bool = objc::msg_send![app, activateWithOptions: 3u64]; // NSApplicationActivateAllWindows | NSApplicationActivateIgnoringOtherApps
    }

    Ok(json!({ "focused": window_id }))
}

#[cfg(target_os = "macos")]
fn get_cf_number(dict: &core_foundation::dictionary::CFDictionary, key: &str) -> Option<i64> {
    use core_foundation::base::TCFType;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    let cf_key = CFString::new(key);
    dict.find(cf_key.as_CFTypeRef() as *const _)
        .map(|v| unsafe { CFNumber::wrap_under_get_rule(*v as *const _) })
        .and_then(|n| n.to_i64())
}

#[cfg(target_os = "macos")]
fn get_cf_string(dict: &core_foundation::dictionary::CFDictionary, key: &str) -> Option<String> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    let cf_key = CFString::new(key);
    dict.find(cf_key.as_CFTypeRef() as *const _)
        .map(|v| unsafe { CFString::wrap_under_get_rule(*v as *const _) })
        .map(|s| s.to_string())
}
```

**Step 3: Wire dispatch**

In `mod.rs` dispatch, replace `METHOD_WINDOW_LIST | METHOD_FOCUS_WINDOW` from the `ERR_NOT_IMPLEMENTED` arm with:

```rust
desktop_bridge::METHOD_WINDOW_LIST => action::handle_window_list(params),
desktop_bridge::METHOD_FOCUS_WINDOW => action::handle_focus_window(params),
```

**Step 4: Verify**

```bash
cargo check -p aleph-tauri
```

**Step 5: Commit**

```bash
git add apps/desktop/src-tauri/Cargo.toml apps/desktop/src-tauri/src/bridge/action.rs apps/desktop/src-tauri/src/bridge/mod.rs
git commit -m "bridge: implement window_list and focus_window (macOS)"
```

---

### Task 3: Canvas Overlay (canvas_show, canvas_hide, canvas_update)

**Files:**
- Create: `apps/desktop/src-tauri/src/bridge/canvas.rs`
- Modify: `apps/desktop/src-tauri/src/bridge/mod.rs`

**Context:** Canvas provides an HTML overlay window using Tauri's WebView. The Swift version uses NSPanel + WKWebView. In Tauri, we create a new WebView window dynamically and manage it via `get_app_handle()`. The A2UI patch protocol injects JavaScript via `window.eval()`.

**Step 1: Create `canvas.rs`**

```rust
//! Canvas overlay handlers — show, hide, and patch HTML overlays.
//!
//! Uses a Tauri WebView window named "canvas". Created dynamically on first
//! `canvas_show` call and reused for subsequent calls.

use aleph_protocol::desktop_bridge::ERR_INTERNAL;
use serde_json::{json, Value};
use tauri::Manager;

/// Handle `desktop.canvas_show` — create/show an HTML overlay window.
///
/// Params: `{ "html": "<h1>Hello</h1>", "position": { "x", "y", "width", "height" } }`
/// Returns: `{ "visible": true, "position": {...} }`
pub fn handle_canvas_show(params: Value) -> Result<Value, (i32, String)> {
    let html = params.get("html").and_then(|v| v.as_str()).unwrap_or("<html><body></body></html>");
    let pos = params.get("position");

    let x = pos.and_then(|p| p.get("x")).and_then(|v| v.as_f64()).unwrap_or(100.0);
    let y = pos.and_then(|p| p.get("y")).and_then(|v| v.as_f64()).unwrap_or(100.0);
    let width = pos.and_then(|p| p.get("width")).and_then(|v| v.as_f64()).unwrap_or(800.0);
    let height = pos.and_then(|p| p.get("height")).and_then(|v| v.as_f64()).unwrap_or(600.0);

    let app = crate::get_app_handle()
        .ok_or_else(|| (ERR_INTERNAL, "App handle not available".into()))?;

    // Reuse existing canvas window or create new one
    if let Some(window) = app.get_webview_window("canvas") {
        // Update existing window
        let _ = window.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
        let _ = window.set_size(tauri::PhysicalSize::new(width as u32, height as u32));

        // Load new HTML content via data URI
        let encoded = base64::engine::general_purpose::STANDARD.encode(html.as_bytes());
        let data_url = format!("data:text/html;base64,{}", encoded);
        if let Ok(parsed) = data_url.parse() {
            let _ = window.navigate(parsed);
        }

        let _ = window.show();

        // Inject A2UI patch handler after page load
        inject_a2ui_handler(&window);
    } else {
        // Create new canvas window
        let builder = tauri::WebviewWindowBuilder::new(
            app,
            "canvas",
            tauri::WebviewUrl::App("about:blank".into()),
        )
        .title("Aleph Canvas")
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .inner_size(width, height)
        .position(x, y)
        .visible(true);

        let window = builder.build()
            .map_err(|e| (ERR_INTERNAL, format!("Failed to create canvas window: {e}")))?;

        // Load HTML content
        let encoded = base64::engine::general_purpose::STANDARD.encode(html.as_bytes());
        let data_url = format!("data:text/html;base64,{}", encoded);
        if let Ok(parsed) = data_url.parse() {
            let _ = window.navigate(parsed);
        }

        // Inject A2UI patch handler
        inject_a2ui_handler(&window);
    }

    Ok(json!({
        "visible": true,
        "position": { "x": x, "y": y, "width": width, "height": height }
    }))
}

/// Handle `desktop.canvas_hide` — hide the canvas overlay.
///
/// Returns: `{ "visible": false }`
pub fn handle_canvas_hide(_params: Value) -> Result<Value, (i32, String)> {
    let app = crate::get_app_handle()
        .ok_or_else(|| (ERR_INTERNAL, "App handle not available".into()))?;

    if let Some(window) = app.get_webview_window("canvas") {
        let _ = window.hide();
    }

    Ok(json!({ "visible": false }))
}

/// Handle `desktop.canvas_update` — apply an A2UI patch to the canvas.
///
/// Params: `{ "patch": [{"type": "surfaceUpdate", "content": "<p>Updated</p>"}] }`
/// Returns: `{ "patched": true }`
pub fn handle_canvas_update(params: Value) -> Result<Value, (i32, String)> {
    let patch = params.get("patch")
        .ok_or((ERR_INTERNAL, "Missing 'patch' parameter".into()))?;

    let app = crate::get_app_handle()
        .ok_or_else(|| (ERR_INTERNAL, "App handle not available".into()))?;

    let window = app.get_webview_window("canvas")
        .ok_or_else(|| (ERR_INTERNAL, "Canvas not shown — call canvas_show first".into()))?;

    let patch_json = serde_json::to_string(patch)
        .map_err(|e| (ERR_INTERNAL, format!("Invalid patch JSON: {e}")))?;

    let script = format!(
        "if (typeof window.alephApplyPatch === 'function') {{ window.alephApplyPatch({}); }}",
        patch_json
    );

    // Evaluate JS (fire and forget — eval errors are non-fatal)
    let _ = window.eval(&script);

    Ok(json!({ "patched": true }))
}

/// Inject the A2UI patch handler into a window.
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
```

**Step 2: Register module and wire dispatch**

In `mod.rs`, add `mod canvas;` after `mod action;`.

In the dispatch match, replace the `METHOD_CANVAS_SHOW | METHOD_CANVAS_HIDE | METHOD_CANVAS_UPDATE` arm:

```rust
desktop_bridge::METHOD_CANVAS_SHOW => canvas::handle_canvas_show(params),
desktop_bridge::METHOD_CANVAS_HIDE => canvas::handle_canvas_hide(params),
desktop_bridge::METHOD_CANVAS_UPDATE => canvas::handle_canvas_update(params),
```

**Step 3: Verify**

```bash
cargo check -p aleph-tauri
```

**Step 4: Commit**

```bash
git add apps/desktop/src-tauri/src/bridge/canvas.rs apps/desktop/src-tauri/src/bridge/mod.rs
git commit -m "bridge: implement canvas overlay via Tauri WebView"
```

---

### Task 4: Tray Status Update

**Files:**
- Modify: `apps/desktop/src-tauri/src/bridge/mod.rs`

**Context:** The `tray.update_status` method lets the server update the tray icon tooltip and status indicator. We use `crate::get_app_handle()` to access the Tauri tray icon.

**Step 1: Add tray handler in `mod.rs`**

Add a new handler function:

```rust
/// Handle `tray.update_status` — update tray icon tooltip.
///
/// Params: `{ "status": "idle"|"thinking"|"acting"|"error", "tooltip": "optional text" }`
/// Returns: `{ "updated": true }`
fn handle_tray_update_status(params: serde_json::Value) -> Result<serde_json::Value, (i32, String)> {
    let status = params.get("status").and_then(|v| v.as_str()).unwrap_or("idle");
    let tooltip = params.get("tooltip").and_then(|v| v.as_str());

    let app = crate::get_app_handle()
        .ok_or_else(|| (ERR_INTERNAL, "App handle not available".into()))?;

    // Update tray tooltip
    if let Some(tray) = app.tray_by_id("main") {
        let tooltip_text = tooltip.unwrap_or(match status {
            "thinking" => "Aleph - Thinking...",
            "acting" => "Aleph - Acting...",
            "error" => "Aleph - Error",
            _ => "Aleph - AI Assistant",
        });
        let _ = tray.set_tooltip(Some(tooltip_text));
    }

    Ok(json!({ "updated": true, "status": status }))
}
```

**Step 2: Wire dispatch**

Replace `METHOD_TRAY_UPDATE_STATUS` from the `ERR_NOT_IMPLEMENTED` arm:

```rust
METHOD_TRAY_UPDATE_STATUS => handle_tray_update_status(params),
```

**Step 3: Verify**

```bash
cargo check -p aleph-tauri
```

**Step 4: Commit**

```bash
git add apps/desktop/src-tauri/src/bridge/mod.rs
git commit -m "bridge: implement tray.update_status"
```

---

### Task 5: OCR (macOS via Vision Framework)

**Files:**
- Modify: `apps/desktop/src-tauri/src/bridge/perception.rs`
- Modify: `apps/desktop/src-tauri/src/bridge/mod.rs`
- Modify: `apps/desktop/src-tauri/Cargo.toml`

**Context:** OCR on macOS uses the Vision framework (`VNRecognizeTextRequest`). We call it via the `objc` crate's raw message-send API. When no image is provided, we capture the screen first (reusing existing xcap logic). Other platforms return `ERR_NOT_IMPLEMENTED`.

**Step 1: Add block dependency for macOS**

The Vision framework callbacks use Objective-C blocks. Add `block` crate for macOS:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
cocoa = "0.26"
objc = "0.2"
core-graphics = "0.24"
block = "0.1"
```

**Step 2: Add OCR handler to `perception.rs`**

Add after the existing `handle_screenshot` function:

```rust
/// Handle `desktop.ocr` — extract text from screen or image.
///
/// Params: `{ "image_base64": "..." }` or `{}` (captures screen)
/// Returns: `{ "text": "...", "lines": [{ "text", "confidence", "bounds" }] }`
pub fn handle_ocr(params: Value) -> Result<Value, (i32, String)> {
    let image_base64 = params.get("image_base64").and_then(|v| v.as_str());

    let image_data: Vec<u8> = if let Some(b64) = image_base64 {
        // Decode provided image
        general_purpose::STANDARD.decode(b64)
            .map_err(|e| (ERR_INTERNAL, format!("Invalid base64: {e}")))?
    } else {
        // Capture screen and encode as PNG
        let monitors = xcap::Monitor::all()
            .map_err(|e| (ERR_INTERNAL, format!("Failed to enumerate monitors: {e}")))?;
        let monitor = monitors.into_iter()
            .find(|m| m.is_primary().unwrap_or(false))
            .ok_or_else(|| (ERR_INTERNAL, "No primary monitor found".to_string()))?;
        let image = monitor.capture_image()
            .map_err(|e| (ERR_INTERNAL, format!("Screen capture failed: {e}")))?;
        let mut buf = Cursor::new(Vec::new());
        image.write_to(&mut buf, image::ImageFormat::Png)
            .map_err(|e| (ERR_INTERNAL, format!("PNG encoding failed: {e}")))?;
        buf.into_inner()
    };

    #[cfg(target_os = "macos")]
    {
        macos_ocr(&image_data)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = image_data;
        Err((aleph_protocol::desktop_bridge::ERR_NOT_IMPLEMENTED,
             "OCR not implemented on this platform".into()))
    }
}

#[cfg(target_os = "macos")]
fn macos_ocr(png_data: &[u8]) -> Result<Value, (i32, String)> {
    use objc::runtime::{Class, Object, BOOL, YES};
    use objc::{msg_send, sel, sel_impl};
    use std::sync::{Arc, Mutex};

    unsafe {
        // Create NSData from PNG bytes
        let nsdata_cls = Class::get("NSData").unwrap();
        let nsdata: *mut Object = msg_send![nsdata_cls, dataWithBytes:png_data.as_ptr()
                                                                length:png_data.len()];

        // Create CGImage via CIImage → CGImage pipeline
        let ciimage_cls = Class::get("CIImage").unwrap();
        let ci_image: *mut Object = msg_send![ciimage_cls, imageWithData:nsdata];
        if ci_image.is_null() {
            return Err((ERR_INTERNAL, "Failed to create CIImage from data".into()));
        }

        let ci_context_cls = Class::get("CIContext").unwrap();
        let ci_context: *mut Object = msg_send![ci_context_cls, context];
        let extent: core_graphics::geometry::CGRect = msg_send![ci_image, extent];
        let cg_image: core_graphics::image::CGImageRef =
            msg_send![ci_context, createCGImage:ci_image fromRect:extent];
        if cg_image.is_null() {
            return Err((ERR_INTERNAL, "Failed to create CGImage".into()));
        }

        // Create VNImageRequestHandler
        let handler_cls = Class::get("VNImageRequestHandler").unwrap();
        let handler: *mut Object = msg_send![handler_cls, alloc];
        let nil: *mut Object = std::ptr::null_mut();
        let handler: *mut Object = msg_send![handler, initWithCGImage:cg_image options:nil];

        // Create VNRecognizeTextRequest
        let request_cls = Class::get("VNRecognizeTextRequest").unwrap();
        let request: *mut Object = msg_send![request_cls, alloc];
        let request: *mut Object = msg_send![request, init];

        // Configure: accurate recognition, language correction, multiple languages
        let recognition_level: i64 = 1; // VNRequestTextRecognitionLevelAccurate
        let _: () = msg_send![request, setRecognitionLevel:recognition_level];
        let _: () = msg_send![request, setUsesLanguageCorrection:YES];

        // Set recognition languages
        let nsstring_cls = Class::get("NSString").unwrap();
        let lang_zh: *mut Object = msg_send![nsstring_cls,
            stringWithUTF8String:"zh-Hans\0".as_ptr()];
        let lang_en: *mut Object = msg_send![nsstring_cls,
            stringWithUTF8String:"en-US\0".as_ptr()];
        let nsarray_cls = Class::get("NSArray").unwrap();
        let languages: *mut Object = msg_send![nsarray_cls,
            arrayWithObjects:lang_zh count:2usize];
        // Build array properly
        let langs_array: [*mut Object; 2] = [lang_zh, lang_en];
        let languages: *mut Object = msg_send![nsarray_cls,
            arrayWithObjects:langs_array.as_ptr() count:2usize];
        let _: () = msg_send![request, setRecognitionLanguages:languages];

        // Perform request
        let requests_array: *mut Object = msg_send![nsarray_cls,
            arrayWithObject:request];
        let mut error: *mut Object = std::ptr::null_mut();
        let success: BOOL = msg_send![handler, performRequests:requests_array error:&mut error];

        if success == objc::runtime::NO {
            let desc: *mut Object = msg_send![error, localizedDescription];
            let utf8: *const std::os::raw::c_char = msg_send![desc, UTF8String];
            let msg = if utf8.is_null() {
                "Unknown OCR error".to_string()
            } else {
                std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned()
            };
            return Err((ERR_INTERNAL, format!("OCR failed: {}", msg)));
        }

        // Extract results
        let results: *mut Object = msg_send![request, results];
        let count: usize = msg_send![results, count];

        let mut lines = Vec::new();
        let mut full_text_parts = Vec::new();

        for i in 0..count {
            let observation: *mut Object = msg_send![results, objectAtIndex:i];
            let candidates: *mut Object = msg_send![observation, topCandidates:1usize];
            let candidate_count: usize = msg_send![candidates, count];
            if candidate_count == 0 { continue; }

            let candidate: *mut Object = msg_send![candidates, objectAtIndex:0usize];
            let string: *mut Object = msg_send![candidate, string];
            let confidence: f32 = msg_send![candidate, confidence];

            let utf8: *const std::os::raw::c_char = msg_send![string, UTF8String];
            if utf8.is_null() { continue; }
            let text = std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned();

            full_text_parts.push(text.clone());
            lines.push(json!({
                "text": text,
                "confidence": confidence,
            }));
        }

        Ok(json!({
            "text": full_text_parts.join("\n"),
            "lines": lines,
        }))
    }
}
```

**Step 3: Wire dispatch**

In `mod.rs`, replace `METHOD_OCR` from the `ERR_NOT_IMPLEMENTED` arm:

```rust
desktop_bridge::METHOD_OCR => perception::handle_ocr(params),
```

**Step 4: Verify**

```bash
cargo check -p aleph-tauri
```

**Step 5: Commit**

```bash
git add apps/desktop/src-tauri/Cargo.toml apps/desktop/src-tauri/src/bridge/perception.rs apps/desktop/src-tauri/src/bridge/mod.rs
git commit -m "bridge: implement OCR via Vision framework (macOS)"
```

---

### Task 6: AX Tree (macOS via Accessibility API)

**Files:**
- Modify: `apps/desktop/src-tauri/src/bridge/perception.rs`
- Modify: `apps/desktop/src-tauri/src/bridge/mod.rs`

**Context:** The Accessibility API (AXUIElement) on macOS provides UI element tree inspection. We use `objc` to call `AXUIElementCreateApplication`, then recursively walk the tree. Requires Accessibility permission (System Preferences → Privacy → Accessibility). Other platforms return `ERR_NOT_IMPLEMENTED`.

**Step 1: Add AX tree handler to `perception.rs`**

```rust
/// Handle `desktop.ax_tree` — inspect accessibility tree.
///
/// Params: `{ "app_bundle_id": "com.apple.Safari" }` or `{}` (frontmost app)
/// Returns: `{ "role", "title", "value", "frame", "children": [...] }`
pub fn handle_ax_tree(params: Value) -> Result<Value, (i32, String)> {
    let app_bundle_id = params.get("app_bundle_id").and_then(|v| v.as_str());

    #[cfg(target_os = "macos")]
    {
        macos_ax_tree(app_bundle_id)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = app_bundle_id;
        Err((aleph_protocol::desktop_bridge::ERR_NOT_IMPLEMENTED,
             "ax_tree not implemented on this platform".into()))
    }
}

#[cfg(target_os = "macos")]
fn macos_ax_tree(app_bundle_id: Option<&str>) -> Result<Value, (i32, String)> {
    use objc::runtime::{Class, Object};
    use objc::{msg_send, sel, sel_impl};
    use core_graphics::geometry::{CGPoint, CGSize};
    use std::os::raw::c_void;

    unsafe {
        let pid: i32 = if let Some(bundle_id) = app_bundle_id {
            // Find running app by bundle ID
            let nsstring_cls = Class::get("NSString").unwrap();
            let bundle_str: *mut Object = msg_send![nsstring_cls,
                stringWithUTF8String:format!("{}\0", bundle_id).as_ptr()];

            let workspace_cls = Class::get("NSWorkspace").unwrap();
            let workspace: *mut Object = msg_send![workspace_cls, sharedWorkspace];
            let running_apps: *mut Object = msg_send![workspace, runningApplications];
            let count: usize = msg_send![running_apps, count];

            let mut found_pid: Option<i32> = None;
            for i in 0..count {
                let app: *mut Object = msg_send![running_apps, objectAtIndex:i];
                let bid: *mut Object = msg_send![app, bundleIdentifier];
                if bid.is_null() { continue; }
                let equal: bool = msg_send![bid, isEqualToString:bundle_str];
                if equal {
                    let p: i32 = msg_send![app, processIdentifier];
                    found_pid = Some(p);
                    break;
                }
            }

            found_pid.ok_or((ERR_INTERNAL, format!("App not running: {}", bundle_id)))?
        } else {
            // Use frontmost application
            let workspace_cls = Class::get("NSWorkspace").unwrap();
            let workspace: *mut Object = msg_send![workspace_cls, sharedWorkspace];
            let front_app: *mut Object = msg_send![workspace, frontmostApplication];
            if front_app.is_null() {
                return Err((ERR_INTERNAL, "No frontmost application".into()));
            }
            msg_send![front_app, processIdentifier]
        };

        // Create AXUIElement for the application
        let ax_app = AXUIElementCreateApplication(pid);
        let tree = ax_element_to_json(ax_app, 0, 5);

        Ok(tree)
    }
}

#[cfg(target_os = "macos")]
unsafe fn ax_element_to_json(element: *const c_void, depth: usize, max_depth: usize) -> Value {
    use std::os::raw::c_void;

    if depth >= max_depth {
        return json!({ "truncated": true });
    }

    let mut result = serde_json::Map::new();

    // Get role
    if let Some(role) = ax_get_string_attr(element, kAXRoleAttribute) {
        result.insert("role".into(), json!(role));
    } else {
        result.insert("role".into(), json!("unknown"));
    }

    // Get title
    if let Some(title) = ax_get_string_attr(element, kAXTitleAttribute) {
        if !title.is_empty() {
            result.insert("title".into(), json!(title));
        }
    }

    // Get value
    if let Some(value) = ax_get_string_attr(element, kAXValueAttribute) {
        if !value.is_empty() {
            result.insert("value".into(), json!(value));
        }
    }

    // Get children
    let mut children_ref: *const c_void = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(
        element,
        cfstring(kAXChildrenAttribute),
        &mut children_ref,
    );
    if err == 0 && !children_ref.is_null() {
        let count: isize = objc::msg_send![children_ref as *mut objc::runtime::Object, count];
        let mut children = Vec::new();
        for i in 0..count {
            let child: *const c_void =
                objc::msg_send![children_ref as *mut objc::runtime::Object, objectAtIndex:i];
            children.push(ax_element_to_json(child, depth + 1, max_depth));
        }
        if !children.is_empty() {
            result.insert("children".into(), json!(children));
        }
        CFRelease(children_ref);
    }

    Value::Object(result)
}

#[cfg(target_os = "macos")]
unsafe fn ax_get_string_attr(element: *const std::os::raw::c_void, attr: &str) -> Option<String> {
    use std::os::raw::c_void;

    let mut value_ref: *const c_void = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, cfstring(attr), &mut value_ref);
    if err != 0 || value_ref.is_null() {
        return None;
    }

    // Check if it's an NSString
    let obj = value_ref as *mut objc::runtime::Object;
    let is_string: bool = objc::msg_send![obj, isKindOfClass:objc::runtime::Class::get("NSString").unwrap()];
    if is_string {
        let utf8: *const std::os::raw::c_char = objc::msg_send![obj, UTF8String];
        let result = if utf8.is_null() {
            None
        } else {
            Some(std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned())
        };
        CFRelease(value_ref);
        result
    } else {
        CFRelease(value_ref);
        None
    }
}

#[cfg(target_os = "macos")]
unsafe fn cfstring(s: &str) -> *const std::os::raw::c_void {
    let nsstring_cls = objc::runtime::Class::get("NSString").unwrap();
    let cstr = std::ffi::CString::new(s).unwrap();
    objc::msg_send![nsstring_cls, stringWithUTF8String:cstr.as_ptr()]
}

// macOS Accessibility extern declarations
#[cfg(target_os = "macos")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> *const std::os::raw::c_void;
    fn AXUIElementCopyAttributeValue(
        element: *const std::os::raw::c_void,
        attribute: *const std::os::raw::c_void,
        value: *mut *const std::os::raw::c_void,
    ) -> i32;
    fn CFRelease(cf: *const std::os::raw::c_void);
}

#[cfg(target_os = "macos")]
const kAXRoleAttribute: &str = "AXRole";
#[cfg(target_os = "macos")]
const kAXTitleAttribute: &str = "AXTitle";
#[cfg(target_os = "macos")]
const kAXValueAttribute: &str = "AXValue";
#[cfg(target_os = "macos")]
const kAXChildrenAttribute: &str = "AXChildren";
```

**Step 2: Wire dispatch**

In `mod.rs`, replace `METHOD_AX_TREE` from the `ERR_NOT_IMPLEMENTED` arm:

```rust
desktop_bridge::METHOD_AX_TREE => perception::handle_ax_tree(params),
```

**Step 3: Verify**

```bash
cargo check -p aleph-tauri
```

**Step 4: Commit**

```bash
git add apps/desktop/src-tauri/src/bridge/perception.rs apps/desktop/src-tauri/src/bridge/mod.rs
git commit -m "bridge: implement AX tree inspection via Accessibility API (macOS)"
```

---

### Task 7: Update Capabilities and Verify End-to-End

**Files:**
- Modify: `apps/desktop/src-tauri/src/bridge/mod.rs`

**Context:** Now that all handlers are wired, the `handle_handshake` should return a more complete capability list reflecting what's actually supported per platform. Also verify the entire dispatch has no remaining `ERR_NOT_IMPLEMENTED` arms (all methods should have real handlers now).

**Step 1: Update handshake capabilities**

In `handle_handshake()`, update the capabilities array to reflect per-platform support:

```rust
fn handle_handshake(params: serde_json::Value) -> Result<serde_json::Value, (i32, String)> {
    let protocol_version = params
        .get("protocol_version")
        .and_then(|v| v.as_str())
        .unwrap_or("1.0");

    tracing::info!(protocol_version, "Handshake received from server");

    let mut capabilities = vec![
        json!({"name": "screen_capture", "version": "1.0"}),
        json!({"name": "webview", "version": "1.0"}),
        json!({"name": "tray", "version": "1.0"}),
        json!({"name": "global_hotkey", "version": "1.0"}),
        json!({"name": "notification", "version": "1.0"}),
        json!({"name": "keyboard_control", "version": "1.0"}),
        json!({"name": "mouse_control", "version": "1.0"}),
        json!({"name": "canvas", "version": "1.0"}),
        json!({"name": "launch_app", "version": "1.0"}),
    ];

    // Platform-specific capabilities
    #[cfg(target_os = "macos")]
    {
        capabilities.push(json!({"name": "ocr", "version": "1.0"}));
        capabilities.push(json!({"name": "ax_inspect", "version": "1.0"}));
        capabilities.push(json!({"name": "window_list", "version": "1.0"}));
    }

    Ok(json!({
        "protocol_version": protocol_version,
        "bridge_type": "desktop",
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "capabilities": capabilities
    }))
}
```

**Step 2: Verify dispatch is complete**

The `ERR_NOT_IMPLEMENTED` arm should now be empty. Remove it entirely or change it to only the pattern `_` catch-all for truly unknown methods. Verify that the dispatch match covers all `desktop_bridge::METHOD_*` constants.

**Step 3: Build server**

```bash
cargo check --bin aleph-server --features control-plane
```

**Step 4: Build bridge**

```bash
cargo check -p aleph-tauri
```

**Step 5: Run tests**

```bash
cargo test --lib -p alephcore -- desktop
cargo test --lib -p alephcore -- gateway::bridge
```

**Step 6: Commit**

```bash
git add apps/desktop/src-tauri/src/bridge/mod.rs
git commit -m "bridge: update handshake capabilities for Phase 3 completion"
```

---

## Summary of Changes

| File | Action | Purpose |
|------|--------|---------|
| `apps/desktop/src-tauri/Cargo.toml` | Modify | Add enigo, core-graphics, block deps |
| `apps/desktop/src-tauri/src/bridge/action.rs` | Create | click, type_text, key_combo, launch_app, window_list, focus_window |
| `apps/desktop/src-tauri/src/bridge/canvas.rs` | Create | canvas_show, canvas_hide, canvas_update via Tauri WebView |
| `apps/desktop/src-tauri/src/bridge/perception.rs` | Modify | Add OCR (Vision) and AX tree handlers |
| `apps/desktop/src-tauri/src/bridge/mod.rs` | Modify | Wire all handlers, update capabilities, update tray handler |

## Dependencies Added

| Crate | Version | Platform | Purpose |
|-------|---------|----------|---------|
| `enigo` | 0.3 | All | Mouse/keyboard simulation |
| `core-graphics` | 0.24 | macOS | CGWindowListCopyWindowInfo |
| `block` | 0.1 | macOS | Objective-C block support |

## Platform Support Matrix

| Capability | macOS | Windows | Linux |
|-----------|-------|---------|-------|
| screenshot | xcap | xcap | xcap |
| click/type/keys | enigo | enigo | enigo |
| launch_app | `open -b` | `cmd /C start` | `xdg-open` |
| canvas | Tauri | Tauri | Tauri |
| tray | Tauri | Tauri | Tauri |
| ocr | Vision | stub | stub |
| ax_tree | AX API | stub | stub |
| window_list | CG API | stub | stub |
| focus_window | NSApp | stub | stub |
