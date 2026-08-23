//! Normalize UI-TARS-style coordinate inputs to pixel space before dispatch.
//!
//! The model (Aleph caller, or a UI-TARS-finetuned VLM) emits coordinates in
//! whichever [`aleph_desktop::CoordinateSpace`] it was prompted with. The
//! actual native bridges (`desktop/macos`, future `desktop/linux`, etc.) only
//! accept pixel coordinates. This module bridges that gap with a pure,
//! synchronously-testable rescale plus a thin async helper that resolves the
//! viewport once at dispatch time.
//!
//! # The third space: `"window"`
//!
//! A window-cropped capture (`screenshot` with `window_id`) hands the model
//! pixels whose origin is the **window's** top-left, not the display's. Resolved
//! against the display viewport — which is what every other space does — such a
//! point lands wherever the window's offset happens to put it, i.e. anywhere.
//! `coord_space:"window"` maps those pixels back through the window's own frame:
//!
//! ```text
//! global = window_bounds.origin + pixel / scale
//! ```
//!
//! and, when the caller also passes `coord_factors:[image_w, image_h]` (the
//! dimensions of the *served* image, which the result-size budget may have
//! downscaled), through the frame's size instead — a fraction is invariant under
//! any downscale, so the mapping stays exact without knowing the scale at all.
//!
//! This is pure geometry over numbers the platform reported. It reads nothing
//! about what the pixels depict (R7/P8).

use crate::sync_primitives::Arc;

use aleph_desktop::{BoundingBox, CoordinateSpace, DesktopPlatform};

use super::types::{DesktopArgs, DesktopOutput};
use crate::error::Result;

const DEFAULT_FACTOR_W: u32 = 1000;
const DEFAULT_FACTOR_H: u32 = 1000;

/// Translate the caller-supplied `coord_space` string + factor pair into a
/// strongly-typed [`CoordinateSpace`].
///
/// Unknown / missing values default to [`CoordinateSpace::Pixel`] so legacy
/// callers (every pre-PR skill) keep working without touching their JSON.
pub fn coord_space_of(args: &DesktopArgs) -> CoordinateSpace {
    match args.coord_space.as_deref().unwrap_or("pixel") {
        "normalized" | "norm" | "uitars" | "ui_tars" => {
            let [fw, fh] = args
                .coord_factors
                .unwrap_or([DEFAULT_FACTOR_W, DEFAULT_FACTOR_H]);
            CoordinateSpace::Normalized {
                factor_w: fw,
                factor_h: fh,
            }
        }
        _ => CoordinateSpace::Pixel,
    }
}

/// Apply the rescale in-place on every coordinate-bearing field of `args`.
///
/// Pure function — separated from the async viewport lookup so the math is
/// fully unit-testable without a `DesktopPlatform` fixture.
///
/// `viewport_w` / `viewport_h` are the **physical pixel** dimensions of the
/// target display (typically the primary display). When `args` is already in
/// pixel space this is a no-op and the args are returned unchanged.
pub fn rescale_args_in_place(args: &mut DesktopArgs, viewport_w: u32, viewport_h: u32) {
    let space = coord_space_of(args);
    if matches!(space, CoordinateSpace::Pixel) {
        return;
    }

    let rescale_point = |x: &mut Option<f64>, y: &mut Option<f64>| {
        if let (Some(xv), Some(yv)) = (*x, *y) {
            let (px, py) = space.to_pixels(xv, yv, viewport_w, viewport_h);
            *x = Some(px);
            *y = Some(py);
        }
    };

    rescale_point(&mut args.x, &mut args.y);
    rescale_point(&mut args.start_x, &mut args.start_y);
    rescale_point(&mut args.end_x, &mut args.end_y);

    if let Some(region) = args.region.as_mut() {
        let (px, py) = space.to_pixels(region.x, region.y, viewport_w, viewport_h);
        let (pw, ph) = space.to_pixels(region.width, region.height, viewport_w, viewport_h);
        region.x = px;
        region.y = py;
        region.width = pw;
        region.height = ph;
    }

    // Mark the args as already-resolved so downstream callers (audit, batch
    // recursion, future tools) see pixel space and never double-convert.
    args.coord_space = Some("pixel".to_string());
    args.coord_factors = None;
}

