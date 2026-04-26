# Canvas Detail Slider Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the Loading-flash flicker on the radial canvas's Detail slider; deliver a silky, interruptible animation as nodes fold/unfold across threshold values without any network round-trip.

**Architecture:** Cache the raw `GraphNeighborsResponse` per center; split the existing fetch+threshold Effect into Effect-fetch (subscribes to `active_request`) and Effect-refold (subscribes to `fold_threshold`); add `NavController::retarget` to drive interruptible Active→Animating and Animating→Animating transitions without entering Loading; preserve viewport across slider ticks via a new `update_graph_state_nodes_only` helper.

**Tech Stack:** Rust 2021, Leptos 0.8 reactive signals (`RwSignal`, `Effect`), wasm-bindgen, single-threaded WASM. Build via `cargo check -p alephcore`, `cargo test -p alephcore --lib`, `just build`.

**Spec:** `docs/superpowers/specs/2026-04-26-canvas-detail-slider-fix-design.md`

---

## File Map

| File | Change | Responsibility |
|---|---|---|
| `interfaces/webchat/src/canvas_engine/tween.rs` | Add `pub(crate) fn build_interpolated_neighborhood` | Move from graph_canvas.rs; central home for all tween helpers (already has `lerp_node`, `ease_in_out`) |
| `interfaces/webchat/src/views/canvas/graph_canvas.rs` | Delete local `build_interpolated_neighborhood`; import from tween | Render-only file should not own pure interpolation logic |
| `interfaces/webchat/src/canvas_engine/navigation.rs` | Add `pub const RETARGET_DURATION_MS`, `pub fn retarget`, 3 new tests | NavController owns all state transitions; retarget is a new high-level entry point that picks correct target state without Loading |
| `interfaces/webchat/src/canvas_engine/prefetch.rs` | Refactor `PrefetchCache` to id-only key, raw response value, explicit `now_ms` timestamp; rewrite 3 cache tests, add 1 new test | Cache must serve any `fold_threshold` from a single raw payload |
| `interfaces/webchat/src/views/canvas/mod.rs` | Add `last_response: RwSignal<Option<(String, GraphNeighborsResponse)>>`; rewrite Effect 2 → Effect-fetch (no `fold_threshold` subscription); add Effect-refold; add `update_graph_state_nodes_only`; adapt Effect 1 + Effect 4 to new cache API | Composition root for radial canvas; coordinates signals, effects, NavController |

---

## Task 1: Migrate `build_interpolated_neighborhood` to `tween.rs`

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/tween.rs` (add function)
- Modify: `interfaces/webchat/src/views/canvas/graph_canvas.rs:2,76-91` (drop unused imports + local fn; add tween import)

This is a pure refactor. No behavior change. Foundation for Task 2 (NavController.retarget will use it).

- [ ] **Step 1: Add `build_interpolated_neighborhood` to `tween.rs`**

Open `interfaces/webchat/src/canvas_engine/tween.rs`. Replace the existing top imports with:

```rust
use crate::canvas_engine::types::{Neighborhood, Vec3};
use std::collections::{HashMap, HashSet};
```

Then, after the `lerp_node` function definition (around line 64) and before the `#[cfg(test)]` block, append:

```rust
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
```

Note: the unused `HashMap` import in the existing file is fine — it's still used by tests.

- [ ] **Step 2: Update `graph_canvas.rs` to import from tween and delete local function**

Open `interfaces/webchat/src/views/canvas/graph_canvas.rs`.

Replace this block (currently around lines 1-17, the use statements):

```rust
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use leptos::callback::Callback;

use crate::canvas_engine::interaction::{CanvasEvent, InteractionState};
use crate::canvas_engine::layout::ForceLayout;
use crate::canvas_engine::navigation::NavController;
use crate::canvas_engine::renderer::{draw_neighborhood, Renderer};
use crate::canvas_engine::tween::lerp_node;
use crate::canvas_engine::types::*;
use crate::canvas_engine::viewport::Viewport;
```

…with:

```rust
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use leptos::callback::Callback;

use crate::canvas_engine::interaction::{CanvasEvent, InteractionState};
use crate::canvas_engine::layout::ForceLayout;
use crate::canvas_engine::navigation::NavController;
use crate::canvas_engine::renderer::{draw_neighborhood, Renderer};
use crate::canvas_engine::tween::build_interpolated_neighborhood;
use crate::canvas_engine::types::*;
use crate::canvas_engine::viewport::Viewport;
```

