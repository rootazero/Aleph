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

/// A display's placement in the global screen space, in that space's own units.
///
/// Extracted from `xcap::Monitor` so the display-selection geometry is a pure
/// function testable without a screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DisplayGeometry {
    pub origin_x: i32,
    pub origin_y: i32,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}

/// Area of the overlap between a display and a global-space rectangle.
fn overlap_area(display: &DisplayGeometry, region: &ScreenRegion) -> u64 {
    let (dx0, dy0) = (i64::from(display.origin_x), i64::from(display.origin_y));
    let (dx1, dy1) = (
        dx0 + i64::from(display.width),
        dy0 + i64::from(display.height),
    );
    let (rx0, ry0) = (i64::from(region.x), i64::from(region.y));
    let (rx1, ry1) = (
        rx0 + i64::from(region.width),
        ry0 + i64::from(region.height),
    );

    let w = (dx1.min(rx1) - dx0.max(rx0)).max(0);
    let h = (dy1.min(ry1) - dy0.max(ry0)).max(0);
    (w as u64) * (h as u64)
}

/// Which display a **global-space** rectangle should be captured from, and that
/// rectangle re-expressed in the display's own pixel space (which is what
/// `xcap::Monitor::capture_region` takes — its coordinates are monitor-relative,
/// not global).
///
/// Without this step a region is captured from the primary monitor at the
/// *global* offset, so on any multi-monitor desktop it either crops the wrong
/// place or is rejected outright for exceeding the primary monitor's size.
///
/// Selection is by largest overlap, primary breaking ties, because a region the
/// model derived from a stitched or mis-remembered geometry should still land on
/// the display it mostly covers rather than fail. The returned rectangle is
/// clamped to the chosen display: a rectangle straddling two monitors cannot be
/// captured in one call, and returning the part that exists — with the served
/// image reporting its real dimensions — beats refusing.
///
/// `None` means no display overlaps the rectangle at all; the caller falls back
/// to the primary monitor, which is the pre-existing behavior.
pub(crate) fn resolve_region_target(
    displays: &[DisplayGeometry],
    region: &ScreenRegion,
) -> Option<(usize, ScreenRegion)> {
    let (index, display) = displays
        .iter()
        .enumerate()
        .map(|(i, d)| (i, d, overlap_area(d, region)))
        .filter(|(_, _, area)| *area > 0)
        .max_by_key(|(_, d, area)| (*area, d.is_primary))
        .map(|(i, d, _)| (i, d))?;

    // Global → monitor-relative, then clamp to the monitor's extent.
    let local_x = i64::from(region.x) - i64::from(display.origin_x);
    let local_y = i64::from(region.y) - i64::from(display.origin_y);
    let x = local_x.clamp(0, i64::from(display.width)) as u32;
    let y = local_y.clamp(0, i64::from(display.height)) as u32;
    let width = (i64::from(region.width) + local_x.min(0))
        .clamp(0, i64::from(display.width) - i64::from(x)) as u32;
    let height = (i64::from(region.height) + local_y.min(0))
        .clamp(0, i64::from(display.height) - i64::from(y)) as u32;

    if width == 0 || height == 0 {
        return None;
    }

    Some((
        index,
        ScreenRegion {
            x,
            y,
            width,
            height,
        },
    ))
}

/// Physical pixels per unit of the coordinate space this process's input,
/// window and accessibility APIs speak, on a display whose DPI ratio is
/// `monitor_scale`.
///
/// On macOS and Linux that ratio *is* the answer (a Retina point is two pixels).
/// On Windows it depends on the process: a per-monitor-DPI-aware process already
/// reads physical pixels everywhere, so reporting the display's DPI ratio there
/// makes every consumer that multiplies by it — the normalized-coordinate
/// viewport, the Set-of-Marks overlay, the window-space mapping — overshoot by
/// exactly that ratio. See [`crate::win_dpi`].
pub(crate) fn coordinate_scale(monitor_scale: f64) -> f64 {
    #[cfg(windows)]
    {
        crate::win_dpi::effective_scale(crate::win_dpi::process_dpi_awareness(), monitor_scale)
    }
    #[cfg(not(windows))]
    {
        if monitor_scale.is_finite() && monitor_scale > 0.0 {
            monitor_scale
        } else {
            1.0
        }
    }
}

