//! Screenshot capture functions.

use base64::{engine::general_purpose, Engine as _};
use std::io::Cursor;
use tracing::debug;

use crate::error::{DesktopError, Result};
use crate::{DisplayInfo, ScreenRegion, Screenshot};

/// Default byte budget for a screenshot returned to the agent as a tool result.
///
/// A paramless, full-resolution PNG of a 4K display is routinely 10–20 MB once
/// base64-encoded. Emitting a result that large is both a performance problem
/// and a *correctness* one: the generic tool-result size budget would truncate
/// the base64 string mid-stream, yielding an image the model cannot decode.
///
/// Screenshots whose encoding exceeds this budget are transparently re-encoded
/// (JPEG, progressively lower quality, then downscaled) until they fit — the
/// same size-bounding discipline `openclaw` applies to its browser captures,
/// here as a native Rust loop over the `image` crate.
pub const DEFAULT_SCREENSHOT_MAX_BYTES: usize = 1_500_000;

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
        scale_factor: None,
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
        scale_factor: None,
    })
}

/// Decode a raw screenshot, optionally resize, and re-encode as JPEG or PNG.
///
/// If the image exceeds `max_width` or `max_height`, it is scaled down using
/// Lanczos3 resampling while preserving the aspect ratio. When `max_bytes` is
/// set and the encoded result still exceeds it, the image is re-encoded as
/// JPEG with progressively lower quality and, if necessary, a smaller scale,
/// until it fits the budget (see [`DEFAULT_SCREENSHOT_MAX_BYTES`]).
///
/// # Parameters
///
/// - `raw_image` — encoded image bytes (PNG, or any format `image` decodes).
/// - `max_width` / `max_height` — Optional resize limits. `None` means no limit.
/// - `format` — `"jpeg"` or `"png"` (default: `"png"` if unrecognised).
/// - `quality` — JPEG quality 1–100; ignored for PNG.
/// - `max_bytes` — Optional encoded-size budget; `None` disables the budget loop.
///
/// # Errors
///
/// - [`DesktopError::ScreenCapture`] if decoding, resizing, or encoding fails.
pub fn process_screenshot(
    raw_image: &[u8],
    max_width: Option<u32>,
    max_height: Option<u32>,
    format: &str,
    quality: u8,
    max_bytes: Option<usize>,
) -> Result<Screenshot> {
    debug!(
        "Processing screenshot: max={}x{}, format={}, quality={}, max_bytes={:?}",
        max_width.unwrap_or(0),
        max_height.unwrap_or(0),
        format,
        quality,
        max_bytes,
    );

    let img = image::load_from_memory(raw_image)
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to decode image: {e}")))?;

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

    let want_jpeg = format == "jpeg" || format == "jpg";
    let encoded = if want_jpeg {
        encode_jpeg(&img, quality)?
    } else {
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)
            .map_err(|e| DesktopError::ScreenCapture(format!("PNG encoding failed: {e}")))?;
        buf.into_inner()
    };

    // Budget enforcement: an over-budget encoding is re-shrunk as JPEG. A
    // lossless PNG the model can never decode (because the result budget
    // truncates it) is strictly worse than a lossy JPEG it can.
    if let Some(limit) = max_bytes {
        if encoded.len() > limit {
            return fit_within_budget(img, quality, limit);
        }
    }

    let (width, height) = (img.width(), img.height());
    let out_format = if want_jpeg { "jpeg" } else { "png" };
    debug!(
        "Processed screenshot: {}x{} as {} ({} bytes)",
        width,
        height,
        out_format,
        encoded.len(),
    );

    Ok(Screenshot {
        image_base64: general_purpose::STANDARD.encode(encoded),
        width,
        height,
        format: out_format.to_string(),
        scale_factor: None,
    })
}

/// Encode `img` as JPEG at the given quality, returning the raw bytes.
fn encode_jpeg(img: &image::DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality)
        .encode_image(img)
        .map_err(|e| DesktopError::ScreenCapture(format!("JPEG encoding failed: {e}")))?;
    Ok(buf.into_inner())
}