Then delete the entire local `build_interpolated_neighborhood` function (currently at lines 73–91, including the doc comment):

```rust
/// Build an interpolated Neighborhood at tween parameter `t` between `from` and `to`.
/// The resulting neighborhood's `target_positions` map contains lerped Vec3 positions
/// for every node id that appears in either neighborhood.
fn build_interpolated_neighborhood(from: &Neighborhood, to: &Neighborhood, t: f32) -> Neighborhood {
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
```

The call site `build_interpolated_neighborhood(&from_neighborhood, &to_neighborhood, t)` (around line 235) is unchanged — it now resolves to the imported function.

- [ ] **Step 3: Verify compile + tests still pass**

Run:

```bash
cargo check -p alephcore
```

Expected: compiles with zero errors, zero warnings (the `lerp_node` import in graph_canvas.rs is removed because nothing else there uses it).

Run:

```bash
cargo test -p alephcore --lib canvas_engine::tween
```

Expected: 5 existing tween tests pass (`ease_endpoints`, `ease_clamps_out_of_range`, `lerp_node_shared_interpolates_position`, `lerp_node_fadeout_only_in_from`, `lerp_node_fadein_only_in_to`).

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/tween.rs interfaces/webchat/src/views/canvas/graph_canvas.rs
git commit -m "canvas(tween): move build_interpolated_neighborhood from graph_canvas to tween"
```

---

## Task 2: Add `NavController::retarget` with TDD

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/navigation.rs` (add const + method + 3 tests)

`retarget` is the new high-level transition method used by Effect-refold to drive the Detail slider's tweens without entering Loading.

- [ ] **Step 1: Write the 3 failing tests in `navigation.rs`**

Open `interfaces/webchat/src/canvas_engine/navigation.rs`. Add a helper near the top of the existing `#[cfg(test)] mod tests` block (right after the existing `nbhd` helper at line 146-160), then append three new tests at the end of the test module (after `breadcrumb_pop_to_truncates`):

```rust
fn nbhd_with_pos(id: &str, marker_id: &str, pos: Vec3) -> Neighborhood {
    let mut n = nbhd(id);
    n.target_positions.insert(marker_id.to_string(), pos);
    n
}

#[test]
fn retarget_from_active_enters_animating_with_same_center() {
    let mut nav = NavController::new();
    nav.fulfilled("a".to_string(), "Alpha".to_string(), nbhd("a"));
    assert!(matches!(nav.state, NavState::Active { .. }));

    nav.retarget(nbhd("a"), 100.0, 150);

    match &nav.state {
        NavState::Animating { from_id, to_id, t, duration_ms, started_at_ms, .. } => {
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
    let from = nbhd_with_pos("a", "x", Vec3::new(0.0, 0.0, 0.0));
    let to_old = nbhd_with_pos("a", "x", Vec3::new(100.0, 0.0, 0.0));
    nav.fulfilled("a".to_string(), "Alpha".to_string(), from.clone());
    nav.start_animation(from, to_old, "a".to_string(), "a".to_string(), 0.0, 1000);
    nav.tick(500.0); // advance to t = 0.5

    let new_to = nbhd_with_pos("a", "x", Vec3::new(200.0, 0.0, 0.0));
    nav.retarget(new_to, 500.0, 150);

    match &nav.state {
        NavState::Animating { from_neighborhood, to_neighborhood, t, .. } => {
            // The new `from` is the snapshot of the previous animation at t=0.5.
            // ease_in_out(0.5) == 0.5 (smoothstep midpoint) so the interpolated x is 50.0.
            let p = from_neighborhood
                .target_positions
                .get("x")
                .copied()
                .expect("interpolated frame must contain x");
            assert!(
                (p.x - 50.0).abs() < 1.0,
                "expected snapshot x ~= 50.0, got {}",
                p.x
            );
            // The new `to` is the third neighborhood we passed in.
            let p_to = to_neighborhood
                .target_positions
                .get("x")
                .copied()
                .expect("new to must contain x");
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
```

- [ ] **Step 2: Run the new tests and verify they fail**

```bash
cargo test -p alephcore --lib canvas_engine::navigation::tests::retarget_
```