/// Read a monitor's placement without letting one unreadable field drop it.
fn geometry_of(m: &xcap::Monitor) -> DisplayGeometry {
    DisplayGeometry {
        origin_x: m.x().unwrap_or(0),
        origin_y: m.y().unwrap_or(0),
        width: m.width().unwrap_or(0),
        height: m.height().unwrap_or(0),
        is_primary: m.is_primary().unwrap_or(false),
    }
}

/// The monitor a paramless capture targets: the primary one, or the first
/// available when no monitor claims to be primary.
fn primary_index(monitors: &[xcap::Monitor]) -> Result<usize> {
    monitors
        .iter()
        .position(|m| m.is_primary().unwrap_or(false))
        .or(if monitors.is_empty() { None } else { Some(0) })
        .ok_or_else(|| DesktopError::ScreenCapture("No monitors found".into()))
}

/// Capture a screenshot, optionally cropped to a **global-space** region.
///
/// A paramless capture takes the primary monitor. A region is captured from the
/// display it actually overlaps (see [`resolve_region_target`]) — its
/// coordinates are in the global screen space, the same space clicks are issued
/// in, not offsets into the primary monitor.
///
/// The image is encoded as PNG and returned as a base64-encoded [`Screenshot`]
/// carrying the display's [`coordinate_scale`], so a consumer never has to
/// re-derive (and mis-derive) the points-to-pixels ratio.
///
/// # Errors
///
/// - [`DesktopError::ScreenCapture`] if no monitors are found or the
///   capture/encoding fails.
pub fn take_screenshot(region: Option<&ScreenRegion>) -> Result<Screenshot> {
    debug!("Taking screenshot, region: {:?}", region);

    let monitors = xcap::Monitor::all()
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to enumerate monitors: {e}")))?;

    let geometries: Vec<DisplayGeometry> = monitors.iter().map(geometry_of).collect();
    let (index, local_region) = match region.and_then(|r| resolve_region_target(&geometries, r)) {
        Some((i, r)) => (i, Some(r)),
        // No display overlaps the rectangle (or none was asked for): fall back
        // to the primary monitor, unchanged from the legacy behavior.
        None => (primary_index(&monitors)?, region.copied()),
    };
    let monitor = &monitors[index];

    let image = local_region
        .as_ref()
        .map_or_else(
            || monitor.capture_image(),
            |r| monitor.capture_region(r.x, r.y, r.width, r.height),
        )
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
        scale_factor: Some(coordinate_scale(f64::from(
            monitor.scale_factor().unwrap_or(1.0),
        ))),
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

    let monitor = &monitors[primary_index(&monitors)?];

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
/// `scale_factor` is the display's [`coordinate_scale`] — pixels per unit of the
/// space this process's geometry is reported in — not the raw DPI ratio. Two
/// consumers multiply `width` by it (the normalized-coordinate viewport in
/// `coord_resolve::resolve_viewport`) and use it to place overlay marks
/// (`set_of_marks`), and both are only correct when the two numbers describe the
/// *same* coordinate space.
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
            name: m.name().unwrap_or_default(),
            width: m.width().unwrap_or(0),
            height: m.height().unwrap_or(0),
            scale_factor: coordinate_scale(f64::from(m.scale_factor().unwrap_or(1.0))),
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

    let image = region
        .map_or_else(
            || monitor.capture_image(),
            |r| monitor.capture_region(r.x, r.y, r.width, r.height),
        )
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
        scale_factor: Some(coordinate_scale(f64::from(
            monitor.scale_factor().unwrap_or(1.0),
        ))),
    })
}

