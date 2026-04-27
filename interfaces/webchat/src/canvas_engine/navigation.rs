use crate::canvas_engine::tween::build_interpolated_neighborhood;
use crate::canvas_engine::types::{NavState, Neighborhood};

pub const BREADCRUMB_MAX: usize = 20;
pub const RETARGET_DURATION_MS: u32 = 150;

pub struct NavController {
    pub state: NavState,
    pub breadcrumb: Vec<(String, String)>, // (id, name)
}

impl NavController {
    pub fn new() -> Self {
        Self {
            state: NavState::Idle,
            breadcrumb: Vec::new(),
        }
    }

    pub fn enter(&mut self, target: String, now_ms: f64) {
        self.state = NavState::Loading {
            target,
            since_ms: now_ms,
        };
    }

    pub fn fulfilled(&mut self, node_id: String, name: String, neighborhood: Neighborhood) {
        // Append to breadcrumb if this is a new id
        if self
            .breadcrumb
            .last()
            .map(|(id, _)| id != &node_id)
            .unwrap_or(true)
        {
            self.breadcrumb.push((node_id.clone(), name));
            if self.breadcrumb.len() > BREADCRUMB_MAX {
                // Remove second element (keep root and recent)
                self.breadcrumb.remove(1);
            }
        }
        self.state = NavState::Active {
            node_id,
            neighborhood,
        };
    }

    pub fn fail(&mut self, target: String, reason: String) {
        self.state = NavState::Error { target, reason };
    }

    pub fn start_animation(
        &mut self,
        from_neighborhood: Neighborhood,
        to_neighborhood: Neighborhood,
        from_id: String,
        to_id: String,
        now_ms: f64,
        duration_ms: u32,
    ) {
        self.state = NavState::Animating {
            from_id,
            to_id,
            from_neighborhood,
            to_neighborhood,
            t: 0.0,
            duration_ms,
            started_at_ms: now_ms,
        };
    }

    pub fn tick(&mut self, now_ms: f64) {
        if let NavState::Animating {
            ref mut t,
            duration_ms,
            started_at_ms,
            to_id,
            to_neighborhood,
            ..
        } = &mut self.state
        {
            *t = ((now_ms - *started_at_ms) as f32 / *duration_ms as f32).clamp(0.0, 1.0);
            if *t >= 1.0 {
                let id = std::mem::take(to_id);
                let nbhd = std::mem::replace(
                    to_neighborhood,
                    Neighborhood {
                        center: Default::default(),
                        one_hop: vec![],
                        two_hop: vec![],
                        orphans: vec![],
                        clusters: vec![],
                        edges: vec![],
                        target_positions: Default::default(),
                        fetched_at_ms: 0.0,
                    },
                );
                self.state = NavState::Active {
                    node_id: id,
                    neighborhood: nbhd,
                };
            }
        }
    }

    /// Re-target the current center to a freshly folded neighborhood without going
    /// through Loading. Used by the detail-slider local re-fold path; center id is
    /// unchanged. Animation duration is short (`RETARGET_DURATION_MS = 150`) so a
    /// fast drag chains many `retarget` calls smoothly.
    ///
    /// State machine:
    /// - `Active(from)`              → `Animating { from, to, t = 0 }`, same center id
    /// - `Animating(from, to_old, t)` → snapshot interpolated frame as new `from`,
    ///   new `to`, t reset to 0 — interruptible chain
    /// - `Loading` / `Idle` / `Error` → `Active(to)`, no animation (defensive: in
    ///   normal flow Effect-refold guards on `last_response.is_some()` which only
    ///   becomes true after a successful fetch, so these arms are unreachable)
    /// True while a `retarget` tween is animating between two neighborhoods.
    /// Used by the canvas view to gate user input that would conflict with the
    /// tween (e.g. starting a node-drag while the focus is mid-flight).
    pub fn is_animating(&self) -> bool {
        matches!(self.state, NavState::Animating { .. })
    }

