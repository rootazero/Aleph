//! Perception handlers — screen capture, OCR, accessibility tree.
//!
//! `desktop.screenshot` and `desktop.ocr` are implemented.
//! OCR uses the macOS Vision framework via `objc` message sends.
//! On non-macOS platforms, OCR returns `ERR_NOT_IMPLEMENTED`.

use aleph_protocol::desktop_bridge::ERR_INTERNAL;
use base64::{engine::general_purpose, Engine as _};
use serde_json::{json, Value};
use std::io::Cursor;

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
    #[cfg(target_os = "macos")]
    {
        macos_ocr(&png_bytes)
    }

    #[cfg(not(target_os = "macos"))]
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

// ── macOS Vision framework OCR ──────────────────────────────────

/// RAII guard for CoreFoundation objects that follow the Create Rule.
/// Ensures CFRelease is called even on early-return error paths.
#[cfg(target_os = "macos")]
struct CfGuard(*mut std::os::raw::c_void);

#[cfg(target_os = "macos")]
impl Drop for CfGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                core_foundation::base::CFRelease(self.0 as *const _);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_ocr(png_bytes: &[u8]) -> Result<Value, (i32, String)> {
    use objc::runtime::{Class, Object, BOOL, YES};
    use std::ffi::CStr;
    use std::os::raw::c_char;

    unsafe {
        // ── 1. NSData from raw bytes ────────────────────────────
        let nsdata_cls = Class::get("NSData")
            .ok_or_else(|| (ERR_INTERNAL, "NSData class not found".into()))?;
        let nsdata: *mut Object = msg_send![
            nsdata_cls,
            dataWithBytes: png_bytes.as_ptr()
            length: png_bytes.len()
        ];
        if nsdata.is_null() {
            return Err((ERR_INTERNAL, "Failed to create NSData from bytes".into()));
        }

        // ── 2. CIImage from NSData ─────────────────────────────
        let ciimage_cls = Class::get("CIImage")
            .ok_or_else(|| (ERR_INTERNAL, "CIImage class not found".into()))?;
        let ci_image: *mut Object = msg_send![ciimage_cls, imageWithData: nsdata];
        if ci_image.is_null() {
            return Err((ERR_INTERNAL, "Failed to create CIImage from data".into()));
        }

        // ── 3. CGImage via CIContext ────────────────────────────
        let cicontext_cls = Class::get("CIContext")
            .ok_or_else(|| (ERR_INTERNAL, "CIContext class not found".into()))?;
        let context: *mut Object = msg_send![cicontext_cls, context];
        if context.is_null() {
            return Err((ERR_INTERNAL, "Failed to create CIContext".into()));
        }

        let extent: core_graphics::geometry::CGRect = msg_send![ci_image, extent];
        let cg_image: *mut std::os::raw::c_void =
            msg_send![context, createCGImage: ci_image fromRect: extent];
        if cg_image.is_null() {
            return Err((ERR_INTERNAL, "Failed to create CGImage from CIImage".into()));
        }
        // RAII guard: ensures CFRelease on all exit paths (Create Rule)
        let _cg_guard = CfGuard(cg_image);

        // ── 4. VNRecognizeTextRequest ───────────────────────────
        let vnreq_cls = Class::get("VNRecognizeTextRequest")
            .ok_or_else(|| (ERR_INTERNAL, "VNRecognizeTextRequest class not found".into()))?;
        let request: *mut Object = msg_send![vnreq_cls, alloc];
        let request: *mut Object = msg_send![request, init];
        if request.is_null() {
            return Err((ERR_INTERNAL, "Failed to create VNRecognizeTextRequest".into()));
        }

        // recognitionLevel = .accurate (1)
        let _: () = msg_send![request, setRecognitionLevel: 1i64];
        // usesLanguageCorrection = true
        let _: () = msg_send![request, setUsesLanguageCorrection: YES];

        // recognitionLanguages = ["zh-Hans", "en-US"]
        let nsstring_cls = Class::get("NSString")
            .ok_or_else(|| (ERR_INTERNAL, "NSString class not found".into()))?;
        let lang_zh: *mut Object = nsstring_from_str(nsstring_cls, "zh-Hans");
        let lang_en: *mut Object = nsstring_from_str(nsstring_cls, "en-US");

        let nsarray_cls = Class::get("NSArray")
            .ok_or_else(|| (ERR_INTERNAL, "NSArray class not found".into()))?;

        // Build a 2-element array via arrayWithObjects:count:
        let lang_objects: [*mut Object; 2] = [lang_zh, lang_en];
        let languages: *mut Object = msg_send![
            nsarray_cls,
            arrayWithObjects: lang_objects.as_ptr()
            count: 2usize
        ];
        let _: () = msg_send![request, setRecognitionLanguages: languages];

        // ── 5. VNImageRequestHandler + perform ──────────────────
        let vnhandler_cls = Class::get("VNImageRequestHandler")
            .ok_or_else(|| (ERR_INTERNAL, "VNImageRequestHandler class not found".into()))?;
        let handler: *mut Object = msg_send![vnhandler_cls, alloc];
        let nil: *mut Object = std::ptr::null_mut();
        let handler: *mut Object =
            msg_send![handler, initWithCGImage: cg_image options: nil];
        if handler.is_null() {
            return Err((
                ERR_INTERNAL,
                "Failed to create VNImageRequestHandler".into(),
            ));
        }

        let requests: *mut Object = msg_send![nsarray_cls, arrayWithObject: request];
        let mut error: *mut Object = std::ptr::null_mut();
        let success: BOOL = msg_send![handler, performRequests: requests error: &mut error];

        if success != YES {
            let err_msg = if !error.is_null() {
                let desc: *mut Object = msg_send![error, localizedDescription];
                if !desc.is_null() {
                    let cstr: *const c_char = msg_send![desc, UTF8String];
                    if !cstr.is_null() {
                        CStr::from_ptr(cstr).to_string_lossy().into_owned()
                    } else {
                        "Unknown Vision error".to_string()
                    }
                } else {
                    "Unknown Vision error".to_string()
                }
            } else {
                "performRequests failed without error object".to_string()
            };
            return Err((ERR_INTERNAL, format!("Vision OCR failed: {err_msg}")));
        }

        // ── 6. Extract results ──────────────────────────────────
        let results: *mut Object = msg_send![request, results];
        if results.is_null() {
            return Ok(json!({ "text": "", "lines": [] }));
        }
        let count: usize = msg_send![results, count];

        let mut full_text = String::new();
        let mut lines: Vec<Value> = Vec::with_capacity(count);

        for i in 0..count {
            let obs: *mut Object = msg_send![results, objectAtIndex: i];
            let candidates: *mut Object = msg_send![obs, topCandidates: 1usize];
            let cand_count: usize = msg_send![candidates, count];
            if cand_count == 0 {
                continue;
            }
            let candidate: *mut Object = msg_send![candidates, objectAtIndex: 0usize];
            let string_obj: *mut Object = msg_send![candidate, string];
            let confidence: f32 = msg_send![candidate, confidence];

            if string_obj.is_null() {
                continue;
            }

            let cstr: *const c_char = msg_send![string_obj, UTF8String];
            if cstr.is_null() {
                continue;
            }
            let text = CStr::from_ptr(cstr).to_string_lossy().into_owned();

            if !full_text.is_empty() {
                full_text.push('\n');
            }
            full_text.push_str(&text);

            lines.push(json!({
                "text": text,
                "confidence": confidence,
            }));
        }

        // CGImage release is handled by _cg_guard (RAII / Drop)

        Ok(json!({
            "text": full_text,
            "lines": lines,
        }))
    }
}

