//! Screenshot capture functions.

use base64::{engine::general_purpose, Engine as _};
use std::io::Cursor;
use tracing::debug;

use crate::error::{DesktopError, Result};
use crate::{DisplayInfo, ScreenRegion, Screenshot};

/// Capture a screenshot of the primary monitor, optionally cropped to a region.
///
/// Uses `xcap::Monitor` to enumerate displays and capture the primary one.
/// The image is encoded as PNG and returned as a base64-encoded [`Screenshot`].
///
/// # Errors
///
/// - [`DesktopError::ScreenCapture`] if no monitors are found, no primary
///   monitor exists, or the capture/encoding fails.
pub fn take_screenshot(region: Option<&ScreenRegion>) -> Result<Screenshot> {
    debug!("Taking screenshot, region: {:?}", region);

    let monitors = xcap::Monitor::all()
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to enumerate monitors: {e}")))?;

    let monitor = monitors
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .ok_or_else(|| DesktopError::ScreenCapture("No primary monitor found".into()))?;

    let image = match region {
        Some(r) => monitor.capture_region(r.x, r.y, r.width, r.height),
        None => monitor.capture_image(),
    }
    .map_err(|e| DesktopError::ScreenCapture(format!("Screen capture failed: {e}")))?;

    let (width, height) = (image.width(), image.height());

    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| DesktopError::ScreenCapture(format!("PNG encoding failed: {e}")))?;

    let image_base64 = general_purpose::STANDARD.encode(buf.into_inner());

    debug!("Screenshot captured: {}x{}", width, height);

    Ok(Screenshot {
        image_base64,
        width,
        height,
        format: "png".to_string(),
    })
}

/// Capture the primary monitor as raw PNG bytes.
///
/// This is a convenience function for OCR input — it captures the full
/// primary monitor and returns the PNG-encoded bytes without base64 encoding.
///
/// # Errors
///
/// Same as [`take_screenshot`].
pub fn capture_screen_png() -> Result<Vec<u8>> {
    debug!("Capturing screen as raw PNG bytes");

    let monitors = xcap::Monitor::all()
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to enumerate monitors: {e}")))?;

    let monitor = monitors
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .ok_or_else(|| DesktopError::ScreenCapture("No primary monitor found".into()))?;

    let image = monitor
        .capture_image()
        .map_err(|e| DesktopError::ScreenCapture(format!("Screen capture failed: {e}")))?;

    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| DesktopError::ScreenCapture(format!("PNG encoding failed: {e}")))?;

    Ok(buf.into_inner())
}

/// Enumerate all connected displays and return their metadata.
///
/// Uses `xcap::Monitor::all()` to discover displays. Fields that are not
/// available on a given platform are filled with sensible defaults (e.g.
/// empty string for `name`, `1.0` for `scale_factor`).
///
/// # Errors
///
/// - [`DesktopError::ScreenCapture`] if monitor enumeration fails.
pub fn list_displays() -> Result<Vec<DisplayInfo>> {
    debug!("Listing displays");

    let monitors = xcap::Monitor::all()
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to enumerate monitors: {e}")))?;

    let displays = monitors
        .into_iter()
        .map(|m| DisplayInfo {
            id: m.id().unwrap_or(0),
            name: m.name().unwrap_or_default().to_string(),
            width: m.width().unwrap_or(0),
            height: m.height().unwrap_or(0),
            scale_factor: m.scale_factor().unwrap_or(1.0) as f64,
            is_primary: m.is_primary().unwrap_or(false),
            origin_x: m.x().unwrap_or(0),
            origin_y: m.y().unwrap_or(0),
        })
        .collect();

    Ok(displays)
}

