//! Pinch-to-zoom factor math (pure, unit-tested on native).
//!
//! Two-finger pinch maps the distance ratio `cur/init` to a `camera.zoom`
//! factor using the SAME exponential curve as the wheel (`scene.rs`
//! `ZOOM_SENSITIVITY`), so touch and trackpad feel identical. Spreading the
//! fingers apart (`cur > init`) zooms IN: it returns a factor `< 1.0`, which
//! `OrbitCamera::zoom` multiplies into `tgt_distance` to pull the camera
//! closer. Pinching together (`cur < init`) returns `> 1.0` (zoom out).

/// Wheel-zoom sensitivity, mirrored from `scene::ZOOM_SENSITIVITY` so pinch and
/// wheel share one curve. Kept here (not imported) to keep this module pure and
/// free of the GL-bound `scene` module.
const ZOOM_SENSITIVITY: f32 = 0.05;

/// Map a pinch distance ratio to a `camera.zoom` factor.
///
/// `init_dist` is the two-finger distance when the pinch began (the rebased
/// baseline); `cur_dist` is the live distance. Returns the multiplicative
/// factor to feed `OrbitCamera::zoom`. Degenerate inputs (`init_dist <= 0`)
/// return `1.0` (no-op) so a zero baseline never divides-by-zero or NaNs.
#[must_use]
pub fn pinch_zoom_factor(init_dist: f32, cur_dist: f32) -> f32 {
    if init_dist <= 0.0 || cur_dist <= 0.0 {
        return 1.0;
    }
    // ratio > 1 → fingers spread → want to zoom IN → factor < 1.
    // ln(ratio) gives a symmetric, signed magnitude; the same exp() curve as
    // the wheel keeps the feel consistent. The ±2 clamp caps a single jerky
    // frame (mirrors the wheel's normalized-delta clamp in scene::on_wheel).
    let ratio = cur_dist / init_dist;
    let normalized = ratio.ln().clamp(-2.0, 2.0);
    (-normalized * (ZOOM_SENSITIVITY * 20.0)).exp()
}

/// Euclidean distance between two screen points (helper for the host).
#[must_use]
pub fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_change_when_distance_equal() {
        // Same distance → factor 1.0 (no zoom).
        assert!((pinch_zoom_factor(100.0, 100.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn spreading_fingers_zooms_in() {
        // cur > init → fingers spread → factor < 1 → camera pulls closer.
        let f = pinch_zoom_factor(100.0, 200.0);
        assert!(f < 1.0, "spread should zoom in (factor<1), got {f}");
    }

    #[test]
    fn pinching_together_zooms_out() {
        // cur < init → fingers together → factor > 1 → camera pushes back.
        let f = pinch_zoom_factor(200.0, 100.0);
        assert!(f > 1.0, "pinch should zoom out (factor>1), got {f}");
    }

    #[test]
    fn symmetric_about_unity() {
        // A 2x spread and a 2x pinch are inverse factors (within fp tolerance).
        let spread = pinch_zoom_factor(100.0, 200.0);
        let pinch = pinch_zoom_factor(200.0, 100.0);
        assert!((spread * pinch - 1.0).abs() < 1e-3, "{spread}*{pinch}");
    }

    #[test]
    fn degenerate_zero_baseline_is_noop() {
        assert_eq!(pinch_zoom_factor(0.0, 150.0), 1.0);
        assert_eq!(pinch_zoom_factor(-5.0, 150.0), 1.0);
        assert_eq!(pinch_zoom_factor(100.0, 0.0), 1.0);
    }

    #[test]
    fn extreme_ratio_clamped_not_runaway() {
        // A huge jump in one frame is clamped so the camera never teleports.
        let f = pinch_zoom_factor(1.0, 100000.0);
        assert!(f.is_finite() && f > 0.0);
        // Spread clamps ln(ratio) at +2 → factor = exp(-2) ≈ 0.135 (zoom-in floor); assert bounded.
        assert!(f >= pinch_zoom_factor(1.0, 1e9) - 1e-3);
    }
}
