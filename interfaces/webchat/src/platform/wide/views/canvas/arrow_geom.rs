//! Arrow geometry — the shaft (straight or bent), the endpoint clipping to a
//! bound shape's outline, and the head marks. Pure functions; the live view
//! (`shape_view::arrow_svg`) and the export serializer draw from the same
//! [`Shaft`], so a bent arrow cannot curve one way on screen and another in
//! the PNG.
//!
//! # Bend → arc → cubics
//!
//! `Arrow.bend` is the perpendicular displacement (world units) of the
//! chord's midpoint; the shaft is the circle through the two endpoints and
//! that displaced point (tldraw's three-point arc). `|bend|` below
//! [`MIN_BEND`] is a straight line. The arc is emitted as a chain of cubic
//! Béziers rather than an SVG `A` command because the wire contract's
//! `PathCmd` has no `A` (spec §5: no elliptical arcs) — and a cubic chain is
//! what the sketch synthesiser and the contract's parser already understand,
//! so a sketched bent arrow costs nothing extra.
//!
//! # Clipping
//!
//! A bound endpoint attaches where the ray from the shape's centre toward
//! the other end crosses the shape's outline: rect and ellipse analytically,
//! every other geo form by intersecting the ray with `geo_cmds`' flattened
//! polygon, non-geo shapes as their bbox. The ray follows the **chord**, not
//! the arc — a bent arrow leaves its shape at the chord's crossing (a known
//! simplification; the arc's tangent is only used for the head marks).

use aleph_protocol::canvas::{ArrowEnd, ArrowHead, GeoForm, PathCmd, Shape};

use super::geo_path;
use super::interaction::Bbox;

/// Arrowhead length along the shaft, world units.
const ARROW_HEAD_LEN: f64 = 12.0;
/// Arrowhead half-width across the shaft, world units.
const ARROW_HEAD_HALF: f64 = 5.0;
/// Radius of an [`ArrowHead::Dot`] cap, world units.
const ARROW_DOT_RADIUS: f64 = 4.0;
/// Bends smaller than this (world units) draw as a straight line — the arc
/// they would describe is indistinguishable from the chord at stroke width.
const MIN_BEND: f64 = 8.0;
/// Longest sweep one cubic approximates (a quarter turn keeps the radial
/// error under 0.03%).
const MAX_SEGMENT_SWEEP: f64 = std::f64::consts::FRAC_PI_2;

/// The circle a bent arrow rides on. `sweep` is signed: positive turns with
/// increasing angle (clockwise on a y-down screen), negative against it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Arc {
    pub(super) center: (f64, f64),
    pub(super) radius: f64,
    pub(super) start_angle: f64,
    pub(super) sweep: f64,
}

impl Arc {
    #[must_use]
    fn point_at(&self, angle: f64) -> (f64, f64) {
        (
            self.center.0 + self.radius * angle.cos(),
            self.center.1 + self.radius * angle.sin(),
        )
    }

    /// Unit direction of travel at `angle`.
    #[must_use]
    fn travel_at(&self, angle: f64) -> (f64, f64) {
        let s = self.sweep.signum();
        (-angle.sin() * s, angle.cos() * s)
    }
}

