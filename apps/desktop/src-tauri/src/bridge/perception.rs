//! Perception handlers — screen capture, OCR, accessibility tree.
//!
//! `desktop.screenshot`, `desktop.ocr`, and `desktop.ax_tree` are implemented.
//!
//! - **Windows**: OCR via WinRT `OcrEngine`; AX tree via UI Automation (`IUIAutomation`).
//! - **Other platforms**: OCR and AX tree return `ERR_NOT_IMPLEMENTED`.
//!
//! Note: macOS is handled by the native Swift app (`apps/macos-native/`).

use aleph_protocol::desktop_bridge::ERR_INTERNAL;
use base64::{engine::general_purpose, Engine as _};
use serde_json::{json, Value};
use std::io::Cursor;

/// Check if Screen Recording permission is granted.
/// Returns `true` on non-macOS platforms (no permission needed).
fn screen_recording_granted() -> bool {
    true
}

struct CaptureRegion {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// Handle `desktop.screenshot` — capture primary monitor (or a region) as PNG.
///
/// Params: `{ "region": { "x", "y", "width", "height" } | null }`
/// Returns: `{ "image_base64", "width", "height", "format": "png" }`
pub fn handle_screenshot(params: Value) -> Result<Value, (i32, String)> {
    if !screen_recording_granted() {
        return Err((ERR_INTERNAL,
            "Screen Recording permission not granted. \
             Enable in System Settings > Privacy & Security > Screen Recording.".into()));
    }

    let region = parse_region(&params);

    let monitors = xcap::Monitor::all()
        .map_err(|e| (ERR_INTERNAL, format!("Failed to enumerate monitors: {e}")))?;
    let monitor = monitors
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .ok_or_else(|| (ERR_INTERNAL, "No primary monitor found".to_string()))?;

    let image = match region {
        Some(r) => monitor.capture_region(r.x, r.y, r.width, r.height),
        None => monitor.capture_image(),
    }
    .map_err(|e| (ERR_INTERNAL, format!("Screen capture failed: {e}")))?;

    let (width, height) = (image.width(), image.height());

    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| (ERR_INTERNAL, format!("PNG encoding failed: {e}")))?;

    let image_base64 = general_purpose::STANDARD.encode(buf.into_inner());

    Ok(json!({
        "image_base64": image_base64,
        "width": width,
        "height": height,
        "format": "png"
    }))
}

fn parse_region(params: &Value) -> Option<CaptureRegion> {
    let region = params.get("region")?;
    if region.is_null() {
        return None;
    }
    Some(CaptureRegion {
        x: region.get("x")?.as_f64()? as u32,
        y: region.get("y")?.as_f64()? as u32,
        width: region.get("width")?.as_f64()? as u32,
        height: region.get("height")?.as_f64()? as u32,
    })
}

// ── OCR handler ──────────────────────────────────────────────────

/// Handle `desktop.ocr` — extract text from screen or provided image.
///
/// Params: `{ "image_base64": "..." }` or `{}` (captures screen first)
/// Returns: `{ "text": "full text", "lines": [{ "text", "confidence" }] }`
pub fn handle_ocr(params: Value) -> Result<Value, (i32, String)> {
    // Step 1: Obtain PNG bytes — either from provided base64 or by capturing the screen.
    let png_bytes = if let Some(b64) = params.get("image_base64").and_then(|v| v.as_str()) {
        general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| (ERR_INTERNAL, format!("Invalid base64: {e}")))?
    } else {
        capture_screen_png()?
    };

    // Step 2: Platform-specific OCR
    #[cfg(target_os = "windows")]
    {
        windows_ocr(&png_bytes)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = png_bytes;
        Err((
            aleph_protocol::desktop_bridge::ERR_NOT_IMPLEMENTED,
            "OCR not implemented on this platform".to_string(),
        ))
    }
}

/// Capture the primary monitor as PNG bytes (reuses screenshot logic).
fn capture_screen_png() -> Result<Vec<u8>, (i32, String)> {
    if !screen_recording_granted() {
        return Err((ERR_INTERNAL,
            "Screen Recording permission not granted. \
             Enable in System Settings > Privacy & Security > Screen Recording.".into()));
    }

    let monitors = xcap::Monitor::all()
        .map_err(|e| (ERR_INTERNAL, format!("Failed to enumerate monitors: {e}")))?;
    let monitor = monitors
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .ok_or_else(|| (ERR_INTERNAL, "No primary monitor found".to_string()))?;

    let image = monitor
        .capture_image()
        .map_err(|e| (ERR_INTERNAL, format!("Screen capture failed: {e}")))?;

    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| (ERR_INTERNAL, format!("PNG encoding failed: {e}")))?;

    Ok(buf.into_inner())
}