    pub fn retarget(&mut self, to_neighborhood: Neighborhood, now_ms: f64, duration_ms: u32) {
        let prev = std::mem::replace(&mut self.state, NavState::Idle);
        self.state = match prev {
            NavState::Active {
                node_id,
                neighborhood: from,
            } => NavState::Animating {
                from_id: node_id.clone(),
                to_id: node_id,
                from_neighborhood: from,
                to_neighborhood,
                t: 0.0,
                duration_ms,
                started_at_ms: now_ms,
            },
            NavState::Animating {
                from_id,
                to_id,
                from_neighborhood,
                to_neighborhood: prev_to,
                t,
                ..
            } => {
                let snapshot = build_interpolated_neighborhood(&from_neighborhood, &prev_to, t);
                NavState::Animating {
                    from_id,
                    to_id,
                    from_neighborhood: snapshot,
                    to_neighborhood,
                    t: 0.0,
                    duration_ms,
                    started_at_ms: now_ms,
                }
            }
            NavState::Loading { target, .. } => NavState::Active {
                node_id: target,
                neighborhood: to_neighborhood,
            },
            NavState::Idle | NavState::Error { .. } => NavState::Active {
                node_id: to_neighborhood.center.id.clone(),
                neighborhood: to_neighborhood,
            },
        };
    }

    pub fn breadcrumb_pop_to(&mut self, target_id: &str) -> Option<String> {
        if let Some(pos) = self.breadcrumb.iter().position(|(id, _)| id == target_id) {
            self.breadcrumb.truncate(pos + 1);
            Some(target_id.to_string())
        } else {
            None
        }
    }
}

impl Default for NavController {
    fn default() -> Self {
        Self::new()
    }
}

