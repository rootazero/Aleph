//! Shared geometry for the drawing-infrastructure shapes — the ONE copy the
//! live renderer (`shape_view.rs`), the export serializer (`export.rs`), the
//! sketch synthesiser (`sketch.rs`) and the arrow clipper (`arrow_geom.rs`)
//! all consume, so none of them can disagree about where a hexagon's
//! corners are, how an ellipse curves, or what a `Path`'s `d` means.
//!
//! [`geo_cmds`] is the command table: every `GeoForm` — rect and ellipse
//! included, there are no native `<rect>`/`<ellipse>` primitives left in the
//! renderers — is a list of contract `PathCmd`s in shape-local coordinates.
//!
//! Everything here is a pure function over the wire contract's types; the
//! `d` string is only ever read through `aleph_protocol::canvas::parse_path_d`
//! (its sole parser) and re-emitted through `cmds_to_d`.

use aleph_protocol::canvas::{cmds_to_d, parse_path_d, GeoForm, PathCmd};

/// Cubic Bézier circle constant: a quarter arc's control handles sit this
/// fraction of the radius along the tangents (4/3 · tan(π/8)).
const KAPPA: f64 = 0.552_284_749_8;

/// The outline of a geo form as absolute path commands in **shape-local**
/// coordinates (origin at the shape's top-left, spanning `w`×`h`). Every
/// form — rect and ellipse included — is a command list, so the sketch
/// synthesiser, the arrow clipper and both renderers read one recipe:
///
/// - `Rect`: four lines (sharp corners; the stroke's round join softens
///   them, the sketch pass rounds them properly).
/// - `Ellipse`: four cubics ([`KAPPA`] handles).
/// - `Pill`: a stadium — four quarter-circle cubics of radius
///   `min(w, h) / 2` joined by the straight runs that remain (a run of zero
///   length is omitted, so a square pill is a circle).
/// - `Diamond` / `Triangle` / `Hexagon` (flat-topped, the top and bottom
///   edges span the middle half): polygons.
#[must_use]
pub(super) fn geo_cmds(form: GeoForm, w: f64, h: f64) -> Vec<PathCmd> {
    let line = |x: f64, y: f64| PathCmd::LineTo { x, y };
    let polygon = |corners: &[(f64, f64)]| -> Vec<PathCmd> {
        let mut cmds = Vec::with_capacity(corners.len() + 1);
        for (i, &(x, y)) in corners.iter().enumerate() {
            cmds.push(if i == 0 {
                PathCmd::MoveTo { x, y }
            } else {
                line(x, y)
            });
        }
        cmds.push(PathCmd::Close);
        cmds
    };
    match form {
        GeoForm::Rect => polygon(&[(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]),
        GeoForm::Diamond => polygon(&[(w / 2.0, 0.0), (w, h / 2.0), (w / 2.0, h), (0.0, h / 2.0)]),
        GeoForm::Triangle => polygon(&[(w / 2.0, 0.0), (w, h), (0.0, h)]),
        GeoForm::Hexagon => {
            let q = w / 4.0;
            polygon(&[
                (q, 0.0),
                (w - q, 0.0),
                (w, h / 2.0),
                (w - q, h),
                (q, h),
                (0.0, h / 2.0),
            ])
        }
        GeoForm::Ellipse => {
            let (rx, ry) = (w / 2.0, h / 2.0);
            let (cx, cy) = (rx, ry);
            let (kx, ky) = (rx * KAPPA, ry * KAPPA);
            vec![
                PathCmd::MoveTo { x: cx + rx, y: cy },
                cubic((cx + rx, cy + ky), (cx + kx, cy + ry), (cx, cy + ry)),
                cubic((cx - kx, cy + ry), (cx - rx, cy + ky), (cx - rx, cy)),
                cubic((cx - rx, cy - ky), (cx - kx, cy - ry), (cx, cy - ry)),
                cubic((cx + kx, cy - ry), (cx + rx, cy - ky), (cx + rx, cy)),
                PathCmd::Close,
            ]
        }
        GeoForm::Pill => {
            let r = w.min(h) / 2.0;
            let k = r * KAPPA;
            let mut cmds = vec![PathCmd::MoveTo { x: r, y: 0.0 }];
            if w - r > r {
                cmds.push(line(w - r, 0.0));
            }
            cmds.push(cubic((w - r + k, 0.0), (w, r - k), (w, r)));
            if h - r > r {
                cmds.push(line(w, h - r));
            }
            cmds.push(cubic((w, h - r + k), (w - r + k, h), (w - r, h)));
            if w - r > r {
                cmds.push(line(r, h));
            }
            cmds.push(cubic((r - k, h), (0.0, h - r + k), (0.0, h - r)));
            if h - r > r {
                cmds.push(line(0.0, r));
            }
            cmds.push(cubic((0.0, r - k), (r - k, 0.0), (r, 0.0)));
            cmds.push(PathCmd::Close);
            cmds
        }
    }
}

fn cubic(c1: (f64, f64), c2: (f64, f64), end: (f64, f64)) -> PathCmd {
    PathCmd::Cubic {
        x1: c1.0,
        y1: c1.1,
        x2: c2.0,
        y2: c2.1,
        x: end.0,
        y: end.1,
    }
}

/// `cmds` shifted by `(dx, dy)` — shape-local commands into world space,
/// which is what the arrow clipper needs and what `scale_path_d` is the
/// scaling twin of.
#[must_use]
pub(super) fn translate_cmds(cmds: &[PathCmd], dx: f64, dy: f64) -> Vec<PathCmd> {
    cmds.iter()
        .map(|cmd| match *cmd {
            PathCmd::MoveTo { x, y } => PathCmd::MoveTo {
                x: x + dx,
                y: y + dy,
            },
            PathCmd::LineTo { x, y } => PathCmd::LineTo {
                x: x + dx,
                y: y + dy,
            },
            PathCmd::Quad { x1, y1, x, y } => PathCmd::Quad {
                x1: x1 + dx,
                y1: y1 + dy,
                x: x + dx,
                y: y + dy,
            },
            PathCmd::Cubic {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => PathCmd::Cubic {
                x1: x1 + dx,
                y1: y1 + dy,
                x2: x2 + dx,
                y2: y2 + dy,
                x: x + dx,
                y: y + dy,
            },
            PathCmd::Close => PathCmd::Close,
        })
        .collect()
}

/// The straight-segment approximation of a command list: every subpath as a
/// polyline (curves sampled at [`CURVE_SAMPLES`] points), closed subpaths
/// closed. What the arrow clipper intersects against.
#[must_use]
pub(super) fn flatten_cmds(cmds: &[PathCmd]) -> Vec<Vec<(f64, f64)>> {
    let mut polylines: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut start = (0.0, 0.0);
    let mut cur = (0.0, 0.0);
    for cmd in cmds {
        match *cmd {
            PathCmd::MoveTo { x, y } => {
                start = (x, y);
                cur = start;
                polylines.push(vec![cur]);
            }
            PathCmd::LineTo { x, y } => {
                cur = (x, y);
                push_point(&mut polylines, cur);
            }
            PathCmd::Quad { x1, y1, x, y } => {
                let (p0, c, p1) = (cur, (x1, y1), (x, y));
                for i in 1..=CURVE_SAMPLES {
                    let t = i as f64 / CURVE_SAMPLES as f64;
                    let u = 1.0 - t;
                    let px = u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p1.0;
                    let py = u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p1.1;
                    push_point(&mut polylines, (px, py));
                }
                cur = p1;
            }
            PathCmd::Cubic {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let (p0, c1, c2, p1) = (cur, (x1, y1), (x2, y2), (x, y));
                for i in 1..=CURVE_SAMPLES {
                    let t = i as f64 / CURVE_SAMPLES as f64;
                    let u = 1.0 - t;
                    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    let px = a * p0.0 + b * c1.0 + c * c2.0 + d * p1.0;
                    let py = a * p0.1 + b * c1.1 + c * c2.1 + d * p1.1;
                    push_point(&mut polylines, (px, py));
                }
                cur = p1;
            }
            PathCmd::Close => {
                push_point(&mut polylines, start);
                cur = start;
            }
        }
    }
    polylines
}

/// Straight segments per curve command when flattening.
const CURVE_SAMPLES: usize = 8;

fn push_point(polylines: &mut Vec<Vec<(f64, f64)>>, p: (f64, f64)) {
    match polylines.last_mut() {
        Some(line) => line.push(p),
        None => polylines.push(vec![(0.0, 0.0), p]),
    }
}

/// The renderable commands for a `Shape::Path`: the wire string parsed to
/// absolute commands by the contract's parser, with a closing `Z` appended
/// when the shape is `closed` and the data did not already end on one.
/// `None` when the string does not parse — the server never admits such a
/// shape, so a `None` here is a document this build cannot vouch for, and it
/// renders nothing rather than guessing.
#[must_use]
pub(super) fn path_cmds(d: &str, closed: bool) -> Option<Vec<PathCmd>> {
    let mut cmds = parse_path_d(d).ok()?;
    if closed && !matches!(cmds.last(), Some(PathCmd::Close)) {
        cmds.push(PathCmd::Close);
    }
    Some(cmds)
}

/// Extent of every coordinate a path's commands carry (control points
/// included, so the box is a conservative superset of the drawn curve):
/// `(min_x, min_y, max_x, max_y)`. `None` for a path with no coordinates or
/// one that does not parse.
#[must_use]
pub(super) fn path_extent(d: &str) -> Option<(f64, f64, f64, f64)> {
    let cmds = parse_path_d(d).ok()?;
    let mut extent: Option<(f64, f64, f64, f64)> = None;
    for cmd in &cmds {
        let coords = cmd.coords();
        for pair in coords.chunks(2) {
            let (px, py) = (pair[0], pair[1]);
            extent = Some(match extent {
                None => (px, py, px, py),
                Some((x0, y0, x1, y1)) => (x0.min(px), y0.min(py), x1.max(px), y1.max(py)),
            });
        }
    }
    extent
}

/// A path's `d` with every coordinate scaled about the shape origin — what a
/// resize does to shape-local path data (the `Ink` points' twin). A string
/// that does not parse is returned unchanged: the resize must not turn an
/// unrenderable shape into a different unrenderable shape.
#[must_use]
pub(super) fn scale_path_d(d: &str, sx: f64, sy: f64) -> String {
    let Ok(cmds) = parse_path_d(d) else {
        return d.to_string();
    };
    let scaled: Vec<PathCmd> = cmds
        .iter()
        .map(|cmd| match *cmd {
            PathCmd::MoveTo { x, y } => PathCmd::MoveTo {
                x: x * sx,
                y: y * sy,
            },
            PathCmd::LineTo { x, y } => PathCmd::LineTo {
                x: x * sx,
                y: y * sy,
            },
            PathCmd::Quad { x1, y1, x, y } => PathCmd::Quad {
                x1: x1 * sx,
                y1: y1 * sy,
                x: x * sx,
                y: y * sy,
            },
            PathCmd::Cubic {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => PathCmd::Cubic {
                x1: x1 * sx,
                y1: y1 * sy,
                x2: x2 * sx,
                y2: y2 * sy,
                x: x * sx,
                y: y * sy,
            },
            PathCmd::Close => PathCmd::Close,
        })
        .collect();
    cmds_to_d(&scaled)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_FORMS: [GeoForm; 6] = [
        GeoForm::Rect,
        GeoForm::Ellipse,
        GeoForm::Diamond,
        GeoForm::Triangle,
        GeoForm::Hexagon,
        GeoForm::Pill,
    ];

    #[test]
    fn polygon_forms_have_their_corners() {
        assert_eq!(
            cmds_to_d(&geo_cmds(GeoForm::Diamond, 100.0, 50.0)),
            "M50 0 L100 25 L50 50 L0 25 Z"
        );
        assert_eq!(
            cmds_to_d(&geo_cmds(GeoForm::Triangle, 20.0, 20.0)),
            "M10 0 L20 20 L0 20 Z"
        );
        assert_eq!(
            cmds_to_d(&geo_cmds(GeoForm::Hexagon, 80.0, 40.0)),
            "M20 0 L60 0 L80 20 L60 40 L20 40 L0 20 Z"
        );
        assert_eq!(
            cmds_to_d(&geo_cmds(GeoForm::Rect, 30.0, 10.0)),
            "M0 0 L30 0 L30 10 L0 10 Z"
        );
    }

    /// Every form is a closed command list spanning exactly its box (the
    /// curved forms touch all four sides at their extremes), and every
    /// coordinate stays inside it — a form that overshot its box would
    /// break hit-testing, which trusts `common.w/h`.
    #[test]
    fn every_form_is_closed_and_fills_its_box() {
        for form in ALL_FORMS {
            let cmds = geo_cmds(form, 120.0, 40.0);
            assert!(
                matches!(cmds.first(), Some(PathCmd::MoveTo { .. })),
                "{form:?}"
            );
            assert!(matches!(cmds.last(), Some(PathCmd::Close)), "{form:?}");
            let pts: Vec<(f64, f64)> = flatten_cmds(&cmds).into_iter().flatten().collect();
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for (x, y) in pts {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
            assert!(
                x0 >= -1e-9 && y0 >= -1e-9 && x1 <= 120.0 + 1e-9 && y1 <= 40.0 + 1e-9,
                "{form:?}"
            );
            assert!(
                x0 < 1e-9 && y0 < 1e-9 && x1 > 120.0 - 1e-9 && y1 > 40.0 - 1e-9,
                "{form:?} touches every side"
            );
        }
    }

    /// The ellipse is four κ-handled cubics; the pill's straight runs vanish
    /// when the box is square (it becomes a circle), and appear on the long
    /// axis otherwise.
    #[test]
    fn ellipse_and_pill_are_cubic_chains() {
        let ellipse = geo_cmds(GeoForm::Ellipse, 100.0, 60.0);
        assert_eq!(
            ellipse
                .iter()
                .filter(|c| matches!(c, PathCmd::Cubic { .. }))
                .count(),
            4
        );
        assert!(ellipse.iter().all(|c| !matches!(c, PathCmd::LineTo { .. })));
        let circle = geo_cmds(GeoForm::Pill, 50.0, 50.0);
        assert_eq!(
            circle
                .iter()
                .filter(|c| matches!(c, PathCmd::LineTo { .. }))
                .count(),
            0
        );
        let wide = geo_cmds(GeoForm::Pill, 100.0, 40.0);
        assert_eq!(
            wide.iter()
                .filter(|c| matches!(c, PathCmd::LineTo { .. }))
                .count(),
            2
        );
        assert!(
            cmds_to_d(&wide).starts_with("M20 0 L80 0 C"),
            "{}",
            cmds_to_d(&wide)
        );
        let tall = geo_cmds(GeoForm::Pill, 40.0, 100.0);
        assert_eq!(
            tall.iter()
                .filter(|c| matches!(c, PathCmd::LineTo { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn translate_cmds_shifts_every_coordinate() {
        let moved = translate_cmds(&geo_cmds(GeoForm::Diamond, 10.0, 10.0), 100.0, 5.0);
        assert_eq!(cmds_to_d(&moved), "M105 5 L110 10 L105 15 L100 10 Z");
    }

    #[test]
    fn flatten_samples_curves_and_closes_subpaths() {
        let lines = flatten_cmds(&geo_cmds(GeoForm::Ellipse, 10.0, 10.0));
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0].len(),
            1 + 4 * CURVE_SAMPLES + 1,
            "start + samples + close"
        );
        assert_eq!(lines[0].first(), lines[0].last());
    }

    #[test]
    fn path_cmds_normalise_to_absolute_and_close_on_request() {
        let d = |s: &str, closed: bool| path_cmds(s, closed).map(|c| cmds_to_d(&c));
        assert_eq!(
            d("m0 0 l10 0 v10", false).as_deref(),
            Some("M0 0 L10 0 L10 10")
        );
        assert_eq!(
            d("m0 0 l10 0 v10", true).as_deref(),
            Some("M0 0 L10 0 L10 10 Z")
        );
        assert_eq!(
            d("M0 0 L1 1 Z", true).as_deref(),
            Some("M0 0 L1 1 Z"),
            "an already-closed path gains no second Z"
        );
        assert!(d("M0 0 A1 1 0 0 1 2 2", false).is_none());
    }

    #[test]
    fn path_extent_covers_control_points_and_is_none_for_nothing() {
        assert_eq!(
            path_extent("M10 10 Q-5 40 20 20"),
            Some((-5.0, 10.0, 20.0, 40.0)),
            "the control point widens the box"
        );
        assert_eq!(path_extent(""), None);
    }

    #[test]
    fn scale_path_d_scales_every_coordinate_and_leaves_garbage_alone() {
        assert_eq!(
            scale_path_d("M1 1 L2 4 Q1 1 3 3 C1 2 3 4 5 6 Z", 2.0, 0.5),
            "M2 0.5 L4 2 Q2 0.5 6 1.5 C2 1 6 2 10 3 Z"
        );
        assert_eq!(scale_path_d("not a path", 2.0, 2.0), "not a path");
    }
}
