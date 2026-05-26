//! Alignment guide detection — tldraw-style snap lines for node drag.
//!
//! Pure data layer: no rendering, no canvas APIs, no DOM. The renderer queries
//! [`detect_guides`] once per drag tick and draws the returned `GuideLine`s as
//! thin gold strokes across the viewport.
//!
//! Reference: `/Volumes/TBU4/Github/tldraw/` shape-aligning UX. The numbers
//! match tldraw's defaults (4 px world snap, single guide per axis).

use crate::canvas_engine::types::{Neighborhood, Vec2};

/// World-space distance within which two nodes are considered aligned.
/// 4 px in world units; the renderer scales by viewport zoom for drawing.
pub const SNAP_THRESHOLD: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Axis {
    /// Vertical line — runs top-to-bottom; nodes share the same `x`.
    Vertical,
    /// Horizontal line — runs left-to-right; nodes share the same `y`.
    Horizontal,
}

/// A single alignment line to draw during a drag.
#[derive(Debug, Clone)]
pub struct GuideLine {
    pub axis: Axis,
    /// World-coord of the line on the perpendicular axis.
    /// For [`Axis::Vertical`]: the shared `x`. For [`Axis::Horizontal`]: the shared `y`.
    pub world_coord: f64,
    /// Id of the anchor node that the dragged node aligned to.
    pub anchor_id: String,
    /// Signed offset (drag − anchor) on the aligned axis, before snap. Callers
    /// can subtract this from the drag position to lock perfect alignment.
    pub delta: f64,
}

/// Detect at most two guides (one per axis) between a dragged node and the
/// candidate set.
///
/// `candidates` MUST exclude the dragged node itself; callers know best which
/// set is being checked, so this function does not filter.
///
/// When multiple anchors qualify on the same axis, the closest one wins —
/// stable on ties via insertion order (first hit kept).
pub fn detect_guides(
    drag_pos: Vec2,
    candidates: &[(String, Vec2)],
    threshold: f64,
) -> Vec<GuideLine> {
    // (abs_distance, anchor_id, signed_delta)
    let mut best_v: Option<(f64, &str, f64)> = None;
    let mut best_h: Option<(f64, &str, f64)> = None;

    for (id, pos) in candidates {
        let dx = drag_pos.x - pos.x;
        let dy = drag_pos.y - pos.y;

        let dxa = dx.abs();
        if dxa <= threshold && best_v.map(|(d, _, _)| dxa < d).unwrap_or(true) {
            best_v = Some((dxa, id.as_str(), dx));
        }

        let dya = dy.abs();
        if dya <= threshold && best_h.map(|(d, _, _)| dya < d).unwrap_or(true) {
            best_h = Some((dya, id.as_str(), dy));
        }
    }

    let mut out = Vec::with_capacity(2);
    if let Some((_, id, dx)) = best_v {
        out.push(GuideLine {
            axis: Axis::Vertical,
            world_coord: drag_pos.x - dx,
            anchor_id: id.to_string(),
            delta: dx,
        });
    }
    if let Some((_, id, dy)) = best_h {
        out.push(GuideLine {
            axis: Axis::Horizontal,
            world_coord: drag_pos.y - dy,
            anchor_id: id.to_string(),
            delta: dy,
        });
    }
    out
}

/// Adapter that pulls candidate anchors out of a [`Neighborhood`] and runs
/// [`detect_guides`] against the dragged node's current world position.
///
/// Anchors use `CanvasNode::position` (the resolved layout position, drift-free)
/// so guides do not jitter per frame as idle drift offsets the visible dots.
/// The dragged node is excluded from candidates by id.
pub fn guides_for_drag(
    drag_pos: Vec2,
    nbhd: &Neighborhood,
    dragged_id: &str,
) -> Vec<GuideLine> {
    let cap = 1 + nbhd.one_hop.len() + nbhd.two_hop.len();
    let mut candidates: Vec<(String, Vec2)> = Vec::with_capacity(cap);
    if nbhd.center.id != dragged_id {
        candidates.push((nbhd.center.id.clone(), nbhd.center.position));
    }
    for n in &nbhd.one_hop {
        if n.id != dragged_id {
            candidates.push((n.id.clone(), n.position));
        }
    }
    for n in &nbhd.two_hop {
        if n.id != dragged_id {
            candidates.push((n.id.clone(), n.position));
        }
    }
    detect_guides(drag_pos, &candidates, SNAP_THRESHOLD)
}