/// Pick a viewport for normalized coordinate resolution.
///
/// Strategy: if a `display_id` is set, find that display in `display_list()`;
/// otherwise fall back to the primary display; otherwise (no display info at
/// all) return `None` and the caller skips rescale. Returns physical pixel
/// dimensions (`width` × `scale_factor`, `height` × `scale_factor`) — those
/// are what every downstream native bridge expects.
pub async fn resolve_viewport(
    platform: &Arc<dyn DesktopPlatform>,
    display_id: Option<u32>,
) -> Option<(u32, u32)> {
    let screen = platform.screen()?;
    let displays = screen.display_list().await.ok()?;
    let pick = match display_id {
        Some(id) => displays
            .iter()
            .find(|d| d.id == id)
            .or_else(|| displays.iter().find(|d| d.is_primary))
            .or_else(|| displays.first())?,
        None => displays
            .iter()
            .find(|d| d.is_primary)
            .or_else(|| displays.first())?,
    };
    let s = pick.scale_factor.max(1.0);
    let w = (f64::from(pick.width) * s).round() as u32;
    let h = (f64::from(pick.height) * s).round() as u32;
    Some((w, h))
}

/// True when the caller's points are pixels of a window-cropped capture.
///
/// Kept out of [`coord_space_of`] on purpose: [`CoordinateSpace`] is the *limb*
/// vocabulary, and a limb cannot resolve this space — it needs the window's
/// frame, which only the tool layer can look up. The shared enum (and every
/// platform implementing it) therefore stays untouched.
fn is_window_space(args: &DesktopArgs) -> bool {
    matches!(args.coord_space.as_deref(), Some("window"))
}

/// Map one point of a window capture back into the global point space.
///
/// With `factors` — the served image's `[width, height]`, which the result-size
/// budget may have downscaled from the raw capture — the point is mapped as a
/// *fraction* of the frame, which is invariant under any downscale. Without
/// them the point is taken as a raw capture pixel and divided by the backing
/// scale. Both land in the same space the window's frame is expressed in: global
/// screen points, which is what clicks are issued in.
fn window_to_global(
    x: f64,
    y: f64,
    frame: &BoundingBox,
    scale: f64,
    factors: Option<[u32; 2]>,
) -> (f64, f64) {
    match factors {
        Some([fw, fh]) => {
            let fw = f64::from(fw.max(1));
            let fh = f64::from(fh.max(1));
            (frame.x + (x / fw) * frame.w, frame.y + (y / fh) * frame.h)
        }
        None => {
            let s = if scale > 0.0 { scale } else { 1.0 };
            (frame.x + x / s, frame.y + y / s)
        }
    }
}

/// Apply the window→global mapping on every coordinate-bearing point field.
///
/// Pure, so the geometry is testable without a live desktop.
fn rescale_window_in_place(args: &mut DesktopArgs, frame: &BoundingBox, scale: f64) {
    let factors = args.coord_factors;
    let map = |x: &mut Option<f64>, y: &mut Option<f64>| {
        if let (Some(xv), Some(yv)) = (*x, *y) {
            let (gx, gy) = window_to_global(xv, yv, frame, scale, factors);
            *x = Some(gx);
            *y = Some(gy);
        }
    };
    map(&mut args.x, &mut args.y);
    map(&mut args.start_x, &mut args.start_y);
    map(&mut args.end_x, &mut args.end_y);

    // Same "already resolved" marker the normalized path sets, so a second pass
    // (batch recursion, audit) never converts twice.
    args.coord_space = Some("pixel".to_string());
    args.coord_factors = None;
}

/// Look up a window's frame (global points) and the backing scale of the display
/// it sits on. `Err` carries a message the model can act on.
async fn window_frame(
    platform: &Arc<dyn DesktopPlatform>,
    window_id: u64,
) -> std::result::Result<(BoundingBox, f64), String> {
    let screen = platform
        .screen()
        .ok_or_else(|| "no screen capability on this platform".to_string())?;

    let resolved = super::window_lookup::ResolvedWindow::lookup(
        screen,
        window_id,
        "a window-space coordinate cannot be mapped back to the screen",
    )
    .await?;
    let frame = resolved.frame;

    // The scale only matters when the caller sent raw capture pixels (no
    // coord_factors). An unknown display is not a reason to refuse — 1.0 is the
    // non-Retina truth and the frame is still exact.
    let scale = screen
        .display_list()
        .await
        .ok()
        .and_then(|displays| {
            displays
                .iter()
                .find(|d| {
                    let (dx, dy) = (f64::from(d.origin_x), f64::from(d.origin_y));
                    frame.x >= dx
                        && frame.y >= dy
                        && frame.x < dx + f64::from(d.width)
                        && frame.y < dy + f64::from(d.height)
                })
                .or_else(|| displays.iter().find(|d| d.is_primary))
                .or_else(|| displays.first())
                .map(|d| d.scale_factor)
        })
        .unwrap_or(1.0);

    Ok((frame, scale.max(1.0)))
}

