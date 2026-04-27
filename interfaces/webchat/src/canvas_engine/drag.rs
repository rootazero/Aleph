//! Node-drag interaction: spring-back and promote-on-drag gestures.
//!
//! The drag controller is a state machine driven by pointer events and ticked
//! once per render frame. Pure Rust — no DOM, no WASM. The view layer in
//! `graph_canvas.rs` translates browser events into method calls and reads
//! the rendering snapshot back out for the renderer.

use crate::canvas_engine::tween::Spring2D;
use crate::canvas_engine::types::Vec2;
use std::collections::VecDeque;

// --- public consts (overridable via tests; values from spec §6.4) ---
pub const CLICK_THRESHOLD_PX: f64 = 5.0;
pub const CLICK_TIME_MS: f64 = 200.0;
pub const HOT_ZONE_RADIUS_FACTOR: f64 = 2.5; // multiplied by center node radius
pub const FLICK_THRESHOLD_PX_PER_S: f64 = 600.0;
pub const MIN_FLICK_SPEED_PX_PER_S: f64 = 400.0;
pub const PROMOTE_TWEEN_MAX_MS: f64 = 280.0;
pub const VELOCITY_HISTORY_LEN: usize = 8;
pub const VELOCITY_AVG_LAST_N: usize = 3;

#[derive(Debug, Clone)]
pub enum DragState {
    Idle,
    Pressed {
        node_id: String,
        start_pos: Vec2,
        start_time_ms: f64,
    },
    Dragging {
        node_id: String,
        anchor_pos: Vec2,
        offset: Vec2,
        velocity_history: VecDeque<(Vec2, f64)>,
    },
    SpringBack {
        node_id: String,
        spring: Spring2D,
        start_time_ms: f64,
    },
    Promoting {
        node_id: String,
        target_node_id: String,
        spring: Spring2D,
        start_time_ms: f64,
    },
}

/// What `DragState::release` decides when the user lifts pointer mid-Dragging.
#[derive(Debug, Clone, PartialEq)]
pub enum ReleaseOutcome {
    Click,
    SpringBack,
    Promote { initial_velocity: Vec2 },
}

impl Default for DragState {
    fn default() -> Self {
        Self::new()
    }
}

impl DragState {
    pub fn new() -> Self {
        DragState::Idle
    }

    pub fn is_active(&self) -> bool {
        !matches!(self, DragState::Idle)
    }

    pub fn active_node_id(&self) -> Option<&str> {
        match self {
            DragState::Idle => None,
            DragState::Pressed { node_id, .. }
            | DragState::Dragging { node_id, .. }
            | DragState::SpringBack { node_id, .. }
            | DragState::Promoting { node_id, .. } => Some(node_id),
        }
    }

    pub fn press(&mut self, node_id: String, anchor_pos: Vec2, now_ms: f64) {
        if !matches!(self, DragState::Idle) {
            return;
        }
        *self = DragState::Pressed {
            node_id,
            start_pos: anchor_pos,
            start_time_ms: now_ms,
        };
    }

    pub fn pointer_move(&mut self, screen_pos: Vec2, now_ms: f64) {
        match self {
            DragState::Pressed {
                node_id,
                start_pos,
                start_time_ms,
            } => {
                let displacement = screen_pos - *start_pos;
                if displacement.length() > CLICK_THRESHOLD_PX {
                    let mut history = VecDeque::with_capacity(VELOCITY_HISTORY_LEN);
                    history.push_back((displacement, now_ms));
                    let _ = start_time_ms;
                    *self = DragState::Dragging {
                        node_id: std::mem::take(node_id),
                        anchor_pos: *start_pos,
                        offset: displacement,
                        velocity_history: history,
                    };
                }
            }
            DragState::Dragging {
                anchor_pos,
                offset,
                velocity_history,
                ..
            } => {
                let new_offset = screen_pos - *anchor_pos;
                *offset = new_offset;
                if velocity_history.len() == VELOCITY_HISTORY_LEN {
                    velocity_history.pop_front();
                }
                velocity_history.push_back((new_offset, now_ms));
            }
            _ => {}
        }
    }