/// Capture one window, cropped to its own frame, without raising it.
///
/// This is the `screenshot { window_id }` path: the model sees a single
/// application instead of the whole desktop, so nothing else on screen leaks
/// into the transcript, and an occluded or background window is still legible.
///
/// The returned [`WindowShot`] carries the window's origin and size in the
/// global coordinate space alongside the pixels. Without that, a crop is a
/// targeting regression — the model would reason in window pixels and click in
/// global ones.
///
/// `window_id` is the id from `window_list`. On Linux that is the X11 window
/// (`_NET_CLIENT_LIST`) or the compositor's own handle, and `xcap` keys its
/// window list by the same X11 id, so the two agree on X11. Under a Wayland
/// compositor `xcap` cannot enumerate windows at all, which surfaces as "no
/// window with that id" rather than a wrong capture.
///
/// # Errors
///
/// - [`DesktopError::ScreenCapture`] if windows cannot be enumerated, the id
///   does not match a capturable window, or the capture/encoding fails.
pub fn take_screenshot_window(window_id: u64) -> Result<crate::WindowShot> {
    debug!("Capturing window {window_id}");

    let windows = xcap::Window::all()
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to enumerate windows: {e}")))?;

    let window = windows
        .into_iter()
        .find(|w| w.id().is_ok_and(|id| u64::from(id) == window_id))
        .ok_or_else(|| {
            DesktopError::ScreenCapture(format!(
                "No capturable window with id {window_id}. Re-run window_list for a current id;                  on a Wayland session single-window capture is not available at all — capture the                  screen instead."
            ))
        })?;

    // Read the frame before capturing: a window that moves between the two is
    // better described by where it was when its pixels were taken than by a
    // rectangle read afterwards.
    let bounds = match (window.x(), window.y(), window.width(), window.height()) {
        (Ok(x), Ok(y), Ok(w), Ok(h)) => Some(crate::BoundingBox {
            x: f64::from(x),
            y: f64::from(y),
            w: f64::from(w),
            h: f64::from(h),
        }),
        // Not told is not zero: a shot whose origin is unknown must say so, so
        // the caller does not map its pixels against a fabricated (0,0).
        _ => None,
    };

    let scale_factor = window
        .current_monitor()
        .ok()
        .and_then(|m| m.scale_factor().ok())
        .map(f64::from);

    let image = window
        .capture_image()
        .map_err(|e| DesktopError::ScreenCapture(format!("Window capture failed: {e}")))?;

    let (width, height) = (image.width(), image.height());
    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| DesktopError::ScreenCapture(format!("PNG encoding failed: {e}")))?;

    debug!("Window {window_id} captured: {width}x{height}");

    Ok(crate::WindowShot {
        image: Screenshot {
            image_base64: general_purpose::STANDARD.encode(buf.into_inner()),
            width,
            height,
            format: "png".to_string(),
            scale_factor,
        },
        window_bounds: bounds,
    })
}

/// Number of pixels the uniformity probe reads before declaring a frame flat.
const UNIFORMITY_SAMPLES: u64 = 4096;

/// True when `shot` carries no information about the screen at all — empty
/// payload, zero-sized, undecodable, or a single flat colour.
///
/// A dead capture chain does not always surface as an `Err`: a locked screen, a
/// revoked screen-recording grant, or a wedged helper each hand back a
/// well-formed `Ok(Screenshot)` full of nothing (usually all black). Forwarded
/// to the model those are *worse* than an error — they are presented as real
/// pixels, and two of them in a row read as "the UI has settled".
///
/// The uniformity probe stride-samples `UNIFORMITY_SAMPLES` pixels rather than
/// scanning the frame: a 4K capture is 8.3M pixels and this runs on every
/// screenshot. Sampling is sound because the property under test is *global* — a
/// frame with any real content differs from its first pixel across a large
/// fraction of positions, not at one rare coordinate — and the walk returns on
/// the first differing pixel, so a healthy frame typically costs two reads. The
/// tradeoff is deliberate: a frame whose only content is smaller than the stride
/// may be called degenerate, which costs a redundant recapture, never a lie.
pub fn is_degenerate(shot: &Screenshot) -> bool {
    if shot.image_base64.is_empty() || shot.width == 0 || shot.height == 0 {
        return true;
    }

    // A payload that will not decode is not pixels the model can look at, so it
    // is degenerate for the same reason an empty one is.
    let Ok(raw) = general_purpose::STANDARD.decode(&shot.image_base64) else {
        return true;
    };
    let Ok(img) = image::load_from_memory(&raw) else {
        return true;
    };

    is_uniform(&img)
}

