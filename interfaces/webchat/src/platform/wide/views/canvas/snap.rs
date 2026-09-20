//! Point snapping for a translate drag — pure, zero DOM, unit-tested on the
//! native target. `interaction.rs` calls [`snap_translate`] on every Move
//! step (preview and commit through the same derivation) and `editor.rs`
//! draws the returned [`Guide`]s as hairlines inside the world transform.
//!
//! # What snaps to what
//!
//! Each bbox contributes its four corners and its center, which per axis
//! collapses to three candidates: `{left, center, right}` on x and
//! `{top, center, bottom}` on y. The moving box's candidates are compared
//! against every other box's on each axis **independently**; the pair with
//! the smallest absolute delta wins, and if that delta is inside the
//! threshold the box is nudged by it and one guide is emitted for the axis.
//! So x may snap while y does not, and a drag can end up on two guides at
//! once (a vertical and a horizontal one).
//!
//! The threshold is in **world** units; the editor passes 8 screen px ÷ zoom
//! (tldraw's constant), so the snap radius stays 8 px on screen at any zoom.
//!
//! # Scope (spec §5)
//!
//! Only translation snaps: creation drags ([`Drag::Create`]), resize
//! handles, ink and arrows are untouched. No gap chains (equal spacing
//! between three boxes), no rotation, no distance labels — point-to-point
//! alignment only.
//!
//! [`Drag::Create`]: super::interaction::Drag::Create

use super::interaction::Bbox;

/// Which axis a guide constrains. An `X` guide is a vertical line at a
/// world x (the boxes agree on an x coordinate); a `Y` guide is horizontal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Axis {
    X,
    Y,
}

/// One alignment line to draw: `at` is the shared coordinate on `axis`;
/// `from`/`to` span the **other** axis from the lowest to the highest
/// extent of the two boxes that agreed, so the line visibly links them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Guide {
    pub axis: Axis,
    pub at: f64,
    pub from: f64,
    pub to: f64,
}

/// The result of [`snap_translate`]: a nudge to add to the raw drag delta
/// (zero on an axis that did not snap) and the guides to draw.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct Snap {
    pub dx: f64,
    pub dy: f64,
    pub guides: Vec<Guide>,
}

/// Snap `moving` (the selection's union bbox at its raw dragged position)
/// against `others`.
///
/// `others` must not contain the moving selection's own boxes: a box is at
/// delta 0 from itself, which would win every comparison and pin the drag
/// with a guide drawn over the box (`a_box_listed_among_others_snaps_to_itself`
/// is the test that documents why the caller filters). A threshold of zero
/// (or less) never snaps — the comparison is strict, so "within 0" is empty
/// rather than "exactly aligned only".
///
/// Ties resolve deterministically: the smallest delta wins; among equal
/// deltas the earliest `others` entry, then the earliest candidate pair.
#[must_use]
pub(super) fn snap_translate(moving: Bbox, others: &[Bbox], threshold: f64) -> Snap {
    let best_x = best_delta(
        x_candidates(moving),
        others.iter().map(|b| x_candidates(*b)),
        threshold,
    );
    let best_y = best_delta(
        y_candidates(moving),
        others.iter().map(|b| y_candidates(*b)),
        threshold,
    );
    let (dx, dy) = (
        best_x.map_or(0.0, |(d, _)| d),
        best_y.map_or(0.0, |(d, _)| d),
    );
    // Guide spans are measured at the *snapped* position on both axes: a
    // vertical guide's y-extent must cover the box where it will be drawn,
    // which a y-snap may have moved.
    let snapped = Bbox {
        x: moving.x + dx,
        y: moving.y + dy,
        ..moving
    };
    let mut guides = Vec::with_capacity(2);
    if let Some((_, (other, candidate))) = best_x {
        let o = others[other];
        guides.push(Guide {
            axis: Axis::X,
            at: x_candidates(o)[candidate],
            from: snapped.y.min(o.y),
            to: (snapped.y + snapped.h).max(o.y + o.h),
        });
    }
    if let Some((_, (other, candidate))) = best_y {
        let o = others[other];
        guides.push(Guide {
            axis: Axis::Y,
            at: y_candidates(o)[candidate],
            from: snapped.x.min(o.x),
            to: (snapped.x + snapped.w).max(o.x + o.w),
        });
    }
    Snap { dx, dy, guides }
}

