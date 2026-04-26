use crate::canvas_engine::types::{Neighborhood, Vec3};
use std::collections::HashMap;

/// Standard smoothstep ease-in-out: 3t² - 2t³.
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    3.0 * t * t - 2.0 * t * t * t
}

/// Linear interpolation between two Vec3s.
pub fn lerp_vec3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    Vec3::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t)
}

/// Result of interpolating one node between two neighborhoods.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TweenResult {
    pub pos: Vec3,
    pub opacity: f32,
}

/// Outward drift used for nodes leaving / entering the view.
pub fn drift_outward(direction: Vec3, magnitude: f32) -> Vec3 {
    let len = (direction.x * direction.x + direction.y * direction.y).sqrt().max(1.0);
    Vec3::new(direction.x / len * magnitude, direction.y / len * magnitude, 0.0)
}

/// Interpolate a single node id between old and new neighborhoods at parameter t.
pub fn lerp_node(
    node_id: &str,
    from: &Neighborhood,
    to: &Neighborhood,
    t: f32,
) -> TweenResult {
    let eased = ease_in_out(t);
    let from_pos = from.target_positions.get(node_id).copied();
    let to_pos = to.target_positions.get(node_id).copied();
    match (from_pos, to_pos) {
        (Some(p1), Some(p2)) => TweenResult {
            pos: lerp_vec3(p1, p2, eased),
            opacity: 1.0,
        },
        (Some(p1), None) => {
            let drift = drift_outward(p1, 30.0 * t);
            let drift_z = lerp_vec3(p1, Vec3::new(p1.x, p1.y, 200.0), t);
            TweenResult {
                pos: Vec3::new(drift_z.x + drift.x, drift_z.y + drift.y, drift_z.z),
                opacity: 1.0 - t,
            }
        }
        (None, Some(p2)) => {
            let drift = drift_outward(p2, 30.0 * (1.0 - t));
            let drift_z = lerp_vec3(Vec3::new(p2.x, p2.y, 200.0), p2, t);
            TweenResult {
                pos: Vec3::new(drift_z.x + drift.x, drift_z.y + drift.y, drift_z.z),
                opacity: t,
            }
        }
        (None, None) => TweenResult {
            pos: Vec3::new(0.0, 0.0, 0.0),
            opacity: 0.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas_engine::types::*;

    fn empty_nbhd() -> Neighborhood {
        Neighborhood {
            center: dummy_node("c"),
            one_hop: vec![],
            two_hop: vec![],
            orphans: vec![],
            clusters: vec![],
            edges: vec![],
            target_positions: HashMap::new(),
            fetched_at_ms: 0.0,
        }
    }

    fn dummy_node(id: &str) -> CanvasNode {
        CanvasNode {
            id: id.to_string(),
            name: id.to_string(),
            category: "concept".to_string(),
            color: Color::new(0, 0, 0),
            radius: 30.0,
            position: Vec2::new(0.0, 0.0),
            velocity: Vec2::new(0.0, 0.0),
            pinned: false,
            z: 0.0,
            hop: 0,
            decay_score: 1.0,
            edge_count: 1,
        }
    }

    #[test]
    fn ease_endpoints() {
        assert!((ease_in_out(0.0) - 0.0).abs() < 1e-6);
        assert!((ease_in_out(1.0) - 1.0).abs() < 1e-6);
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn ease_clamps_out_of_range() {
        assert_eq!(ease_in_out(-0.5), 0.0);
        assert_eq!(ease_in_out(1.5), 1.0);
    }

    #[test]
    fn lerp_node_shared_interpolates_position() {
        let mut from = empty_nbhd();
        let mut to = empty_nbhd();
        from.target_positions.insert("x".to_string(), Vec3::new(0.0, 0.0, 0.0));
        to.target_positions.insert("x".to_string(), Vec3::new(100.0, 0.0, 0.0));
        let r = lerp_node("x", &from, &to, 0.5);
        assert!((r.pos.x - 50.0).abs() < 1e-3);
        assert!((r.opacity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn lerp_node_fadeout_only_in_from() {
        let mut from = empty_nbhd();
        let to = empty_nbhd();
        from.target_positions.insert("y".to_string(), Vec3::new(220.0, 0.0, 60.0));
        let r = lerp_node("y", &from, &to, 1.0);
        assert!((r.opacity - 0.0).abs() < 1e-3, "should fade out fully at t=1");
    }

    #[test]
    fn lerp_node_fadein_only_in_to() {
        let from = empty_nbhd();
        let mut to = empty_nbhd();
        to.target_positions.insert("z".to_string(), Vec3::new(220.0, 0.0, 60.0));
        let r = lerp_node("z", &from, &to, 0.0);
        assert!((r.opacity - 0.0).abs() < 1e-3, "should fade in from 0 at t=0");
        let r2 = lerp_node("z", &from, &to, 1.0);
        assert!((r2.opacity - 1.0).abs() < 1e-3, "should be fully visible at t=1");
    }
}
