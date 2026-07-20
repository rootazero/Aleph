# Canvas Elastic Node Drag — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add direct-manipulation drag for 1-hop neighbor nodes with spring-back and promote-on-drag gestures.

**Architecture:** Build pure-logic primitives first (`Spring2D`, `DragState` machine, `evaluate_release` gesture detector) — all unit-testable in plain Rust. Then wire into the existing rAF render loop in `graph_canvas.rs` and the existing pointer-event handlers. Renderer changes are additive (new edge-stretch + glow paths) and never modify `Neighborhood` data.

**Tech Stack:** Rust + Leptos 0.8 (WASM); existing `interfaces/webchat/src/canvas_engine/` modules; `wasm-bindgen` for browser pointer events.

**Spec:** `docs/superpowers/specs/2026-04-27-canvas-elastic-node-drag-design.md`

---

## File Structure

| File | Role | Change |
|---|---|---|
| `interfaces/webchat/src/canvas_engine/tween.rs` | Add `Spring2D` (critically-damped) primitive | Append |
| `interfaces/webchat/src/canvas_engine/drag.rs` | New module — `DragState` machine, `evaluate_release`, default consts, `tick()` | Create |
| `interfaces/webchat/src/canvas_engine/mod.rs` | Register `pub mod drag;` | Modify (1 line) |
| `interfaces/webchat/src/canvas_engine/interaction.rs` | Add `CanvasIntent::PromoteNode(String)` variant; populate from `DragState` outputs | Modify |
| `interfaces/webchat/src/canvas_engine/renderer.rs` | Read drag overlay; draw stretched edge + glow on dragged node + scale-up on center when threshold near | Modify |
| `interfaces/webchat/src/views/canvas/graph_canvas.rs` | Wire `pointerdown/move/up/cancel` on the canvas; integrate `drag.tick(dt)` into rAF loop | Modify |

**Spec correction:** Spec §5.1 lists `views/canvas/mod.rs` for pointer wiring; the actual canvas element + existing `on:mousedown/move/up` handlers live in `views/canvas/graph_canvas.rs`. This plan uses the correct file.

**Constant adaptation:** `Vec2` in `canvas_engine/types.rs` is `f64`-based; `Spring2D` and all drag math use `f64` to match. Spec §6.4's `f32` types should be read as `f64`.

**Intent variant:** Spec §6.1 says promote success emits `CanvasIntent::Navigate(target_node_id)`. The existing `CanvasIntent` enum has no `Navigate` variant; the closest is `SetActive(String)` which is the click-to-navigate intent. To keep promote semantically distinct (so callers can tell drag-promote from click-promote and apply different transition behavior), we add `PromoteNode(String)` as a new variant. The dispatcher in `views/canvas/mod.rs` will handle it identically to `SetActive` for now (route to `active_request.set(...)`).

---

## Task 1: `Spring2D` primitive in `tween.rs`

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/tween.rs` (append; existing 171 lines)

`Spring2D` is a critically-damped 2D spring used for both spring-back (target=initial slot) and promote-tween (target=center). Critical damping means no overshoot — feel is "snappy return" not "bouncy".

- [ ] **Step 1: Write the 5 failing tests**

Append inside the existing `#[cfg(test)] mod tests` block (or create one if none exists at end of file):

```rust
    use super::Spring2D;
    use crate::canvas_engine::types::Vec2;

    fn run_until_settled(spring: &mut Spring2D, dt_s: f64, max_steps: usize) -> usize {
        for i in 0..max_steps {
            spring.tick(dt_s);
            if spring.settled() { return i + 1; }
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
        assert!(steps < 120, "expected settle within ~2s of frames; took {steps}");
        let p = s.position();
        assert!(p.length() < 0.5, "final position should be near target, got {p:?}");
    }

    #[test]
    fn spring_with_initial_velocity_carries_momentum() {
        let mut s = Spring2D::new(Vec2::zero(), Vec2::new(500.0, 0.0), Vec2::new(100.0, 0.0));
        s.tick(0.016);
        let p1 = s.position();
        assert!(p1.x > 0.0, "spring should move toward velocity direction; pos={p1:?}");
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
            if s.settled() { break; }
        }
        assert!(max_x <= 50.001, "should not exceed initial displacement; max={max_x}");
        assert!(min_x >= -0.5, "should not overshoot below target; min={min_x}");
    }

    #[test]
    fn large_dt_substepping_keeps_stable() {
        let mut s = Spring2D::new(Vec2::new(50.0, 0.0), Vec2::zero(), Vec2::zero());
        s.tick(0.100);
        let p = s.position();
        assert!(p.x.is_finite() && p.x.abs() < 100.0,
            "100ms tick should sub-step internally and remain stable; pos={p:?}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p webchat --lib canvas_engine::tween::tests::spring_ -- --nocapture
```

Expected: compile errors — `cannot find type Spring2D in this scope` (5 errors).

- [ ] **Step 3: Implement `Spring2D` (append to `tween.rs`)**

```rust
use crate::canvas_engine::types::Vec2;

const SPRING_K: f64 = 220.0;                                  // stiffness
const SPRING_C: f64 = 2.0 * 14.832396974191326;               // critical damping = 2*sqrt(K)
const SPRING_SETTLE_POS_EPS: f64 = 0.5;                       // px
const SPRING_SETTLE_VEL_EPS: f64 = 5.0;                       // px/s
const SPRING_MAX_STEP_DT: f64 = 0.016;                        // sub-step cap (one 60fps frame)

/// Critically-damped 2D spring. Used for both drag spring-back (target=initial slot)
/// and promote tween-into-center (target=screen center).
///
/// Numerically stable: `tick(dt)` sub-steps if dt > 16ms.
#[derive(Debug, Clone)]
pub struct Spring2D {
    pos: Vec2,
    vel: Vec2,
    target: Vec2,
}

impl Spring2D {
    /// Create a spring at `initial_pos` moving with `initial_vel`, pulled toward `target`.
    pub fn new(initial_pos: Vec2, initial_vel: Vec2, target: Vec2) -> Self {
        Self { pos: initial_pos, vel: initial_vel, target }
    }

    /// Advance the spring by `dt` seconds. Returns the new position.
    pub fn tick(&mut self, dt: f64) -> Vec2 {
        let mut remaining = dt;
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

    pub fn position(&self) -> Vec2 { self.pos }
    pub fn velocity(&self) -> Vec2 { self.vel }
    pub fn target(&self) -> Vec2 { self.target }
}
```