/// Re-encode `img` as JPEG, stepping quality down and then scale down, until
/// the encoded size fits `max_bytes`.
///
/// Returns the first encoding that fits the budget. If none does — a
/// worst-case dense screenshot already at the minimum legible size — the
/// smallest encoding produced is returned anyway: a slightly oversized but
/// *valid* JPEG always beats a truncated, undecodable one.
fn fit_within_budget(
    img: image::DynamicImage,
    requested_quality: u8,
    max_bytes: usize,
) -> Result<Screenshot> {
    /// Quality ladder, descending — capped at the caller's requested quality.
    const QUALITY_LADDER: &[u8] = &[82, 68, 55, 44];
    /// Stop downscaling here — below this width on-screen text is unreadable.
    const MIN_WIDTH: u32 = 800;
    /// Bound the downscale rounds (the loop also terminates on `MIN_WIDTH`).
    const MAX_ROUNDS: usize = 6;

    let ceiling = requested_quality.clamp(44, 90);
    let mut current = img;
    let mut smallest: Option<(Vec<u8>, u32, u32)> = None;

    for _ in 0..MAX_ROUNDS {
        let mut last_quality: Option<u8> = None;
        for &step in QUALITY_LADDER {
            let q = step.min(ceiling);
            if last_quality == Some(q) {
                continue; // the ceiling already produced this quality
            }
            last_quality = Some(q);

            let encoded = encode_jpeg(&current, q)?;
            let (w, h) = (current.width(), current.height());
            if encoded.len() <= max_bytes {
                return Ok(jpeg_screenshot(encoded, w, h));
            }
            let is_smaller = match &smallest {
                Some((best, _, _)) => encoded.len() < best.len(),
                None => true,
            };
            if is_smaller {
                smallest = Some((encoded, w, h));
            }
        }

        let next_width = current.width() * 3 / 4;
        if next_width < MIN_WIDTH {
            break;
        }
        current = current.resize(next_width, u32::MAX, image::imageops::FilterType::Lanczos3);
    }

    let (bytes, w, h) = smallest.expect("budget loop always encodes at least once");
    debug!(
        "Screenshot could not reach {}-byte budget; returning smallest encoding ({} bytes, {}x{})",
        max_bytes,
        bytes.len(),
        w,
        h,
    );
    Ok(jpeg_screenshot(bytes, w, h))
}

/// Wrap raw JPEG bytes into a base64 [`Screenshot`].
fn jpeg_screenshot(bytes: Vec<u8>, width: u32, height: u32) -> Screenshot {
    Screenshot {
        image_base64: general_purpose::STANDARD.encode(bytes),
        width,
        height,
        format: "jpeg".to_string(),
        scale_factor: None,
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod budget_tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    /// Encode an image to PNG bytes for use as `process_screenshot` input.
    fn png_bytes(img: &image::DynamicImage) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)
            .expect("PNG encode");
        buf.into_inner()
    }

    /// A high-entropy image that resists compression (worst case for a budget).
    fn noisy_image(w: u32, h: u32) -> image::DynamicImage {
        let buf = ImageBuffer::from_fn(w, h, |x, y| {
            let v = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) as u8;
            Rgb([v, v.wrapping_add(85), v.wrapping_add(170)])
        });
        image::DynamicImage::ImageRgb8(buf)
    }

    /// A solid-colour image that compresses to almost nothing.
    fn solid_image(w: u32, h: u32) -> image::DynamicImage {
        let buf = ImageBuffer::from_fn(w, h, |_, _| Rgb([200u8, 30, 30]));
        image::DynamicImage::ImageRgb8(buf)
    }

    #[test]
    fn small_screenshot_passes_through_unbudgeted() {
        let png = png_bytes(&solid_image(64, 64));
        let out = process_screenshot(
            &png,
            None,
            None,
            "png",
            90,
            Some(DEFAULT_SCREENSHOT_MAX_BYTES),
        )
        .expect("processing should succeed");
        assert_eq!(out.format, "png");
        assert_eq!((out.width, out.height), (64, 64));
    }

    #[test]
    fn oversized_screenshot_is_shrunk_to_valid_jpeg() {
        // A 1600x1600 noisy PNG is far above any sane budget.
        let png = png_bytes(&noisy_image(1600, 1600));
        let out = process_screenshot(&png, None, None, "png", 90, Some(60_000))
            .expect("processing should succeed");
        // The budget loop must have converted PNG → JPEG.
        assert_eq!(out.format, "jpeg");
        // The output must remain a valid, decodable image — never truncated.
        let raw = general_purpose::STANDARD
            .decode(&out.image_base64)
            .expect("output must be valid base64");
        assert!(
            image::load_from_memory(&raw).is_ok(),
            "budget output must still decode as an image"
        );
        // And it must be substantially smaller than the raw capture.
        assert!(
            raw.len() < png.len(),
            "budget loop should shrink the image ({} >= {})",
            raw.len(),
            png.len()
        );
    }

    #[test]
    fn budget_is_met_for_compressible_image() {
        // A large solid-colour image compresses well under a modest budget.
        let png = png_bytes(&solid_image(3000, 2000));
        let budget = 100_000;
        let out = process_screenshot(&png, None, None, "png", 90, Some(budget))
            .expect("processing should succeed");
        let raw = general_purpose::STANDARD
            .decode(&out.image_base64)
            .expect("valid base64");
        assert!(
            raw.len() <= budget,
            "compressible image must fit the budget ({} > {})",
            raw.len(),
            budget
        );
    }

    #[test]
    fn no_budget_keeps_full_resolution() {
        let png = png_bytes(&noisy_image(900, 700));
        let out = process_screenshot(&png, None, None, "png", 90, None)
            .expect("processing should succeed");
        // Without a budget a large image is returned untouched.
        assert_eq!(out.format, "png");
        assert_eq!((out.width, out.height), (900, 700));
    }

    #[test]
    fn explicit_resize_is_honoured() {
        let png = png_bytes(&solid_image(2000, 1000));
        let out = process_screenshot(
            &png,
            Some(800),
            None,
            "png",
            90,
            Some(DEFAULT_SCREENSHOT_MAX_BYTES),
        )
        .expect("processing should succeed");
        assert_eq!(out.width, 800);
        assert_eq!(out.height, 400); // aspect ratio preserved
    }
}