// ── Windows WinRT OCR ───────────────────────────────────────────

/// Perform OCR using the Windows WinRT `OcrEngine` API.
///
/// Steps:
/// 1. Decode PNG bytes into a `SoftwareBitmap` via `BitmapDecoder`.
/// 2. Create an `OcrEngine` (prefer zh-Hans, fallback to en, then user default).
/// 3. Call `RecognizeAsync` to extract text and line bounding boxes.
#[cfg(target_os = "windows")]
fn windows_ocr(png_bytes: &[u8]) -> Result<Value, (i32, String)> {
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapDecoder, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::{
        DataWriter, InMemoryRandomAccessStream, IRandomAccessStream,
    };

    // 1. Write PNG bytes into an IRandomAccessStream via DataWriter.
    let stream = InMemoryRandomAccessStream::new()
        .map_err(|e| (ERR_INTERNAL, format!("Failed to create memory stream: {e}")))?;

    let writer = DataWriter::CreateDataWriter(
        &stream.cast::<windows::Storage::Streams::IOutputStream>()
            .map_err(|e| (ERR_INTERNAL, format!("Stream cast failed: {e}")))?,
    )
    .map_err(|e| (ERR_INTERNAL, format!("Failed to create DataWriter: {e}")))?;

    writer
        .WriteBytes(png_bytes)
        .map_err(|e| (ERR_INTERNAL, format!("WriteBytes failed: {e}")))?;
    writer
        .StoreAsync()
        .map_err(|e| (ERR_INTERNAL, format!("StoreAsync failed: {e}")))?
        .get()
        .map_err(|e| (ERR_INTERNAL, format!("StoreAsync.get failed: {e}")))?;
    writer
        .FlushAsync()
        .map_err(|e| (ERR_INTERNAL, format!("FlushAsync failed: {e}")))?
        .get()
        .map_err(|e| (ERR_INTERNAL, format!("FlushAsync.get failed: {e}")))?;

    // Seek to beginning before decoding.
    stream
        .Seek(0)
        .map_err(|e| (ERR_INTERNAL, format!("Seek failed: {e}")))?;

    // 2. Decode the PNG into a SoftwareBitmap.
    let decoder = BitmapDecoder::CreateAsync(
        &stream.cast::<IRandomAccessStream>()
            .map_err(|e| (ERR_INTERNAL, format!("Stream cast to IRandomAccessStream failed: {e}")))?,
    )
    .map_err(|e| (ERR_INTERNAL, format!("BitmapDecoder::CreateAsync failed: {e}")))?
    .get()
    .map_err(|e| (ERR_INTERNAL, format!("BitmapDecoder async get failed: {e}")))?;

    let bitmap: SoftwareBitmap = decoder
        .GetSoftwareBitmapAsync()
        .map_err(|e| (ERR_INTERNAL, format!("GetSoftwareBitmapAsync failed: {e}")))?
        .get()
        .map_err(|e| (ERR_INTERNAL, format!("SoftwareBitmap async get failed: {e}")))?;

    // 3. Create OcrEngine — prefer zh-Hans, fallback to en-US, then user default.
    let engine = {
        let zh = Language::CreateLanguage(&windows::core::HSTRING::from("zh-Hans")).ok();
        let en = Language::CreateLanguage(&windows::core::HSTRING::from("en-US")).ok();

        let try_create = |lang: &Language| -> Option<OcrEngine> {
            if OcrEngine::IsLanguageSupported(lang).unwrap_or(false) {
                OcrEngine::TryCreateFromLanguage(lang).ok()
            } else {
                None
            }
        };

        zh.as_ref()
            .and_then(try_create)
            .or_else(|| en.as_ref().and_then(try_create))
            .or_else(|| OcrEngine::TryCreateFromUserProfileLanguages().ok())
            .ok_or_else(|| {
                (ERR_INTERNAL, "No OCR language available on this system".to_string())
            })?
    };

    // 4. Recognize text.
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| (ERR_INTERNAL, format!("RecognizeAsync failed: {e}")))?
        .get()
        .map_err(|e| (ERR_INTERNAL, format!("OCR result async get failed: {e}")))?;

    let full_text = result
        .Text()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();

    // 5. Build lines array with bounding boxes.
    let ocr_lines = result
        .Lines()
        .map_err(|e| (ERR_INTERNAL, format!("Failed to get OCR lines: {e}")))?;

    let mut lines: Vec<Value> = Vec::new();
    for line in &ocr_lines {
        let text = line
            .Text()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();

        // Merge bounding boxes of all words in this line.
        let words = line.Words()
            .map_err(|e| (ERR_INTERNAL, format!("Failed to get words: {e}")))?;

        let mut min_x: f64 = f64::MAX;
        let mut min_y: f64 = f64::MAX;
        let mut max_x: f64 = f64::MIN;
        let mut max_y: f64 = f64::MIN;
        let mut has_bounds = false;

        for word in &words {
            if let Ok(rect) = word.BoundingRect() {
                has_bounds = true;
                min_x = min_x.min(rect.X as f64);
                min_y = min_y.min(rect.Y as f64);
                max_x = max_x.max((rect.X + rect.Width) as f64);
                max_y = max_y.max((rect.Y + rect.Height) as f64);
            }
        }

        let mut line_json = json!({ "text": text });
        if has_bounds {
            line_json["bounding_box"] = json!({
                "x": min_x,
                "y": min_y,
                "w": max_x - min_x,
                "h": max_y - min_y,
            });
        }
        lines.push(line_json);
    }

    Ok(json!({
        "full_text": full_text,
        "lines": lines,
    }))
}