Note: `Vec2` already implements `Add`, `Sub`, `Mul<f64>`, `AddAssign` (per `types.rs`). All ops above compile against existing trait impls. `Mul<f64>` for `Vec2 * f64` is defined, but `f64 * Vec2` is not — keep multiplication on the left side as `vec * scalar`.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p webchat --lib canvas_engine::tween::tests::spring_ -- --nocapture
```

Expected: 5 tests pass.

```bash
cargo test -p webchat --lib canvas_engine::tween
```

Expected: all existing tween tests still green plus the 5 new ones.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/tween.rs
git commit -m "canvas(tween): add critically-damped Spring2D primitive"
```

---

## Task 2: `DragState` machine in new `drag.rs`

**Files:**
- Create: `interfaces/webchat/src/canvas_engine/drag.rs`
- Modify: `interfaces/webchat/src/canvas_engine/mod.rs` (add `pub mod drag;`)

This task implements the state enum + transitions but no rendering / no event wiring yet. All transitions are unit-testable.

- [ ] **Step 1: Add `pub mod drag;` to `canvas_engine/mod.rs`**

In `interfaces/webchat/src/canvas_engine/mod.rs`, between existing `pub mod cluster;` and `pub mod interaction;` (alphabetical):

```rust
pub mod drag;
```

- [ ] **Step 2: Write the failing tests in new file `drag.rs`**

Create `interfaces/webchat/src/canvas_engine/drag.rs` with:

```rust
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
pub const HOT_ZONE_RADIUS_FACTOR: f64 = 2.5;     // multiplied by center node radius
pub const FLICK_THRESHOLD_PX_PER_S: f64 = 600.0;
pub const MIN_FLICK_SPEED_PX_PER_S: f64 = 400.0;
pub const PROMOTE_TWEEN_MAX_MS: f64 = 280.0;
pub const VELOCITY_HISTORY_LEN: usize = 8;
pub const VELOCITY_AVG_LAST_N: usize = 3;

#[derive(Debug, Clone)]
pub enum DragState {
    Idle,
    Pressed { node_id: String, start_pos: Vec2, start_time_ms: f64 },
    Dragging {
        node_id: String,
        anchor_pos: Vec2,                          // node's screen position at press
        offset: Vec2,                              // current displacement from anchor
        velocity_history: VecDeque<(Vec2, f64)>,   // (offset, time_ms) ring buffer
    },
    SpringBack { node_id: String, spring: Spring2D, start_time_ms: f64 },
    Promoting { node_id: String, target_node_id: String, spring: Spring2D, start_time_ms: f64 },
}

/// What `DragState::release` decides when the user lifts pointer mid-Dragging.
#[derive(Debug, Clone, PartialEq)]
pub enum ReleaseOutcome {
    Click,                                          // press was short + small movement
    SpringBack,                                     // lifted outside hot zone, no centripetal flick
    Promote { initial_velocity: Vec2 },             // hot zone hit OR centripetal flick
}

impl DragState {
    pub fn new() -> Self { DragState::Idle }

    /// Returns true if the controller is in any non-Idle state.
    pub fn is_active(&self) -> bool { !matches!(self, DragState::Idle) }

    /// Returns the node id currently being interacted with, if any.
    pub fn active_node_id(&self) -> Option<&str> {
        match self {
            DragState::Idle => None,
            DragState::Pressed { node_id, .. }
            | DragState::Dragging { node_id, .. }
            | DragState::SpringBack { node_id, .. }
            | DragState::Promoting { node_id, .. } => Some(node_id),
        }
    }

    /// Begin a new press on the given neighbor node. Idempotent: ignored unless Idle.
    pub fn press(&mut self, node_id: String, anchor_pos: Vec2, now_ms: f64) {
        if !matches!(self, DragState::Idle) { return; }
        *self = DragState::Pressed {
            node_id,
            start_pos: anchor_pos,
            start_time_ms: now_ms,
        };
    }

    /// Update with a pointer-move event. Promotes Pressed → Dragging once movement
    /// exceeds the click threshold. In Dragging, updates offset and velocity history.
    pub fn pointer_move(&mut self, screen_pos: Vec2, now_ms: f64) {
        match self {
            DragState::Pressed { node_id, start_pos, start_time_ms } => {
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
            DragState::Dragging { anchor_pos, offset, velocity_history, .. } => {
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
        let outcome = match self {
            DragState::Pressed { start_time_ms, .. } => {
                if now_ms - *start_time_ms < CLICK_TIME_MS {
                    ReleaseOutcome::Click
                } else {
                    ReleaseOutcome::SpringBack
                }
            }
            DragState::Dragging { offset, anchor_pos, velocity_history, .. } => {
                let absolute_pos = *anchor_pos + *offset;
                let distance_to_center = absolute_pos.distance_to(&center_pos);
                let in_hot_zone = distance_to_center < center_radius_px * HOT_ZONE_RADIUS_FACTOR;
                let avg_velocity = average_recent_velocity(velocity_history, VELOCITY_AVG_LAST_N);
                let to_center = (center_pos - absolute_pos).normalized();
                let speed = avg_velocity.length();
                let centripetal_speed = avg_velocity.x * to_center.x + avg_velocity.y * to_center.y;
                let centripetal_flick = centripetal_speed > FLICK_THRESHOLD_PX_PER_S
                                     && speed > MIN_FLICK_SPEED_PX_PER_S;
                if in_hot_zone || centripetal_flick {
                    ReleaseOutcome::Promote { initial_velocity: avg_velocity }
                } else {
                    ReleaseOutcome::SpringBack
                }
            }
            _ => return ReleaseOutcome::SpringBack, // defensive — shouldn't happen
        };

        // Apply transition based on outcome
        match (&outcome, std::mem::replace(self, DragState::Idle)) {
            (ReleaseOutcome::Click, _) => {
                // Idle already set above
            }
            (ReleaseOutcome::SpringBack, DragState::Pressed { node_id, .. })
            | (ReleaseOutcome::SpringBack, DragState::Dragging { node_id, .. }) => {
                let (offset, vel) = match self_pseudo_unreachable() { _ => (Vec2::zero(), Vec2::zero()) };
                let _ = (offset, vel);
                // Recompute spring from any leftover offset/velocity
                // (For Pressed there's no offset; spring trivially settles)
                *self = DragState::SpringBack {
                    node_id,
                    spring: Spring2D::new(Vec2::zero(), Vec2::zero(), Vec2::zero()),
                    start_time_ms: now_ms,
                };
            }
            (ReleaseOutcome::Promote { initial_velocity }, DragState::Dragging { node_id, anchor_pos, offset, .. }) => {
                let displacement = anchor_pos + offset - center_pos;
                *self = DragState::Promoting {
                    node_id,
                    target_node_id: target_neighbor_id.to_string(),
                    spring: Spring2D::new(displacement, *initial_velocity, Vec2::zero()),
                    start_time_ms: now_ms,
                };
            }
            _ => {} // unreachable defensive paths
        }

        outcome
    }

    /// Cancel any active drag — used for `pointercancel` events. Snaps to Idle, no animation.
    pub fn cancel(&mut self) { *self = DragState::Idle; }
}

#[inline]
fn self_pseudo_unreachable() -> u8 { 0 }

fn average_recent_velocity(history: &VecDeque<(Vec2, f64)>, last_n: usize) -> Vec2 {
    if history.len() < 2 { return Vec2::zero(); }
    let take = last_n.min(history.len());
    let start_idx = history.len() - take;
    let (start_off, start_t) = history[start_idx];
    let (end_off, end_t) = history[history.len() - 1];
    let dt = (end_t - start_t).max(1.0); // ms
    let dx = end_off - start_off;
    // dt is in ms; convert to per-second
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
        s.pointer_move(Vec2::new(2.0, 1.0), 1010.0); // total < 5px
        assert!(matches!(s, DragState::Pressed { .. }));
    }

    #[test]
    fn pressed_large_move_transitions_to_dragging() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(0.0, 0.0), 1000.0);
        s.pointer_move(Vec2::new(10.0, 0.0), 1010.0); // 10 > 5 threshold
        assert!(matches!(s, DragState::Dragging { .. }));
    }

    #[test]
    fn quick_release_emits_click() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(0.0, 0.0), 1000.0);
        let out = s.release(Vec2::new(0.0, 0.0), 30.0, "n1", 1100.0); // < 200ms
        assert_eq!(out, ReleaseOutcome::Click);
        assert!(matches!(s, DragState::Idle));
    }

    #[test]
    fn release_in_hot_zone_promotes() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(100.0, 0.0), 1000.0);
        s.pointer_move(Vec2::new(20.0, 0.0), 1100.0);  // moved into hot zone of (0,0)
        let out = s.release(Vec2::new(0.0, 0.0), 30.0, "n1", 1200.0);
        assert!(matches!(out, ReleaseOutcome::Promote { .. }));
        assert!(matches!(s, DragState::Promoting { .. }));
    }

    #[test]
    fn slow_release_outside_zone_springs_back() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(200.0, 0.0), 1000.0);
        s.pointer_move(Vec2::new(210.0, 0.0), 1100.0);  // tiny move, far from center (0,0)
        s.pointer_move(Vec2::new(220.0, 0.0), 1200.0);  // slow drift
        let out = s.release(Vec2::new(0.0, 0.0), 30.0, "n1", 1500.0);
        assert_eq!(out, ReleaseOutcome::SpringBack);
        assert!(matches!(s, DragState::SpringBack { .. }));
    }

    #[test]
    fn centripetal_flick_promotes_outside_zone() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(300.0, 0.0), 1000.0);
        s.pointer_move(Vec2::new(295.0, 0.0), 1010.0); // small movement to enter Dragging
        s.pointer_move(Vec2::new(280.0, 0.0), 1020.0); // velocity ~ -1500 px/s toward (0,0)
        s.pointer_move(Vec2::new(260.0, 0.0), 1030.0); // continued centripetal flick
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
        // Velocity is fast but pointed AWAY from center → should NOT promote.
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(100.0, 0.0), 1000.0);
        s.pointer_move(Vec2::new(120.0, 0.0), 1010.0); // velocity ~ +2000 px/s away from (0,0)
        s.pointer_move(Vec2::new(160.0, 0.0), 1020.0); // continued centrifugal flick
        let out = s.release(Vec2::new(0.0, 0.0), 30.0, "n1", 1030.0);
        assert_eq!(out, ReleaseOutcome::SpringBack,
            "centrifugal velocity should not trigger promote even when fast");
    }

    #[test]
    fn promote_tween_carries_release_velocity() {
        // Release velocity should be passed through as the Promoting spring's initial velocity.
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(300.0, 0.0), 1000.0);
        s.pointer_move(Vec2::new(280.0, 0.0), 1010.0); // -2000 px/s toward center
        s.pointer_move(Vec2::new(260.0, 0.0), 1020.0); // continued
        let out = s.release(Vec2::new(0.0, 0.0), 30.0, "n1", 1030.0);
        let expected_vel = match out {
            ReleaseOutcome::Promote { initial_velocity } => initial_velocity,
            other => panic!("expected Promote, got {other:?}"),
        };
        match s {
            DragState::Promoting { spring, .. } => {
                let v = spring.velocity();
                assert!((v.x - expected_vel.x).abs() < 1.0 && (v.y - expected_vel.y).abs() < 1.0,
                    "spring initial velocity {:?} should match release velocity {:?}", v, expected_vel);
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
        // 30px over 20ms = 1500 px/s
        assert!((v.x - 1500.0).abs() < 1.0, "got vx={}", v.x);
    }
}
```

