//! Normalize UI-TARS-style coordinate inputs to pixel space before dispatch.
//!
//! The model (Aleph caller, or a UI-TARS-finetuned VLM) emits coordinates in
//! whichever [`aleph_desktop::CoordinateSpace`] it was prompted with. The
//! actual native bridges (`desktop/macos`, future `desktop/linux`, etc.) only
//! accept pixel coordinates. This module bridges that gap with a pure,
//! synchronously-testable rescale plus a thin async helper that resolves the
//! viewport once at dispatch time.

use crate::sync_primitives::Arc;

use aleph_desktop::{CoordinateSpace, DesktopPlatform};

use super::types::DesktopArgs;
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

/// Convenience wrapper for the common path.
pub async fn maybe_normalize(
    args: &mut DesktopArgs,
    platform: &Arc<dyn DesktopPlatform>,
) -> Result<()> {
    if matches!(coord_space_of(args), CoordinateSpace::Pixel) {
        return Ok(());
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
    Ok(())
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
}