// ── AX Tree handler ─────────────────────────────────────────────

/// Handle `desktop.ax_tree` — inspect accessibility tree of an application.
///
/// Params: `{ "app_bundle_id": "com.apple.Safari" }` or `{}` (frontmost app)
/// Returns: `{ "role": "AXApplication", "title": "...", "children": [...] }`
pub fn handle_ax_tree(params: Value) -> Result<Value, (i32, String)> {
    #[cfg(target_os = "windows")]
    {
        windows_ax_tree(&params)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = params;
        Err((
            aleph_protocol::desktop_bridge::ERR_NOT_IMPLEMENTED,
            "AX tree inspection not implemented on this platform".to_string(),
        ))
    }
}

// ── Windows UI Automation (AX tree) ─────────────────────────────

/// Default maximum tree depth for Windows UI Automation walk.
#[cfg(target_os = "windows")]
const WIN_AX_MAX_DEPTH: usize = 5;

/// Inspect the UI Automation accessibility tree on Windows.
///
/// Params:
/// - `hwnd` (optional): Window handle to inspect. If omitted, uses the focused element.
/// - `max_depth` (optional): Maximum recursion depth (default 5).
///
/// Returns a JSON tree: `{ "role", "name", "bounds": { "x","y","w","h" }, "children": [...] }`
#[cfg(target_os = "windows")]
fn windows_ax_tree(params: &Value) -> Result<Value, (i32, String)> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize,
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation8, IUIAutomation, IUIAutomationElement,
        IUIAutomationTreeWalker,
    };

    let max_depth = params
        .get("max_depth")
        .and_then(|v| v.as_u64())
        .map(|d| d as usize)
        .unwrap_or(WIN_AX_MAX_DEPTH);

    // Initialize COM (ignore already-initialized errors).
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    // Create the IUIAutomation instance.
    let automation: IUIAutomation = unsafe {
        CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| (ERR_INTERNAL, format!("CoCreateInstance(CUIAutomation8) failed: {e}")))?
    };

    // Obtain root element: use hwnd if provided, otherwise focused element.
    let root: IUIAutomationElement = if let Some(hwnd_val) = params.get("hwnd") {
        let hwnd_raw = hwnd_val.as_i64().ok_or_else(|| {
            (ERR_INTERNAL, "hwnd must be an integer".to_string())
        })?;

        use windows::Win32::Foundation::HWND;
        let hwnd = HWND(hwnd_raw as *mut _);
        unsafe {
            automation.ElementFromHandle(hwnd)
                .map_err(|e| (ERR_INTERNAL, format!("ElementFromHandle failed: {e}")))?
        }
    } else {
        unsafe {
            automation.GetFocusedElement()
                .map_err(|e| (ERR_INTERNAL, format!("GetFocusedElement failed: {e}")))?
        }
    };

    // Create a tree walker for the raw view (includes all elements).
    let raw_view_condition = unsafe {
        automation.RawViewCondition()
            .map_err(|e| (ERR_INTERNAL, format!("RawViewCondition failed: {e}")))?
    };
    let walker = unsafe {
        automation.CreateTreeWalker(&raw_view_condition)
            .map_err(|e| (ERR_INTERNAL, format!("CreateTreeWalker failed: {e}")))?
    };

    // Recursively build the JSON tree.
    let tree = win_uia_element_to_json(&walker, &root, 0, max_depth);

    unsafe { CoUninitialize() };

    Ok(tree)
}