/// The three-point circle for a bend, or `None` for a straight shaft
/// (`|bend| < MIN_BEND`, coincident endpoints, or a circle too degenerate to
/// solve).
#[must_use]
pub(super) fn arc_info(a: (f64, f64), b: (f64, f64), bend: f64) -> Option<Arc> {
    if bend.abs() < MIN_BEND || !bend.is_finite() {
        return None;
    }
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let chord = (dx * dx + dy * dy).sqrt();
    if chord < 1e-6 {
        return None;
    }
    let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let normal = (-dy / chord, dx / chord);
    let c = (mid.0 + normal.0 * bend, mid.1 + normal.1 * bend);
    // Circumcentre of a, c, b.
    let d = 2.0 * (a.0 * (c.1 - b.1) + c.0 * (b.1 - a.1) + b.0 * (a.1 - c.1));
    if d.abs() < 1e-9 {
        return None;
    }
    let (a2, b2, c2) = (
        a.0 * a.0 + a.1 * a.1,
        b.0 * b.0 + b.1 * b.1,
        c.0 * c.0 + c.1 * c.1,
    );
    let ux = (a2 * (c.1 - b.1) + c2 * (b.1 - a.1) + b2 * (a.1 - c.1)) / d;
    let uy = (a2 * (b.0 - c.0) + c2 * (a.0 - b.0) + b2 * (c.0 - a.0)) / d;
    let center = (ux, uy);
    let radius = ((a.0 - ux).powi(2) + (a.1 - uy).powi(2)).sqrt();
    let start_angle = (a.1 - uy).atan2(a.0 - ux);
    let end_angle = (b.1 - uy).atan2(b.0 - ux);
    let via_angle = (c.1 - uy).atan2(c.0 - ux);
    // The arc from a to b that passes through c: take the positive sweep if
    // c lies on it, else the complementary negative one.
    let tau = std::f64::consts::TAU;
    let pos = (end_angle - start_angle).rem_euclid(tau);
    let via = (via_angle - start_angle).rem_euclid(tau);
    let sweep = if via <= pos { pos } else { pos - tau };
    Some(Arc {
        center,
        radius,
        start_angle,
        sweep,
    })
}

/// A drawable shaft: its path commands plus the two points the head marks
/// aim from and the label anchor.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Shaft {
    /// The resolved endpoints — the head tips.
    pub(super) start: (f64, f64),
    pub(super) end: (f64, f64),
    /// `MoveTo a` then a `LineTo b`, or a chain of cubics ending exactly on
    /// `b`.
    pub(super) cmds: Vec<PathCmd>,
    /// A point behind the start along the shaft's tangent — the start head
    /// points from here to `a`.
    pub(super) start_from: (f64, f64),
    /// A point behind the end along the shaft's tangent — the end head
    /// points from here to `b`.
    pub(super) end_from: (f64, f64),
    /// Where the label sits: the chord midpoint, or the arc's apex.
    pub(super) mid: (f64, f64),
}

/// The shaft for `a`→`b` with `bend`.
#[must_use]
pub(super) fn arrow_shaft(a: (f64, f64), b: (f64, f64), bend: f64) -> Shaft {
    let Some(arc) = arc_info(a, b, bend) else {
        return Shaft {
            start: a,
            end: b,
            cmds: vec![
                PathCmd::MoveTo { x: a.0, y: a.1 },
                PathCmd::LineTo { x: b.0, y: b.1 },
            ],
            start_from: b,
            end_from: a,
            mid: ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0),
        };
    };
    let mut cmds = vec![PathCmd::MoveTo { x: a.0, y: a.1 }];
    cmds.extend(arc_to_cubics(&arc, b));
    let end_angle = arc.start_angle + arc.sweep;
    let t0 = arc.travel_at(arc.start_angle);
    let t1 = arc.travel_at(end_angle);
    Shaft {
        start: a,
        end: b,
        cmds,
        start_from: (a.0 + t0.0 * ARROW_HEAD_LEN, a.1 + t0.1 * ARROW_HEAD_LEN),
        end_from: (b.0 - t1.0 * ARROW_HEAD_LEN, b.1 - t1.1 * ARROW_HEAD_LEN),
        mid: arc.point_at(arc.start_angle + arc.sweep / 2.0),
    }
}