/// Helper: create an NSString from a Rust &str via UTF-8.
#[cfg(target_os = "macos")]
unsafe fn nsstring_from_str(nsstring_cls: &objc::runtime::Class, s: &str) -> *mut objc::runtime::Object {
    use std::ffi::CString;
    let cstr = CString::new(s).expect("NSString source must not contain NUL bytes");
    msg_send![nsstring_cls, stringWithUTF8String: cstr.as_ptr()]
}

// ── AX Tree handler ─────────────────────────────────────────────

/// Handle `desktop.ax_tree` — inspect accessibility tree of an application.
///
/// Params: `{ "app_bundle_id": "com.apple.Safari" }` or `{}` (frontmost app)
/// Returns: `{ "role": "AXApplication", "title": "...", "children": [...] }`
pub fn handle_ax_tree(params: Value) -> Result<Value, (i32, String)> {
    #[cfg(target_os = "macos")]
    {
        macos_ax_tree(&params)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = params;
        Err((
            aleph_protocol::desktop_bridge::ERR_NOT_IMPLEMENTED,
            "AX tree inspection not implemented on this platform".to_string(),
        ))
    }
}

// ── macOS Accessibility API ─────────────────────────────────────

#[cfg(target_os = "macos")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> *const std::os::raw::c_void;
    fn AXUIElementCopyAttributeValue(
        element: *const std::os::raw::c_void,
        attribute: *const std::os::raw::c_void,
        value: *mut *const std::os::raw::c_void,
    ) -> i32;
}

/// Maximum depth to recurse into the AX tree.
#[cfg(target_os = "macos")]
const AX_MAX_DEPTH: usize = 5;