/// Recursively convert a UI Automation element into a JSON tree.
#[cfg(target_os = "windows")]
fn win_uia_element_to_json(
    walker: &windows::Win32::UI::Accessibility::IUIAutomationTreeWalker,
    element: &windows::Win32::UI::Accessibility::IUIAutomationElement,
    depth: usize,
    max_depth: usize,
) -> Value {
    if depth >= max_depth {
        return json!({"truncated": true});
    }

    let mut node = serde_json::Map::new();

    // Control type (numeric ID mapped to human-readable name).
    if let Ok(ct) = unsafe { element.CurrentControlType() } {
        node.insert("role".into(), json!(win_control_type_name(ct.0)));
    }

    // Name
    if let Ok(name) = unsafe { element.CurrentName() } {
        let s = name.to_string();
        if !s.is_empty() {
            node.insert("name".into(), json!(s));
        }
    }

    // Bounding rectangle
    if let Ok(rect) = unsafe { element.CurrentBoundingRectangle() } {
        node.insert("bounds".into(), json!({
            "x": rect.left as f64,
            "y": rect.top as f64,
            "w": (rect.right - rect.left) as f64,
            "h": (rect.bottom - rect.top) as f64,
        }));
    }

    // Children
    let mut children: Vec<Value> = Vec::new();
    if let Ok(first_child) = unsafe { walker.GetFirstChildElement(element) } {
        win_walk_siblings(walker, &first_child, depth + 1, max_depth, &mut children);
    }
    if !children.is_empty() {
        node.insert("children".into(), json!(children));
    }

    Value::Object(node)
}

/// Walk sibling elements and collect into the children vector.
#[cfg(target_os = "windows")]
fn win_walk_siblings(
    walker: &windows::Win32::UI::Accessibility::IUIAutomationTreeWalker,
    element: &windows::Win32::UI::Accessibility::IUIAutomationElement,
    depth: usize,
    max_depth: usize,
    out: &mut Vec<Value>,
) {
    // Guard: cap total children to prevent runaway traversal.
    const MAX_SIBLINGS: usize = 128;

    let mut current = element.clone();
    for _ in 0..MAX_SIBLINGS {
        out.push(win_uia_element_to_json(walker, &current, depth, max_depth));
        match unsafe { walker.GetNextSiblingElement(&current) } {
            Ok(next) => current = next,
            Err(_) => break,
        }
    }
}

/// Map a UIA_ControlTypeId to a human-readable role name.
#[cfg(target_os = "windows")]
fn win_control_type_name(id: i32) -> &'static str {
    // UIA Control Type IDs (from UIAutomationClient.h)
    match id {
        50000 => "Button",
        50001 => "Calendar",
        50002 => "CheckBox",
        50003 => "ComboBox",
        50004 => "Edit",
        50005 => "Hyperlink",
        50006 => "Image",
        50007 => "ListItem",
        50008 => "List",
        50009 => "Menu",
        50010 => "MenuBar",
        50011 => "MenuItem",
        50012 => "ProgressBar",
        50013 => "RadioButton",
        50014 => "ScrollBar",
        50015 => "Slider",
        50016 => "Spinner",
        50017 => "StatusBar",
        50018 => "Tab",
        50019 => "TabItem",
        50020 => "Text",
        50021 => "ToolBar",
        50022 => "ToolTip",
        50023 => "Tree",
        50024 => "TreeItem",
        50025 => "Custom",
        50026 => "Group",
        50027 => "Thumb",
        50028 => "DataGrid",
        50029 => "DataItem",
        50030 => "Document",
        50031 => "SplitButton",
        50032 => "Window",
        50033 => "Pane",
        50034 => "Header",
        50035 => "HeaderItem",
        50036 => "Table",
        50037 => "TitleBar",
        50038 => "Separator",
        50039 => "SemanticZoom",
        50040 => "AppBar",
        _ => "Unknown",
    }
}
