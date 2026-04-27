# Canvas Elastic Node Drag — Design Spec

**Date:** 2026-04-27
**Status:** Approved (pending implementation plan)
**Related:**
- `docs/superpowers/specs/2026-04-26-canvas-detail-slider-fix-design.md` (Effect-fetch / Effect-refold split that this builds on)
- `docs/superpowers/specs/2026-04-27-canvas-agent-id-unification-design.md` (predecessor; shipped 2026-04-27)

---

## 1. Problem

The memory canvas (`interfaces/webchat/src/views/canvas/`) renders a focus node with its 1-hop neighbors, but feels **stiff**: nodes are positioned by deterministic layout and respond to nothing except clicks. The user reports two related pains:

1. **"不能拉动节点"** — nodes are not directly manipulable. Clicking a neighbor jumps focus, but there is no tactile sense of grabbing/pulling.
2. **"缺少弹性效果"** — overall canvas lacks physical feedback. Animations are limited to the existing retarget tween; the static frames between feel rigid.

The existing `interaction.rs:16-17` already has `is_dragging_node: bool` and `dragged_node_idx: Option<usize>` fields with no implementation — drag was scaffolded and never built.

## 2. Goals

- Add **direct-manipulation drag** for 1-hop neighbor nodes with spring-back on release.
- Add **promote-on-drag** as a second navigation gesture: dragging a neighbor toward the center promotes it to the new focus.
- Maintain a **strict performance contract**: zero CPU/memory cost when idle; per-frame work bounded to one dragged node, one connecting edge, one threshold check.
- Reuse existing infrastructure (`tween.rs`, `viewport.rs`, `interaction.rs` scaffolding, `navigation.rs::retarget`).

## 3. Non-Goals

| Excluded | Why |
|---|---|
| Force-directed layout (continuous N-body simulation) | Conflicts with "no CPU burden" constraint and with Top-K folding semantics |
| Pan/zoom inertia + overscroll bounce | Worthwhile but separate from "can't drag nodes" pain; ship later |
| Cluster bubble / center node / orphan node dragging | YAGNI; promote-on-drag has no defined semantics for these |
| Neighbor reactions during drag (other nodes leaning toward dragged) | N springs/frame violates cost constraint; visually dilutes focus |
| Snap-and-relayout or two-phase swap on promote | Too programmatic / too complex; tween-into-center is the right balance |
| Multi-touch / pinch gestures | Out of scope; primary pointer only |
| User-configurable physics constants | Defaults are sufficient; settings panel is a follow-up |
| Prefetch during drag | Drag is the navigation; pre-fetching neighbors-of-neighbors adds nothing |

## 4. Approach

Two interaction primitives that share one gesture (grab + drag):

- **(a) Spring-back drag** — node follows cursor; releasing without triggering promote returns it to its layout slot via critically-damped spring.
- **(c) Promote-on-drag** — releasing inside a center hot-zone OR with sufficient centripetal flick velocity tweens the node into the center position, then triggers `navigate(node_id)`.

Compositional structure:
- One gesture, two outcomes — predictable and discoverable.
- Visual feedback during drag is **(B) node + edge stretch**, with **(D-subset) threshold cue** (center grows slightly + dragged node glows when promote becomes imminent).
- Promote transition is **(B) tween-into-center** carrying release velocity as initial condition — gesture and result are physically continuous.

## 5. Architecture

New code lives in `interfaces/webchat/src/canvas_engine/`. No new crates, no rendering pipeline rewrite.

### 5.1 Module map

| Module | File | Role | Change kind |
|---|---|---|---|
| Drag controller | `canvas_engine/drag.rs` | State machine; pointer event handling; gesture detection; per-frame tick coordination | New file |
| Spring physics | `canvas_engine/tween.rs` | Adds `Spring2D` (critically-damped); `Tween2D` wrapper for promote | Extend |
| Pointer wiring | `views/canvas/mod.rs` | Subscribe `pointerdown/move/up/cancel` on `<canvas>`; forward to `DragState` | Modify |
| Renderer | `canvas_engine/renderer.rs` | Read `(drag_node_idx, drag_offset, glow_amount)` overlay; draw stretched edge to dragged node | Extend |
| Interaction state | `canvas_engine/interaction.rs` | Activate existing `is_dragging_node` / `dragged_node_idx` fields; integrate `DragState` | Modify |
| Frame loop | `canvas_engine/mod.rs` | Add `drag.tick(dt)` between existing tween tick and renderer | Modify |

### 5.2 Data flow

```
DOM pointer event → Leptos signal → CanvasView::on_pointer_*
                                        ↓
                                  DragState::transition(event)
                                        ↓
                                  triggers redraw if state changed

Per frame:
  drag.tick(dt) → maybe transition (SpringBack/Promoting completing)
                ↓
                emits CanvasIntent::Navigate(node_id) on Promote success
                ↓
  renderer.set_drag_overlay(drag.snapshot()) → render
```

### 5.3 What this does NOT touch