Expected: compilation fails with `no method named 'retarget'`. (The 3 retarget tests cannot even build.)

- [ ] **Step 3: Add the `retarget` implementation**

In `interfaces/webchat/src/canvas_engine/navigation.rs`, at the top of the file replace:

```rust
use crate::canvas_engine::types::{NavState, Neighborhood};

pub const BREADCRUMB_MAX: usize = 20;
```

with:

```rust
use crate::canvas_engine::tween::build_interpolated_neighborhood;
use crate::canvas_engine::types::{NavState, Neighborhood};

pub const BREADCRUMB_MAX: usize = 20;
pub const RETARGET_DURATION_MS: u32 = 150;
```

Then, inside `impl NavController { ... }`, after the existing `tick` method (around line 101) and before `breadcrumb_pop_to`, insert:

```rust
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
    pub fn retarget(&mut self, to_neighborhood: Neighborhood, now_ms: f64, duration_ms: u32) {
        let prev = std::mem::replace(&mut self.state, NavState::Idle);
        self.state = match prev {
            NavState::Active { node_id, neighborhood: from } => NavState::Animating {
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
```

- [ ] **Step 4: Run all navigation tests and verify they pass**

```bash
cargo test -p alephcore --lib canvas_engine::navigation
```

Expected: 8 tests pass total — 5 pre-existing (`start_idle`, `enter_then_fulfill_appends_breadcrumb`, `breadcrumb_truncates_at_max`, `animation_completes_at_t_1`, `breadcrumb_pop_to_truncates`) plus 3 new (`retarget_from_active_enters_animating_with_same_center`, `retarget_from_animating_snapshots_current_interpolated_frame`, `retarget_from_loading_falls_back_to_active_no_animation`).

- [ ] **Step 5: Verify the whole canvas_engine compiles cleanly**

```bash
cargo check -p alephcore
```

Expected: zero errors, zero warnings.

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/navigation.rs
git commit -m "canvas(nav): NavController::retarget for slider re-fold tweens"
```

---

## Task 3: Refactor `PrefetchCache` to id-only key with raw response

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/prefetch.rs` (replace struct, methods, tests)
- Modify: `interfaces/webchat/src/views/canvas/mod.rs` (every call site of `prefetch.get/put` switches to new signatures)

Cache value becomes `GraphNeighborsResponse` (raw server payload). Key drops the `threshold` dimension. Timestamp is tracked in the entry tuple because raw response has no `fetched_at_ms` field.

After this task, slider behavior is **still flicker-prone** because Effect 2 still subscribes to `fold_threshold` and refetches. The flicker fix lands in Task 4. This task keeps compilation green throughout.

- [ ] **Step 1: Write rewritten cache tests in `prefetch.rs`**

