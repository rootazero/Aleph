//! Viewport math and input-normalization logic for the whiteboard editor —
//! every function here is pure (no `web_sys`), unit-tested on the native
//! target. The DOM handlers in `editor.rs` are thin wiring over these.
//!
//! # Coordinate model
//!
//! [`Camera`] (in `state/canvas.rs`, so the state can hold one) reads:
//! `x`/`y` are the **world** coordinates at the viewport's top-left corner,
//! `zoom` is screen-pixels-per-world-unit. So:
//!
//! ```text
//! world  = camera.xy + screen / zoom      (screen_to_world)
//! screen = (world − camera.xy) * zoom     (world_to_screen)
//! ```
//!
//! # Gesture mapping (spec §4)
//!
//! - Ctrl/⌘ + wheel = zoom ([`wheel_intent`]). A trackpad **pinch** arrives
//!   from the browser as exactly this (a wheel event with `ctrlKey` set), so
//!   the pinch gesture needs no second path. Two-finger *touchscreen* pinch
//!   is not handled here — pointer gestures beyond panning are the Task 13
//!   interaction state machine.
//! - Bare wheel = pan (both axes — trackpads report `deltaX` too).
//! - Space + drag, middle-button drag, or the Pan tool = pointer pan
//!   ([`pan_gesture`], [`PanDrag`]).
//!
//! Wheel deltas are normalized to pixels first ([`normalized_wheel_px`]) —
//! the galaxy `pointer_input.rs` three-arm `delta_mode` idiom: Firefox
//! reports LINE mode (≈3 lines per notch), and an unnormalized LINE delta
//! would zoom ~50× slower than Chrome's pixel delta.

use crate::state::canvas::{Camera, CanvasTool};

/// Zoom clamp. The low bound keeps a runaway zoom-out from collapsing every
/// shape into sub-pixel noise; the high bound keeps one wheel notch from
/// crossing float-precision territory on a large canvas.
pub(super) const MIN_ZOOM: f64 = 0.05;
pub(super) const MAX_ZOOM: f64 = 8.0;

/// A wheel notch in line mode (Firefox's default) — one line ≈ 16 px.
/// Same constant as the galaxy's `WHEEL_LINE_PX`.
const WHEEL_LINE_PX: f64 = 16.0;
/// A wheel notch in page mode — one page ≈ a viewport.
const WHEEL_PAGE_PX: f64 = 400.0;

/// Zoom rate per normalized wheel pixel: `factor = exp(−px · RATE)`.
/// One firm mouse notch (~100 px) ≈ ×0.86, ten notches ≈ ×0.22 — matches the
/// feel of tldraw/Figma wheel zoom closely enough to not surprise.
const WHEEL_ZOOM_RATE: f64 = 0.0015;

/// Screen (viewport-local CSS px) → world.
#[must_use]
pub(super) fn screen_to_world(cam: Camera, sx: f64, sy: f64) -> (f64, f64) {
    (cam.x + sx / cam.zoom, cam.y + sy / cam.zoom)
}

/// World → screen (viewport-local CSS px).
#[must_use]
pub(super) fn world_to_screen(cam: Camera, wx: f64, wy: f64) -> (f64, f64) {
    ((wx - cam.x) * cam.zoom, (wy - cam.y) * cam.zoom)
}

/// Multiply the zoom by `factor`, keeping the world point under `cursor`
/// (viewport-local CSS px) exactly where it is on screen — the anchor
/// property the test below pins. The zoom is clamped; the anchor holds for
/// the clamped value too (the anchor is solved against the zoom actually
/// applied, not the zoom requested).
#[must_use]
pub(super) fn zoom_at(cam: Camera, cursor: (f64, f64), factor: f64) -> Camera {
    let zoom = (cam.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
    let (wx, wy) = screen_to_world(cam, cursor.0, cursor.1);
    Camera {
        x: wx - cursor.0 / zoom,
        y: wy - cursor.1 / zoom,
        zoom,
    }
}

/// Drag the *content* by a screen-space delta: the world point under the
/// pointer follows the pointer. (The camera moves opposite the content.)
#[must_use]
pub(super) fn pan_by(cam: Camera, dx_px: f64, dy_px: f64) -> Camera {
    Camera {
        x: cam.x - dx_px / cam.zoom,
        y: cam.y - dy_px / cam.zoom,
        zoom: cam.zoom,
    }
}

/// Bare-wheel pan: scroll down (positive `deltaY`) moves the content up,
/// like every scrollable surface — i.e. the content drags by the *negated*
/// wheel delta.
#[must_use]
pub(super) fn wheel_pan(cam: Camera, dx_px: f64, dy_px: f64) -> Camera {
    pan_by(cam, -dx_px, -dy_px)
}

/// Normalize a wheel delta to pixels across the three `delta_mode`s
/// (0 = PIXEL, 1 = LINE, 2 = PAGE) — the galaxy idiom, verbatim.
#[must_use]
pub(super) fn normalized_wheel_px(delta_mode: u32, delta: f64) -> f64 {
    match delta_mode {
        1 => delta * WHEEL_LINE_PX,
        2 => delta * WHEEL_PAGE_PX,
        _ => delta,
    }
}

/// Wheel-pixels → zoom factor. Positive delta (scroll down / pinch in)
/// zooms out (< 1), negative zooms in (> 1), zero is exactly 1.
#[must_use]
pub(super) fn wheel_zoom_factor(px: f64) -> f64 {
    (-px * WHEEL_ZOOM_RATE).exp()
}

/// What a wheel event means. Trackpad pinches arrive as Ctrl-wheel, so this
/// single predicate covers both the chord and the gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WheelIntent {
    Zoom,
    Pan,
}