/// `{left, center, right}` — the four corners plus the center, projected
/// onto x.
fn x_candidates(b: Bbox) -> [f64; 3] {
    [b.x, b.x + b.w / 2.0, b.x + b.w]
}

/// `{top, center, bottom}` — the same set projected onto y.
fn y_candidates(b: Bbox) -> [f64; 3] {
    [b.y, b.y + b.h / 2.0, b.y + b.h]
}

/// The nearest (other candidate − moving candidate) strictly inside
/// `threshold`, as `(delta, (other index, other candidate index))`. Strict
/// `<` on every comparison: a later pair only replaces the best on a
/// strictly smaller distance, which is what makes ties resolve to the
/// earliest entry.
fn best_delta(
    moving: [f64; 3],
    others: impl Iterator<Item = [f64; 3]>,
    threshold: f64,
) -> Option<(f64, (usize, usize))> {
    let mut best: Option<(f64, (usize, usize))> = None;
    for (other, candidates) in others.enumerate() {
        for (ci, c) in candidates.iter().enumerate() {
            for m in moving {
                let delta = c - m;
                let dist = delta.abs();
                if dist < threshold && best.is_none_or(|(d, _)| dist < d.abs()) {
                    best = Some((delta, (other, ci)));
                }
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bbox(x: f64, y: f64, w: f64, h: f64) -> Bbox {
        Bbox { x, y, w, h }
    }

    #[test]
    fn a_left_edge_within_the_threshold_snaps_and_emits_a_vertical_guide() {
        // Moving box's left edge (103) is 3 from the other's left edge (100)
        // — inside 8 — while nothing on y is close (400 vs 0..100).
        let moving = bbox(103.0, 400.0, 40.0, 40.0);
        let others = [bbox(100.0, 0.0, 100.0, 100.0)];
        let snap = snap_translate(moving, &others, 8.0);
        assert_eq!((snap.dx, snap.dy), (-3.0, 0.0));
        assert_eq!(
            snap.guides,
            vec![Guide {
                axis: Axis::X,
                at: 100.0,
                from: 0.0,
                to: 440.0,
            }],
            "one guide, spanning from the other's top to the moving box's bottom"
        );
    }

    #[test]
    fn outside_the_threshold_nothing_snaps() {
        let moving = bbox(120.0, 400.0, 20.0, 20.0);
        let others = [bbox(100.0, 0.0, 100.0, 100.0)];
        // Nearest x pair is right-vs-center: 140 vs 150 (10); y is far.
        let snap = snap_translate(moving, &others, 8.0);
        assert_eq!(snap, Snap::default());
    }

    #[test]
    fn the_nearest_candidate_wins_over_a_farther_one_inside_the_threshold() {
        // Two others: "far" has a left edge 6 away, "near" a right edge 2 away.
        let moving = bbox(100.0, 400.0, 50.0, 50.0);
        let others = [
            bbox(106.0, 0.0, 10.0, 10.0), // left 106 → delta +6
            bbox(0.0, 0.0, 98.0, 10.0),   // right 98 → delta −2
        ];
        let snap = snap_translate(moving, &others, 8.0);
        assert_eq!(snap.dx, -2.0);
        assert_eq!(snap.guides[0].at, 98.0);
    }

    #[test]
    fn equal_deltas_resolve_to_the_earliest_other() {
        let moving = bbox(100.0, 400.0, 50.0, 50.0);
        // Both have an edge exactly 4 from the moving left edge (100), in
        // opposite directions: the first's left at 104, the second's right
        // at 96.
        let others = [bbox(104.0, 0.0, 10.0, 10.0), bbox(86.0, 0.0, 10.0, 10.0)];
        let snap = snap_translate(moving, &others, 8.0);
        assert_eq!(snap.dx, 4.0, "the first listed box wins the tie");
        let flipped = [others[1], others[0]];
        assert_eq!(snap_translate(moving, &flipped, 8.0).dx, -4.0);
    }

    #[test]
    fn the_axes_snap_independently() {
        // x: left edges 2 apart (snaps). y: nearest pair is 10 apart (does
        // not).
        let moving = bbox(102.0, 130.0, 50.0, 50.0);
        let others = [bbox(100.0, 100.0, 40.0, 40.0)];
        let snap = snap_translate(moving, &others, 8.0);
        assert_eq!((snap.dx, snap.dy), (-2.0, 0.0));
        assert_eq!(snap.guides.len(), 1);
        assert_eq!(snap.guides[0].axis, Axis::X);

        // Both inside: two guides, one per axis.
        let moving = bbox(102.0, 103.0, 50.0, 50.0);
        let snap = snap_translate(moving, &others, 8.0);
        assert_eq!((snap.dx, snap.dy), (-2.0, -3.0));
        let axes: Vec<Axis> = snap.guides.iter().map(|g| g.axis).collect();
        assert_eq!(axes, vec![Axis::X, Axis::Y]);
    }

    #[test]
    fn centers_snap_too() {
        // Moving center x = 125 + 25 = 150; other center x = 100 + 50 = 150,
        // offset by 1 → delta −1 wins over left/left (25) and right/right (25).
        let moving = bbox(126.0, 400.0, 50.0, 50.0);
        let others = [bbox(100.0, 0.0, 100.0, 100.0)];
        let snap = snap_translate(moving, &others, 8.0);
        assert_eq!(snap.dx, -1.0);
        assert_eq!(snap.guides[0].at, 150.0);
    }

    /// The contract behind the caller's filtering: a box listed among
    /// `others` is at delta 0 from itself on both axes, wins every
    /// comparison, and "snaps" the drag nowhere while drawing two guides
    /// through its own edges.
    #[test]
    fn a_box_listed_among_others_snaps_to_itself() {
        let moving = bbox(103.0, 400.0, 50.0, 50.0);
        let others = [bbox(100.0, 0.0, 100.0, 100.0), moving];
        let snap = snap_translate(moving, &others, 8.0);
        assert_eq!((snap.dx, snap.dy), (0.0, 0.0));
        assert_eq!(snap.guides.len(), 2);
        assert!(snap.guides.iter().all(|g| g.from
            == match g.axis {
                Axis::X => moving.y,
                Axis::Y => moving.x,
            }));
    }

    #[test]
    fn guide_spans_cover_both_boxes_at_the_snapped_position() {
        // x snaps by −3 and y by +5: the vertical guide's y-span must use
        // the y-snapped box, the horizontal guide's x-span the x-snapped one.
        let moving = bbox(103.0, 195.0, 50.0, 50.0);
        let others = [bbox(100.0, 200.0, 20.0, 30.0)];
        let snap = snap_translate(moving, &others, 8.0);
        assert_eq!((snap.dx, snap.dy), (-3.0, 5.0));
        let [gx, gy] = snap.guides[..] else {
            panic!("expected two guides, got {:?}", snap.guides);
        };
        // Vertical line at x=100 from the shared top (200) to the moving
        // box's snapped bottom (200 + 50).
        assert_eq!(
            (gx.axis, gx.at, gx.from, gx.to),
            (Axis::X, 100.0, 200.0, 250.0)
        );
        // Horizontal line at y=200 from the shared left (100) to the moving
        // box's snapped right (100 + 50); the other's bottom (230) sits
        // inside the vertical span, its right (120) inside the horizontal.
        assert_eq!(
            (gy.axis, gy.at, gy.from, gy.to),
            (Axis::Y, 200.0, 100.0, 150.0)
        );
    }

    #[test]
    fn a_zero_threshold_never_snaps_even_when_exactly_aligned() {
        let moving = bbox(100.0, 400.0, 50.0, 50.0);
        let others = [bbox(100.0, 0.0, 100.0, 100.0)];
        assert_eq!(snap_translate(moving, &others, 0.0), Snap::default());
        assert_eq!(snap_translate(moving, &others, -1.0), Snap::default());
    }

    #[test]
    fn no_others_means_no_snap() {
        let moving = bbox(100.0, 100.0, 50.0, 50.0);
        assert_eq!(snap_translate(moving, &[], 8.0), Snap::default());
    }
}