Open `interfaces/webchat/src/canvas_engine/prefetch.rs`. Replace the entire `#[cfg(test)] mod tests { ... }` block (currently lines 81-161) with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas_engine::adapter::{GraphNeighborsResponse, NoteNodeDto};
    use std::collections::HashMap;

    fn raw_resp(id: &str) -> GraphNeighborsResponse {
        GraphNeighborsResponse {
            center: NoteNodeDto {
                id: id.to_string(),
                name: id.to_string(),
                path: format!("{id}.md"),
                category: "concept".to_string(),
                tags: vec![],
                link_count: 1,
            },
            nodes: vec![],
            edges: vec![],
            hop_depth: HashMap::new(),
        }
    }

    #[test]
    fn cache_put_then_get() {
        let mut c = PrefetchCache::new();
        c.put("a".to_string(), raw_resp("a"), 0.0);
        assert!(c.get("a", 100.0).is_some());
    }

    #[test]
    fn cache_expires_after_ttl() {
        let mut c = PrefetchCache::new();
        c.put("a".to_string(), raw_resp("a"), 0.0);
        assert!(c.get("a", CACHE_TTL_MS + 1.0).is_none());
    }

    #[test]
    fn cache_evicts_oldest_at_capacity() {
        let mut c = PrefetchCache::new();
        for i in 0..(CACHE_CAPACITY + 5) {
            c.put(format!("n{i}"), raw_resp(&format!("n{i}")), 0.0);
        }
        assert_eq!(c.len(), CACHE_CAPACITY);
        assert!(c.get("n0", 0.0).is_none());
        assert!(c.get(&format!("n{}", CACHE_CAPACITY + 4), 0.0).is_some());
    }

    #[test]
    fn cache_serves_any_threshold_for_same_id() {
        // The cache no longer discriminates by fold_threshold; a single put serves
        // any caller-side threshold via `to_neighborhood(raw, _, threshold)`.
        let mut c = PrefetchCache::new();
        c.put("a".to_string(), raw_resp("a"), 0.0);
        let v = c.get("a", 100.0).expect("present");
        assert_eq!(v.center.id, "a");
    }

    #[test]
    fn debounce_fires_after_threshold() {
        let mut d = HoverDebouncer::new();
        assert_eq!(d.note_hover(Some("x"), 0.0), None);
        assert_eq!(d.note_hover(Some("x"), 100.0), None);
        assert_eq!(d.note_hover(Some("x"), 151.0), Some("x".to_string()));
    }

    #[test]
    fn debounce_resets_on_target_change() {
        let mut d = HoverDebouncer::new();
        d.note_hover(Some("x"), 0.0);
        assert_eq!(d.note_hover(Some("y"), 100.0), None);
        assert_eq!(d.note_hover(Some("y"), 251.0), Some("y".to_string()));
    }
}
```

- [ ] **Step 2: Run rewritten tests and verify they fail to compile**

```bash
cargo test -p alephcore --lib canvas_engine::prefetch
```

Expected: compile errors — `put` / `get` signatures don't match the new tests.

- [ ] **Step 3: Refactor the `PrefetchCache` struct and methods**

In `interfaces/webchat/src/canvas_engine/prefetch.rs`, replace the existing top imports + struct + impl block (currently lines 1-44) with:

```rust
use crate::canvas_engine::adapter::GraphNeighborsResponse;
use std::collections::VecDeque;

pub const HOVER_DEBOUNCE_MS: f64 = 150.0;
pub const CACHE_TTL_MS: f64 = 60_000.0;
pub const CACHE_CAPACITY: usize = 20;

/// Bounded LRU cache of raw `GraphNeighborsResponse` payloads, keyed by center id.
/// Each entry carries its own fetched-at timestamp because the raw payload has no
/// such field (unlike `Neighborhood`).
pub struct PrefetchCache {
    entries: VecDeque<(String, GraphNeighborsResponse, f64)>,
    capacity: usize,
    ttl_ms: f64,
}