// Provide Default for CanvasNode so the swap-out trick in tick() compiles.
impl Default for crate::canvas_engine::types::CanvasNode {
    fn default() -> Self {
        use crate::canvas_engine::types::*;
        Self {
            id: String::new(),
            name: String::new(),
            category: String::new(),
            color: Color::new(0, 0, 0),
            radius: 0.0,
            position: Vec2::new(0.0, 0.0),
            velocity: Vec2::new(0.0, 0.0),
            pinned: false,
            z: 0.0,
            hop: 0,
            decay_score: 0.0,
            edge_count: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas_engine::types::*;
    use std::collections::HashMap;

    fn nbhd(id: &str) -> Neighborhood {
        Neighborhood {
            center: CanvasNode {
                id: id.to_string(),
                ..Default::default()
            },
            one_hop: vec![],
            two_hop: vec![],
            orphans: vec![],
            clusters: vec![],
            edges: vec![],
            target_positions: HashMap::new(),
            fetched_at_ms: 0.0,
        }
    }

    fn nbhd_with_pos(id: &str, marker_id: &str, pos: Vec3) -> Neighborhood {
        let mut n = nbhd(id);
        n.target_positions.insert(marker_id.to_string(), pos);
        n
    }

    #[test]
    fn start_idle() {
        let nav = NavController::new();
        assert!(matches!(nav.state, NavState::Idle));
        assert!(nav.breadcrumb.is_empty());
    }

    #[test]
    fn enter_then_fulfill_appends_breadcrumb() {
        let mut nav = NavController::new();
        nav.enter("a".to_string(), 0.0);
        assert!(matches!(nav.state, NavState::Loading { .. }));
        nav.fulfilled("a".to_string(), "Alpha".to_string(), nbhd("a"));
        assert!(matches!(nav.state, NavState::Active { .. }));
        assert_eq!(nav.breadcrumb.len(), 1);
    }

    #[test]
    fn breadcrumb_truncates_at_max() {
        let mut nav = NavController::new();
        for i in 0..(BREADCRUMB_MAX + 5) {
            let id = format!("n{i}");
            nav.fulfilled(id.clone(), id.clone(), nbhd(&id));
        }
        assert_eq!(nav.breadcrumb.len(), BREADCRUMB_MAX);
    }

    #[test]
    fn animation_completes_at_t_1() {
        let mut nav = NavController::new();
        nav.fulfilled("a".to_string(), "A".to_string(), nbhd("a"));
        nav.start_animation(
            nbhd("a"),
            nbhd("b"),
            "a".to_string(),
            "b".to_string(),
            0.0,
            400,
        );
        nav.tick(200.0);
        assert!(matches!(nav.state, NavState::Animating { .. }));
        nav.tick(500.0);
        assert!(matches!(nav.state, NavState::Active { .. }));
    }

    #[test]
    fn breadcrumb_pop_to_truncates() {
        let mut nav = NavController::new();
        nav.fulfilled("a".to_string(), "A".to_string(), nbhd("a"));
        nav.fulfilled("b".to_string(), "B".to_string(), nbhd("b"));
        nav.fulfilled("c".to_string(), "C".to_string(), nbhd("c"));
        nav.breadcrumb_pop_to("a");
        assert_eq!(nav.breadcrumb.len(), 1);
        assert_eq!(nav.breadcrumb[0].0, "a");
    }

    #[test]
    fn retarget_from_active_enters_animating_with_same_center() {
        let mut nav = NavController::new();
        nav.fulfilled("a".to_string(), "Alpha".to_string(), nbhd("a"));
        assert!(matches!(nav.state, NavState::Active { .. }));

        nav.retarget(nbhd("a"), 100.0, 150);

        match &nav.state {
            NavState::Animating {
                from_id,
                to_id,
                t,
                duration_ms,
                started_at_ms,
                ..
            } => {
                assert_eq!(from_id, "a");
                assert_eq!(to_id, "a");
                assert!((*t - 0.0).abs() < 1e-6);
                assert_eq!(*duration_ms, 150);
                assert!((*started_at_ms - 100.0).abs() < 1e-6);
            }
            other => panic!("expected Animating, got {:?}", other),
        }
    }

    #[test]
    fn retarget_from_animating_snapshots_current_interpolated_frame() {
        let mut nav = NavController::new();
        // The marker id must match a node id that build_interpolated_neighborhood
        // recognises (center.id or a one_hop/two_hop id), otherwise the position is
        // copied verbatim from `to`. We use the center id "a" itself.
        let from = nbhd_with_pos("a", "a", Vec3::new(0.0, 0.0, 0.0));
        let to_old = nbhd_with_pos("a", "a", Vec3::new(100.0, 0.0, 0.0));
        nav.fulfilled("a".to_string(), "Alpha".to_string(), from.clone());
        nav.start_animation(from, to_old, "a".to_string(), "a".to_string(), 0.0, 1000);
        nav.tick(500.0); // advance to t = 0.5

        let new_to = nbhd_with_pos("a", "a", Vec3::new(200.0, 0.0, 0.0));
        nav.retarget(new_to, 500.0, 150);

        match &nav.state {
            NavState::Animating {
                from_neighborhood,
                to_neighborhood,
                t,
                ..
            } => {
                // The new `from` is the snapshot of the previous animation at t=0.5.
                // ease_in_out(0.5) == 0.5 (smoothstep midpoint) so the interpolated x is 50.0.
                let p = from_neighborhood
                    .target_positions
                    .get("a")
                    .copied()
                    .expect("interpolated frame must contain a");
                assert!(
                    (p.x - 50.0).abs() < 1.0,
                    "expected snapshot x ~= 50.0, got {}",
                    p.x
                );
                // The new `to` is the third neighborhood we passed in.
                let p_to = to_neighborhood
                    .target_positions
                    .get("a")
                    .copied()
                    .expect("new to must contain a");
                assert!((p_to.x - 200.0).abs() < 1e-3);
                assert!((*t - 0.0).abs() < 1e-6);
            }
            other => panic!("expected Animating, got {:?}", other),
        }
    }

    #[test]
    fn retarget_from_loading_falls_back_to_active_no_animation() {
        let mut nav = NavController::new();
        nav.enter("a".to_string(), 0.0);
        assert!(matches!(nav.state, NavState::Loading { .. }));

        nav.retarget(nbhd("a"), 100.0, 150);

        match &nav.state {
            NavState::Active { node_id, .. } => assert_eq!(node_id, "a"),
            other => panic!("expected Active, got {:?}", other),
        }
    }

    #[test]
    fn is_animating_reflects_state() {
        let mut nav = NavController::new();
        // Freshly fulfilled Active state: not animating.
        nav.fulfilled("a".to_string(), "Alpha".to_string(), nbhd("a"));
        assert!(
            !nav.is_animating(),
            "freshly fulfilled Active state should not be animating"
        );

        // Trigger a retarget — Active → Animating.
        nav.retarget(nbhd("b"), 1000.0, 300);
        assert!(
            nav.is_animating(),
            "after retarget, state should be Animating"
        );

        // Tick past the animation duration — should fall out of Animating.
        nav.tick(1000.0 + 300.0 + 1.0);
        assert!(
            !nav.is_animating(),
            "after animation duration, should no longer be animating"
        );
    }
}