- Top-K folding (`adapter.rs`) — drag is a transient visual offset, never modifies `nbhd.neighbors`
- Cluster bubbles (`cluster.rs`) — bubbles are not draggable
- Prefetch (`prefetch.rs`) — disabled during drag
- Detail slider, search, breadcrumb — independent code paths

## 6. Components

### 6.1 `DragState` state machine (in `drag.rs`)

```rust
pub enum DragState {
    Idle,
    Pressed { node_idx: usize, start_pos: Vec2, start_time: f64 },
    Dragging {
        node_idx: usize,
        offset: Vec2,
        velocity_history: VecDeque<(Vec2, f64)>,  // for flick detection
    },
    SpringBack { node_idx: usize, spring: Spring2D },     // failed promote
    Promoting { node_idx: usize, tween: Tween2D, target_node_id: String },
}
```

**Transitions:**

| From | Event | To |
|---|---|---|
| `Idle` | pointer_down on neighbor node | `Pressed` |
| `Pressed` | pointer_move (cumulative > CLICK_THRESHOLD px) | `Dragging` |
| `Pressed` | pointer_up (within CLICK_THRESHOLD px AND < CLICK_TIME_MS) | `Idle` + emit click intent |
| `Dragging` | pointer_move | `Dragging` (offset + velocity update) |
| `Dragging` | pointer_up, gesture passes | `Promoting` |
| `Dragging` | pointer_up, gesture fails | `SpringBack` |
| `SpringBack` | tick, `spring.settled()` true | `Idle` |
| `Promoting` | tick, `tween.reached()` true | `Idle` + emit `CanvasIntent::Navigate(target_node_id)` |
| any | pointer_cancel | `Idle` (immediate, no animation) |

### 6.2 `Spring2D` / `Tween2D` (extending `tween.rs`)

```rust
pub struct Spring2D {
    pos: Vec2,        // current displacement from rest
    vel: Vec2,        // current velocity
    target: Vec2,     // rest position
    k: f32,           // stiffness
    c: f32,           // damping (critical: c = 2 * sqrt(k))
}

impl Spring2D {
    pub fn new(initial_offset: Vec2, initial_vel: Vec2, target: Vec2) -> Self;
    pub fn tick(&mut self, dt: f32) -> Vec2;  // returns new pos
    pub fn settled(&self) -> bool;            // |pos - target| < EPS && |vel| < EPS
}
```

`Tween2D` is a thin wrapper around `Spring2D` whose `target` is the center `(0, 0)` and whose `reached()` is `settled()` plus an upper time bound (`PROMOTE_TWEEN_MAX_MS`).

**Numerical stability:** `tick(dt)` sub-steps if `dt > 16ms` to avoid overshoot from frame stutters.

### 6.3 Gesture detector (in `drag.rs`, called from `Dragging → ?`)

```rust
fn evaluate_release(state: &Dragging, center_world: Vec2) -> ReleaseOutcome {
    let in_hot_zone = state.offset.distance(center_world) < HOT_ZONE_RADIUS;
    let avg_velocity = state.velocity_history.windowed_avg(last_n=3);
    let toward_center = (center_world - state.current_pos()).normalize();
    let centripetal_flick = avg_velocity.dot(toward_center) > FLICK_THRESHOLD
                         && avg_velocity.magnitude() > MIN_FLICK_SPEED;
    if in_hot_zone || centripetal_flick {
        ReleaseOutcome::Promote { initial_velocity: avg_velocity }
    } else {
        ReleaseOutcome::SpringBack
    }
}
```

### 6.4 Default parameters (consts at top of `drag.rs`)

| Param | Default | Notes |
|---|---|---|
| `CLICK_THRESHOLD_PX` | 5 | Drag less than this = click |
| `CLICK_TIME_MS` | 200 | Press shorter than this = click candidate |
| `HOT_ZONE_RADIUS` | center node radius × 2.5 | Promote hit area |
| `FLICK_THRESHOLD` | 600 px/s (centripetal) | Velocity component toward center |
| `MIN_FLICK_SPEED` | 400 px/s (total) | Anti-misfire |
| `SPRING_K` | 220 | Spring stiffness |
| `SPRING_C` | `2 * sqrt(SPRING_K)` ≈ 29.6 | Critical damping (no overshoot) |
| `PROMOTE_TWEEN_MAX_MS` | 280 | Upper bound on promote tween duration |
| `GLOW_RADIUS` | center node radius × 4 | Distance at which glow starts |
| `VELOCITY_HISTORY_SIZE` | 8 | Ring buffer; flick averages last 3 |

## 7. Performance Contract

| Phase | CPU per frame | Memory |
|---|---|---|
| Idle | 0 ops | 0 (zero-sized variant) |
| Pressed (held, not dragged) | 0 ops | ~32 B |
| Dragging | 1 vec2 sub + push to deque (≤8) | ~96 B |
| SpringBack | 1 spring tick (~8 fp ops) + 1 settled check | ~32 B |
| Promoting | 1 spring tick + 1 navigate trigger (one-shot) | ~96 B |
| Renderer overhead during drag | 1 hot-zone distance test + 1 glow alpha lerp | 0 |