/// Capture a screenshot of the display identified by `display_id`,
/// optionally cropped to `region`.
///
/// # Errors
///
/// - [`DesktopError::ScreenCapture`] if the display is not found or capture fails.
pub fn take_screenshot_display(
    display_id: u32,
    region: Option<&ScreenRegion>,
) -> Result<Screenshot> {
    debug!(
        "Taking screenshot for display {}, region: {:?}",
        display_id, region
    );

    let monitors = xcap::Monitor::all()
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to enumerate monitors: {e}")))?;

    let monitor = monitors
        .into_iter()
        .find(|m| m.id().unwrap_or(u32::MAX) == display_id)
        .ok_or_else(|| {
            DesktopError::ScreenCapture(format!("Display with id {display_id} not found"))
        })?;

    let image = match region {
        Some(r) => monitor.capture_region(r.x, r.y, r.width, r.height),
        None => monitor.capture_image(),
    }
    .map_err(|e| DesktopError::ScreenCapture(format!("Screen capture failed: {e}")))?;

    let (width, height) = (image.width(), image.height());

    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| DesktopError::ScreenCapture(format!("PNG encoding failed: {e}")))?;

    let image_base64 = general_purpose::STANDARD.encode(buf.into_inner());

    debug!(
        "Screenshot captured for display {}: {}x{}",
        display_id, width, height
    );

    Ok(Screenshot {
        image_base64,
        width,
        height,
        format: "png".to_string(),
    })
}

/// Decode a raw PNG, optionally resize, and re-encode as JPEG or PNG.
///
/// If the image exceeds `max_width` or `max_height`, it is scaled down
/// using Lanczos3 resampling while preserving the aspect ratio. The output
/// is base64-encoded and returned as a [`Screenshot`].
///
/// # Parameters
///
/// - `raw_png` — PNG-encoded image bytes.
/// - `max_width` / `max_height` — Optional resize limits. `None` means no limit.
/// - `format` — `"jpeg"` or `"png"` (default: `"png"` if unrecognised).
/// - `quality` — JPEG quality 1–100; ignored for PNG.
///
/// # Errors
///
/// - [`DesktopError::ScreenCapture`] if decoding, resizing, or encoding fails.
pub fn process_screenshot(
    raw_png: &[u8],
    max_width: Option<u32>,
    max_height: Option<u32>,
    format: &str,
    quality: u8,
) -> Result<Screenshot> {
    debug!(
        "Processing screenshot: max={}x{}, format={}, quality={}",
        max_width.unwrap_or(0),
        max_height.unwrap_or(0),
        format,
        quality
    );

    let img = image::load_from_memory(raw_png)
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to decode PNG: {e}")))?;

    // Optionally resize while maintaining aspect ratio.
    let img = match (max_width, max_height) {
        (Some(mw), Some(mh)) if img.width() > mw || img.height() > mh => {
            img.resize(mw, mh, image::imageops::FilterType::Lanczos3)
        }
        (Some(mw), None) if img.width() > mw => {
            img.resize(mw, u32::MAX, image::imageops::FilterType::Lanczos3)
        }
        (None, Some(mh)) if img.height() > mh => {
            img.resize(u32::MAX, mh, image::imageops::FilterType::Lanczos3)
        }
        _ => img,
    };

    let (width, height) = (img.width(), img.height());
    let mut buf = Cursor::new(Vec::new());

    let out_format = if format == "jpeg" || format == "jpg" {
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
        encoder
            .encode_image(&img)
            .map_err(|e| DesktopError::ScreenCapture(format!("JPEG encoding failed: {e}")))?;
        "jpeg"
    } else {
        img.write_to(&mut buf, image::ImageFormat::Png)
            .map_err(|e| DesktopError::ScreenCapture(format!("PNG encoding failed: {e}")))?;
        "png"
    };

    let image_base64 = general_purpose::STANDARD.encode(buf.into_inner());

    debug!(
        "Processed screenshot: {}x{} as {}",
        width, height, out_format
    );

    Ok(Screenshot {
        image_base64,
        width,
        height,
        format: out_format.to_string(),
    })
}