/// Resolve the caller's coordinate space before dispatch.
///
/// Returns `Some(refusal)` when a window-space point cannot be mapped: passing
/// the raw pixels through would click at an arbitrary place on the screen, so
/// this fails closed and tells the model what to do instead (A2 — the error goes
/// back into the context, the model picks the next move).
pub async fn maybe_normalize(
    args: &mut DesktopArgs,
    platform: &Arc<dyn DesktopPlatform>,
) -> Result<Option<DesktopOutput>> {
    if is_window_space(args) {
        // A region is a rectangle of a *display* (that is what the capture path
        // takes); there is no window-space meaning for it. Refusing beats
        // silently resolving half the args.
        if args.region.is_some() {
            return Ok(Some(refusal(
                "coord_space:\"window\" applies to points (x/y, start/end), not to `region` — \
                 a region is a rectangle of the display. Capture a window with \
                 `screenshot {window_id}` and omit `region`."
                    .to_string(),
            )));
        }
        let Some(window_id) = args.window_id.map(u64::from) else {
            return Ok(Some(refusal(
                "coord_space:\"window\" needs `window_id` — the window whose capture these \
                 pixels came from. Get it from window_list, or drop coord_space to address \
                 the display instead."
                    .to_string(),
            )));
        };
        return match window_frame(platform, window_id).await {
            Ok((frame, scale)) => {
                rescale_window_in_place(args, &frame, scale);
                Ok(None)
            }
            Err(message) => Ok(Some(refusal(message))),
        };
    }

    if matches!(coord_space_of(args), CoordinateSpace::Pixel) {
        return Ok(None);
    }
    if let Some((w, h)) = resolve_viewport(platform, args.display_id).await {
        rescale_args_in_place(args, w, h);
    } else {
        // No display info available — leave the coords alone rather than
        // silently producing a click at (0, 0). Downstream will dispatch the
        // normalized values which most platforms will reject loudly.
        tracing::warn!(
            "desktop tool: coord_space=normalized but no display info available; \
             passing through raw coordinates"
        );
    }
    Ok(None)
}