/// The arc as cubic Béziers (no leading `MoveTo`), each spanning at most
/// [`MAX_SEGMENT_SWEEP`]; the last one ends exactly on `end` so the chain
/// meets the stored endpoint bit-for-bit.
fn arc_to_cubics(arc: &Arc, end: (f64, f64)) -> Vec<PathCmd> {
    let segments = ((arc.sweep.abs() / MAX_SEGMENT_SWEEP).ceil() as usize).max(1);
    let step = arc.sweep / segments as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let mut out = Vec::with_capacity(segments);
    for i in 0..segments {
        let a0 = arc.start_angle + step * i as f64;
        let a1 = a0 + step;
        let p0 = arc.point_at(a0);
        let p3 = if i + 1 == segments {
            end
        } else {
            arc.point_at(a1)
        };
        let r = arc.radius;
        out.push(PathCmd::Cubic {
            x1: p0.0 - k * r * a0.sin(),
            y1: p0.1 + k * r * a0.cos(),
            x2: p3.0 + k * r * a1.sin(),
            y2: p3.1 - k * r * a1.cos(),
            x: p3.0,
            y: p3.1,
        });
    }
    out
}

/// Where an arrow endpoint bound to a shape attaches: the point where the
/// ray from the outline's centre toward `toward` crosses the outline of
/// `form` inside `b` (module doc). Degenerate cases (a zero-extent box,
/// `toward` at the centre, no crossing found) answer the centre itself — an
/// anchor must always exist.
#[must_use]
pub(super) fn clip_to_outline(b: Bbox, form: Option<GeoForm>, toward: (f64, f64)) -> (f64, f64) {
    let (cx, cy) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
    let (dx, dy) = (toward.0 - cx, toward.1 - cy);
    if (dx.abs() < 1e-9 && dy.abs() < 1e-9) || b.w <= 0.0 || b.h <= 0.0 {
        return (cx, cy);
    }
    match form {
        None | Some(GeoForm::Rect) => {
            let tx = if dx.abs() > 1e-9 {
                (b.w / 2.0) / dx.abs()
            } else {
                f64::INFINITY
            };
            let ty = if dy.abs() > 1e-9 {
                (b.h / 2.0) / dy.abs()
            } else {
                f64::INFINITY
            };
            let t = tx.min(ty);
            if !t.is_finite() {
                return (cx, cy);
            }
            (cx + dx * t, cy + dy * t)
        }
        Some(GeoForm::Ellipse) => {
            let (rx, ry) = (b.w / 2.0, b.h / 2.0);
            let t = 1.0 / ((dx / rx).powi(2) + (dy / ry).powi(2)).sqrt();
            if !t.is_finite() {
                return (cx, cy);
            }
            (cx + dx * t, cy + dy * t)
        }
        Some(form) => {
            let cmds = geo_path::translate_cmds(&geo_path::geo_cmds(form, b.w, b.h), b.x, b.y);
            let mut best: Option<f64> = None;
            for line in geo_path::flatten_cmds(&cmds) {
                for seg in line.windows(2) {
                    if let Some(t) = ray_segment_t((cx, cy), (dx, dy), seg[0], seg[1]) {
                        best = Some(best.map_or(t, |b| b.min(t)));
                    }
                }
            }
            best.map_or((cx, cy), |t| (cx + dx * t, cy + dy * t))
        }
    }
}

/// Parameter `t > 0` where the ray `o + t·d` crosses the segment `p`–`q`,
/// if it does.
fn ray_segment_t(o: (f64, f64), d: (f64, f64), p: (f64, f64), q: (f64, f64)) -> Option<f64> {
    let (ex, ey) = (q.0 - p.0, q.1 - p.1);
    let denom = d.0 * ey - d.1 * ex;
    if denom.abs() < 1e-12 {
        return None;
    }
    let (ox, oy) = (p.0 - o.0, p.1 - o.1);
    let t = (ox * ey - oy * ex) / denom;
    let s = (ox * d.1 - oy * d.0) / denom;
    (t > 0.0 && (0.0..=1.0).contains(&s)).then_some(t)
}

/// The outline a shape presents to an arrow: its bbox, and its geo form when
/// it has one (every other shape clips as a rectangle).
#[must_use]
fn outline_of(shape: &Shape) -> (Bbox, Option<GeoForm>) {
    let form = match shape {
        Shape::Geo { form, .. } => Some(*form),
        _ => None,
    };
    (Bbox::of_shape(shape), form)
}