/// Apply the detected guides to a position, returning the snapped result.
///
/// At most one guide per axis is honoured (the same invariant
/// [`detect_guides`] enforces). Position is returned unchanged when no guide
/// targets the corresponding axis.
pub fn snap_to_guides(pos: Vec2, guides: &[GuideLine]) -> Vec2 {
    let mut snapped = pos;
    for g in guides {
        match g.axis {
            Axis::Vertical => snapped.x = g.world_coord,
            Axis::Horizontal => snapped.y = g.world_coord,
        }
    }
    snapped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(x: f64, y: f64) -> Vec2 {
        Vec2 { x, y }
    }

    #[test]
    fn detects_vertical_guide_when_x_within_threshold() {
        let drag = pos(100.0, 200.0);
        let candidates = vec![("a".to_string(), pos(102.0, 0.0))];
        let guides = detect_guides(drag, &candidates, SNAP_THRESHOLD);
        assert_eq!(guides.len(), 1);
        assert_eq!(guides[0].axis, Axis::Vertical);
        assert_eq!(guides[0].anchor_id, "a");
        assert!((guides[0].world_coord - 102.0).abs() < 1e-9);
        assert!((guides[0].delta - (-2.0)).abs() < 1e-9);
    }

    #[test]
    fn detects_horizontal_guide_when_y_within_threshold() {
        let drag = pos(100.0, 50.0);
        let candidates = vec![("a".to_string(), pos(500.0, 51.5))];
        let guides = detect_guides(drag, &candidates, SNAP_THRESHOLD);
        assert_eq!(guides.len(), 1);
        assert_eq!(guides[0].axis, Axis::Horizontal);
        assert!((guides[0].world_coord - 51.5).abs() < 1e-9);
    }

    #[test]
    fn detects_both_axes_when_drag_corners_align() {
        let drag = pos(100.0, 50.0);
        let candidates = vec![
            ("x".to_string(), pos(101.0, 999.0)), // vertical match
            ("y".to_string(), pos(999.0, 51.0)),  // horizontal match
        ];
        let guides = detect_guides(drag, &candidates, SNAP_THRESHOLD);
        assert_eq!(guides.len(), 2);
        let by_axis: std::collections::HashMap<_, _> =
            guides.iter().map(|g| (g.axis, g.anchor_id.clone())).collect();
        assert_eq!(by_axis[&Axis::Vertical], "x".to_string());
        assert_eq!(by_axis[&Axis::Horizontal], "y".to_string());
    }

    #[test]
    fn no_guide_when_all_candidates_outside_threshold() {
        let drag = pos(100.0, 50.0);
        let candidates = vec![("a".to_string(), pos(120.0, 80.0))];
        let guides = detect_guides(drag, &candidates, SNAP_THRESHOLD);
        assert!(guides.is_empty());
    }

    #[test]
    fn closest_anchor_wins_on_same_axis() {
        let drag = pos(100.0, 50.0);
        let candidates = vec![
            ("far".to_string(), pos(103.5, 0.0)),  // dx=3.5
            ("near".to_string(), pos(101.0, 0.0)), // dx=1.0
            ("ok".to_string(), pos(102.0, 0.0)),   // dx=2.0
        ];
        let guides = detect_guides(drag, &candidates, SNAP_THRESHOLD);
        assert_eq!(guides.len(), 1);
        assert_eq!(guides[0].anchor_id, "near");
    }

    #[test]
    fn empty_candidates_yields_empty_guides() {
        let drag = pos(100.0, 50.0);
        let guides = detect_guides(drag, &[], SNAP_THRESHOLD);
        assert!(guides.is_empty());
    }

    #[test]
    fn snap_to_guides_locks_drag_to_anchor_position() {
        let drag = pos(100.0, 50.0);
        let candidates = vec![
            ("v".to_string(), pos(102.0, 0.0)),
            ("h".to_string(), pos(0.0, 51.5)),
        ];
        let guides = detect_guides(drag, &candidates, SNAP_THRESHOLD);
        let snapped = snap_to_guides(drag, &guides);
        assert!((snapped.x - 102.0).abs() < 1e-9);
        assert!((snapped.y - 51.5).abs() < 1e-9);
    }

    #[test]
    fn snap_to_guides_passthrough_when_empty() {
        let p = pos(100.0, 50.0);
        let snapped = snap_to_guides(p, &[]);
        assert_eq!(snapped.x, p.x);
        assert_eq!(snapped.y, p.y);
    }

    fn mk_node(id: &str, x: f64, y: f64) -> crate::canvas_engine::types::CanvasNode {
        crate::canvas_engine::types::CanvasNode {
            id: id.into(),
            name: id.into(),
            category: String::new(),
            color: crate::canvas_engine::types::Color { r: 0, g: 0, b: 0 },
            radius: 6.0,
            position: Vec2::new(x, y),
            velocity: Vec2::zero(),
            z: 0.0,
            hop: 1,
            decay_score: 1.0,
            edge_count: 0,
        }
    }

    fn mk_nbhd(
        center: crate::canvas_engine::types::CanvasNode,
        one_hop: Vec<crate::canvas_engine::types::CanvasNode>,
        two_hop: Vec<crate::canvas_engine::types::CanvasNode>,
    ) -> Neighborhood {
        Neighborhood {
            center,
            one_hop,
            two_hop,
            orphans: Vec::new(),
            clusters: Vec::new(),
            edges: Vec::new(),
            target_positions: std::collections::HashMap::new(),
            fetched_at_ms: 0.0,
        }
    }

    #[test]
    fn guides_for_drag_excludes_dragged_node() {
        // Dragged node at (100,50) — if its own position is treated as a candidate
        // the helper would self-match with delta=0, producing 2 spurious guides.
        let nbhd = mk_nbhd(
            mk_node("center", 0.0, 0.0),
            vec![mk_node("dragged", 100.0, 50.0)],
            vec![],
        );
        let guides = guides_for_drag(Vec2::new(100.0, 50.0), &nbhd, "dragged");
        // No other candidate within threshold of (100,50) → empty guides.
        assert!(
            guides.is_empty(),
            "self-match should be filtered; got {} guides",
            guides.len()
        );
    }

    #[test]
    fn guides_for_drag_includes_center_and_two_hop() {
        let nbhd = mk_nbhd(
            mk_node("center", 100.0, 0.0), // vertical match @ drag.x=100
            vec![mk_node("dragged", 200.0, 200.0)],
            vec![mk_node("far", 999.0, 50.5)], // horizontal match @ drag.y=50
        );
        let guides = guides_for_drag(Vec2::new(100.0, 50.0), &nbhd, "dragged");
        assert_eq!(guides.len(), 2, "expected one guide per axis");
        let anchors: std::collections::HashSet<_> =
            guides.iter().map(|g| g.anchor_id.clone()).collect();
        assert!(anchors.contains("center"));
        assert!(anchors.contains("far"));
    }

    #[test]
    fn guides_for_drag_skips_when_dragged_is_center() {
        let nbhd = mk_nbhd(
            mk_node("dragged", 100.0, 50.0),
            vec![mk_node("anchor", 102.0, 80.0)],
            vec![],
        );
        let guides = guides_for_drag(Vec2::new(100.0, 50.0), &nbhd, "dragged");
        // Center filtered out; anchor at x=102 within threshold → vertical guide.
        assert_eq!(guides.len(), 1);
        assert_eq!(guides[0].anchor_id, "anchor");
        assert_eq!(guides[0].axis, Axis::Vertical);
    }

    #[test]
    fn threshold_at_exact_boundary_is_inclusive() {
        // dx == SNAP_THRESHOLD exactly → still considered aligned
        let drag = pos(100.0, 0.0);
        let candidates = vec![("edge".to_string(), pos(100.0 + SNAP_THRESHOLD, 0.0))];
        let guides = detect_guides(drag, &candidates, SNAP_THRESHOLD);
        assert_eq!(guides.len(), 2, "exact threshold should match both axes (dy=0 too)");
    }
}
