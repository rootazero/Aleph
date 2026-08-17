//! Pure perfect-freehand-style stroke outline — the pressure-aware closed
//! silhouette an ink polyline is rendered as. A hand-written port of the
//! perfect-freehand algorithm's shape (streamline the input, derive a
//! per-point radius from pressure, offset perpendicular to the travel
//! direction, close with round caps), kept deliberately small (R3: no new
//! dependency — the npm original stays out).
//!
//! Input is the wire format of `Shape::Ink::points` — `[x, y, pressure]`
//! triplets in shape-local coordinates. Output is a **closed** outline the
//! renderer *fills* (`fill`, not `stroke`): the polygon IS the stroke's
//! silhouette, which is what makes the width follow pressure point by point.
//!
//! All math is f64; the wire's f32 points widen at the boundary.

/// Input smoothing: how strongly each raw point is pulled toward its
/// predecessor (0 = raw input, 1 = it never moves). The interpolation factor
/// applied to each new point is `0.15 + (1 - STREAMLINE) * 0.85`,
/// perfect-freehand's own mapping.
pub(super) const STREAMLINE: f64 = 0.5;
/// Pressure → radius influence. 0 keeps the width uniform; at the default,
/// pressure 0 draws at half the base radius and pressure 1 at 1.5×.
pub(super) const THINNING: f64 = 0.5;
/// Direction smoothing: how much an interior point's offset direction blends
/// from its forward segment toward the central difference of its neighbors —
/// what rounds corners instead of mitering them.
pub(super) const SMOOTHING: f64 = 0.5;
/// Segments in each semicircular end cap (a dot uses 2×).
const CAP_SEGMENTS: usize = 8;
/// Streamlined points closer than this collapse into one.
const EPSILON: f64 = 0.01;

/// Streamline pass: every point after the first is interpolated toward its
/// (already-streamlined) predecessor, and near-duplicates collapse.
fn streamlined(points: &[[f32; 3]]) -> Vec<(f64, f64, f64)> {
    let t = 0.15 + (1.0 - STREAMLINE) * 0.85;
    let mut out: Vec<(f64, f64, f64)> = Vec::with_capacity(points.len());
    for p in points {
        let (x, y, pressure) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
        match out.last() {
            None => out.push((x, y, pressure)),
            Some(&(lx, ly, _)) => {
                let nx = lx + (x - lx) * t;
                let ny = ly + (y - ly) * t;
                if ((nx - lx).powi(2) + (ny - ly).powi(2)).sqrt() >= EPSILON {
                    out.push((nx, ny, pressure));
                }
            }
        }
    }
    out
}

/// Stroke radius at one point: `size` is the base diameter, pressure scales
/// it through [`THINNING`]. Clamped above zero — a zero radius would emit
/// coincident outline vertices.
fn radius_for(size: f64, pressure: f64) -> f64 {
    ((size / 2.0) * (0.5 - THINNING * (0.5 - pressure))).max(0.05)
}

fn rotate(v: (f64, f64), angle: f64) -> (f64, f64) {
    let (s, c) = angle.sin_cos();
    (v.0 * c - v.1 * s, v.0 * s + v.1 * c)
}

fn normalize(v: (f64, f64)) -> Option<(f64, f64)> {
    let len = (v.0 * v.0 + v.1 * v.1).sqrt();
    (len > 1e-12).then(|| (v.0 / len, v.1 / len))
}

/// The closed outline polygon for an ink stroke. Consecutive vertices are
/// implicitly connected and the last connects back to the first (the path
/// builder emits `Z`).
///
/// - No points → empty (the caller renders nothing).
/// - One point (a tap) → a full circle: a pen tap must leave a dot.
/// - Two or more → left offsets forward, a semicircular end cap, right
///   offsets backward, a semicircular start cap. Both caps rotate by −π so
///   the polygon keeps one consistent winding.
pub(super) fn stroke_outline(points: &[[f32; 3]], size: f64) -> Vec<(f64, f64)> {
    let pts = streamlined(points);
    let Some(&(fx, fy, fp)) = pts.first() else {
        return Vec::new();
    };
    if pts.len() == 1 {
        let r = radius_for(size, fp);
        let n = CAP_SEGMENTS * 2;
        return (0..n)
            .map(|i| {
                let a = std::f64::consts::TAU * (i as f64) / (n as f64);
                (fx + r * a.cos(), fy + r * a.sin())
            })
            .collect();
    }
    let n = pts.len();
    let seg = |i: usize| (pts[i + 1].0 - pts[i].0, pts[i + 1].1 - pts[i].1);
    // Per-point unit travel direction; interior points blend toward the
    // central difference of their neighbors (SMOOTHING).
    let mut dirs: Vec<(f64, f64)> = Vec::with_capacity(n);
    for i in 0..n {
        let forward = if i + 1 < n { seg(i) } else { seg(i - 1) };
        let base = normalize(forward).unwrap_or((1.0, 0.0));
        let dir = if i > 0 && i + 1 < n {
            let central = normalize((pts[i + 1].0 - pts[i - 1].0, pts[i + 1].1 - pts[i - 1].1))
                .unwrap_or(base);
            normalize((
                base.0 + (central.0 - base.0) * SMOOTHING,
                base.1 + (central.1 - base.1) * SMOOTHING,
            ))
            .unwrap_or(base)
        } else {
            base
        };
        dirs.push(dir);
    }
    let mut left: Vec<(f64, f64)> = Vec::with_capacity(n);
    let mut right: Vec<(f64, f64)> = Vec::with_capacity(n);
    for i in 0..n {
        let (px, py, pressure) = pts[i];
        let r = radius_for(size, pressure);
        let (ox, oy) = (-dirs[i].1 * r, dirs[i].0 * r);
        left.push((px + ox, py + oy));
        right.push((px - ox, py - oy));
    }
    let mut outline = left;
    let (ex, ey, _) = pts[n - 1];
    let end_offset = (outline[n - 1].0 - ex, outline[n - 1].1 - ey);
    for k in 1..CAP_SEGMENTS {
        let v = rotate(
            end_offset,
            -std::f64::consts::PI * (k as f64) / (CAP_SEGMENTS as f64),
        );
        outline.push((ex + v.0, ey + v.1));
    }
    for i in (0..n).rev() {
        outline.push(right[i]);
    }
    let start_offset = (right[0].0 - fx, right[0].1 - fy);
    for k in 1..CAP_SEGMENTS {
        let v = rotate(
            start_offset,
            -std::f64::consts::PI * (k as f64) / (CAP_SEGMENTS as f64),
        );
        outline.push((fx + v.0, fy + v.1));
    }
    outline
}