**Invariants:**
- Idle = strictly zero allocation, zero computation
- Any drag cycle terminates within ~1 second (spring settle or tween bound)
- No prefetch, no fetch, no layout recompute during drag
- Existing `tween.rs` `tick()` is called as before; new `drag.tick()` adds at most one more call

## 8. Edge Cases

| Case | Handling |
|---|---|
| Background data refresh during drag (`fold_threshold` change, `last_response` updates) | DragState references node by **id**, not array index; on refresh, re-resolve idx; if node disappeared, transition to `SpringBack` to original screen position then `Idle` |
| `pointercancel` (touch interrupted, focus loss) | Immediate `Idle`, no spring animation |
| Multi-touch / non-primary pointer | Ignore; only `event.isPrimary` is processed |
| Drag past canvas viewport edge | No bound on offset; if not promoted, springs back from wherever |
| `pointerdown` while a `retarget` tween is already running (e.g., user clicked navigate then immediately tries to drag) | Ignore the new `pointerdown` until retarget completes — prevents visual conflict between two tweens |
| `Promoting` succeeds but downstream `navigate` fetch fails | Tween still completes visually; navigation error path is owned by existing `navigation.rs`, no new behavior |
| User starts drag, then `dt` jumps (browser tab background) | Sub-step in `tick()` clamps any `dt > 16ms` into 16ms slices |

## 9. Testing

### 9.1 Unit tests (`drag.rs` `#[cfg(test)] mod tests`)

| Test | Asserts |
|---|---|
| `pressed_within_threshold_emits_click` | Press → quick release within 5px → click intent, no Drag |
| `release_in_hot_zone_promotes` | offset inside hot zone → `Promoting`, regardless of velocity |
| `centripetal_flick_promotes` | offset outside zone but centripetal velocity above threshold → `Promoting` |
| `centrifugal_drag_springs_back` | offset outside zone, velocity away from center → `SpringBack` |
| `slow_release_outside_zone_springs_back` | outside zone, velocity below threshold → `SpringBack` |
| `spring_settles_within_one_second` | `SpringBack` reaches `Idle` in ≤ 1s simulated time |
| `promote_tween_carries_release_velocity` | Tween initial velocity equals release velocity (centripetal component) |
| `pointer_cancel_resets_to_idle` | `pointercancel` from any state → `Idle` immediately |
| `drag_blocked_during_retarget_tween` | `pointerdown` during active retarget is ignored |

### 9.2 Integration (optional, deferred)

`wasm-bindgen-test` driving simulated pointer event sequences and asserting emitted `CanvasIntent` sequence. Useful for regression but not blocking initial ship.

### 9.3 What is NOT tested

- Specific physics parameter values (these are tuning, not correctness)
- Pixel-level rendering output (visual regression is manual)
- 60fps frame budget (not in scope of unit tests; performance contract enforced by §7 invariants)

## 10. Risks & Mitigations

| Risk | Mitigation |
|---|---|
| `pointer*` events lost or reordered in Leptos WASM bridge | Use `PointerEvent` capture semantics; rely on `pointercancel` as the universal reset path |
| Hit-test coordinate system drifts from renderer | Hit-test reuses `renderer::node_world_to_screen` inverse transform — single source of truth |
| New tween conflicts with existing `retarget` tween | §8 rule: retarget-in-progress blocks `pointerdown`; drag-in-progress blocks new retarget |
| Spring numerically unstable on dt spikes | Critical damping + sub-stepping (max 16ms slices) |
| WASM debugging difficulty | Add `tracing::debug!` on every state transition; visible in browser DevTools console |

## 11. Definition of Done

1. Unit tests §9.1 all green (9 tests)
2. `cargo build --release` clean
3. `cargo clippy -- -D warnings` clean
4. Browser smoke test (manual) on `http://127.0.0.1:18790/memory`:
   - Drag a neighbor node — node follows cursor with no perceptible lag
   - Release outside hot zone with low velocity — node springs back to original layout slot
   - Drag into hot zone, release — node tweens into center, navigation fires, new focus loads
   - Flick a neighbor toward center, release outside hot zone — promote triggers via velocity path
   - Click (no movement) — existing click navigation still works
   - During retarget tween, attempt drag — input ignored, no visual glitch
5. Idle CPU usage equal to baseline (verified via macOS Activity Monitor sampling)
6. Existing detail slider, click navigation, cluster expansion behaviors unchanged (regression check)

## 12. Out of Scope (formal record)

The following were considered and explicitly deferred:

- **D1 — Force-directed layout**: violates CPU constraint; conflicts with Top-K folding
- **D2 — Pan/zoom inertia + overscroll bounce**: independent value; ship later as separate feature
- **D3 — Drag of cluster bubbles / center node / orphan nodes**: no defined promote semantics
- **D4 — Neighbor reactions during drag**: cost vs visual payoff unfavorable
- **D5 — Configurable physics via settings UI**: defaults are sufficient; add only if user feedback warrants
- **D6 — Multi-touch gestures**: separate scope (pinch zoom, two-finger pan)
- **D7 — Prefetch during drag**: drag is the navigation; no value in pre-fetching