#[must_use]
pub(super) fn wheel_intent(ctrl: bool, meta: bool) -> WheelIntent {
    if ctrl || meta {
        WheelIntent::Zoom
    } else {
        WheelIntent::Pan
    }
}

/// Does this pointer-down start a pan? Middle button always; left button
/// only while Space is held or the Pan tool is active.
#[must_use]
pub(super) fn pan_gesture(button: i16, space_down: bool, tool: CanvasTool) -> bool {
    button == 1 || (button == 0 && (space_down || tool == CanvasTool::Pan))
}

/// An in-flight pointer pan: which pointer owns it and where it last was
/// (client px — deltas only, so no element origin is needed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PanDrag {
    pub(super) pointer_id: i32,
    last: (f64, f64),
}

impl PanDrag {
    #[must_use]
    pub(super) fn begin(pointer_id: i32, x: f64, y: f64) -> Self {
        Self {
            pointer_id,
            last: (x, y),
        }
    }

    /// Advance to a new pointer position, returning the screen delta since
    /// the last sample — or `None` for a pointer that does not own this drag
    /// (a second finger must not yank the camera).
    #[must_use]
    pub(super) fn advance(&mut self, pointer_id: i32, x: f64, y: f64) -> Option<(f64, f64)> {
        if pointer_id != self.pointer_id {
            return None;
        }
        let (lx, ly) = self.last;
        self.last = (x, y);
        Some((x - lx, y - ly))
    }
}

/// SVG group transform for the shape layer: world → screen as a single
/// `translate(…) scale(…)`. The translate component is, by definition, the
/// screen position of the world origin — so it is *derived* from
/// [`world_to_screen`] rather than re-stating the algebra.
#[must_use]
pub(super) fn svg_transform(cam: Camera) -> String {
    let (tx, ty) = world_to_screen(cam, 0.0, 0.0);
    format!("translate({tx} {ty}) scale({})", cam.zoom)
}