    /// Pointer release. Returns the gesture outcome and transitions internal state
    /// to either SpringBack, Promoting, or Idle (in the Click case). Caller applies
    /// the corresponding emit (e.g. SetActive on Click, PromoteNode on Promote-success).
    pub fn release(
        &mut self,
        center_pos: Vec2,
        center_radius_px: f64,
        target_neighbor_id: &str,
        now_ms: f64,
    ) -> ReleaseOutcome {
        let outcome = self.evaluate_release(center_pos, center_radius_px, now_ms);
        let prev = std::mem::replace(self, DragState::Idle);
        match (&outcome, prev) {
            (ReleaseOutcome::Click, _) => {}
            (ReleaseOutcome::SpringBack, DragState::Pressed { node_id, .. })
            | (ReleaseOutcome::SpringBack, DragState::Dragging { node_id, .. }) => {
                *self = DragState::SpringBack {
                    node_id,
                    spring: Spring2D::new(Vec2::zero(), Vec2::zero(), Vec2::zero()),
                    start_time_ms: now_ms,
                };
            }
            (
                ReleaseOutcome::Promote { initial_velocity },
                DragState::Dragging {
                    node_id,
                    anchor_pos,
                    offset,
                    ..
                },
            ) => {
                let displacement = anchor_pos + offset - center_pos;
                *self = DragState::Promoting {
                    node_id,
                    target_node_id: target_neighbor_id.to_string(),
                    spring: Spring2D::new(displacement, *initial_velocity, Vec2::zero()),
                    start_time_ms: now_ms,
                };
            }
            _ => {}
        }
        outcome
    }

    fn evaluate_release(
        &self,
        center_pos: Vec2,
        center_radius_px: f64,
        now_ms: f64,
    ) -> ReleaseOutcome {
        match self {
            DragState::Pressed { start_time_ms, .. } => {
                if now_ms - *start_time_ms < CLICK_TIME_MS {
                    ReleaseOutcome::Click
                } else {
                    ReleaseOutcome::SpringBack
                }
            }
            DragState::Dragging {
                offset,
                anchor_pos,
                velocity_history,
                ..
            } => {
                let absolute_pos = *anchor_pos + *offset;
                let distance_to_center = absolute_pos.distance_to(&center_pos);
                let in_hot_zone = distance_to_center < center_radius_px * HOT_ZONE_RADIUS_FACTOR;
                let avg_velocity = average_recent_velocity(velocity_history, VELOCITY_AVG_LAST_N);
                let to_center_unnormalized = center_pos - absolute_pos;
                let to_center = if to_center_unnormalized.length() > 0.0 {
                    to_center_unnormalized.normalized()
                } else {
                    Vec2::zero()
                };
                let speed = avg_velocity.length();
                let centripetal_speed = avg_velocity.x * to_center.x + avg_velocity.y * to_center.y;
                let centripetal_flick = centripetal_speed > FLICK_THRESHOLD_PX_PER_S
                    && speed > MIN_FLICK_SPEED_PX_PER_S;
                if in_hot_zone || centripetal_flick {
                    ReleaseOutcome::Promote {
                        initial_velocity: avg_velocity,
                    }
                } else {
                    ReleaseOutcome::SpringBack
                }
            }
            _ => ReleaseOutcome::SpringBack,
        }
    }

    /// Cancel any active drag — used for `pointercancel` events. Snaps to Idle, no animation.
    pub fn cancel(&mut self) {
        *self = DragState::Idle;
    }
}