/// True when every sampled pixel of `img` matches its top-left pixel.
fn is_uniform(img: &image::DynamicImage) -> bool {
    use image::GenericImageView as _;

    let (width, height) = img.dimensions();
    let total = u64::from(width) * u64::from(height);
    if total == 0 {
        return true;
    }

    // Odd stride: against an even row width an even stride walks a single column
    // and would miss vertical structure entirely.
    let stride = (total / UNIFORMITY_SAMPLES).max(1) | 1;
    let first = img.get_pixel(0, 0);

    let mut index = 0u64;
    while index < total {
        let x = (index % u64::from(width)) as u32;
        let y = (index / u64::from(width)) as u32;
        if img.get_pixel(x, y) != first {
            return false;
        }
        index += stride;
    }

    true
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
/// The output's `scale_factor` is `None` — this entry point takes raw bytes,
/// which carry no display metadata. Callers re-encoding a [`Screenshot`] from
/// [`take_screenshot`] should use [`process_screenshot_with_scale`] so the
/// display's backing scale survives the re-encode.
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
    process_screenshot_with_scale(
        raw_image, max_width, max_height, format, quality, max_bytes, None,
    )
}

/// [`process_screenshot`] plus `scale_factor` passthrough: the value is
/// copied verbatim onto the output [`Screenshot`] (including through the
/// budget re-encode loop), so a consumer of a re-encoded capture keeps the
/// points-to-pixels ratio [`take_screenshot`] reported.
///
/// # Errors
///
/// - [`DesktopError::ScreenCapture`] if decoding, resizing, or encoding fails.
pub fn process_screenshot_with_scale(
    raw_image: &[u8],
    max_width: Option<u32>,
    max_height: Option<u32>,
    format: &str,
    quality: u8,
    max_bytes: Option<usize>,
    scale_factor: Option<f64>,
) -> Result<Screenshot> {
    debug!(
        "Processing screenshot: max={}x{}, format={}, quality={}, max_bytes={:?}",
        max_width.unwrap_or(0),
        max_height.unwrap_or(0),
        format,
        quality,
        max_bytes,
    );

    // JPEG quality must live in the encoder's supported range; a caller that
    // passes 0 or >100 would otherwise panic `image::JpegEncoder`.
    let quality = quality.clamp(1, 100);

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
            return fit_within_budget(img, quality, limit, scale_factor);
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
        scale_factor,
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
///
/// `scale_factor` is passed through to the output untouched (see
/// [`process_screenshot_with_scale`]).
fn fit_within_budget(
    img: image::DynamicImage,
    requested_quality: u8,
    max_bytes: usize,
    scale_factor: Option<f64>,
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
                return Ok(jpeg_screenshot(encoded, w, h, scale_factor));
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

    let (bytes, w, h) = smallest.ok_or_else(|| {
        DesktopError::ScreenCapture("screenshot budget loop produced no encoding".into())
    })?;
    debug!(
        "Screenshot could not reach {}-byte budget; returning smallest encoding ({} bytes, {}x{})",
        max_bytes,
        bytes.len(),
        w,
        h,
    );
    Ok(jpeg_screenshot(bytes, w, h, scale_factor))
}

/// Wrap raw JPEG bytes into a base64 [`Screenshot`].
fn jpeg_screenshot(
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    scale_factor: Option<f64>,
) -> Screenshot {
    Screenshot {
        image_base64: general_purpose::STANDARD.encode(bytes),
        width,
        height,
        format: "jpeg".to_string(),
        scale_factor,
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod region_target_tests {
    use super::*;

    /// A left-to-right desktop: 1920-wide primary at the origin, 2560-wide
    /// secondary immediately to its right.
    fn two_monitors() -> Vec<DisplayGeometry> {
        vec![
            DisplayGeometry {
                origin_x: 0,
                origin_y: 0,
                width: 1920,
                height: 1080,
                is_primary: true,
            },
            DisplayGeometry {
                origin_x: 1920,
                origin_y: 0,
                width: 2560,
                height: 1440,
                is_primary: false,
            },
        ]
    }

    fn region(x: u32, y: u32, width: u32, height: u32) -> ScreenRegion {
        ScreenRegion {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_region_on_the_primary_stays_where_it_is() {
        let (index, local) =
            resolve_region_target(&two_monitors(), &region(100, 50, 400, 300)).expect("resolved");
        assert_eq!(index, 0);
        assert_eq!(local, region(100, 50, 400, 300));
    }

    #[test]
    fn a_region_on_the_second_monitor_picks_it_and_is_made_monitor_relative() {
        // This is the case that silently cropped the wrong pixels: the global
        // x=2000 was handed to the primary monitor as an offset into *it*.
        let (index, local) =
            resolve_region_target(&two_monitors(), &region(2000, 100, 640, 480)).expect("resolved");
        assert_eq!(index, 1, "must select the display the region lives on");
        assert_eq!(local.x, 80, "global 2000 is 80px into a monitor at x=1920");
        assert_eq!(local.y, 100);
        assert_eq!((local.width, local.height), (640, 480));
    }

    #[test]
    fn a_straddling_region_lands_on_the_display_it_mostly_covers_and_is_clamped() {
        // 1800..2200: 120px on the primary, 280px on the secondary.
        let (index, local) =
            resolve_region_target(&two_monitors(), &region(1800, 0, 400, 200)).expect("resolved");
        assert_eq!(index, 1, "largest overlap wins");
        assert_eq!(
            local.x, 0,
            "the part left of the monitor is cut, not wrapped"
        );
        assert_eq!(
            local.width, 280,
            "width shrinks by the amount that fell off the left edge"
        );
    }

    #[test]
    fn a_region_wider_than_its_display_is_clamped_to_the_display() {
        // xcap rejects a region that overruns the monitor; clamping keeps the
        // capture alive and the served image reports its real dimensions.
        let (index, local) =
            resolve_region_target(&two_monitors(), &region(1000, 0, 4000, 900)).expect("resolved");
        assert_eq!(index, 1);
        assert!(
            local.x + local.width <= 2560,
            "clamped region must fit its display, got {local:?}"
        );
    }

    #[test]
    fn a_region_off_every_display_falls_back_to_the_caller() {
        // `None` is the signal to use the primary monitor unchanged — the
        // legacy behavior, kept rather than turned into a hard error.
        assert!(resolve_region_target(&two_monitors(), &region(9000, 9000, 10, 10)).is_none());
    }

    #[test]
    fn a_zero_area_region_is_not_resolvable() {
        assert!(resolve_region_target(&two_monitors(), &region(10, 10, 0, 100)).is_none());
    }

    #[test]
    fn vertical_stacking_is_handled_on_the_y_axis_too() {
        let displays = vec![
            DisplayGeometry {
                origin_x: 0,
                origin_y: 0,
                width: 1920,
                height: 1080,
                is_primary: true,
            },
            DisplayGeometry {
                origin_x: 0,
                origin_y: 1080,
                width: 1920,
                height: 1080,
                is_primary: false,
            },
        ];
        let (index, local) =
            resolve_region_target(&displays, &region(10, 1200, 100, 100)).expect("resolved");
        assert_eq!(index, 1);
        assert_eq!(local.y, 120);
    }
}

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

    #[test]
    fn scale_factor_passes_through_plain_and_budget_paths() {
        // Plain path: no resize, no budget pressure.
        let png = png_bytes(&solid_image(64, 64));
        let out = process_screenshot_with_scale(&png, None, None, "png", 90, None, Some(2.0))
            .expect("processing should succeed");
        assert_eq!(out.scale_factor, Some(2.0));

        // Budget path: the JPEG re-encode loop must not drop it either.
        let png = png_bytes(&noisy_image(1600, 1600));
        let out =
            process_screenshot_with_scale(&png, None, None, "png", 90, Some(60_000), Some(2.0))
                .expect("processing should succeed");
        assert_eq!(out.format, "jpeg");
        assert_eq!(out.scale_factor, Some(2.0));

        // The legacy entry point keeps reporting "unknown".
        let png = png_bytes(&solid_image(64, 64));
        let out = process_screenshot(&png, None, None, "png", 90, None)
            .expect("processing should succeed");
        assert_eq!(out.scale_factor, None);
    }
}

#[cfg(test)]
mod degenerate_tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    /// PNG-encode `img` into the `Screenshot` shape a capture backend returns.
    fn shot_of(img: &image::DynamicImage) -> Screenshot {
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)
            .expect("PNG encode");
        Screenshot {
            image_base64: general_purpose::STANDARD.encode(buf.into_inner()),
            width: img.width(),
            height: img.height(),
            format: "png".to_string(),
            scale_factor: None,
        }
    }

    fn filled(w: u32, h: u32, colour: [u8; 3]) -> image::DynamicImage {
        let buf = ImageBuffer::from_fn(w, h, |_, _| Rgb(colour));
        image::DynamicImage::ImageRgb8(buf)
    }

    #[test]
    fn empty_payload_is_degenerate() {
        let shot = Screenshot {
            image_base64: String::new(),
            width: 1920,
            height: 1080,
            format: "png".to_string(),
            scale_factor: None,
        };
        assert!(is_degenerate(&shot));
    }

    #[test]
    fn zero_width_is_degenerate() {
        let mut shot = shot_of(&filled(64, 64, [10, 20, 30]));
        shot.width = 0;
        assert!(is_degenerate(&shot));
    }

    #[test]
    fn undecodable_payload_is_degenerate() {
        let shot = Screenshot {
            image_base64: "not-base64-!!!".to_string(),
            width: 800,
            height: 600,
            format: "png".to_string(),
            scale_factor: None,
        };
        assert!(is_degenerate(&shot));
    }

    #[test]
    fn all_black_frame_is_degenerate() {
        let shot = shot_of(&filled(1280, 800, [0, 0, 0]));
        assert!(is_degenerate(&shot), "an all-black capture is a dead chain");
    }

    #[test]
    fn uniform_non_black_frame_is_degenerate() {
        // The invariant is "one flat colour", not "black": a blanked display can
        // come back solid white or solid grey just as easily.
        let shot = shot_of(&filled(1280, 800, [255, 255, 255]));
        assert!(is_degenerate(&shot));
    }

    #[test]
    fn dark_frame_with_content_is_not_degenerate() {
        // A legitimately dark screenshot (dark-mode editor at night) must pass:
        // near-black everywhere, but structured.
        let buf = ImageBuffer::from_fn(1280, 800, |x, y| {
            if (x / 17 + y / 13) % 2 == 0 {
                Rgb([6u8, 6, 8])
            } else {
                Rgb([14u8, 15, 18])
            }
        });
        let shot = shot_of(&image::DynamicImage::ImageRgb8(buf));
        assert!(
            !is_degenerate(&shot),
            "a dark but structured frame must not be flagged"
        );
    }
}
