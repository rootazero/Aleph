//! Perception handlers — screen capture, OCR, accessibility tree.
//!
//! Currently only `desktop.screenshot` is implemented.
//! Promote to `perception/mod.rs` when OCR/AXTree are added.

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