impl PrefetchCache {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: CACHE_CAPACITY,
            ttl_ms: CACHE_TTL_MS,
        }
    }

    pub fn put(&mut self, id: String, raw: GraphNeighborsResponse, now_ms: f64) {
        self.entries.retain(|(k, _, _)| k != &id);
        self.entries.push_back((id, raw, now_ms));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    pub fn get(&self, id: &str, now_ms: f64) -> Option<&GraphNeighborsResponse> {
        self.entries.iter().rev().find_map(|(k, v, fetched)| {
            if k == id && now_ms - fetched <= self.ttl_ms {
                Some(v)
            } else {
                None
            }
        })
    }

    pub fn has(&self, id: &str, now_ms: f64) -> bool {
        self.get(id, now_ms).is_some()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
```

The `HoverDebouncer` struct and its impl (lines 46-79 in the original) stay verbatim — do not modify them.

- [ ] **Step 4: Adapt `mod.rs` call sites to new cache signatures**

`interfaces/webchat/src/views/canvas/mod.rs` has three call sites that need updates. Compilation will be broken until all three are done. Apply all in this step.

**Call site 1** — Effect 1 initial mount, currently around line 143:

Replace:

```rust
                    prefetch_inner.borrow_mut().put(entry_id.clone(), threshold, nbhd_for_cache);
```

with:

```rust
                    prefetch_inner.borrow_mut().put(entry_id.clone(), resp.clone(), now_ms);
```

Then, just above this line, the local variable `let nbhd_for_cache = nbhd.clone();` (currently around line 140) becomes dead. Delete that line.

**Call site 2** — Effect 2, currently around line 172:

Replace:

```rust
        let cached = prefetch_req.borrow().get(&id, threshold, now_ms).cloned();
        if let Some(nbhd) = cached {
            let name = nbhd.center.name.clone();
```

with:

```rust
        let cached = prefetch_req.borrow().get(&id, now_ms).cloned();
        if let Some(raw) = cached {
            let mut nbhd = to_neighborhood(&raw, now_ms, threshold);
            let dtos = all_dtos.get_untracked();
            populate_orphans(&mut nbhd, &dtos);
            let name = nbhd.center.name.clone();
```

The rest of the cache-hit branch (one_hop_len / total_len / neighbor_ids / seed_graph_state / nav_req.borrow_mut().fulfilled / set_focus_id etc.) stays unchanged.

**Call site 3** — Effect 4 hover prefetch, currently around lines 260-273:

Replace:

```rust
        let now = now_ms();
        let threshold = fold_threshold.get_untracked();
        // Skip if already cached and not stale
        if prefetch_e4.borrow().get(&id, threshold, now).is_some() {
            return;
        }

        let prefetch_inner = prefetch_e4.clone();
        spawn_local(async move {
            match GraphApi::neighbors(&state, &id, 3, 200).await {
                Ok(resp) => {
                    let mut nbhd = to_neighborhood(&resp, now, threshold);
                    let dtos = all_dtos.get_untracked();
                    populate_orphans(&mut nbhd, &dtos);
                    prefetch_inner.borrow_mut().put(id, threshold, nbhd);
                }
                Err(_) => {
                    // Prefetch failures are silently ignored — they will retry on next dwell
                }
            }
        });
```

with:

```rust
        let now = now_ms();
        // Skip if already cached and not stale
        if prefetch_e4.borrow().has(&id, now) {
            return;
        }

        let prefetch_inner = prefetch_e4.clone();
        spawn_local(async move {
            match GraphApi::neighbors(&state, &id, 3, 200).await {
                Ok(resp) => {
                    prefetch_inner.borrow_mut().put(id, resp, now);
                }
                Err(_) => {
                    // Prefetch failures are silently ignored — they will retry on next dwell
                }
            }
        });
```

Note: hover prefetch no longer folds or populates orphans — that's now Effect-fetch's job on click.

- [ ] **Step 5: Verify compile + all tests pass**

```bash
cargo check -p alephcore
```

Expected: zero errors, zero warnings.

```bash
cargo test -p alephcore --lib canvas_engine
```

Expected: all canvas_engine tests pass — 4 prefetch tests (rewritten + new) + 2 debounce tests + 8 navigation tests + adapter / cluster / interaction / mini_map / tween tests.

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/prefetch.rs interfaces/webchat/src/views/canvas/mod.rs
git commit -m "canvas(prefetch): cache raw responses keyed by id only"
```

---

## Task 4: Add `last_response` signal, Effect-fetch, Effect-refold, helper

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/mod.rs`

After this task, the slider works as designed: zero flicker, smooth tween animation, no network refetch.

- [ ] **Step 1: Add the `last_response` signal declaration**

Open `interfaces/webchat/src/views/canvas/mod.rs`. Find the existing signal declarations near line 62 (`let active_request: RwSignal<Option<String>> = RwSignal::new(None);`). Immediately above that line, add:

```rust
    // Raw-response snapshot for the current center. Set after a successful Effect-fetch
    // (or prefetch hit), cleared at the start of every Effect-fetch invocation.
    // Effect-refold reads this to perform local re-fold without a network round-trip.
    let last_response: RwSignal<Option<(String, GraphNeighborsResponse)>> = RwSignal::new(None);
```

Then make sure `GraphNeighborsResponse` is in scope. Find the existing import block (around line 19):

```rust
use crate::canvas_engine::adapter::{
    populate_orphans, to_neighborhood, GraphQueryResponse, NoteDetailResponse, NoteNodeDto,
};
```

Replace with:

```rust
use crate::canvas_engine::adapter::{
    populate_orphans, to_neighborhood, GraphNeighborsResponse, GraphQueryResponse,
    NoteDetailResponse, NoteNodeDto,
};
```

Also import `RETARGET_DURATION_MS`. Find:

```rust
use crate::canvas_engine::navigation::NavController;
```

Replace with:

```rust
use crate::canvas_engine::navigation::{NavController, RETARGET_DURATION_MS};
```

- [ ] **Step 2: Wire `last_response` writes into Effect 1 (initial mount)**

In Effect 1's success branch, find this block (currently around lines 140-147):

```rust
                    let nbhd_for_cache = nbhd.clone();
                    seed_graph_state(&gs_inner, &nbhd, Some(entry_id.clone()));
                    nav_inner.borrow_mut().fulfilled(entry_id.clone(), name, nbhd);
                    prefetch_inner.borrow_mut().put(entry_id.clone(), resp.clone(), now_ms);
                    active_request.set(Some(entry_id.clone()));
                    set_focus_id.set(Some(entry_id));
                    set_focus_neighbors.set(neighbor_ids);
                    set_visible_counts.set((one_hop_len, total_len));
```

Note: `let nbhd_for_cache = nbhd.clone();` should already be deleted from Task 3 step 4. If it's still there, delete it now.

Replace the surviving block with:

```rust
                    seed_graph_state(&gs_inner, &nbhd, Some(entry_id.clone()));
                    nav_inner.borrow_mut().fulfilled(entry_id.clone(), name, nbhd);
                    prefetch_inner.borrow_mut().put(entry_id.clone(), resp.clone(), now_ms);
                    last_response.set(Some((entry_id.clone(), resp)));
                    active_request.set(Some(entry_id.clone()));
                    set_focus_id.set(Some(entry_id));
                    set_focus_neighbors.set(neighbor_ids);
                    set_visible_counts.set((one_hop_len, total_len));
```

- [ ] **Step 3: Transform Effect 2 into Effect-fetch (drop `fold_threshold` subscription, add `last_response` writes)**

Replace the entire body of Effect 2 (currently lines 158-219, the second `Effect::new` block) with:

```rust
    // -----------------------------------------------------------------------
    // Effect-fetch: subscribes to `active_request` only.
    // Fired on center change (node click / search / breadcrumb). Network fetch
    // path; transitions to Loading then Active. Slider re-folds use Effect-refold.
    // -----------------------------------------------------------------------
    let nav_req = nav.clone();
    let prefetch_req = prefetch.clone();
    let gs_req = graph_state.clone();
    Effect::new(move || {
        let Some(id) = active_request.get() else { return };
        let now_ms = now_ms();

        // Sync prelude — invalidate stale snapshot, enter Loading
        last_response.set(None);
        nav_req.borrow_mut().enter(id.clone(), now_ms);

        // Prefetch cache hit → fold + apply locally, no network
        let cached = prefetch_req.borrow().get(&id, now_ms).cloned();
        if let Some(raw) = cached {
            let threshold = fold_threshold.get_untracked();
            let mut nbhd = to_neighborhood(&raw, now_ms, threshold);
            let dtos = all_dtos.get_untracked();
            populate_orphans(&mut nbhd, &dtos);
            let name = nbhd.center.name.clone();
            let one_hop_len = nbhd.one_hop.len();
            let total_len = one_hop_len
                + nbhd.clusters.iter().map(|c| c.member_ids.len()).sum::<usize>();
            let neighbor_ids: Vec<String> =
                nbhd.one_hop.iter().map(|n| n.id.clone()).collect();
            seed_graph_state(&gs_req, &nbhd, Some(id.clone()));
            nav_req.borrow_mut().fulfilled(id.clone(), name, nbhd);
            last_response.set(Some((id.clone(), raw)));
            set_focus_id.set(Some(id));
            set_focus_neighbors.set(neighbor_ids);
            set_visible_counts.set((one_hop_len, total_len));
            return;
        }

        // Cache miss — fetch from network
        let nav_fetch = nav_req.clone();
        let gs_fetch = gs_req.clone();
        let prefetch_fetch = prefetch_req.clone();
        spawn_local(async move {
            match GraphApi::neighbors(&state, &id, 3, 200).await {
                Ok(resp) => {
                    let threshold = fold_threshold.get_untracked();
                    let mut nbhd = to_neighborhood(&resp, now_ms, threshold);
                    let dtos = all_dtos.get_untracked();
                    populate_orphans(&mut nbhd, &dtos);
                    let name = nbhd.center.name.clone();
                    let one_hop_len = nbhd.one_hop.len();
                    let total_len = one_hop_len
                        + nbhd.clusters.iter().map(|c| c.member_ids.len()).sum::<usize>();
                    let neighbor_ids: Vec<String> =
                        nbhd.one_hop.iter().map(|n| n.id.clone()).collect();
                    seed_graph_state(&gs_fetch, &nbhd, Some(id.clone()));
                    nav_fetch.borrow_mut().fulfilled(id.clone(), name, nbhd);
                    prefetch_fetch.borrow_mut().put(id.clone(), resp.clone(), now_ms);
                    last_response.set(Some((id.clone(), resp)));
                    set_focus_id.set(Some(id));
                    set_focus_neighbors.set(neighbor_ids);
                    set_visible_counts.set((one_hop_len, total_len));
                }
                Err(e) => {
                    nav_fetch.borrow_mut().fail(id.clone(), e.clone());
                    web_sys::console::error_1(
                        &format!("RadialCanvasView: neighbor fetch failed: {e}").into(),
                    );
                }
            }
        });
    });
```

Key change vs original Effect 2: the `let threshold = fold_threshold.get();` reactive read is gone — Effect-fetch no longer re-fires when the slider moves. All threshold reads are `get_untracked()`. `last_response.set(...)` writes happen on both cache-hit and post-fetch success paths, with `last_response.set(None)` in the synchronous prelude.

- [ ] **Step 4: Add the `update_graph_state_nodes_only` helper**

In `interfaces/webchat/src/views/canvas/mod.rs`, find the existing `seed_graph_state` function (currently around line 457). Immediately after it, add:

```rust
/// Refresh GraphState's node/edge buffers from a freshly folded `Neighborhood`
/// without resetting viewport, scale, drag offset, selected node, or layout.
/// Used by the slider re-fold path so the user's pan/zoom/drag survives a slider tick.
fn update_graph_state_nodes_only(
    gs: &Rc<RefCell<GraphState>>,
    nbhd: &crate::canvas_engine::types::Neighborhood,
) {
    let nodes: Vec<_> = std::iter::once(nbhd.center.clone())
        .chain(nbhd.one_hop.iter().cloned())
        .chain(nbhd.two_hop.iter().cloned())
        .chain(nbhd.orphans.iter().cloned())
        .collect();
    let edges = nbhd.edges.clone();
    let mut gs = gs.borrow_mut();
    gs.nodes = nodes;
    gs.edges = edges;
    // Intentionally NOT modified: viewport.{offset,scale}, drag_offset,
    // selected_node, layout (no wake — radial uses target_positions, not physics).
}
```

- [ ] **Step 5: Add Effect-refold**

In `mod.rs`, immediately after the closing brace of Effect 4 (the hover prefetch effect, currently ending around line 280), insert this new Effect block:

```rust
    // -----------------------------------------------------------------------
    // Effect-refold: subscribes to `fold_threshold` only.
    // Fired on slider drag. Locally re-folds the cached raw response and drives
    // an interruptible NavController.retarget tween. No network, no Loading frame.
    // -----------------------------------------------------------------------
    let nav_refold = nav.clone();
    let gs_refold = graph_state.clone();
    Effect::new(move || {
        let threshold = fold_threshold.get().clamp(1, 1000);

        // Snapshot last_response and active id without subscribing to them.
        let Some((cached_id, raw)) = last_response.get_untracked() else { return };
        if active_request.get_untracked().as_ref() != Some(&cached_id) {
            return; // race: slider fired during a center transition
        }

        let now = now_ms();
        let mut nbhd = to_neighborhood(&raw, now, threshold);
        let dtos = all_dtos.get_untracked();
        populate_orphans(&mut nbhd, &dtos);

        let one_hop_len = nbhd.one_hop.len();
        let total_len =
            one_hop_len + nbhd.clusters.iter().map(|c| c.member_ids.len()).sum::<usize>();
        let neighbor_ids: Vec<String> = nbhd.one_hop.iter().map(|n| n.id.clone()).collect();

        update_graph_state_nodes_only(&gs_refold, &nbhd);
        nav_refold
            .borrow_mut()
            .retarget(nbhd, now, RETARGET_DURATION_MS);

        set_focus_neighbors.set(neighbor_ids);
        set_visible_counts.set((one_hop_len, total_len));
    });
```

- [ ] **Step 6: Verify compile + all tests pass**

```bash
cargo check -p alephcore
```

Expected: zero errors, zero warnings.

```bash
cargo test -p alephcore --lib
```

Expected: all tests pass — same count as Task 3 step 5 (no new tests added in Task 4; Effect-refold and the helper are exercised by the manual checklist in Task 5, not by unit tests since they live inside a Leptos component).

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/views/canvas/mod.rs
git commit -m "canvas(slider): local re-fold via Effect-refold + retarget tween"
```

---

## Task 5: Manual acceptance + final verification

**Files:** none modified

This task verifies the bug-fix lands as designed (no flicker, smooth tween, viewport preserved). Required because the slider behavior lives inside a Leptos component and is not directly unit-testable.

- [ ] **Step 1: Build the full release artifact**

```bash
just build
```

Expected: WASM bundle + server release binary built without errors.

- [ ] **Step 2: Restart aleph-server cleanly per CLAUDE.md process management rules**

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
ps aux | grep "[a]leph-server" | grep -v zsh | grep -v cp | grep -v tail
target/release/aleph-server start
```

Expected: no leftover processes, single server starts cleanly.

- [ ] **Step 3: Open the canvas in a browser and run the manual checklist**

Open the webchat UI, navigate to the canvas view. Wait for the radial graph to render around the auto-picked entry node.

Verify each of these in turn:

1. **No-flicker slider drag**: drag the Detail slider full sweep 4 → 30 → 4. **No** "Loading…" placeholder appears at any point. Nodes visibly tween between cluster-folded and expanded states. The "(K of N)" counter updates live.

2. **Viewport preservation**: pan the canvas (mouse drag on empty area), zoom in via mouse wheel, optionally drag a node to a new spot. Then drag the Detail slider. The pan, zoom level, and drag offset are preserved across the slider drag — the canvas does not recenter.

3. **Race during center change**: click a different node to trigger center change. While the Loading placeholder is visible, drag the Detail slider — nothing changes on screen, no console errors, no crash. After Loading clears, drag the slider again — works normally with smooth tween.

4. **Hover prefetch path**: hover over a neighbor node for ~150 ms (the prefetch dwell), then click it. The neighborhood appears without a Loading frame; the slider continues to work locally on the new center.

If any of the four checks fail, the implementation is incomplete — debug, fix, and re-run all four.

- [ ] **Step 4: Final test sweep**

```bash
cargo test -p alephcore --lib
cargo check -p alephcore
```

Expected: all tests green, zero warnings.

- [ ] **Step 5: No commit**

This task is verification-only; nothing to commit. If any fixes were needed during the manual checklist, they should land as their own follow-up commits with descriptive messages (e.g., `canvas(slider): fix focus_neighbors not updating on retarget`).

---

## Spec Coverage Check

| Spec section | Implementing task |
|---|---|
| §5.1 `last_response` signal | Task 4 step 1 |
| §5.2 Effect-fetch (replaces Effect 2) | Task 4 step 3 |
| §5.2 Effect-refold (new) | Task 4 step 5 |
| §5.3 `NavController::retarget` + 3 state arms | Task 2 step 3 |
| §5.3 `RETARGET_DURATION_MS` const | Task 2 step 3 |
| §5.4 `build_interpolated_neighborhood` migration | Task 1 |
| §5.5 `update_graph_state_nodes_only` helper | Task 4 step 4 |
| §5.6 PrefetchCache id-only key + raw value + timestamp | Task 3 step 3 |
| §5.6 New PrefetchCache API (put/get/has) | Task 3 step 3 |
| §6.1 Slider tick data flow | Task 4 step 5 (Effect-refold body) |
| §6.2 Center change data flow | Task 4 step 3 (Effect-fetch body) |
| §6.3 Race: slider during in-flight fetch | Task 4 step 3 (sync prelude clears `last_response`) + Task 4 step 5 (refold guards on `is_some()` and matching id) — verified by Task 5 step 3 check 3 |
| §7 Edge case: out-of-range threshold | Task 4 step 5 (`clamp(1, 1000)` in Effect-refold) |
| §7 Edge case: empty raw response | Implicit — `to_neighborhood` already handles empty `nodes` (existing test `to_neighborhood_basic_shape` indirectly covers); no new code needed |
| §9.1 navigation.rs 3 retarget tests | Task 2 step 1 |
| §9.2 prefetch.rs cache tests rewritten | Task 3 step 1 |
| §9.2 `cache_serves_any_threshold_for_same_id` new test | Task 3 step 1 |
| §9.3 Manual acceptance | Task 5 step 3 |
| §10 All 5 modified files touched | Tasks 1, 2, 3, 4 cover all five |

No gaps.