/// The HTML overlay's style: the *same* world → screen transform, with
/// `transform-origin: 0 0` — CSS defaults to the element's center, which
/// would silently shear the overlay away from the SVG layer under zoom.
#[must_use]
pub(super) fn css_transform(cam: Camera) -> String {
    let (tx, ty) = world_to_screen(cam, 0.0, 0.0);
    format!(
        "transform: translate({tx}px, {ty}px) scale({}); transform-origin: 0 0;",
        cam.zoom
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(x: f64, y: f64, zoom: f64) -> Camera {
        Camera { x, y, zoom }
    }

    /// Deterministic LCG in [0, 1) — a property test without a rand dep.
    fn lcg(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((*seed >> 33) as f64) / ((1u64 << 31) as f64)
    }

    #[test]
    fn screen_and_world_are_inverse_of_each_other() {
        let c = cam(123.5, -77.25, 2.5);
        let (wx, wy) = screen_to_world(c, 400.0, 300.0);
        let (sx, sy) = world_to_screen(c, wx, wy);
        assert!((sx - 400.0).abs() < 1e-9 && (sy - 300.0).abs() < 1e-9);
    }

    /// The zoom anchor property: for any camera, cursor and factor, the world
    /// point under the cursor before `zoom_at` is still under it after.
    #[test]
    fn zoom_at_keeps_the_world_point_under_the_cursor_fixed() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..500 {
            let c = cam(
                (lcg(&mut seed) - 0.5) * 2e4,
                (lcg(&mut seed) - 0.5) * 2e4,
                MIN_ZOOM + lcg(&mut seed) * (MAX_ZOOM - MIN_ZOOM),
            );
            let cursor = (lcg(&mut seed) * 2000.0, lcg(&mut seed) * 1200.0);
            let factor = 0.2 + lcg(&mut seed) * 4.8;
            let before = screen_to_world(c, cursor.0, cursor.1);
            let zoomed = zoom_at(c, cursor, factor);
            let after = screen_to_world(zoomed, cursor.0, cursor.1);
            let tol = 1e-9 * (1.0 + before.0.abs().max(before.1.abs()));
            assert!(
                (after.0 - before.0).abs() <= tol && (after.1 - before.1).abs() <= tol,
                "anchor drifted: cam={c:?} cursor={cursor:?} factor={factor} \
                 before={before:?} after={after:?}"
            );
        }
    }

    /// Clamping changes the zoom that is applied, not the anchor property:
    /// the cursor's world point must hold for the *clamped* zoom too.
    #[test]
    fn zoom_at_clamps_and_still_anchors_the_cursor() {
        let c = cam(10.0, 20.0, 4.0);
        let cursor = (300.0, 200.0);
        let before = screen_to_world(c, cursor.0, cursor.1);
        let zoomed = zoom_at(c, cursor, 100.0);
        assert!(
            (zoomed.zoom - MAX_ZOOM).abs() < f64::EPSILON,
            "a runaway factor must clamp to MAX_ZOOM, got {}",
            zoomed.zoom
        );
        let after = screen_to_world(zoomed, cursor.0, cursor.1);
        assert!((after.0 - before.0).abs() < 1e-9 && (after.1 - before.1).abs() < 1e-9);
    }

    /// Panning by a screen delta drags the content with the pointer: the
    /// world point that was under the pointer's start is under its end.
    #[test]
    fn pan_by_keeps_the_world_point_under_the_moving_pointer() {
        let c = cam(-50.0, 340.0, 0.75);
        let (sx, sy) = (120.0, 480.0);
        let (dx, dy) = (37.5, -12.25);
        let before = screen_to_world(c, sx, sy);
        let panned = pan_by(c, dx, dy);
        let after = screen_to_world(panned, sx + dx, sy + dy);
        assert!((after.0 - before.0).abs() < 1e-9 && (after.1 - before.1).abs() < 1e-9);
    }

    /// Scroll down (positive deltaY) moves the content up — the camera's
    /// world-y grows — exactly like a scrollable page.
    #[test]
    fn wheel_pan_scrolls_content_like_a_page() {
        let c = cam(0.0, 0.0, 2.0);
        let panned = wheel_pan(c, 0.0, 100.0);
        assert!(
            panned.y > c.y,
            "scroll down must move the viewport down in world space"
        );
        assert!((panned.y - 50.0).abs() < 1e-9, "screen px / zoom = world");
    }

    /// The three-arm `delta_mode` normalization, verbatim from the galaxy:
    /// LINE ×16, PAGE ×400, PIXEL untouched.
    #[test]
    fn wheel_delta_modes_normalize_to_pixels() {
        assert!((normalized_wheel_px(0, 120.0) - 120.0).abs() < f64::EPSILON);
        assert!((normalized_wheel_px(1, 3.0) - 48.0).abs() < f64::EPSILON);
        assert!((normalized_wheel_px(2, 1.0) - 400.0).abs() < f64::EPSILON);
    }

    #[test]
    fn wheel_zoom_factor_direction_matches_scroll_direction() {
        assert!(wheel_zoom_factor(100.0) < 1.0, "scroll down zooms out");
        assert!(wheel_zoom_factor(-100.0) > 1.0, "scroll up zooms in");
        assert!((wheel_zoom_factor(0.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn wheel_intent_is_zoom_only_under_ctrl_or_meta() {
        assert_eq!(wheel_intent(true, false), WheelIntent::Zoom);
        assert_eq!(wheel_intent(false, true), WheelIntent::Zoom);
        assert_eq!(wheel_intent(false, false), WheelIntent::Pan);
    }

    #[test]
    fn pan_gesture_covers_middle_button_space_and_the_pan_tool() {
        assert!(pan_gesture(1, false, CanvasTool::Select), "middle button");
        assert!(pan_gesture(0, true, CanvasTool::Select), "space + left");
        assert!(pan_gesture(0, false, CanvasTool::Pan), "pan tool + left");
        assert!(
            !pan_gesture(0, false, CanvasTool::Select),
            "bare left drag is not a pan"
        );
        assert!(
            !pan_gesture(2, true, CanvasTool::Pan),
            "right button never pans"
        );
    }

    #[test]
    fn pan_drag_yields_deltas_only_for_its_own_pointer() {
        let mut drag = PanDrag::begin(7, 100.0, 200.0);
        assert_eq!(
            drag.advance(9, 500.0, 500.0),
            None,
            "a second finger is ignored"
        );
        assert_eq!(drag.advance(7, 110.0, 195.0), Some((10.0, -5.0)));
        assert_eq!(
            drag.advance(7, 110.0, 195.0),
            Some((0.0, 0.0)),
            "advance must update the sample point"
        );
    }

    /// Both layer transforms encode the same world→screen mapping, with the
    /// CSS one carrying the mandatory `transform-origin: 0 0`.
    #[test]
    fn the_svg_and_css_transforms_agree() {
        let c = cam(100.0, 50.0, 2.0);
        assert_eq!(svg_transform(c), "translate(-200 -100) scale(2)");
        assert_eq!(
            css_transform(c),
            "transform: translate(-200px, -100px) scale(2); transform-origin: 0 0;"
        );
    }
}