#[cfg(target_os = "macos")]
fn macos_ax_tree(params: &Value) -> Result<Value, (i32, String)> {
    use objc::runtime::{Class, Object};

    let bundle_id = params.get("app_bundle_id").and_then(|v| v.as_str());

    unsafe {
        let workspace_cls = Class::get("NSWorkspace")
            .ok_or_else(|| (ERR_INTERNAL, "NSWorkspace class not found".into()))?;
        let workspace: *mut Object = msg_send![workspace_cls, sharedWorkspace];
        if workspace.is_null() {
            return Err((ERR_INTERNAL, "Failed to get shared workspace".into()));
        }

        let pid: i32 = if let Some(bid) = bundle_id {
            // Find running app by bundle identifier
            let running_apps: *mut Object = msg_send![workspace, runningApplications];
            if running_apps.is_null() {
                return Err((ERR_INTERNAL, "Failed to get running applications".into()));
            }

            let count: usize = msg_send![running_apps, count];
            let nsstring_cls = Class::get("NSString")
                .ok_or_else(|| (ERR_INTERNAL, "NSString class not found".into()))?;

            let mut found_pid: Option<i32> = None;
            for i in 0..count {
                let app: *mut Object = msg_send![running_apps, objectAtIndex: i];
                let app_bid: *mut Object = msg_send![app, bundleIdentifier];
                if app_bid.is_null() {
                    continue;
                }

                let target_ns = nsstring_from_str(nsstring_cls, bid);
                let is_equal: bool = msg_send![app_bid, isEqualToString: target_ns];
                if is_equal {
                    let p: i32 = msg_send![app, processIdentifier];
                    found_pid = Some(p);
                    break;
                }
            }

            found_pid.ok_or_else(|| {
                (
                    ERR_INTERNAL,
                    format!("No running app found with bundle ID: {}", bid),
                )
            })?
        } else {
            // Use frontmost application
            let front_app: *mut Object = msg_send![workspace, frontmostApplication];
            if front_app.is_null() {
                return Err((ERR_INTERNAL, "No frontmost application found".into()));
            }
            msg_send![front_app, processIdentifier]
        };

        // Create AX element for the application
        let ax_app = AXUIElementCreateApplication(pid);
        if ax_app.is_null() {
            return Err((
                ERR_INTERNAL,
                format!("Failed to create AXUIElement for PID {}", pid),
            ));
        }

        let result = ax_element_to_json(ax_app, 0, AX_MAX_DEPTH);

        // Release the application AX element (follows Create Rule)
        core_foundation::base::CFRelease(ax_app);

        Ok(result)
    }
}

/// Create a CFString (NSString) from a Rust string for use as an AX attribute name.
#[cfg(target_os = "macos")]
unsafe fn cfstring(s: &str) -> *const std::os::raw::c_void {
    use objc::runtime::Class;
    use std::ffi::CString;

    let cls = Class::get("NSString").unwrap();
    let cstr = CString::new(s).unwrap();
    msg_send![cls, stringWithUTF8String: cstr.as_ptr()]
}

/// Get a string attribute from an AX element, returning `None` if unavailable.
#[cfg(target_os = "macos")]
unsafe fn ax_get_string(
    element: *const std::os::raw::c_void,
    attr: &str,
) -> Option<String> {
    use objc::runtime::{Class, Object};
    use std::ffi::CStr;
    use std::os::raw::c_char;

    let mut value_ref: *const std::os::raw::c_void = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, cfstring(attr), &mut value_ref);
    if err != 0 || value_ref.is_null() {
        return None;
    }

    let obj = value_ref as *mut Object;
    let nsstring_cls = Class::get("NSString")?;
    let is_string: bool = msg_send![obj, isKindOfClass: nsstring_cls];
    if is_string {
        let utf8: *const c_char = msg_send![obj, UTF8String];
        let result = if utf8.is_null() {
            None
        } else {
            Some(CStr::from_ptr(utf8).to_string_lossy().into_owned())
        };
        core_foundation::base::CFRelease(value_ref);
        result
    } else {
        core_foundation::base::CFRelease(value_ref);
        None
    }
}

/// Recursively convert an AXUIElement into a JSON tree structure.
#[cfg(target_os = "macos")]
unsafe fn ax_element_to_json(
    element: *const std::os::raw::c_void,
    depth: usize,
    max_depth: usize,
) -> Value {
    use objc::runtime::Object;

    if depth >= max_depth {
        return json!({"truncated": true});
    }

    let mut result = serde_json::Map::new();

    // Get role
    if let Some(role) = ax_get_string(element, "AXRole") {
        result.insert("role".into(), json!(role));
    }

    // Get title
    if let Some(title) = ax_get_string(element, "AXTitle") {
        if !title.is_empty() {
            result.insert("title".into(), json!(title));
        }
    }

    // Get value
    if let Some(value) = ax_get_string(element, "AXValue") {
        if !value.is_empty() {
            result.insert("value".into(), json!(value));
        }
    }

    // Get children
    let mut children_ref: *const std::os::raw::c_void = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(
        element,
        cfstring("AXChildren"),
        &mut children_ref,
    );
    if err == 0 && !children_ref.is_null() {
        let count: usize = msg_send![children_ref as *mut Object, count];
        let children: Vec<Value> = (0..count)
            .map(|i| {
                let child: *const std::os::raw::c_void =
                    msg_send![children_ref as *mut Object, objectAtIndex: i];
                ax_element_to_json(child, depth + 1, max_depth)
            })
            .collect();
        if !children.is_empty() {
            result.insert("children".into(), json!(children));
        }
        core_foundation::base::CFRelease(children_ref);
    }

    Value::Object(result)
}