/// Resolve an arrow's endpoints against its bound shapes: a bound end
/// projects onto its shape's outline ([`clip_to_outline`]), aimed at the
/// other end's reference point (that end's bound shape's *centre*, or its
/// stored coordinates). An end whose bound shape vanished falls back to its
/// stored x/y — the wire contract calls them "the recomputed fallback".
#[must_use]
pub(super) fn resolve_arrow_ends(
    shapes: &[Shape],
    start: &ArrowEnd,
    end: &ArrowEnd,
) -> ((f64, f64), (f64, f64)) {
    let outline = |bind: &Option<String>| -> Option<(Bbox, Option<GeoForm>)> {
        bind.as_ref()
            .and_then(|id| shapes.iter().find(|s| s.id() == id))
            .map(outline_of)
    };
    let center = |b: Bbox| (b.x + b.w / 2.0, b.y + b.h / 2.0);
    let start_outline = outline(&start.bind);
    let end_outline = outline(&end.bind);
    let start_ref = start_outline.map_or((start.x, start.y), |(b, _)| center(b));
    let end_ref = end_outline.map_or((end.x, end.y), |(b, _)| center(b));
    (
        start_outline.map_or((start.x, start.y), |(b, f)| clip_to_outline(b, f, end_ref)),
        end_outline.map_or((end.x, end.y), |(b, f)| clip_to_outline(b, f, start_ref)),
    )
}

/// `points=` polygon string for an arrowhead at `tip`, pointing away from
/// `from`. Empty when the two coincide — a polygon with NaN vertices is an
/// SVG parse error, not an invisible triangle.
#[must_use]
pub(super) fn arrow_head_points(from: (f64, f64), tip: (f64, f64)) -> String {
    let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return String::new();
    }
    let (ux, uy) = (dx / len, dy / len);
    let (bx, by) = (tip.0 - ux * ARROW_HEAD_LEN, tip.1 - uy * ARROW_HEAD_LEN);
    let (px, py) = (-uy, ux);
    format!(
        "{},{} {},{} {},{}",
        tip.0,
        tip.1,
        bx + px * ARROW_HEAD_HALF,
        by + py * ARROW_HEAD_HALF,
        bx - px * ARROW_HEAD_HALF,
        by - py * ARROW_HEAD_HALF
    )
}

/// One rendered arrow cap — the same value the live view and the export
/// serializer draw, so the two agree on every head kind.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum HeadMark {
    /// `points=`; `filled` paints it in the stroke color, else outlined.
    Polygon {
        points: String,
        filled: bool,
    },
    Circle {
        cx: f64,
        cy: f64,
        r: f64,
    },
    Line {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
}

