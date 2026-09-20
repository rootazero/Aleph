//! Hand-drawn stroke synthesis — the `StrokeKind::Sketch` look, in the shape
//! of tldraw's `PathBuilder.toDrawD` (NOT rough.js: no hachure fill, no
//! dependency, no second geometry).
//!
//! # What it does
//!
//! A clean outline (the `PathCmd` list `geo_path.rs` / `arrow_geom.rs`
//! produce) becomes two overlaid passes of a wobbly one:
//!
//! - every vertex is nudged by a seeded random offset of up to
//!   [`OFFSET_RATIO`] × stroke width on each axis;
//! - corners between straight segments are rounded through a quadratic
//!   curve whose control point is the corner itself, entering and leaving
//!   [`ROUNDNESS_RATIO`] × stroke width along each side — clamped to half
//!   the shorter adjacent segment, so a tiny segment never inverts;
//! - a second pass is re-seeded (same seed, pass index folded in) and
//!   appended to the same `d`, so the two nearly-coincident strokes read as
//!   pencil overlap.
//!
//! Curve commands keep their control points attached: a `Q`/`C` end point is
//! jittered like any vertex and its control points move by an interpolation
//! of the deltas at the two ends, so the curve bends with its endpoints
//! instead of kinking. Curves are not corner-rounded (they are already
//! smooth).
//!
//! # Determinism
//!
//! The seed is the **shape id** (the wire contract's promise: "seeded from
//! the shape id — the same document renders the same everywhere"). The RNG
//! is a xorshift128 over an FNV-1a hash of the seed string, and every
//! coordinate is rounded to two decimals, so the `d` string is byte-identical
//! across clients and across the live view / export pair. Coordinates are
//! shape-local, so moving a shape does not re-roll its wobble.

use aleph_protocol::canvas::{cmds_to_d, PathCmd};

/// Vertex jitter, as a fraction of the stroke width (tldraw: `offset = w/3`).
const OFFSET_RATIO: f64 = 1.0 / 3.0;
/// Corner rounding reach, as a multiple of the stroke width (tldraw:
/// `roundness = 2w`).
const ROUNDNESS_RATIO: f64 = 2.0;
/// Overlaid strokes per outline.
const PASSES: u32 = 2;
/// Segments shorter than this get no corner treatment (a zero-length edge
/// has no direction to round along).
const MIN_SEGMENT: f64 = 1e-6;
/// Corners turning by less than this (radians) stay sharp: a quadratic
/// through a nearly-straight joint adds nothing but bytes.
const MIN_TURN: f64 = 0.15;

/// FNV-1a 64 over a string — the seed hash. `pub(super)`: `reveal.rs` keys
/// its CSS ids with the same hash, so there is one hash in this module tree.
#[must_use]
pub(super) fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// xorshift128, seeded from a string. Deterministic; `next_f64` in [0, 1).
#[derive(Debug, Clone)]
pub(super) struct Rng {
    s: [u32; 4],
}

impl Rng {
    /// Four state words from two FNV hashes (the seed, and the seed with a
    /// domain byte appended), each forced non-zero — an all-zero xorshift
    /// state is a fixed point that never leaves zero.
    #[must_use]
    pub(super) fn seeded(seed: &str) -> Self {
        let a = fnv1a64(seed);
        let b = fnv1a64(&format!("{seed}\u{1}"));
        let word = |v: u64| {
            let w = v as u32;
            if w == 0 {
                0x9e37_79b9
            } else {
                w
            }
        };
        Self {
            s: [word(a), word(a >> 32), word(b), word(b >> 32)],
        }
    }

    fn next_u32(&mut self) -> u32 {
        let mut t = self.s[3];
        let s = self.s[0];
        self.s[3] = self.s[2];
        self.s[2] = self.s[1];
        self.s[1] = s;
        t ^= t << 11;
        t ^= t >> 8;
        self.s[0] = t ^ s ^ (s >> 19);
        self.s[0]
    }

    #[must_use]
    pub(super) fn next_f64(&mut self) -> f64 {
        f64::from(self.next_u32()) / 4_294_967_296.0
    }

