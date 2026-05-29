//! Quadratic-Bézier helpers for the new edge rendering.

use crate::canvas_engine::types::Vec2;

/// Sag coefficient: how much the curve bows out perpendicular to its chord.
/// `0.0` = straight line; `0.12` is the default the spec calls for.
pub(crate) const DEFAULT_SAG: f64 = 0.12;

/// Edge kinds that are *directional*. Consumed by the JSON Canvas export
/// (`json_canvas::convert::edge_to_canvas`), which sets `to_end: Arrow` for
/// these kinds; everything else (`None`, `"related"`, any unknown string)
/// exports as a plain edge. The on-canvas 2D renderer currently draws all
/// edges as plain Béziers — arrow heads live only in the export.
pub(crate) const DIRECTIONAL_KINDS: &[&str] = &["refers", "derives", "follows"];

/// Quadratic-Bézier control point: midpoint, shoved perpendicular to
/// the chord by `length × sag_coef`. Sign of `sag_coef` controls which
/// side of the chord the curve bows toward.
pub(crate) fn edge_control_point(from: Vec2, to: Vec2, sag_coef: f64) -> Vec2 {
    let mid = Vec2 { x: (from.x + to.x) * 0.5, y: (from.y + to.y) * 0.5 };
    let dir = {
        let dx = to.x - from.x;
        let dy = to.y - from.y;
        let len = (dx * dx + dy * dy).sqrt().max(1.0e-6);
        Vec2 { x: dx / len, y: dy / len }
    };
    let perp = Vec2 { x: -dir.y, y: dir.x };  // 90° CCW
    let len = {
        let dx = to.x - from.x;
        let dy = to.y - from.y;
        (dx * dx + dy * dy).sqrt()
    };
    let sag = len * sag_coef;
    Vec2 { x: mid.x + perp.x * sag, y: mid.y + perp.y * sag }
}

/// Position along the Bézier at `t ∈ [0, 1]` for a quadratic curve
/// with end-points `p0`, `p2` and control point `p1`.
pub(crate) fn bezier_point(p0: Vec2, p1: Vec2, p2: Vec2, t: f64) -> Vec2 {
    let mt = 1.0 - t;
    Vec2 {
        x: mt * mt * p0.x + 2.0 * mt * t * p1.x + t * t * p2.x,
        y: mt * mt * p0.y + 2.0 * mt * t * p1.y + t * t * p2.y,
    }
}

/// Tangent angle (radians) at `t` on the Bézier. Used to align edge labels.
/// Clamped to `[-π/4, π/4]` outside this function by callers (so labels never invert).
pub(crate) fn bezier_tangent(p0: Vec2, p1: Vec2, p2: Vec2, t: f64) -> f64 {
    let mt = 1.0 - t;
    let dx = 2.0 * mt * (p1.x - p0.x) + 2.0 * t * (p2.x - p1.x);
    let dy = 2.0 * mt * (p1.y - p0.y) + 2.0 * t * (p2.y - p1.y);
    dy.atan2(dx)
}

/// Per-hop visual layer for edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HopLayer { One, Two }

/// `(max_alpha, line_width)` tuple per layer.
pub(crate) fn hop_style(layer: HopLayer) -> (f64, f64) {
    match layer {
        // 1-hop stroke at 2.0px matches the code-call-graph-editor edge weight.
        HopLayer::One => (0.85, 2.0),
        HopLayer::Two => (0.55, 1.2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 { Vec2 { x, y } }

    #[test]
    fn bezier_control_point_deterministic() {
        let a = edge_control_point(v(0.0, 0.0), v(100.0, 0.0), DEFAULT_SAG);
        let b = edge_control_point(v(0.0, 0.0), v(100.0, 0.0), DEFAULT_SAG);
        assert_eq!(a.x, b.x);
        assert_eq!(a.y, b.y);
    }

    #[test]
    fn control_point_perpendicular_to_edge_axis() {
        // Horizontal edge → control point displaced purely in y
        let cp = edge_control_point(v(0.0, 0.0), v(100.0, 0.0), DEFAULT_SAG);
        assert!((cp.x - 50.0).abs() < 0.1);
        assert!(cp.y.abs() > 5.0, "expected vertical offset, got y={}", cp.y);
    }

    #[test]
    fn bezier_point_at_t_05_matches_formula() {
        // B(0.5) = 0.25·p0 + 0.5·p1 + 0.25·p2
        let p0 = v(0.0, 0.0);
        let p2 = v(100.0, 0.0);
        let p1 = edge_control_point(p0, p2, DEFAULT_SAG);
        let mid = bezier_point(p0, p1, p2, 0.5);
        let expected = Vec2 {
            x: 0.25 * p0.x + 0.5 * p1.x + 0.25 * p2.x,
            y: 0.25 * p0.y + 0.5 * p1.y + 0.25 * p2.y,
        };
        assert!((mid.x - expected.x).abs() < 1e-4);
        assert!((mid.y - expected.y).abs() < 1e-4);
    }

    #[test]
    fn hop_style_layered_by_hop() {
        let (a1, w1) = hop_style(HopLayer::One);
        let (a2, w2) = hop_style(HopLayer::Two);
        assert!(a1 > a2);
        assert!(w1 > w2);
    }
}