**Refactoring note:** the `release` body above contains a `self_pseudo_unreachable` defensive guard that's awkward. Replace it with a cleaner two-phase implementation:

```rust
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
            (ReleaseOutcome::Promote { initial_velocity }, DragState::Dragging { node_id, anchor_pos, offset, .. }) => {
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

    fn evaluate_release(&self, center_pos: Vec2, center_radius_px: f64, now_ms: f64) -> ReleaseOutcome {
        match self {
            DragState::Pressed { start_time_ms, .. } => {
                if now_ms - *start_time_ms < CLICK_TIME_MS {
                    ReleaseOutcome::Click
                } else {
                    ReleaseOutcome::SpringBack
                }
            }
            DragState::Dragging { offset, anchor_pos, velocity_history, .. } => {
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
                    ReleaseOutcome::Promote { initial_velocity: avg_velocity }
                } else {
                    ReleaseOutcome::SpringBack
                }
            }
            _ => ReleaseOutcome::SpringBack,
        }
    }
```

**Use only the cleaner version above** — the first `release` shown is illustrative scaffolding; do not include `self_pseudo_unreachable` in the final file.

- [ ] **Step 3: Run tests to verify they fail then pass**

```bash
cargo test -p webchat --lib canvas_engine::drag::tests -- --nocapture
```

Expected: 9 tests pass on first compile (Test-First was satisfied by writing tests + impl in same step here since impl is small and tests directly drive the API).