/// SVG `d` string for the closed outline: `M … L … Z`, empty for no points.
#[must_use]
pub(super) fn outline_path_d(points: &[[f32; 3]], size: f64) -> String {
    let outline = stroke_outline(points, size);
    let mut iter = outline.iter();
    let Some(first) = iter.next() else {
        return String::new();
    };
    let mut d = format!("M {:.2} {:.2}", first.0, first.1);
    for p in iter {
        d.push_str(&format!(" L {:.2} {:.2}", p.0, p.1));
    }
    d.push_str(" Z");
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn orient(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
        (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
    }

    /// Proper (interior) crossing of two segments — shared endpoints and
    /// collinear touching do not count.
    fn segments_cross(p1: (f64, f64), p2: (f64, f64), p3: (f64, f64), p4: (f64, f64)) -> bool {
        let d1 = orient(p3, p4, p1);
        let d2 = orient(p3, p4, p2);
        let d3 = orient(p1, p2, p3);
        let d4 = orient(p1, p2, p4);
        d1 * d2 < 0.0 && d3 * d4 < 0.0
    }

    /// The closed polygon has no two non-adjacent edges properly crossing.
    fn is_simple_polygon(pts: &[(f64, f64)]) -> bool {
        let n = pts.len();
        for i in 0..n {
            for j in (i + 1)..n {
                let adjacent = j == i + 1 || (i == 0 && j == n - 1);
                if adjacent {
                    continue;
                }
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                let (c, d) = (pts[j], pts[(j + 1) % n]);
                if segments_cross(a, b, c, d) {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn empty_and_single_point_inputs_do_not_panic() {
        assert!(stroke_outline(&[], 8.0).is_empty(), "no points, no outline");
        assert_eq!(outline_path_d(&[], 8.0), "");

        let dot = stroke_outline(&[[10.0, 20.0, 0.5]], 8.0);
        assert!(dot.len() >= 8, "a tap must leave a dot ring, got {dot:?}");
        for (x, y) in &dot {
            let dist = ((x - 10.0).powi(2) + (y - 20.0).powi(2)).sqrt();
            assert!(dist > 0.0 && dist < 8.0, "dot vertex off the ring: {dist}");
        }
    }

    #[test]
    fn a_two_point_stroke_yields_a_closed_non_self_intersecting_outline() {
        let outline = stroke_outline(&[[0.0, 0.0, 0.5], [100.0, 0.0, 0.5]], 8.0);
        assert!(
            outline.len() >= 2 * CAP_SEGMENTS,
            "two sides plus two caps expected, got {} vertices",
            outline.len()
        );
        assert!(
            is_simple_polygon(&outline),
            "the outline must not self-intersect: {outline:?}"
        );
        // Closed: the path builder terminates with Z.
        let d = outline_path_d(&[[0.0, 0.0, 0.5], [100.0, 0.0, 0.5]], 8.0);
        assert!(d.starts_with("M ") && d.ends_with(" Z"), "got {d}");
        // The silhouette straddles the stroke's spine (y = 0).
        assert!(outline.iter().any(|(_, y)| *y > 0.0));
        assert!(outline.iter().any(|(_, y)| *y < 0.0));
    }

    #[test]
    fn pressure_widens_the_outline() {
        let width_at = |p: f32| -> f64 {
            stroke_outline(&[[0.0, 0.0, p], [100.0, 0.0, p]], 8.0)
                .iter()
                .map(|(_, y)| y.abs())
                .fold(0.0, f64::max)
        };
        assert!(
            width_at(1.0) > width_at(0.1),
            "a heavier press must draw a wider stroke"
        );
    }

    #[test]
    fn streamlining_pulls_each_point_toward_its_predecessor() {
        // t = 0.15 + (1 - STREAMLINE) * 0.85 = 0.575 at the default.
        let pts = streamlined(&[[0.0, 0.0, 0.5], [100.0, 0.0, 0.5]]);
        assert_eq!(pts.len(), 2);
        assert!(
            pts[1].0 > 0.0 && pts[1].0 < 100.0,
            "the second point must land strictly between, got {:?}",
            pts[1]
        );
        // Coincident input collapses to a single point (a dot, not a panic).
        let dup = streamlined(&[[5.0, 5.0, 0.5], [5.0, 5.0, 0.5], [5.0, 5.0, 0.5]]);
        assert_eq!(dup.len(), 1);
    }
}
