use crate::canvas_engine::types::{NavState, Neighborhood};

pub const BREADCRUMB_MAX: usize = 20;

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
            clusters: vec![],
            edges: vec![],
            target_positions: HashMap::new(),
            fetched_at_ms: 0.0,
        }
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
}
