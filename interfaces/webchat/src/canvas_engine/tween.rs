use crate::canvas_engine::types::{Neighborhood, Vec2, Vec3};
use std::collections::{HashMap, HashSet};

const SPRING_K: f64 = 220.0; // stiffness
const SPRING_C: f64 = 2.0 * 14.832396974191326; // critical damping = 2*sqrt(K)
const SPRING_SETTLE_POS_EPS: f64 = 0.5; // px
const SPRING_SETTLE_VEL_EPS: f64 = 5.0; // px/s
const SPRING_MAX_STEP_DT: f64 = 0.016; // sub-step cap (one 60fps frame)

/// Critically-damped 2D spring. Used for both drag spring-back (target=initial slot)
/// and promote tween-into-center (target=screen center).
///
/// Numerically stable: `tick(dt)` sub-steps if dt > 16ms.
#[derive(Debug, Clone, Copy)]
pub struct Spring2D {
    pos: Vec2,
    vel: Vec2,
    target: Vec2,
}

impl Spring2D {
    /// Create a spring at `initial_pos` moving with `initial_vel`, pulled toward `target`.
    pub fn new(initial_pos: Vec2, initial_vel: Vec2, target: Vec2) -> Self {
        Self {
            pos: initial_pos,
            vel: initial_vel,
            target,
        }
    }

    /// Advance the spring by `dt` seconds. Returns the new position.
    pub fn tick(&mut self, dt: f64) -> Vec2 {
        if !dt.is_finite() || dt <= 0.0 {
            return self.pos;
        }
        // Cap runaway dt (e.g., a paused-then-resumed browser tab) at 1 second
        // so the spring continues to animate but never spins on a huge delta.
        let mut remaining = dt.min(1.0);
        while remaining > 0.0 {
            let step = remaining.min(SPRING_MAX_STEP_DT);
            self.step(step);
            remaining -= step;
        }
        self.pos
    }

    fn step(&mut self, dt: f64) {
        // F = -k*(pos - target) - c*vel;  m=1 so a = F
        let displacement = self.pos - self.target;
        let accel = displacement * (-SPRING_K) - self.vel * SPRING_C;
        self.vel += accel * dt;
        self.pos += self.vel * dt;
    }

    /// True when both position is near target and velocity is near zero.
    pub fn settled(&self) -> bool {
        let displacement = self.pos - self.target;
        displacement.length() < SPRING_SETTLE_POS_EPS && self.vel.length() < SPRING_SETTLE_VEL_EPS
    }

    pub fn position(&self) -> Vec2 {
        self.pos
    }
    pub fn velocity(&self) -> Vec2 {
        self.vel
    }
    pub fn target(&self) -> Vec2 {
        self.target
    }
}

/// Standard smoothstep ease-in-out: 3t² - 2t³.
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    3.0 * t * t - 2.0 * t * t * t
}

/// Linear interpolation between two Vec3s.
pub fn lerp_vec3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    Vec3::new(
        a.x + (b.x - a.x) * t,
        a.y + (b.y - a.y) * t,
        a.z + (b.z - a.z) * t,
    )
}

/// Result of interpolating one node between two neighborhoods.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TweenResult {
    pub pos: Vec3,
    pub opacity: f32,
}

/// Outward drift used for nodes leaving / entering the view.
pub fn drift_outward(direction: Vec3, magnitude: f32) -> Vec3 {
    let len = (direction.x * direction.x + direction.y * direction.y)
        .sqrt()
        .max(1.0);
    Vec3::new(
        direction.x / len * magnitude,
        direction.y / len * magnitude,
        0.0,
    )
}