    /// Uniform in `[-amplitude, amplitude]`.
    fn jitter(&mut self, amplitude: f64) -> f64 {
        (self.next_f64() * 2.0 - 1.0) * amplitude
    }
}

/// One vertex of a subpath after the MoveTo: how the outline reaches it.
#[derive(Debug, Clone, Copy)]
enum Reach {
    Line,
    Quad { c: (f64, f64) },
    Cubic { c1: (f64, f64), c2: (f64, f64) },
}

/// One subpath: its start, its vertices in order, and whether it closes.
struct Subpath {
    start: (f64, f64),
    verts: Vec<(Reach, (f64, f64))>,
    closed: bool,
}

/// Split a command list into subpaths (a `MoveTo` starts one, `Close` ends
/// it). Commands before any `MoveTo` are anchored at the origin, matching
/// how `parse_path_d` resolves a relative first command.
fn subpaths(cmds: &[PathCmd]) -> Vec<Subpath> {
    let mut out: Vec<Subpath> = Vec::new();
    for cmd in cmds {
        match *cmd {
            PathCmd::MoveTo { x, y } => out.push(Subpath {
                start: (x, y),
                verts: Vec::new(),
                closed: false,
            }),
            PathCmd::Close => {
                if let Some(sp) = out.last_mut() {
                    sp.closed = true;
                }
            }
            PathCmd::LineTo { x, y } => push_vert(&mut out, Reach::Line, (x, y)),
            PathCmd::Quad { x1, y1, x, y } => {
                push_vert(&mut out, Reach::Quad { c: (x1, y1) }, (x, y));
            }
            PathCmd::Cubic {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => push_vert(
                &mut out,
                Reach::Cubic {
                    c1: (x1, y1),
                    c2: (x2, y2),
                },
                (x, y),
            ),
        }
    }
    out
}

fn push_vert(out: &mut Vec<Subpath>, reach: Reach, p: (f64, f64)) {
    // A closed subpath followed by more drawing commands (without a new
    // MoveTo) continues from the closed subpath's start — SVG semantics.
    match out.last_mut() {
        Some(sp) if !sp.closed => sp.verts.push((reach, p)),
        Some(sp) => {
            let start = sp.start;
            out.push(Subpath {
                start,
                verts: vec![(reach, p)],
                closed: false,
            });
        }
        None => out.push(Subpath {
            start: (0.0, 0.0),
            verts: vec![(reach, p)],
            closed: false,
        }),
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn sub(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 - b.0, a.1 - b.1)
}

fn add(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 + b.0, a.1 + b.1)
}

fn scale(a: (f64, f64), k: f64) -> (f64, f64) {
    (a.0 * k, a.1 * k)
}

fn len(a: (f64, f64)) -> f64 {
    (a.0 * a.0 + a.1 * a.1).sqrt()
}

/// `from` moved `dist` toward `to`; `from` itself when the two coincide.
fn toward(from: (f64, f64), to: (f64, f64), dist: f64) -> (f64, f64) {
    let d = sub(to, from);
    let l = len(d);
    if l < MIN_SEGMENT {
        return from;
    }
    add(from, scale(d, dist / l))
}

/// The unsigned turn angle at `at` between the incoming direction (from
/// `prev`) and the outgoing direction (to `next`); 0 for a straight joint.
fn turn_angle(prev: (f64, f64), at: (f64, f64), next: (f64, f64)) -> f64 {
    let a = sub(at, prev);
    let b = sub(next, at);
    let (la, lb) = (len(a), len(b));
    if la < MIN_SEGMENT || lb < MIN_SEGMENT {
        return 0.0;
    }
    let cos = ((a.0 * b.0 + a.1 * b.1) / (la * lb)).clamp(-1.0, 1.0);
    cos.acos()
}

/// One pass over one subpath: jittered vertices, rounded straight corners.
/// Appends absolute commands (starting with a `MoveTo`) to `out`.
fn sketch_subpath(
    sp: &Subpath,
    rng: &mut Rng,
    offset: f64,
    roundness: f64,
    out: &mut Vec<PathCmd>,
) {
    // Jittered points: index 0 is the start, then one per vertex.
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(sp.verts.len() + 1);
    let mut deltas: Vec<(f64, f64)> = Vec::with_capacity(sp.verts.len() + 1);
    for p in std::iter::once(sp.start).chain(sp.verts.iter().map(|(_, p)| *p)) {
        let d = (rng.jitter(offset), rng.jitter(offset));
        deltas.push(d);
        pts.push(add(p, d));
    }
    let n = sp.verts.len();
    if n == 0 {
        out.push(PathCmd::MoveTo {
            x: round2(pts[0].0),
            y: round2(pts[0].1),
        });
        return;
    }
    // A corner at vertex `i` (point index `i`) is rounded when both the
    // segment reaching it and the one leaving it are straight and long
    // enough, and the joint actually turns. For a closed subpath the start
    // is such a corner too (between the closing segment and the first).
    let reach_of = |point_idx: usize| -> Reach {
        // point 0 is reached by the closing segment (a line) when closed.
        if point_idx == 0 {
            Reach::Line
        } else {
            sp.verts[point_idx - 1].0
        }
    };
    let leave_of = |point_idx: usize| -> Reach {
        if point_idx == n {
            Reach::Line
        } else {
            sp.verts[point_idx].0
        }
    };
    let prev_of = |i: usize| -> usize {
        if i == 0 {
            n
        } else {
            i - 1
        }
    };
    let next_of = |i: usize| -> usize {
        if i == n {
            0
        } else {
            i + 1
        }
    };
    let corner_reach = |i: usize| -> f64 {
        let straight_in = matches!(reach_of(i), Reach::Line);
        let straight_out = matches!(leave_of(i), Reach::Line);
        let is_end = !sp.closed && (i == 0 || i == n);
        if !straight_in || !straight_out || is_end {
            return 0.0;
        }
        let (p, q) = (pts[prev_of(i)], pts[next_of(i)]);
        let (lin, lout) = (len(sub(pts[i], p)), len(sub(q, pts[i])));
        if lin < MIN_SEGMENT || lout < MIN_SEGMENT || turn_angle(p, pts[i], q) < MIN_TURN {
            return 0.0;
        }
        roundness.min(lin / 2.0).min(lout / 2.0)
    };

    let mv = |p: (f64, f64)| PathCmd::MoveTo {
        x: round2(p.0),
        y: round2(p.1),
    };
    let ln = |p: (f64, f64)| PathCmd::LineTo {
        x: round2(p.0),
        y: round2(p.1),
    };

    // Where the pen starts: past the rounded start corner when closed.
    let r0 = corner_reach(0);
    let start_pt = if r0 > 0.0 {
        toward(pts[0], pts[1], r0)
    } else {
        pts[0]
    };
    out.push(mv(start_pt));

    // Walk vertices 1..=n, then (if closed) back to 0.
    let last = if sp.closed { n + 1 } else { n };
    for step in 1..=last {
        let i = if step == n + 1 { 0 } else { step };
        let from_idx = prev_of(i);
        let (reach, target) = if i == 0 {
            (Reach::Line, pts[0])
        } else {
            (sp.verts[i - 1].0, pts[i])
        };
        match reach {
            Reach::Line => {
                let r = corner_reach(i);
                if r > 0.0 {
                    let before = toward(target, pts[from_idx], r);
                    let after = toward(target, pts[next_of(i)], r);
                    out.push(ln(before));
                    out.push(PathCmd::Quad {
                        x1: round2(target.0),
                        y1: round2(target.1),
                        x: round2(after.0),
                        y: round2(after.1),
                    });
                } else {
                    out.push(ln(target));
                }
            }
            Reach::Quad { c } => {
                let d = scale(add(deltas[from_idx], deltas[i]), 0.5);
                let c = add(c, d);
                out.push(PathCmd::Quad {
                    x1: round2(c.0),
                    y1: round2(c.1),
                    x: round2(target.0),
                    y: round2(target.1),
                });
            }
            Reach::Cubic { c1, c2 } => {
                let c1 = add(c1, deltas[from_idx]);
                let c2 = add(c2, deltas[i]);
                out.push(PathCmd::Cubic {
                    x1: round2(c1.0),
                    y1: round2(c1.1),
                    x2: round2(c2.0),
                    y2: round2(c2.1),
                    x: round2(target.0),
                    y: round2(target.1),
                });
            }
        }
    }
    if sp.closed {
        out.push(PathCmd::Close);
    }
}

/// The sketched `d` for `cmds`: [`PASSES`] wobbly copies of every subpath,
/// concatenated. `stroke_w` scales both the jitter and the corner rounding.
/// Output is valid absolute path data (`cmds_to_d` of contract commands), so
/// `parse_path_d` reads it back.
#[must_use]
pub(super) fn sketch_d(cmds: &[PathCmd], seed: &str, stroke_w: f64) -> String {
    let offset = stroke_w * OFFSET_RATIO;
    let roundness = stroke_w * ROUNDNESS_RATIO;
    let paths = subpaths(cmds);
    let mut out: Vec<PathCmd> = Vec::new();
    for pass in 0..PASSES {
        let mut rng = Rng::seeded(&format!("{seed}#{pass}"));
        for sp in &paths {
            sketch_subpath(sp, &mut rng, offset, roundness, &mut out);
        }
    }
    cmds_to_d(&out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::canvas::parse_path_d;

    fn square() -> Vec<PathCmd> {
        vec![
            PathCmd::MoveTo { x: 0.0, y: 0.0 },
            PathCmd::LineTo { x: 100.0, y: 0.0 },
            PathCmd::LineTo { x: 100.0, y: 100.0 },
            PathCmd::LineTo { x: 0.0, y: 100.0 },
            PathCmd::Close,
        ]
    }

    #[test]
    fn the_rng_is_deterministic_per_seed_and_uniform_in_unit_range() {
        let mut a = Rng::seeded("shape-1");
        let mut b = Rng::seeded("shape-1");
        let mut c = Rng::seeded("shape-2");
        let xs: Vec<f64> = (0..8).map(|_| a.next_f64()).collect();
        let ys: Vec<f64> = (0..8).map(|_| b.next_f64()).collect();
        let zs: Vec<f64> = (0..8).map(|_| c.next_f64()).collect();
        assert_eq!(xs, ys, "same seed, same stream");
        assert_ne!(xs, zs, "a different seed must diverge");
        assert!(xs.iter().all(|v| (0.0..1.0).contains(v)), "{xs:?}");
        assert!(xs.iter().any(|v| *v > 0.5) && xs.iter().any(|v| *v < 0.5));
    }

    #[test]
    fn same_seed_same_output_and_different_seed_differs() {
        let a = sketch_d(&square(), "s-1", 2.0);
        let b = sketch_d(&square(), "s-1", 2.0);
        let c = sketch_d(&square(), "s-2", 2.0);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, cmds_to_d(&square()), "a sketch is not the clean outline");
    }

    #[test]
    fn output_is_valid_path_data_with_two_passes() {
        let d = sketch_d(&square(), "s-1", 2.0);
        let cmds = parse_path_d(&d).expect("sketch output must parse back");
        assert_eq!(
            cmds.iter()
                .filter(|c| matches!(c, PathCmd::MoveTo { .. }))
                .count(),
            2,
            "two passes, one MoveTo each: {d}"
        );
        assert_eq!(
            cmds.iter().filter(|c| matches!(c, PathCmd::Close)).count(),
            2,
            "both passes close: {d}"
        );
        assert!(
            cmds.iter().any(|c| matches!(c, PathCmd::Quad { .. })),
            "square corners are rounded through quads: {d}"
        );
    }

    /// A closed outline's rounded start corner is left and re-entered on
    /// the same coordinate, so the two passes do not gap at the seam.
    #[test]
    fn a_closed_path_ends_where_it_started() {
        let d = sketch_d(&square(), "seam", 2.0);
        let cmds = parse_path_d(&d).unwrap();
        let mut start = None;
        for (i, c) in cmds.iter().enumerate() {
            match c {
                PathCmd::MoveTo { x, y } => start = Some((*x, *y)),
                PathCmd::Close => {
                    let last = &cmds[i - 1];
                    let end = match last {
                        PathCmd::Quad { x, y, .. } | PathCmd::LineTo { x, y } => (*x, *y),
                        other => panic!("unexpected command before Z: {other:?}"),
                    };
                    assert_eq!(Some(end), start, "pass must end on its MoveTo: {d}");
                }
                _ => {}
            }
        }
    }

    /// Zero-length segments — coincident vertices, a degenerate shape — must
    /// yield finite numbers, never NaN from normalising a zero vector.
    #[test]
    fn zero_length_segments_do_not_produce_nan() {
        let degenerate = vec![
            PathCmd::MoveTo { x: 5.0, y: 5.0 },
            PathCmd::LineTo { x: 5.0, y: 5.0 },
            PathCmd::LineTo { x: 5.0, y: 5.0 },
            PathCmd::Close,
        ];
        let d = sketch_d(&degenerate, "dot", 2.0);
        assert!(!d.contains("NaN") && !d.contains("inf"), "{d}");
        let cmds = parse_path_d(&d).unwrap();
        assert!(cmds.iter().flat_map(PathCmd::coords).all(f64::is_finite));
        let hairline = vec![
            PathCmd::MoveTo { x: 0.0, y: 0.0 },
            PathCmd::LineTo { x: 0.0, y: 0.0 },
            PathCmd::LineTo { x: 40.0, y: 0.0 },
        ];
        let d = sketch_d(&hairline, "hair", 2.0);
        assert!(parse_path_d(&d).is_ok(), "{d}");
    }

    /// Curves keep their control points attached to the jittered ends and
    /// are never corner-rounded; an open path starts and ends on its
    /// jittered end points (no rounding at the ends).
    #[test]
    fn curves_survive_and_open_ends_are_not_rounded() {
        let arc = vec![
            PathCmd::MoveTo { x: 0.0, y: 0.0 },
            PathCmd::Cubic {
                x1: 10.0,
                y1: -20.0,
                x2: 30.0,
                y2: -20.0,
                x: 40.0,
                y: 0.0,
            },
            PathCmd::LineTo { x: 80.0, y: 0.0 },
        ];
        let d = sketch_d(&arc, "arc", 3.0);
        let cmds = parse_path_d(&d).unwrap();
        assert_eq!(cmds.len(), 6, "M C L per pass, nothing rounded: {d}");
        assert!(matches!(cmds[1], PathCmd::Cubic { .. }));
        for c in cmds.iter().flat_map(PathCmd::coords) {
            assert!(c.is_finite());
        }
        // Every jittered coordinate stays within the offset of its source.
        if let PathCmd::LineTo { x, y } = cmds[2] {
            assert!((x - 80.0).abs() <= 1.01 && y.abs() <= 1.01, "{d}");
        } else {
            panic!("{d}");
        }
    }

    /// The rounding reach clamps to half the shorter adjacent side, so a
    /// short edge is split between its two corners and never inverts.
    #[test]
    fn corner_rounding_clamps_to_half_the_shorter_side() {
        let sliver = vec![
            PathCmd::MoveTo { x: 0.0, y: 0.0 },
            PathCmd::LineTo { x: 100.0, y: 0.0 },
            PathCmd::LineTo { x: 100.0, y: 1.0 },
            PathCmd::LineTo { x: 0.0, y: 1.0 },
            PathCmd::Close,
        ];
        // Huge stroke width → roundness far beyond the 1-unit sides.
        let d = sketch_d(&sliver, "sliver", 20.0);
        let cmds = parse_path_d(&d).unwrap();
        assert!(
            cmds.iter().flat_map(PathCmd::coords).all(f64::is_finite),
            "{d}"
        );
        // No x coordinate escapes the jittered box by more than the reach.
        for c in cmds.iter().flat_map(PathCmd::coords) {
            assert!(c > -60.0 && c < 160.0, "{c} in {d}");
        }
    }
}