If any test fails, fix the implementation, not the test.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/drag.rs interfaces/webchat/src/canvas_engine/mod.rs
git commit -m "canvas(drag): add DragState machine with press/move/release/cancel"
```

---

## Task 3: `tick()` and Idle/Settle transitions

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/drag.rs`

`tick(dt_s)` advances the active spring/tween and may transition `SpringBack` or `Promoting` back to `Idle` when settled. Returns whether a `PromoteNode` intent should be emitted (only when a `Promoting` state has just completed).

- [ ] **Step 1: Write the failing tests**

Append to the existing `mod tests` in `drag.rs`:

```rust
    #[test]
    fn springback_settles_to_idle_within_two_seconds() {
        let mut s = DragState::new();
        // Manually construct a SpringBack with significant displacement
        s = DragState::SpringBack {
            node_id: "n1".into(),
            spring: Spring2D::new(Vec2::new(50.0, 0.0), Vec2::zero(), Vec2::zero()),
            start_time_ms: 0.0,
        };
        let mut steps = 0;
        for _ in 0..200 {
            s.tick(0.016);
            steps += 1;
            if matches!(s, DragState::Idle) { break; }
        }
        assert!(matches!(s, DragState::Idle), "should reach Idle, took {steps} steps");
        assert!(steps < 130, "should settle within ~2s of frames; took {steps}");
    }

    #[test]
    fn promoting_emits_intent_on_completion() {
        let mut s = DragState::Promoting {
            node_id: "n1".into(),
            target_node_id: "target_id_42".into(),
            spring: Spring2D::new(Vec2::new(50.0, 0.0), Vec2::zero(), Vec2::zero()),
            start_time_ms: 0.0,
        };
        let mut emitted = None;
        for _ in 0..200 {
            if let Some(id) = s.tick(0.016) {
                emitted = Some(id);
                break;
            }
        }
        assert_eq!(emitted.as_deref(), Some("target_id_42"));
        assert!(matches!(s, DragState::Idle));
    }

    #[test]
    fn promoting_caps_at_max_tween_duration() {
        let mut s = DragState::Promoting {
            node_id: "n1".into(),
            target_node_id: "t".into(),
            spring: Spring2D::new(Vec2::new(50.0, 0.0), Vec2::zero(), Vec2::zero()),
            start_time_ms: 0.0,
        };
        // Tick once with a duration > PROMOTE_TWEEN_MAX_MS to force the cap path
        let result = s.tick((PROMOTE_TWEEN_MAX_MS + 10.0) / 1000.0);
        assert_eq!(result.as_deref(), Some("t"), "should emit intent when tween-max exceeded");
        assert!(matches!(s, DragState::Idle));
    }

    #[test]
    fn idle_tick_is_noop() {
        let mut s = DragState::new();
        let r = s.tick(0.016);
        assert!(r.is_none());
        assert!(matches!(s, DragState::Idle));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p webchat --lib canvas_engine::drag::tests -- --nocapture
```

Expected: compile errors — `expected enum/struct, found method tick` (4 errors).

- [ ] **Step 3: Implement `tick()` (append to `impl DragState`)**

Add inside `impl DragState`:

```rust
    /// Advance any active animation by `dt_s` seconds. Returns `Some(target_node_id)`
    /// exactly once when a Promoting state completes — the caller should then emit
    /// `CanvasIntent::PromoteNode(id)` to drive navigation.
    pub fn tick(&mut self, dt_s: f64) -> Option<String> {
        match self {
            DragState::SpringBack { spring, .. } => {
                spring.tick(dt_s);
                if spring.settled() {
                    *self = DragState::Idle;
                }
                None
            }
            DragState::Promoting { spring, target_node_id, start_time_ms, .. } => {
                spring.tick(dt_s);
                let elapsed_ms = (*start_time_ms + dt_s * 1000.0) - *start_time_ms;
                let _ = elapsed_ms;
                // Track real elapsed via accumulating into start_time_ms field
                *start_time_ms += dt_s * 1000.0;
                let total_elapsed = *start_time_ms;
                if spring.settled() || total_elapsed > PROMOTE_TWEEN_MAX_MS {
                    let id = std::mem::take(target_node_id);
                    *self = DragState::Idle;
                    return Some(id);
                }
                None
            }
            _ => None,
        }
    }
```

**Note:** The `start_time_ms` field of `Promoting` is repurposed in `tick()` as an "elapsed accumulator" rather than the original timestamp. This is fine because once the state enters `Promoting`, the original press timestamp is no longer needed. Reset to 0 on entry from `release` for correctness:

In the `release` `Promoting` branch (Task 2), change `start_time_ms: now_ms` to `start_time_ms: 0.0` so the accumulator starts from zero. Make this edit before running tests.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p webchat --lib canvas_engine::drag::tests -- --nocapture
```

Expected: all 13 tests in this module pass (9 from Task 2 + 4 new).

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/drag.rs
git commit -m "canvas(drag): add tick() and Idle/settle transitions for SpringBack and Promoting"
```

---