/// A coordinate-resolution refusal, carrying its recovery hint.
fn refusal(message: String) -> DesktopOutput {
    DesktopOutput {
        success: false,
        data: None,
        message: Some(super::recovery::with_hint(message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_tools::desktop::types::{DesktopArgs, ScreenRegion};

    fn baseline_args() -> DesktopArgs {
        DesktopArgs {
            action: "click".into(),
            region: None,
            image_base64: None,
            x: Some(500.0),
            y: Some(500.0),
            button: None,
            text: None,
            keys: None,
            bundle_id: None,
            app: None,
            window_id: None,
            start_x: None,
            start_y: None,
            end_x: None,
            end_y: None,
            delta_x: None,
            delta_y: None,
            width: None,
            height: None,
            duration_ms: None,
            press_action: None,
            duration: None,
            fps: None,
            with_audio: None,
            display_id: None,
            format: None,
            quality: None,
            max_width: None,
            max_height: None,
            timeout_ms: None,
            actions: vec![],
            coord_space: None,
            coord_factors: None,
            script: None,
            describe: None,
            role: None,
            element_title: None,
            ax_action_name: None,
            pid: None,
            observe: None,
            force: None,
            expect: Vec::new(),
            stable_samples: None,
        }
    }

    #[test]
    fn pixel_default_is_passthrough() {
        let mut args = baseline_args();
        rescale_args_in_place(&mut args, 1920, 1080);
        assert_eq!(args.x, Some(500.0));
        assert_eq!(args.y, Some(500.0));
    }

    #[test]
    fn normalized_rescales_click_to_pixels() {
        let mut args = baseline_args();
        args.coord_space = Some("normalized".into());
        rescale_args_in_place(&mut args, 1920, 1080);
        assert!((args.x.unwrap() - 960.0).abs() < 0.001);
        assert!((args.y.unwrap() - 540.0).abs() < 0.001);
        // After rescale the args should report pixel space so any further
        // dispatcher pass does not double-convert.
        assert_eq!(args.coord_space.as_deref(), Some("pixel"));
        assert!(args.coord_factors.is_none());
    }

    #[test]
    fn normalized_rescales_drag_endpoints() {
        let mut args = baseline_args();
        args.action = "drag".into();
        args.x = None;
        args.y = None;
        args.start_x = Some(100.0);
        args.start_y = Some(200.0);
        args.end_x = Some(900.0);
        args.end_y = Some(800.0);
        args.coord_space = Some("normalized".into());
        args.coord_factors = Some([1000, 1000]);
        rescale_args_in_place(&mut args, 1920, 1080);
        assert!((args.start_x.unwrap() - 192.0).abs() < 0.001);
        assert!((args.start_y.unwrap() - 216.0).abs() < 0.001);
        assert!((args.end_x.unwrap() - 1728.0).abs() < 0.001);
        assert!((args.end_y.unwrap() - 864.0).abs() < 0.001);
    }

    #[test]
    fn normalized_rescales_region() {
        let mut args = baseline_args();
        args.action = "screenshot".into();
        args.x = None;
        args.y = None;
        args.region = Some(ScreenRegion {
            x: 250.0,
            y: 250.0,
            width: 500.0,
            height: 500.0,
        });
        args.coord_space = Some("normalized".into());
        rescale_args_in_place(&mut args, 2000, 1000);
        let r = args.region.unwrap();
        assert!((r.x - 500.0).abs() < 0.001, "x got {}", r.x);
        assert!((r.y - 250.0).abs() < 0.001, "y got {}", r.y);
        assert!((r.width - 1000.0).abs() < 0.001);
        assert!((r.height - 500.0).abs() < 0.001);
    }

    #[test]
    fn unknown_coord_space_string_defaults_to_pixel() {
        let mut args = baseline_args();
        args.coord_space = Some("weird-value".into());
        rescale_args_in_place(&mut args, 1920, 1080);
        assert_eq!(args.x, Some(500.0));
    }

    #[test]
    fn normalized_with_custom_factor() {
        let mut args = baseline_args();
        args.x = Some(750.0);
        args.y = Some(500.0);
        args.coord_space = Some("normalized".into());
        args.coord_factors = Some([1500, 1000]);
        rescale_args_in_place(&mut args, 1920, 1080);
        assert!((args.x.unwrap() - 960.0).abs() < 0.001);
        assert!((args.y.unwrap() - 540.0).abs() < 0.001);
    }

    // ── window space ──────────────────────────────────────────────────

    /// A window at (100, 60), 1200×800 points, captured on a 2× display.
    fn window() -> BoundingBox {
        BoundingBox {
            x: 100.0,
            y: 60.0,
            w: 1200.0,
            h: 800.0,
        }
    }

    #[test]
    fn window_pixel_round_trips_to_the_right_global_point() {
        // The capture is 2400×1600 px. Its centre pixel (1200, 800) is the
        // window's centre point: (100 + 600, 60 + 400).
        let (gx, gy) = window_to_global(1200.0, 800.0, &window(), 2.0, None);
        assert!((gx - 700.0).abs() < 0.001, "x got {gx}");
        assert!((gy - 460.0).abs() < 0.001, "y got {gy}");

        // The window's own origin pixel maps to the window's origin point.
        let (ox, oy) = window_to_global(0.0, 0.0, &window(), 2.0, None);
        assert!((ox - 100.0).abs() < 0.001);
        assert!((oy - 60.0).abs() < 0.001);
    }

    #[test]
    fn window_pixel_with_served_factors_survives_downscaling() {
        // The result budget downscaled the 2400×1600 capture to 600×400. The
        // model reads the centre off *that* image and sends its dimensions as
        // coord_factors: a fraction of the frame is scale-free, so the point
        // must still be the window's centre.
        let (gx, gy) = window_to_global(300.0, 200.0, &window(), 2.0, Some([600, 400]));
        assert!((gx - 700.0).abs() < 0.001, "x got {gx}");
        assert!((gy - 460.0).abs() < 0.001, "y got {gy}");
    }

    #[test]
    fn window_space_maps_every_point_field_and_marks_args_resolved() {
        let mut args = baseline_args();
        args.action = "drag".into();
        args.x = None;
        args.y = None;
        args.start_x = Some(0.0);
        args.start_y = Some(0.0);
        args.end_x = Some(2400.0);
        args.end_y = Some(1600.0);
        args.coord_space = Some("window".into());
        rescale_window_in_place(&mut args, &window(), 2.0);
        assert_eq!(args.start_x, Some(100.0));
        assert_eq!(args.start_y, Some(60.0));
        // Bottom-right pixel of the capture = bottom-right corner of the frame.
        assert_eq!(args.end_x, Some(1300.0));
        assert_eq!(args.end_y, Some(860.0));
        // Downstream must never convert a second time.
        assert_eq!(args.coord_space.as_deref(), Some("pixel"));
        assert!(args.coord_factors.is_none());
    }

    #[test]
    fn window_space_is_not_the_display_viewport() {
        // The regression this space exists to prevent: the same pixel resolved
        // against the display would ignore the window's offset entirely.
        let (gx, gy) = window_to_global(0.0, 0.0, &window(), 2.0, None);
        assert_ne!((gx, gy), (0.0, 0.0));
    }
}