/// Interpolate a single node id between old and new neighborhoods at parameter t.
pub fn lerp_node(node_id: &str, from: &Neighborhood, to: &Neighborhood, t: f32) -> TweenResult {
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

/// Build an interpolated `Neighborhood` at tween parameter `t` between `from` and `to`.
///
/// The resulting neighborhood is structurally identical to `to` (same nodes, edges,
/// clusters), but its `target_positions` map contains lerped Vec3 positions for every
/// node id that appears in either neighborhood. Used by the rAF render loop and by
/// `NavController::retarget` to snapshot the currently visible frame.
pub(crate) fn build_interpolated_neighborhood(
    from: &Neighborhood,
    to: &Neighborhood,
    t: f32,
) -> Neighborhood {
    let mut all_ids: HashSet<String> = HashSet::new();
    all_ids.insert(from.center.id.clone());
    all_ids.insert(to.center.id.clone());
    all_ids.extend(from.one_hop.iter().map(|n| n.id.clone()));
    all_ids.extend(from.two_hop.iter().map(|n| n.id.clone()));
    all_ids.extend(to.one_hop.iter().map(|n| n.id.clone()));
    all_ids.extend(to.two_hop.iter().map(|n| n.id.clone()));

    let mut interp = to.clone();
    for id in all_ids {
        let r = lerp_node(&id, from, to, t);
        interp.target_positions.insert(id, r.pos);
    }
    interp
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
        from.target_positions
            .insert("x".to_string(), Vec3::new(0.0, 0.0, 0.0));
        to.target_positions
            .insert("x".to_string(), Vec3::new(100.0, 0.0, 0.0));
        let r = lerp_node("x", &from, &to, 0.5);
        assert!((r.pos.x - 50.0).abs() < 1e-3);
        assert!((r.opacity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn lerp_node_fadeout_only_in_from() {
        let mut from = empty_nbhd();
        let to = empty_nbhd();
        from.target_positions
            .insert("y".to_string(), Vec3::new(220.0, 0.0, 60.0));
        let r = lerp_node("y", &from, &to, 1.0);
        assert!(
            (r.opacity - 0.0).abs() < 1e-3,
            "should fade out fully at t=1"
        );
    }

    #[test]
    fn lerp_node_fadein_only_in_to() {
        let from = empty_nbhd();
        let mut to = empty_nbhd();
        to.target_positions
            .insert("z".to_string(), Vec3::new(220.0, 0.0, 60.0));
        let r = lerp_node("z", &from, &to, 0.0);
        assert!(
            (r.opacity - 0.0).abs() < 1e-3,
            "should fade in from 0 at t=0"
        );
        let r2 = lerp_node("z", &from, &to, 1.0);
        assert!(
            (r2.opacity - 1.0).abs() < 1e-3,
            "should be fully visible at t=1"
        );
    }

    use super::Spring2D;

    fn run_until_settled(spring: &mut Spring2D, dt_s: f64, max_steps: usize) -> usize {
        for i in 0..max_steps {
            spring.tick(dt_s);
            if spring.settled() {
                return i + 1;
            }
        }
        panic!("spring did not settle in {max_steps} steps");
    }

    #[test]
    fn spring_at_target_with_zero_velocity_is_settled() {
        let s = Spring2D::new(Vec2::zero(), Vec2::zero(), Vec2::zero());
        assert!(s.settled());
        assert_eq!(s.position(), Vec2::zero());
    }

    #[test]
    fn spring_returns_to_target_from_displacement() {
        let mut s = Spring2D::new(Vec2::new(100.0, 0.0), Vec2::zero(), Vec2::zero());
        let steps = run_until_settled(&mut s, 0.016, 200);
        assert!(
            steps < 120,
            "expected settle within ~2s of frames; took {steps}"
        );
        let p = s.position();
        assert!(
            p.length() < 0.5,
            "final position should be near target, got {p:?}"
        );
    }

    #[test]
    fn spring_with_initial_velocity_carries_momentum() {
        let mut s = Spring2D::new(Vec2::zero(), Vec2::new(500.0, 0.0), Vec2::new(100.0, 0.0));
        s.tick(0.016);
        let p1 = s.position();
        assert!(
            p1.x > 0.0,
            "spring should move toward velocity direction; pos={p1:?}"
        );
    }

    #[test]
    fn critically_damped_spring_does_not_overshoot() {
        let mut s = Spring2D::new(Vec2::new(50.0, 0.0), Vec2::zero(), Vec2::zero());
        let mut max_x: f64 = 0.0;
        let mut min_x: f64 = 50.0;
        for _ in 0..200 {
            s.tick(0.016);
            let p = s.position();
            max_x = max_x.max(p.x);
            min_x = min_x.min(p.x);
            if s.settled() {
                break;
            }
        }
        assert!(
            max_x <= 50.001,
            "should not exceed initial displacement; max={max_x}"
        );
        assert!(
            min_x >= -0.5,
            "should not overshoot below target; min={min_x}"
        );
    }

    #[test]
    fn large_dt_substepping_keeps_stable() {
        let mut s = Spring2D::new(Vec2::new(50.0, 0.0), Vec2::zero(), Vec2::zero());
        s.tick(0.100);
        let p = s.position();
        assert!(
            p.x.is_finite() && p.x.abs() < 100.0,
            "100ms tick should sub-step internally and remain stable; pos={p:?}"
        );
    }

    #[test]
    fn spring_tick_rejects_non_finite_and_non_positive_dt() {
        let mut s = Spring2D::new(Vec2::new(10.0, 0.0), Vec2::zero(), Vec2::zero());
        let p_before = s.position();
        s.tick(f64::NAN);
        s.tick(f64::INFINITY);
        s.tick(-1.0);
        s.tick(0.0);
        assert_eq!(
            s.position(),
            p_before,
            "non-finite/non-positive dt must be a no-op"
        );
    }

    #[test]
    fn critical_damping_constant_matches_formula() {
        // SPRING_C must equal 2*sqrt(SPRING_K) for critical damping (no overshoot).
        // If this fails, update SPRING_C after changing SPRING_K.
        assert!((SPRING_C - 2.0 * SPRING_K.sqrt()).abs() < 1e-12);
    }
}
