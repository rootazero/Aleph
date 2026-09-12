//! Shared geometry for the drawing-infrastructure shapes — the ONE copy the
//! live renderer (`shape_view.rs`) and the export serializer (`export.rs`)
//! both consume, so the two cannot disagree about where a hexagon's corners
//! are or what a `Path`'s `d` means.
//!
//! Everything here is a pure function over the wire contract's types; the
//! `d` string is only ever read through `aleph_protocol::canvas::parse_path_d`
//! (its sole parser) and re-emitted through `cmds_to_d`.

use aleph_protocol::canvas::{cmds_to_d, parse_path_d, GeoForm, PathCmd};

/// `points=` for the polygonal geo forms, inside the box `(x, y, w, h)`.
/// `None` for the forms that are not polygons (`Rect`, `Ellipse`, `Pill` —
/// those render as their native SVG primitives).
#[must_use]
pub(super) fn polygon_points(form: GeoForm, x: f64, y: f64, w: f64, h: f64) -> Option<String> {
    let corners: Vec<(f64, f64)> = match form {
        GeoForm::Rect | GeoForm::Ellipse | GeoForm::Pill => return None,
        GeoForm::Diamond => vec![
            (x + w / 2.0, y),
            (x + w, y + h / 2.0),
            (x + w / 2.0, y + h),
            (x, y + h / 2.0),
        ],
        GeoForm::Triangle => vec![(x + w / 2.0, y), (x + w, y + h), (x, y + h)],
        GeoForm::Hexagon => {
            // Flat-topped: the top and bottom edges span the middle half.
            let q = w / 4.0;
            vec![
                (x + q, y),
                (x + w - q, y),
                (x + w, y + h / 2.0),
                (x + w - q, y + h),
                (x + q, y + h),
                (x, y + h / 2.0),
            ]
        }
    };
    Some(
        corners
            .iter()
            .map(|(px, py)| format!("{px},{py}"))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// The renderable `d` for a `Shape::Path`: the wire string re-emitted in
/// absolute form through the contract's parser, with a closing `Z` appended
/// when the shape is `closed` and the data did not already end on one.
/// `None` when the string does not parse — the server never admits such a
/// shape, so a `None` here is a document this build cannot vouch for, and it
/// renders nothing rather than guessing.
#[must_use]
pub(super) fn path_d(d: &str, closed: bool) -> Option<String> {
    let mut cmds = parse_path_d(d).ok()?;
    if closed && !matches!(cmds.last(), Some(PathCmd::Close)) {
        cmds.push(PathCmd::Close);
    }
    Some(cmds_to_d(&cmds))
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
            PathCmd::MoveTo { x, y } => PathCmd::MoveTo { x: x * sx, y: y * sy },
            PathCmd::LineTo { x, y } => PathCmd::LineTo { x: x * sx, y: y * sy },
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

    #[test]
    fn polygon_forms_have_their_corners_and_primitive_forms_have_none() {
        assert_eq!(
            polygon_points(GeoForm::Diamond, 0.0, 0.0, 100.0, 50.0).as_deref(),
            Some("50,0 100,25 50,50 0,25")
        );
        assert_eq!(
            polygon_points(GeoForm::Triangle, 10.0, 10.0, 20.0, 20.0).as_deref(),
            Some("20,10 30,30 10,30")
        );
        assert_eq!(
            polygon_points(GeoForm::Hexagon, 0.0, 0.0, 80.0, 40.0).as_deref(),
            Some("20,0 60,0 80,20 60,40 20,40 0,20")
        );
        for form in [GeoForm::Rect, GeoForm::Ellipse, GeoForm::Pill] {
            assert!(polygon_points(form, 0.0, 0.0, 1.0, 1.0).is_none(), "{form:?}");
        }
    }

    #[test]
    fn path_d_normalises_to_absolute_and_closes_on_request() {
        assert_eq!(
            path_d("m0 0 l10 0 v10", false).as_deref(),
            Some("M0 0 L10 0 L10 10")
        );
        assert_eq!(
            path_d("m0 0 l10 0 v10", true).as_deref(),
            Some("M0 0 L10 0 L10 10 Z")
        );
        assert_eq!(
            path_d("M0 0 L1 1 Z", true).as_deref(),
            Some("M0 0 L1 1 Z"),
            "an already-closed path gains no second Z"
        );
        assert!(path_d("M0 0 A1 1 0 0 1 2 2", false).is_none());
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