fn average_recent_velocity(history: &VecDeque<(Vec2, f64)>, last_n: usize) -> Vec2 {
    if history.len() < 2 {
        return Vec2::zero();
    }
    let take = last_n.min(history.len());
    let start_idx = history.len() - take;
    let (start_off, start_t) = history[start_idx];
    let (end_off, end_t) = history[history.len() - 1];
    let dt = (end_t - start_t).max(1.0); // ms
    let dx = end_off - start_off;
    Vec2::new(dx.x * 1000.0 / dt, dx.y * 1000.0 / dt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deque_one(off: Vec2, t: f64) -> VecDeque<(Vec2, f64)> {
        let mut d = VecDeque::new();
        d.push_back((off, t));
        d
    }

    #[test]
    fn idle_press_transitions_to_pressed() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(10.0, 20.0), 1000.0);
        assert!(matches!(s, DragState::Pressed { .. }));
        assert_eq!(s.active_node_id(), Some("n1"));
    }

    #[test]
    fn pressed_small_move_stays_pressed() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(0.0, 0.0), 1000.0);
        s.pointer_move(Vec2::new(2.0, 1.0), 1010.0);
        assert!(matches!(s, DragState::Pressed { .. }));
    }

    #[test]
    fn pressed_large_move_transitions_to_dragging() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(0.0, 0.0), 1000.0);
        s.pointer_move(Vec2::new(10.0, 0.0), 1010.0);
        assert!(matches!(s, DragState::Dragging { .. }));
    }

    #[test]
    fn quick_release_emits_click() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(0.0, 0.0), 1000.0);
        let out = s.release(Vec2::new(0.0, 0.0), 30.0, "n1", 1100.0);
        assert_eq!(out, ReleaseOutcome::Click);
        assert!(matches!(s, DragState::Idle));
    }

    #[test]
    fn release_in_hot_zone_promotes() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(100.0, 0.0), 1000.0);
        s.pointer_move(Vec2::new(20.0, 0.0), 1100.0);
        let out = s.release(Vec2::new(0.0, 0.0), 30.0, "n1", 1200.0);
        assert!(matches!(out, ReleaseOutcome::Promote { .. }));
        assert!(matches!(s, DragState::Promoting { .. }));
    }

    #[test]
    fn slow_release_outside_zone_springs_back() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(200.0, 0.0), 1000.0);
        s.pointer_move(Vec2::new(210.0, 0.0), 1100.0);
        s.pointer_move(Vec2::new(220.0, 0.0), 1200.0);
        let out = s.release(Vec2::new(0.0, 0.0), 30.0, "n1", 1500.0);
        assert_eq!(out, ReleaseOutcome::SpringBack);
        assert!(matches!(s, DragState::SpringBack { .. }));
    }

    #[test]
    fn centripetal_flick_promotes_outside_zone() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(300.0, 0.0), 1000.0);
        s.pointer_move(Vec2::new(295.0, 0.0), 1010.0);
        s.pointer_move(Vec2::new(280.0, 0.0), 1020.0);
        s.pointer_move(Vec2::new(260.0, 0.0), 1030.0);
        let out = s.release(Vec2::new(0.0, 0.0), 30.0, "n1", 1040.0);
        assert!(matches!(out, ReleaseOutcome::Promote { .. }));
    }

    #[test]
    fn cancel_resets_from_dragging() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(0.0, 0.0), 1000.0);
        s.pointer_move(Vec2::new(20.0, 0.0), 1010.0);
        s.cancel();
        assert!(matches!(s, DragState::Idle));
    }

    #[test]
    fn centrifugal_fast_drag_springs_back() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(100.0, 0.0), 1000.0);
        s.pointer_move(Vec2::new(120.0, 0.0), 1010.0);
        s.pointer_move(Vec2::new(160.0, 0.0), 1020.0);
        let out = s.release(Vec2::new(0.0, 0.0), 30.0, "n1", 1030.0);
        assert_eq!(
            out,
            ReleaseOutcome::SpringBack,
            "centrifugal velocity should not trigger promote even when fast"
        );
    }

    #[test]
    fn promote_tween_carries_release_velocity() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(300.0, 0.0), 1000.0);
        s.pointer_move(Vec2::new(280.0, 0.0), 1010.0);
        s.pointer_move(Vec2::new(260.0, 0.0), 1020.0);
        let out = s.release(Vec2::new(0.0, 0.0), 30.0, "n1", 1030.0);
        let expected_vel = match out {
            ReleaseOutcome::Promote { initial_velocity } => initial_velocity,
            other => panic!("expected Promote, got {other:?}"),
        };
        match s {
            DragState::Promoting { spring, .. } => {
                let v = spring.velocity();
                assert!(
                    (v.x - expected_vel.x).abs() < 1.0 && (v.y - expected_vel.y).abs() < 1.0,
                    "spring initial velocity {:?} should match release velocity {:?}",
                    v,
                    expected_vel
                );
            }
            other => panic!("expected Promoting state, got {other:?}"),
        }
    }

    #[test]
    fn average_recent_velocity_three_samples() {
        let mut d = deque_one(Vec2::new(0.0, 0.0), 1000.0);
        d.push_back((Vec2::new(10.0, 0.0), 1010.0));
        d.push_back((Vec2::new(30.0, 0.0), 1020.0));
        let v = average_recent_velocity(&d, 3);
        assert!((v.x - 1500.0).abs() < 1.0, "got vx={}", v.x);
    }
}
