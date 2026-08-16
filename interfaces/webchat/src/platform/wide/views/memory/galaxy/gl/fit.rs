//! Content extents for the camera: the bounding sphere the viewport must frame,
//! and the neighbourhood radius a focus fly-to must frame. Pure — unit-tested on
//! native.
//!
//! A bounding SPHERE, never an AABB: the orbit camera can sit at any
//! azimuth/elevation, so a box-derived fit would clip the instant the user
//! rotates. The sphere fit is orbit-invariant.

use super::math::Vec3;

/// Bounding sphere of the node cloud: centroid + max radial distance.
/// Returns `None` for an empty cloud (the caller keeps the current camera).
/// The radius floors at 1.0 so a single node (or a degenerate cloud) can never
/// produce a zero fit distance.
pub fn bounding_sphere(pos: &[Vec3]) -> Option<(Vec3, f32)> {
    if pos.is_empty() {
        return None;
    }
    let c = pos
        .iter()
        .fold(Vec3::zero(), |a, p| a.add(p))
        .scale(1.0 / pos.len() as f32);
    let r = pos
        .iter()
        .map(|p| p.sub(&c).length())
        .fold(0.0_f32, f32::max)
        .max(1.0);
    Some((c, r))
}

/// Radius that frames a node PLUS its 1-hop neighbourhood, so "focus this note"
/// shows the note in its context instead of one isolated star. Falls back to a
/// fraction of the whole-graph radius for isolated nodes (and clamps the hub
/// case) so focus never degenerates into a speck or into a full zoom-out.
pub fn focus_radius(idx: u32, pos: &[Vec3], edges: &[(u32, u32)], graph_r: f32) -> f32 {
    let Some(p) = pos.get(idx as usize) else {
        return graph_r;
    };
    let mut r = 0.0_f32;
    for &(a, b) in edges {
        let other = if a == idx {
            Some(b)
        } else if b == idx {
            Some(a)
        } else {
            None
        };
        if let Some(o) = other.and_then(|o| pos.get(o as usize)) {
            r = r.max(o.sub(p).length());
        }
    }
    r.clamp(0.06 * graph_r, 0.5 * graph_r).max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounding_sphere_empty_is_none() {
        assert!(bounding_sphere(&[]).is_none());
    }

    #[test]
    fn bounding_sphere_single_point_has_floor_radius() {
        let (c, r) = bounding_sphere(&[Vec3::new(10.0, -4.0, 2.0)]).unwrap();
        assert!((c.x - 10.0).abs() < 1e-4 && (c.y + 4.0).abs() < 1e-4);
        assert!((r - 1.0).abs() < 1e-4, "r={r}");
    }

    #[test]
    fn bounding_sphere_contains_all_points() {
        let pts = vec![
            Vec3::new(100.0, 0.0, 0.0),
            Vec3::new(-100.0, 0.0, 0.0),
            Vec3::new(0.0, 60.0, 0.0),
            Vec3::new(0.0, 0.0, -260.0),
        ];
        let (c, r) = bounding_sphere(&pts).unwrap();
        // Centroid is the mean; every point must sit inside the radius.
        assert!((c.z + 65.0).abs() < 1e-3, "c.z={}", c.z);
        for p in &pts {
            assert!(
                p.sub(&c).length() <= r + 1e-3,
                "point outside sphere: {:?} r={r}",
                p
            );
        }
        // And the radius is tight: at least one point sits ON the sphere.
        let max = pts
            .iter()
            .map(|p| p.sub(&c).length())
            .fold(0.0_f32, f32::max);
        assert!((max - r).abs() < 1e-3);
    }

    #[test]
    fn focus_radius_spans_the_one_hop_neighbourhood() {
        let pos = vec![
            Vec3::zero(),
            Vec3::new(200.0, 0.0, 0.0),
            Vec3::new(0.0, 90.0, 0.0),
        ];
        // graph_r large enough that neither clamp bites: 200 ∈ [0.06*1000, 0.5*1000].
        let r = focus_radius(0, &pos, &[(0, 1), (0, 2)], 1000.0);
        assert!((r - 200.0).abs() < 1e-3, "r={r}");
    }

    #[test]
    fn focus_radius_isolated_node_falls_back_to_graph_fraction() {
        let pos = vec![Vec3::zero(), Vec3::new(500.0, 0.0, 0.0)];
        let r = focus_radius(0, &pos, &[], 1000.0);
        assert!((r - 60.0).abs() < 1e-3, "expected 0.06*graph_r, got {r}");
    }

    #[test]
    fn focus_radius_hub_is_capped_at_half_the_graph() {
        let pos = vec![Vec3::zero(), Vec3::new(0.0, 0.0, 900.0)];
        let r = focus_radius(0, &pos, &[(0, 1)], 1000.0);
        assert!((r - 500.0).abs() < 1e-3, "expected 0.5*graph_r, got {r}");
    }
}