/// The cap at `tip` for `kind`, oriented away from `from` (the shaft's
/// tangent point — [`Shaft::end_from`] / [`Shaft::start_from`]). `None` for
/// [`ArrowHead::None`] and when `from` coincides with `tip` (the NaN rule of
/// [`arrow_head_points`]).
#[must_use]
pub(super) fn arrow_head_mark(
    kind: ArrowHead,
    from: (f64, f64),
    tip: (f64, f64),
) -> Option<HeadMark> {
    let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return None;
    }
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux);
    Some(match kind {
        ArrowHead::None => return None,
        ArrowHead::Arrow => HeadMark::Polygon {
            points: arrow_head_points(from, tip),
            filled: true,
        },
        ArrowHead::Triangle => HeadMark::Polygon {
            points: arrow_head_points(from, tip),
            filled: false,
        },
        ArrowHead::Dot => HeadMark::Circle {
            cx: tip.0,
            cy: tip.1,
            r: ARROW_DOT_RADIUS,
        },
        ArrowHead::Bar => HeadMark::Line {
            x1: tip.0 + px * ARROW_HEAD_HALF,
            y1: tip.1 + py * ARROW_HEAD_HALF,
            x2: tip.0 - px * ARROW_HEAD_HALF,
            y2: tip.1 - py * ARROW_HEAD_HALF,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::canvas::{FracIndex, ShapeCommon, ShapeStyle};

    fn bbox(x: f64, y: f64, w: f64, h: f64) -> Bbox {
        Bbox { x, y, w, h }
    }

    #[track_caller]
    fn assert_close(got: (f64, f64), want: (f64, f64)) {
        assert!(
            (got.0 - want.0).abs() < 1e-6 && (got.1 - want.1).abs() < 1e-6,
            "got {got:?}, want {want:?}"
        );
    }

    fn end_of(cmd: &PathCmd) -> (f64, f64) {
        match *cmd {
            PathCmd::MoveTo { x, y } | PathCmd::LineTo { x, y } => (x, y),
            PathCmd::Quad { x, y, .. } | PathCmd::Cubic { x, y, .. } => (x, y),
            PathCmd::Close => unreachable!("no Close in a shaft"),
        }
    }

    #[test]
    fn a_bend_below_the_threshold_is_the_straight_chord() {
        let a = (0.0, 0.0);
        let b = (100.0, 0.0);
        let straight = arrow_shaft(a, b, 0.0);
        assert_eq!(
            straight.cmds,
            vec![
                PathCmd::MoveTo { x: 0.0, y: 0.0 },
                PathCmd::LineTo { x: 100.0, y: 0.0 }
            ]
        );
        assert_eq!(arrow_shaft(a, b, MIN_BEND - 0.01), straight);
        assert_eq!(straight.mid, (50.0, 0.0));
        assert_eq!(straight.end_from, a);
        assert_eq!(straight.start_from, b);
        assert!(
            arc_info(a, a, 50.0).is_none(),
            "coincident ends have no circle"
        );
    }

    #[test]
    fn a_bent_shaft_is_cubics_from_a_to_b_through_the_displaced_midpoint() {
        let a = (0.0, 0.0);
        let b = (100.0, 0.0);
        let shaft = arrow_shaft(a, b, 30.0);
        assert!(matches!(shaft.cmds[0], PathCmd::MoveTo { x, y } if x == 0.0 && y == 0.0));
        assert!(shaft.cmds[1..]
            .iter()
            .all(|c| matches!(c, PathCmd::Cubic { .. })));
        assert_eq!(
            end_of(shaft.cmds.last().unwrap()),
            b,
            "the chain ends exactly on b"
        );
        // The apex is the displaced midpoint, 30 units off the chord.
        assert_close(shaft.mid, (50.0, 30.0));
        // Head tangents lean along the arc, not the chord.
        assert!(
            shaft.end_from.1 > 0.0 && shaft.start_from.1 > 0.0,
            "{shaft:?}"
        );
        // The flattened curve passes within a hair of the apex.
        let pts: Vec<(f64, f64)> = geo_path::flatten_cmds(&shaft.cmds)
            .into_iter()
            .flatten()
            .collect();
        let nearest = pts
            .iter()
            .map(|p| ((p.0 - 50.0).powi(2) + (p.1 - 30.0).powi(2)).sqrt())
            .fold(f64::MAX, f64::min);
        assert!(nearest < 1.0, "nearest sample to the apex is {nearest}");
    }

    #[test]
    fn positive_and_negative_bends_mirror_across_the_chord() {
        let a = (10.0, 20.0);
        let b = (110.0, 20.0);
        let up = arrow_shaft(a, b, 40.0);
        let down = arrow_shaft(a, b, -40.0);
        assert_eq!(up.cmds.len(), down.cmds.len());
        for (u, d) in up.cmds.iter().zip(&down.cmds) {
            let (uc, dc) = (u.coords(), d.coords());
            for (i, (x, y)) in uc.iter().zip(&dc).enumerate() {
                if i % 2 == 0 {
                    assert!((x - y).abs() < 1e-6, "x must match: {u:?} vs {d:?}");
                } else {
                    assert!(
                        (x - 20.0 + (y - 20.0)).abs() < 1e-6,
                        "y must mirror about 20: {u:?} vs {d:?}"
                    );
                }
            }
        }
        assert_close(down.mid, (60.0, -20.0));
        assert_close(up.mid, (60.0, 60.0));
    }

    #[test]
    fn a_large_bend_splits_into_more_cubics() {
        let big = arrow_shaft((0.0, 0.0), (100.0, 0.0), 200.0);
        assert!(
            big.cmds.len() > 2,
            "more than a quarter turn needs several cubics: {}",
            big.cmds.len()
        );
        assert_eq!(end_of(big.cmds.last().unwrap()), (100.0, 0.0));
    }

    #[test]
    fn clip_lands_on_the_correct_rect_edge_in_all_four_quadrants() {
        // Box (0,0)–(100,60), centre (50,30).
        let b = bbox(0.0, 0.0, 100.0, 60.0);
        for form in [None, Some(GeoForm::Rect)] {
            assert_close(clip_to_outline(b, form, (200.0, -120.0)), (80.0, 0.0));
            assert_close(clip_to_outline(b, form, (250.0, 130.0)), (100.0, 55.0));
            assert_close(clip_to_outline(b, form, (-150.0, 230.0)), (20.0, 60.0));
            assert_close(clip_to_outline(b, form, (-50.0, -70.0)), (20.0, 0.0));
        }
    }

    #[test]
    fn clip_degenerate_targets_answer_the_centre() {
        let b = bbox(0.0, 0.0, 100.0, 60.0);
        for form in [None, Some(GeoForm::Ellipse), Some(GeoForm::Diamond)] {
            assert_eq!(
                clip_to_outline(b, form, (50.0, 30.0)),
                (50.0, 30.0),
                "{form:?}"
            );
            let hairline = bbox(10.0, 10.0, 0.0, 0.0);
            assert_eq!(
                clip_to_outline(hairline, form, (99.0, 99.0)),
                (10.0, 10.0),
                "{form:?}"
            );
        }
    }

    #[test]
    fn clip_follows_the_ellipse_and_the_polygon_outlines() {
        // Circle of radius 50 centred at (50,50): every anchor is 50 from
        // the centre, and the axis-aligned one is on the rim.
        let circle = bbox(0.0, 0.0, 100.0, 100.0);
        assert_close(
            clip_to_outline(circle, Some(GeoForm::Ellipse), (300.0, 50.0)),
            (100.0, 50.0),
        );
        let diag = clip_to_outline(circle, Some(GeoForm::Ellipse), (200.0, 200.0));
        let r = ((diag.0 - 50.0).powi(2) + (diag.1 - 50.0).powi(2)).sqrt();
        assert!((r - 50.0).abs() < 1e-6, "{diag:?} is {r} from the centre");
        // Ellipse 200×100: the vertical ray exits at the top of the box.
        let ellipse = bbox(0.0, 0.0, 200.0, 100.0);
        assert_close(
            clip_to_outline(ellipse, Some(GeoForm::Ellipse), (100.0, -500.0)),
            (100.0, 0.0),
        );
        // Diamond 100×100: the diagonal ray hits the edge's midpoint (75,75),
        // where a rect would have answered the corner (100,100).
        let diamond = bbox(0.0, 0.0, 100.0, 100.0);
        assert_close(
            clip_to_outline(diamond, Some(GeoForm::Diamond), (200.0, 200.0)),
            (75.0, 75.0),
        );
        assert_close(
            clip_to_outline(diamond, Some(GeoForm::Diamond), (400.0, 50.0)),
            (100.0, 50.0),
        );
        // Pill 100×40 aimed straight right: the rim of the right semicircle.
        let pill = bbox(0.0, 0.0, 100.0, 40.0);
        let p = clip_to_outline(pill, Some(GeoForm::Pill), (900.0, 20.0));
        assert!(
            (p.0 - 100.0).abs() < 0.5 && (p.1 - 20.0).abs() < 1e-6,
            "{p:?}"
        );
    }

    #[test]
    fn resolve_arrow_ends_follows_bound_shapes_and_falls_back_when_they_vanish() {
        let target = Shape::Note {
            common: ShapeCommon {
                id: "n1".to_string(),
                x: 200.0,
                y: 0.0,
                w: 100.0,
                h: 60.0,
                z: FracIndex::first(),
                parent_id: None,
                reveal: None,
            },
            style: ShapeStyle::default(),
            text: String::new(),
        };
        let start = ArrowEnd {
            x: 0.0,
            y: 30.0,
            bind: None,
        };
        let end = ArrowEnd {
            x: 210.0, // stale drawn coordinate — the binding overrides it
            y: 10.0,
            bind: Some("n1".to_string()),
        };
        let (s, e) = resolve_arrow_ends(std::slice::from_ref(&target), &start, &end);
        assert_eq!(s, (0.0, 30.0), "an unbound end keeps its coordinates");
        assert_close(e, (200.0, 30.0));
        let (_, e) = resolve_arrow_ends(&[], &start, &end);
        assert_eq!(e, (210.0, 10.0));
        // A bound geo form clips on its own outline, not its box.
        let ellipse = Shape::Geo {
            common: ShapeCommon {
                id: "e1".to_string(),
                x: 200.0,
                y: 0.0,
                w: 100.0,
                h: 100.0,
                z: FracIndex::first(),
                parent_id: None,
                reveal: None,
            },
            form: GeoForm::Ellipse,
            style: ShapeStyle::default(),
            text: String::new(),
        };
        let from_corner = ArrowEnd {
            x: 0.0,
            y: -200.0,
            bind: None,
        };
        let to_ellipse = ArrowEnd {
            bind: Some("e1".to_string()),
            ..end
        };
        let (_, e) = resolve_arrow_ends(std::slice::from_ref(&ellipse), &from_corner, &to_ellipse);
        let r = ((e.0 - 250.0).powi(2) + (e.1 - 50.0).powi(2)).sqrt();
        assert!(
            (r - 50.0).abs() < 1e-6,
            "{e:?} must sit on the circle's rim"
        );
    }

    #[test]
    fn arrow_head_is_symmetric_and_empty_for_a_degenerate_arrow() {
        assert_eq!(arrow_head_points((3.0, 4.0), (3.0, 4.0)), "");
        assert_eq!(
            arrow_head_points((0.0, 0.0), (100.0, 0.0)),
            "100,0 88,5 88,-5"
        );
    }

    /// Every head kind yields its own mark (or none), and a degenerate arrow
    /// yields none for every kind.
    #[test]
    fn arrow_head_marks_cover_every_kind_and_vanish_when_degenerate() {
        let (s, e) = ((0.0, 0.0), (100.0, 0.0));
        assert_eq!(arrow_head_mark(ArrowHead::None, s, e), None);
        assert_eq!(
            arrow_head_mark(ArrowHead::Arrow, s, e),
            Some(HeadMark::Polygon {
                points: "100,0 88,5 88,-5".to_string(),
                filled: true
            })
        );
        assert_eq!(
            arrow_head_mark(ArrowHead::Triangle, s, e),
            Some(HeadMark::Polygon {
                points: "100,0 88,5 88,-5".to_string(),
                filled: false
            })
        );
        assert_eq!(
            arrow_head_mark(ArrowHead::Dot, s, e),
            Some(HeadMark::Circle {
                cx: 100.0,
                cy: 0.0,
                r: ARROW_DOT_RADIUS
            })
        );
        assert_eq!(
            arrow_head_mark(ArrowHead::Bar, s, e),
            Some(HeadMark::Line {
                x1: 100.0,
                y1: 5.0,
                x2: 100.0,
                y2: -5.0
            })
        );
        assert_eq!(
            arrow_head_mark(ArrowHead::Arrow, e, s),
            Some(HeadMark::Polygon {
                points: "0,0 12,-5 12,5".to_string(),
                filled: true
            })
        );
        for kind in [
            ArrowHead::Arrow,
            ArrowHead::Triangle,
            ArrowHead::Dot,
            ArrowHead::Bar,
        ] {
            assert_eq!(arrow_head_mark(kind, s, s), None, "{kind:?}");
        }
    }
}