## Task 4: Add `CanvasIntent::PromoteNode` variant

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/interaction.rs`

Add the new intent variant so promote-on-drag has a distinct identity. The `views/canvas/mod.rs` dispatcher in Task 7 will handle it identically to `SetActive` for now.

- [ ] **Step 1: Write the failing test**

Append inside `mod intent_tests` in `interaction.rs`:

```rust
    #[test]
    fn promote_node_intent_carries_target_id() {
        let intent = CanvasIntent::PromoteNode("target_42".to_string());
        match intent {
            CanvasIntent::PromoteNode(id) => assert_eq!(id, "target_42"),
            _ => panic!("expected PromoteNode"),
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p webchat --lib canvas_engine::interaction::intent_tests::promote_ -- --nocapture
```

Expected: compile error — `no variant or associated item named PromoteNode found for enum CanvasIntent`.

- [ ] **Step 3: Add the variant**

In `interfaces/webchat/src/canvas_engine/interaction.rs`, edit the `pub enum CanvasIntent { ... }` definition (currently has SetActive, PrefetchNeighbor, ExpandCluster, BreadcrumbBack, BreadcrumbForward, OpenSearch, ToggleGlobal, CloseDetail, HoverFocus, None — append `PromoteNode(String)` after `SetActive(String)`):

```rust
pub enum CanvasIntent {
    None,
    SetActive(String),
    PromoteNode(String),                   // emitted on drag-promote-on-drag completion
    PrefetchNeighbor(String),
    ExpandCluster(String),
    BreadcrumbBack,
    BreadcrumbForward,
    OpenSearch,
    ToggleGlobal,
    CloseDetail,
    HoverFocus(Direction),
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p webchat --lib canvas_engine::interaction
```

Expected: all interaction tests pass (4 existing + 1 new).

If clippy warns about exhaustiveness in any `match CanvasIntent { ... }` site, leave the warning for Task 7 to address (where the view-layer match is wired up).

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/interaction.rs
git commit -m "canvas(interaction): add CanvasIntent::PromoteNode variant"
```

---

## Task 5: Drag overlay snapshot for renderer

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/drag.rs`

Renderer needs a stable read-only snapshot of "what to draw on top of base canvas" each frame. This task adds `DragState::overlay_snapshot()` returning the (node_id, current_screen_pos, glow_alpha) triple. Pure logic + tests.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `drag.rs`:

```rust
    #[test]
    fn idle_overlay_snapshot_is_none() {
        let s = DragState::new();
        assert!(s.overlay_snapshot(Vec2::zero(), 30.0).is_none());
    }

    #[test]
    fn dragging_overlay_returns_node_pos_and_glow() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(100.0, 0.0), 1000.0);
        s.pointer_move(Vec2::new(60.0, 0.0), 1010.0);  // moved 40px toward (0,0)
        let snap = s.overlay_snapshot(Vec2::new(0.0, 0.0), 30.0).unwrap();
        assert_eq!(snap.node_id, "n1");
        assert_eq!(snap.position.x, 60.0);
        // 60px from center, GLOW_RADIUS = 30 * 4 = 120, so glow_alpha > 0
        assert!(snap.glow_alpha > 0.0 && snap.glow_alpha <= 1.0,
            "glow should be partial; got {}", snap.glow_alpha);
    }

    #[test]
    fn dragging_far_outside_glow_radius_has_zero_glow() {
        let mut s = DragState::new();
        s.press("n1".into(), Vec2::new(500.0, 0.0), 1000.0);
        s.pointer_move(Vec2::new(490.0, 0.0), 1010.0); // ~490 from center, far outside glow
        let snap = s.overlay_snapshot(Vec2::new(0.0, 0.0), 30.0).unwrap();
        assert_eq!(snap.glow_alpha, 0.0);
    }

    #[test]
    fn springback_overlay_returns_current_spring_position() {
        let mut s = DragState::SpringBack {
            node_id: "n1".into(),
            spring: Spring2D::new(Vec2::new(50.0, 0.0), Vec2::zero(), Vec2::zero()),
            start_time_ms: 0.0,
        };
        // anchor_pos for SpringBack is implicitly the node's layout slot, which the
        // renderer knows; overlay_snapshot returns spring offset relative to that slot.
        let snap = s.overlay_snapshot(Vec2::zero(), 30.0).unwrap();
        assert_eq!(snap.node_id, "n1");
        assert_eq!(snap.position.x, 50.0); // spring still at initial displacement
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p webchat --lib canvas_engine::drag::tests::idle_overlay_snapshot_is_none -- --nocapture
```

Expected: compile error — `no method named overlay_snapshot found`.

- [ ] **Step 3: Implement `overlay_snapshot` and `DragOverlay` struct**

Append to `drag.rs` (struct at top of file alongside `DragState`, method inside `impl DragState`):

```rust
/// Read-only snapshot for the renderer.
///
/// `position` is in the same coordinate frame as the input pointer events
/// (canvas-local screen pixels). For SpringBack/Promoting it is the offset
/// **from the node's anchor slot** (renderer adds it to the layout position).
/// For Dragging it is the current pointer-relative position.
#[derive(Debug, Clone)]
pub struct DragOverlay {
    pub node_id: String,
    pub position: Vec2,
    pub glow_alpha: f64,           // 0.0..=1.0; non-zero when promote-imminent
}

impl DragState {
    pub fn overlay_snapshot(&self, center_pos: Vec2, center_radius_px: f64) -> Option<DragOverlay> {
        let glow_radius = center_radius_px * 4.0;
        match self {
            DragState::Idle | DragState::Pressed { .. } => None,
            DragState::Dragging { node_id, anchor_pos, offset, .. } => {
                let absolute_pos = *anchor_pos + *offset;
                let dist_to_center = absolute_pos.distance_to(&center_pos);
                let glow_alpha = if dist_to_center < glow_radius {
                    1.0 - (dist_to_center / glow_radius).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                Some(DragOverlay {
                    node_id: node_id.clone(),
                    position: absolute_pos,
                    glow_alpha,
                })
            }
            DragState::SpringBack { node_id, spring, .. } => Some(DragOverlay {
                node_id: node_id.clone(),
                position: spring.position(),
                glow_alpha: 0.0,
            }),
            DragState::Promoting { node_id, spring, .. } => {
                // Promote tween position is offset from center; renderer must add center
                let abs_pos = center_pos + spring.position();
                let dist_to_center = spring.position().length();
                let glow_alpha = 1.0 - (dist_to_center / glow_radius).clamp(0.0, 1.0);
                Some(DragOverlay {
                    node_id: node_id.clone(),
                    position: abs_pos,
                    glow_alpha,
                })
            }
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p webchat --lib canvas_engine::drag::tests -- --nocapture
```

Expected: all 17 tests in `drag::tests` pass (13 from Tasks 2-3 + 4 new).

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/drag.rs
git commit -m "canvas(drag): add DragOverlay snapshot for renderer"
```

---

## Task 6: Renderer overlay — drag offset + glow

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/renderer.rs`

Extend `draw_neighborhood` (or its sub-functions) to accept an optional `Option<&DragOverlay>` parameter. When `Some`, the renderer:
1. Draws the dragged node at `overlay.position` instead of its layout position
2. Stretches the edge from center to dragged node along the new path
3. Draws a glow ring around the dragged node with `overlay.glow_alpha` opacity

**Approach for backward compatibility:** add an optional drag overlay parameter rather than modifying every call site. Renderer call sites in `graph_canvas.rs` will be updated in Task 7.

- [ ] **Step 1: Read renderer signatures**

Run:

```bash
grep -n "pub fn draw_neighborhood\|pub fn draw_node\|pub fn draw_edges_for_node\|fn parallax_offset" interfaces/webchat/src/canvas_engine/renderer.rs
```

Identify the public entry point (`draw_neighborhood`). The internal helpers `draw_node`, `draw_edges_for_node` already accept a `drag: (f32, f32)` parameter (viewport pan); we'll add a second parameter `node_drag: Option<&DragOverlay>` that the helpers can use to override the dragged node's position.

- [ ] **Step 2: Add `node_drag` parameter to `draw_neighborhood`**

In `interfaces/webchat/src/canvas_engine/renderer.rs`, change the signature of `pub fn draw_neighborhood(...)` to add a final parameter:

```rust
pub fn draw_neighborhood(
    ctx: &CanvasRenderingContext2d,
    viewport: &Viewport,
    nbhd: &Neighborhood,
    drag: (f32, f32),
    selected: Option<&str>,
    hovered: Option<&str>,
    node_drag: Option<&crate::canvas_engine::drag::DragOverlay>,    // NEW
) { ... }
```

Inside the function, before each `draw_node(ctx, n, drag, ...)` call for one_hop nodes, check if `node_drag.map(|o| &o.node_id) == Some(&n.id)`. If yes, compute an alternate position via the overlay's `position` field and pass it to a new helper `draw_node_at_pos`. Otherwise, draw normally.

For the edge that connects center → dragged node, similarly check overlap and stretch the line endpoints.

The minimum useful diff:

```rust
    for (idx, n) in nbhd.one_hop.iter().enumerate() {
        let _ = idx;
        if let Some(overlay) = node_drag.filter(|o| o.node_id == n.id) {
            // Draw at overlay position with glow
            draw_dragged_node(ctx, n, viewport, overlay, selected, hovered);
        } else {
            draw_edges_for_node(ctx, n, nbhd, drag);
            draw_node(ctx, n, drag, selected, hovered);
        }
    }
```

Add the new `draw_dragged_node` helper at the bottom of the file:

```rust
fn draw_dragged_node(
    ctx: &CanvasRenderingContext2d,
    n: &crate::canvas_engine::types::CanvasNode,
    viewport: &crate::canvas_engine::viewport::Viewport,
    overlay: &crate::canvas_engine::drag::DragOverlay,
    selected: Option<&str>,
    hovered: Option<&str>,
) {
    let _ = (selected, hovered);
    let world = crate::canvas_engine::types::Vec2::new(overlay.position.x, overlay.position.y);
    let screen = viewport.world_to_screen(world);
    // Edge from center (0,0 in world) to current overlay position
    let center_screen = viewport.world_to_screen(crate::canvas_engine::types::Vec2::zero());
    ctx.set_stroke_style_str("rgba(167, 139, 250, 0.6)");
    ctx.set_line_width(1.5);
    ctx.begin_path();
    ctx.move_to(center_screen.x, center_screen.y);
    ctx.line_to(screen.x, screen.y);
    let _ = ctx.stroke();
    // Node body
    let radius = crate::canvas_engine::types::note_radius(n.link_count);
    ctx.set_fill_style_str("#a78bfa");
    ctx.begin_path();
    let _ = ctx.arc(screen.x, screen.y, radius, 0.0, std::f64::consts::TAU);
    ctx.fill();
    // Glow ring
    if overlay.glow_alpha > 0.01 {
        ctx.set_stroke_style_str(&format!("rgba(252, 211, 77, {:.3})", overlay.glow_alpha));
        ctx.set_line_width(3.0);
        ctx.begin_path();
        let _ = ctx.arc(screen.x, screen.y, radius + 6.0, 0.0, std::f64::consts::TAU);
        let _ = ctx.stroke();
    }
}
```

The exact field name `n.id` and accessor for `link_count` need to match the existing `CanvasNode` definition in `types.rs`. If `link_count` doesn't exist on the existing struct, use the same accessor that `draw_node` uses (read line ~290 of `renderer.rs` for the existing call to `note_radius` or equivalent).

- [ ] **Step 3: Compile check (no test for renderer; visual)**

```bash
cargo build -p webchat
```

Expected: clean build. Renderer is WASM target, so a successful `cargo build -p webchat` is sufficient — actual visual correctness is verified in Task 9.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/renderer.rs
git commit -m "canvas(renderer): draw drag overlay (stretched edge + glow)"
```

---

## Task 7: Pointer wiring + rAF integration in `graph_canvas.rs`

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/graph_canvas.rs`
- Modify: `interfaces/webchat/src/views/canvas/mod.rs` (CanvasEvent dispatcher to handle PromoteNode)

This is the most invasive change. We add three new pointer event handlers (mousedown discriminates between node hit-test and empty-canvas pan), a per-frame `drag.tick(dt)` call inside the existing rAF closure, and a path that emits `CanvasIntent::PromoteNode` when the drag controller signals completion.

**Read first:** lines 95-300 of `graph_canvas.rs` to understand the existing rAF closure structure, and lines 478-490 to see the `<canvas>` element's existing event handler bindings.

- [ ] **Step 1: Add `Rc<RefCell<DragState>>` to the component**

In `interfaces/webchat/src/views/canvas/graph_canvas.rs`, near the top of the component function (right after `let canvas_ref = NodeRef::...`), add:

```rust
    use crate::canvas_engine::drag::{DragState, DragOverlay};
    let drag_state: Rc<RefCell<DragState>> = Rc::new(RefCell::new(DragState::new()));
```

Place the `use` line at the top of the file with other imports if cleaner.

- [ ] **Step 2: Wire up new mousedown/mousemove/mouseup handlers (extend, don't replace)**

The existing handlers `on_mousedown`, `on_mousemove`, `on_mouseup` handle viewport pan. We add new logic at the **start** of each handler that:
- `on_mousedown`: hit-test for a neighbor node; if hit, call `drag_state.press(...)` and return early (do not start pan)
- `on_mousemove`: if `drag_state.is_active()`, call `drag_state.pointer_move(...)` and return early (do not pan)
- `on_mouseup`: if `drag_state.is_active()`, call `drag_state.release(...)` and process the outcome (emit Click → existing click path, Promote → set active_request, SpringBack → no-op besides state transition); return early

Find the existing handler closures (search for `let on_mousedown = move |ev: web_sys::MouseEvent| {`). Add the gating logic at the top of each.

Pseudo-diff (real edits depend on the existing closures' captures):

```rust
    let on_mousedown = {
        let drag_state = drag_state.clone();
        let graph_state = graph_state.clone();
        move |ev: web_sys::MouseEvent| {
            let screen = Vec2::new(ev.offset_x() as f64, ev.offset_y() as f64);
            let now = now_ms();
            let gs = graph_state.borrow();
            let one_hop = gs.current_one_hop();      // assumes a method that returns &[CanvasNode]
            if let Some(hit_idx) = gs.viewport.hit_test(screen, one_hop) {
                let node = &one_hop[hit_idx];
                let world = gs.viewport.screen_to_world(screen);
                drag_state.borrow_mut().press(node.id.clone(), world, now);
                return; // skip pan
            }
            drop(gs);
            // ... existing pan-start code ...
        }
    };
```

If `GraphState::current_one_hop()` does not exist, do NOT add a method on `GraphState`. Instead, since the rAF closure already calls `nav_rc.borrow().state.clone()` to get the `NavState`, replicate that pattern in `on_mousedown` by capturing `nav: Rc<RefCell<NavController>>` and reading the current neighborhood directly:

```rust
    let on_mousedown = {
        let drag_state = drag_state.clone();
        let graph_state = graph_state.clone();
        let nav_for_md = nav.clone();
        move |ev: web_sys::MouseEvent| {
            let screen = Vec2::new(ev.offset_x() as f64, ev.offset_y() as f64);
            let now = now_ms();
            // Read current neighborhood from NavController
            let one_hop_owned: Vec<CanvasNode> = nav_for_md.as_ref()
                .and_then(|n| match &n.borrow().state {
                    NavState::Active { neighborhood, .. } => Some(neighborhood.one_hop.clone()),
                    NavState::Animating { to_neighborhood, .. } => Some(to_neighborhood.one_hop.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let gs = graph_state.borrow();
            if let Some(hit_idx) = gs.viewport.hit_test(screen, &one_hop_owned) {
                let node = &one_hop_owned[hit_idx];
                let world = gs.viewport.screen_to_world(screen);
                drop(gs);
                drag_state.borrow_mut().press(node.id.clone(), world, now);
                return; // skip pan
            }
            drop(gs);
            // ... existing pan-start code ...
        }
    };
```

The `Vec<CanvasNode>` clone is O(N) per mousedown (one-shot, ~20 nodes typical) — acceptable since mousedown fires at most a few times per second from a human user.

Similar for `on_mousemove` and `on_mouseup`. For mouseup, the outcome handling:

```rust
        let outcome = {
            let mut ds = drag_state.borrow_mut();
            let center_world = Vec2::zero(); // Aleph canvas center is world (0, 0)
            let center_radius = /* radius of center node (look up from current Neighborhood) */;
            let target = ds.active_node_id().map(|s| s.to_string());
            match (ds.active_node_id(), target) {
                (Some(_), Some(t)) => Some(ds.release(center_world, center_radius, &t, now_ms())),
                _ => None,
            }
        };
        match outcome {
            Some(crate::canvas_engine::drag::ReleaseOutcome::Click) => {
                // route through existing click handler
                // (or directly call active_request.set(...) with the node id)
            }
            Some(crate::canvas_engine::drag::ReleaseOutcome::SpringBack) => {} // no-op; tick() will animate
            Some(crate::canvas_engine::drag::ReleaseOutcome::Promote { .. }) => {} // no-op; tick() emits intent
            None => { /* fall through to existing pan-release */ }
        }
```

- [ ] **Step 3: Add `drag.tick(dt)` to the rAF closure**

First, declare a per-frame timestamp tracker just above the rAF closure construction (next to `raf_handle`):

```rust
    use std::cell::Cell;
    let last_frame_ms: Rc<Cell<f64>> = Rc::new(Cell::new(0.0));
```

Then capture clones for the closure: `let last_frame_ms_inner = last_frame_ms.clone();` and `let drag_state_inner = drag_state.clone();` and `let active_request_for_promote = active_request.write_only();` (use the existing `active_request` `RwSignal<Option<String>>`'s `WriteSignal` alias, however it's named in the file; if it's already a `WriteSignal`, just `.clone()` it).

Inside the rAF `closure: Closure<dyn FnMut()>` body (around line 165-180, immediately after `nav_rc.borrow_mut().tick(now);`), add:

```rust
                // Compute frame dt with cap (protect against backgrounded-tab spikes)
                let prev = last_frame_ms_inner.get();
                let dt_s: f64 = if prev > 0.0 {
                    ((now - prev) / 1000.0).min(0.05)
                } else {
                    0.016
                };
                last_frame_ms_inner.set(now);

                // Tick the drag controller; it may emit a PromoteNode target on completion
                if let Some(promote_target) = drag_state_inner.borrow_mut().tick(dt_s) {
                    // Promote uses the same fetch path as click navigation
                    active_request_for_promote.set(Some(promote_target));
                }
```

The `0.05` cap = 50ms = 3 frames at 60fps; any frame slower than that is treated as exactly 50ms to avoid a single huge spring step.

Naming check: the existing rAF closure already binds `let now = now_ms();` near the top. Reuse that — do not re-call `now_ms()`. If `now` is bound later in the closure, hoist it.

- [ ] **Step 4: Pass `DragOverlay` to renderer in the rAF closure**

In the same closure, where `draw_neighborhood(...)` is called (lines ~205-220), retrieve the overlay snapshot and pass it as the new last argument:

```rust
                // Center radius is derived from the same `note_radius()` used by the renderer
                // for the center node. We pass world (0,0) as center_pos because the radial
                // layout always anchors the focus at world origin.
                let center_radius = crate::canvas_engine::types::note_radius(
                    neighborhood.center.link_count
                );
                let overlay = drag_state_inner.borrow().overlay_snapshot(
                    crate::canvas_engine::types::Vec2::zero(),
                    center_radius,
                );
                draw_neighborhood(
                    &ctx,
                    &viewport,
                    &neighborhood,
                    drag,
                    selected.as_deref(),
                    hovered.as_deref(),
                    overlay.as_ref(),
                );
```

Apply the same change to the `Animating` branch's `draw_neighborhood` call (read `to_neighborhood.center.link_count` for the `Animating` case since the new center is the right reference).

If `link_count` is not a field of `CanvasNode`, fall back to a fixed `let center_radius = 24.0;` — but check `types.rs:CanvasNode` first; the `note_radius(link_count)` helper at `types.rs:89` strongly implies the field exists.

- [ ] **Step 5: Add `pointercancel` listener (single line in JSX)**

In the `<canvas>` element JSX (around line 478):

```rust
        <canvas
            node_ref=canvas_ref
            class="absolute inset-0 w-full h-full"
            on:mousedown=on_mousedown
            on:mousemove=on_mousemove
            on:mouseup=on_mouseup
            on:mouseleave=move |_| {                                     // NEW
                drag_state_for_cancel.borrow_mut().cancel();
            }
        />
```

Capture `drag_state.clone()` as `drag_state_for_cancel` outside the JSX block.

- [ ] **Step 6: Compile**

```bash
cargo build -p webchat
```

Expected: clean build. If there are missing-field errors on `GraphState::current_one_hop`, implement the helper (Step 2 alternative).

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/views/canvas/graph_canvas.rs interfaces/webchat/src/views/canvas/mod.rs
git commit -m "canvas(view): wire pointer events and rAF tick to DragState"
```

---

## Task 8: Block drag during retarget tween

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/graph_canvas.rs`

Per spec §8 edge case: while a `NavController::retarget` tween is running, ignore new `mousedown` events to prevent two tweens overlapping.

- [ ] **Step 1: Identify how to detect "retarget tween in progress"**

Run:

```bash
grep -n "NavState::Animating\|fn is_animating\|NavController::tick" interfaces/webchat/src/canvas_engine/navigation.rs
```

If `NavState::Animating { .. }` is the indicator, expose a public helper on `NavController`. Add to `impl NavController` block in `navigation.rs`:

```rust
    /// True while a `retarget` tween is animating between two neighborhoods.
    /// Used by the canvas view to gate user input that would conflict with the tween
    /// (e.g. starting a node-drag while the focus is mid-flight).
    pub fn is_animating(&self) -> bool {
        matches!(self.state, NavState::Animating { .. })
    }
```

Then write the failing test (append inside the existing `#[cfg(test)] mod tests` block, after the existing `retarget_*` tests around line 343):

```rust
    #[test]
    fn is_animating_reflects_state() {
        // Idle (Active without ongoing tween): is_animating == false
        let mut nc = NavController::new();
        // Default state is Idle/Loading; explicitly construct an Active state to start
        nc.fulfilled(
            "n1".to_string(),
            "Node 1".to_string(),
            crate::canvas_engine::types::Neighborhood::default(),
        );
        assert!(!nc.is_animating(), "freshly fulfilled Active state should not be animating");

        // Trigger a retarget — this transitions Active → Animating
        let to = crate::canvas_engine::types::Neighborhood::default();
        nc.retarget(to, 1000.0, 300);
        assert!(nc.is_animating(), "after retarget, state should be Animating");

        // Tick past the animation duration — should fall out of Animating
        nc.tick(1000.0 + 300.0 + 1.0);
        assert!(!nc.is_animating(), "after animation duration, should no longer be animating");
    }
```

If `Neighborhood` does not implement `Default`, replace `Neighborhood::default()` with the same construction pattern used in the existing `retarget_*` tests at line 281 onwards (read those test bodies and copy the construction).

Run the test:

```bash
cargo test -p webchat --lib canvas_engine::navigation::tests::is_animating_reflects_state -- --nocapture
```

Expected: pass after `is_animating()` is added; fail (compile error) before.

- [ ] **Step 2: Gate `on_mousedown` in `graph_canvas.rs`**

In the closure body of `on_mousedown`, immediately after computing the screen position but before hit_test, add:

```rust
        if let Some(nav) = nav_for_mousedown.as_ref() {
            if nav.borrow().is_animating() {
                return;
            }
        }
```

Where `nav_for_mousedown` is `nav.clone()` captured at closure creation.

- [ ] **Step 3: Build and verify clean**

```bash
cargo build -p webchat
cargo test -p webchat --lib canvas_engine::navigation
```

Expected: clean build; navigation tests still pass.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/canvas/graph_canvas.rs interfaces/webchat/src/canvas_engine/navigation.rs
git commit -m "canvas(view): block drag while NavController retarget is animating"
```

---

## Task 9: End-to-end verification + cleanup

**Files:** read-only

- [ ] **Step 1: Full crate test**

```bash
cargo test -p alephcore --lib
cargo test -p webchat --lib
```

Expected: all tests green (no regressions); the new tests (5 spring + 17 drag + 1 intent + 1 navigation `is_animating`) all pass.

- [ ] **Step 2: Release build**

```bash
just build
```

Or, if `just` is unavailable:

```bash
cargo build --release --bin aleph-server
cd interfaces/webchat && wasm-pack build --release && cd ../..
```

Expected: clean build, no warnings about unused `DragState`/`DragOverlay`/`Spring2D`.

- [ ] **Step 3: Restart server**

Per CLAUDE.md vault-safety rules:

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
ps aux | grep "[a]leph-server" | grep -v zsh
# Expected: no processes left

target/release/aleph-server start &
sleep 5
ps aux | grep "[a]leph-server" | grep -v grep
# Expected: exactly one process
```

- [ ] **Step 4: Browser smoke test on `http://127.0.0.1:18790/memory`**

Manual verification — open the canvas, then for each scenario observe behavior:

| Scenario | Expected |
|---|---|
| Click a neighbor node (no movement) | Existing navigation: focus changes, retarget animation runs. No drag artifacts. |
| Slow drag of a neighbor outward, release > 100px from center | Node follows cursor with visible edge stretching to center. Glow appears as you near center, fades when far. On release, node springs back to original layout slot in <1s, no overshoot. |
| Drag a neighbor toward center until inside the inner ~75px hot zone, release | Node tweens into center, navigation fires (new neighborhood loads). |
| Flick a neighbor toward center (release outside hot zone but with high inward velocity) | Node tweens into center via the velocity path; navigation fires. |
| Quick mousedown + mouseup with no movement | Click navigation (existing behavior); no drag transition. |
| During an active retarget animation (after click), attempt to mousedown a neighbor | Mousedown ignored — no drag started, no visual glitch. |
| Drag a neighbor, then move mouse out of canvas window | `mouseleave` triggers `DragState::cancel()` → state snaps to Idle, no spring animation. |

If any scenario fails, identify the failing task, fix the root cause, re-run tests.

- [ ] **Step 5: Idle CPU baseline check (macOS)**

In Activity Monitor, sample the `aleph-server` process CPU% and the Chrome tab CPU% with the canvas open and idle (no mouse movement). Compare to baseline before this PR — should be within ±0.1% of baseline.

- [ ] **Step 6: Final commit (if any cleanup happened)**

If steps 1-5 surfaced fixes that needed additional commits, this step is a no-op. Otherwise nothing to commit here.

---

## Rollback

If the drag interaction causes unforeseen issues:

```bash
# Revert all 8 commits in this batch (find the first commit's parent SHA via git log)
git revert <first-drag-commit>..HEAD
cargo build --release
target/release/aleph-server start
```

Alternatively, set `DragState::Idle` permanently by short-circuiting `press()` (one-line patch) — the renderer overlay path is silent on `None` overlay so nothing visible breaks.

---

## Out of Scope (formal record from spec)

Per spec §3 / §12 — explicitly NOT implemented in this plan:

- Force-directed layout (D1)
- Pan/zoom inertia + overscroll bounce (D2)
- Cluster bubble / center node / orphan dragging (D3)
- Neighbor reactions during drag (D4)
- User-configurable physics constants (D5)
- Multi-touch gestures (D6)
- Prefetch during drag (D7)
