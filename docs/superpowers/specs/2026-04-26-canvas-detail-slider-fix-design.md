# Canvas Detail Slider Fix — Design

**Date:** 2026-04-26
**Status:** Spec — pending implementation plan
**Scope:** Bug fix for the radial canvas's "Detail" slider; companion deferral notice for three follow-up items.

---

## 1. Problem

In the radial canvas (`interfaces/webchat/src/views/canvas/mod.rs::RadialCanvasView`), the toolbar's Detail slider drives the `fold_threshold` signal (range 4..=30). Users dragging the slider observe the canvas's nodes and edges flicker — a Loading placeholder briefly clears the graph between every threshold tick — and never see the intended "expand from cluster / fold into cluster" zoom-in/zoom-out feel.

### Root cause

`fold_threshold` controls a purely client-side top-K cut applied inside `to_neighborhood` (`interfaces/webchat/src/canvas_engine/adapter.rs:107-126`). It does not require any server data the client doesn't already have. Despite this, the existing Effect 2 (`mod.rs:164-219`) subscribes to both `active_request` and `fold_threshold` together:

- Cache key is `(id, threshold)` — every new threshold value misses the prefetch cache
- On miss, the Effect calls `nav.enter(id, now_ms)` which transitions `NavController` into `NavState::Loading`
- The `Loading` branch in `graph_canvas.rs:246-248` calls `draw_placeholder("Loading…")` — wiping the graph
- A network round-trip then refetches the same neighborhood the client already had, just to re-fold it locally

Result: 27 distinct threshold values × one network request each, with a Loading frame inserted between every tick. Visually this reads as flicker, not as a smooth detail-zoom transition.

## 2. Goals

- Eliminate the Loading flash and the network round-trip during slider drag.
- Produce a silky, interruptible animation as the user drags through threshold values, so nodes appear to fly out of clusters (threshold up) or fold back into clusters (threshold down).
- Preserve the user's pan / zoom / drag offset across slider ticks (no recentering when only the threshold changed).
- Keep CPU and memory cost lower than today (no per-threshold cache entries, no per-tick fetch).

## 3. Non-goals

- Removing the `ViewMode` enum or `DashboardState::canvas_radial_navigation` field. They stay as no-ops to avoid churning unrelated downstream files (see §8).
- Reducing `Neighborhood` cloning. The current per-tick clone is microseconds-scale; deeper reductions are a separate, lower-priority project.
- Renaming the breadcrumb "global" home label. Cosmetic only.
- Surfacing animation duration as a user preference. 150 ms is a fixed UX choice.
- Slider input throttling / debouncing. The browser's rAF loop and the microsecond cost of `to_neighborhood` make explicit throttling unnecessary.

## 4. Approach overview

Local re-fold + interruptible tween animation, layered over the existing radial nav data flow:

1. Cache the **raw `GraphNeighborsResponse`** for the current center in a new signal (`last_response`). Slider ticks read this, never the network.
2. Split today's Effect 2 into two Effects with disjoint reactive dependencies:
   - **Effect-fetch** subscribes to `active_request` only. Drives center-change fetches and Loading state.
   - **Effect-refold** subscribes to `fold_threshold` only. Re-folds locally from `last_response` and re-targets the navigation animation.
3. Add a high-level `NavController::retarget` method that picks the correct state transition without ever entering `Loading`. Quick (~150 ms) animation with mid-flight interruption support.
4. Add an `update_graph_state_nodes_only` helper that refreshes node/edge buffers without resetting the viewport.

## 5. Architecture

### 5.1 New reactive state

Added inside `RadialCanvasView` next to the existing signals:

| Name | Type | Purpose |
|---|---|---|
| `last_response` | `RwSignal<Option<(String, GraphNeighborsResponse)>>` | Raw server response for the current center, indexed by id; Effect-refold reads this snapshot to re-run `to_neighborhood` locally |

### 5.2 Effect split

Replace the existing Effect 2 (`mod.rs:158-219`) with two Effects.

#### Effect-fetch (subscribes to `active_request`)

Synchronous prelude, before any `spawn_local`:

```text
last_response.set(None)              // invalidate stale snapshot for the previous center
nav.borrow_mut().enter(id, now_ms)   // transition to Loading
```

Then check the prefetch cache. If hit:

```text
let raw = prefetch.borrow().get(&id).cloned()
let threshold = fold_threshold.get_untracked()
let mut nbhd = to_neighborhood(&raw, now_ms, threshold)
populate_orphans(&mut nbhd, &all_dtos.get_untracked())
last_response.set(Some((id.clone(), raw)))
seed_graph_state(...)
nav.borrow_mut().fulfilled(id, name, nbhd)
emit focus_neighbors / visible_counts
```

Cache miss: same as today, but on success additionally write `last_response.set(Some((id, raw)))` before `nav.fulfilled`. The full raw response is stored — the cost is one extra `Vec<NoteNodeDto>` per center transition (kilobytes).

#### Effect-refold (subscribes to `fold_threshold`)

Reactive read of `fold_threshold.get()` is the only subscription. All other reads use `get_untracked()`:

```text
let threshold = fold_threshold.get().clamp(1, 1000)

let last = last_response.get_untracked()
let Some((cached_id, raw)) = last else { return }
if active_request.get_untracked().as_ref() != Some(&cached_id) { return }

let now = now_ms()
let mut nbhd = to_neighborhood(&raw, now, threshold)
populate_orphans(&mut nbhd, &all_dtos.get_untracked())

let one_hop_len = nbhd.one_hop.len()
let total_len = one_hop_len + nbhd.clusters.iter().map(|c| c.member_ids.len()).sum::<usize>()
let neighbor_ids: Vec<String> = nbhd.one_hop.iter().map(|n| n.id.clone()).collect()

update_graph_state_nodes_only(&gs_refold, &nbhd)
nav_refold.borrow_mut().retarget(nbhd, now, RETARGET_DURATION_MS)

set_focus_neighbors.set(neighbor_ids)
set_visible_counts.set((one_hop_len, total_len))
```

Effect-refold fires once on creation as well — at that point `last_response` is `None`, so it returns early. Subsequent firings only happen on slider input.

### 5.3 NavController.retarget

```rust
const RETARGET_DURATION_MS: u32 = 150;

impl NavController {
    /// Re-target the current center to a freshly folded neighborhood without
    /// going through Loading. Used by the detail-slider local re-fold path.
    /// Center id is unchanged; only target_positions / one_hop / clusters / edges differ.
    /// Animation duration is short so a fast drag chains many `retarget` calls smoothly.
    pub fn retarget(&mut self, to_neighborhood: Neighborhood, now_ms: f64, duration_ms: u32) {
        match std::mem::replace(&mut self.state, NavState::Idle) {
            NavState::Active { node_id, neighborhood: from } => {
                self.state = NavState::Animating {
                    from_id: node_id.clone(),
                    to_id: node_id,
                    from_neighborhood: from,
                    to_neighborhood,
                    t: 0.0,
                    duration_ms,
                    started_at_ms: now_ms,
                };
            }
            NavState::Animating { from_id, to_id, from_neighborhood, to_neighborhood: prev_to, t, .. } => {
                // Interruptible: snapshot the current interpolated frame as the new `from`
                let snapshot = build_interpolated_neighborhood(&from_neighborhood, &prev_to, t);
                self.state = NavState::Animating {
                    from_id,
                    to_id,
                    from_neighborhood: snapshot,
                    to_neighborhood,
                    t: 0.0,
                    duration_ms,
                    started_at_ms: now_ms,
                };
            }
            NavState::Loading { target, .. } => {
                // Defensive: Effect-refold guards on `last_response.is_some()`, and
                // Effect-fetch's sync prelude clears `last_response` before entering
                // Loading, so refold cannot run during Loading. Promote without
                // animation in case this is ever reached.
                self.state = NavState::Active { node_id: target, neighborhood: to_neighborhood };
            }
            NavState::Idle | NavState::Error { .. } => {
                // Defensive only: Effect-refold guards on `last_response.is_some()`,
                // which is itself only set after a successful fetch, so this branch is
                // not reached in normal flow. Promote directly without animation.
                let id = to_neighborhood.center.id.clone();
                self.state = NavState::Active { node_id: id, neighborhood: to_neighborhood };
            }
        }
    }
}
```

Notes:
- `breadcrumb` is **not** touched. Center has not changed.
- The `mem::replace` swap-out trick mirrors the pattern already used in `tick` (`navigation.rs:81-94`).
- The `Animating → Animating` branch is what makes fast drags feel smooth: each `retarget` continues from the **currently visible** frame, not from where the previous animation started.

### 5.4 `build_interpolated_neighborhood` migration

Currently a private helper in `graph_canvas.rs:76-91`. Move to `canvas_engine/tween.rs` (which already houses `lerp_node`) and make it `pub(crate)`. `graph_canvas.rs` and `navigation.rs` both `use` it.

### 5.5 `update_graph_state_nodes_only` helper

Added next to `seed_graph_state` in `mod.rs`:

```rust
/// Refresh GraphState's node/edge lists from a freshly folded Neighborhood
/// without touching viewport / scale / drag_offset / selected_node / layout.
/// Used by the slider re-fold path so the user's interaction state survives.
fn update_graph_state_nodes_only(
    gs: &Rc<RefCell<GraphState>>,
    nbhd: &Neighborhood,
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

### 5.6 PrefetchCache simplification

| Aspect | Current | New |
|---|---|---|
| Key | `(id, threshold)` | `id` |
| Value | `Neighborhood` | `(GraphNeighborsResponse, fetched_at_ms: f64)` |
| Capacity | unchanged (`CACHE_CAPACITY = 20`) | unchanged |
| TTL | unchanged (`CACHE_TTL_MS = 60_000`) | unchanged, but read from the tuple's `fetched_at_ms` instead of `Neighborhood::fetched_at_ms` |
| Hover writer (Effect 4) | folds + writes Neighborhood | writes raw response + `now_ms` |
| Click reader (Effect-fetch) | uses cached Neighborhood directly | folds raw response with current `fold_threshold.get_untracked()` |

Rationale: raw responses serve any threshold without refetch. The timestamp must be tracked alongside the value because `GraphNeighborsResponse` (in `adapter.rs:23-26`) carries no `fetched_at_ms` field, unlike `Neighborhood`.

New `PrefetchCache` API:

```rust
pub fn put(&mut self, id: String, raw: GraphNeighborsResponse, now_ms: f64);
pub fn get(&self, id: &str, now_ms: f64) -> Option<&GraphNeighborsResponse>;
pub fn has(&self, id: &str, now_ms: f64) -> bool;
```

Internal storage: `VecDeque<(String, GraphNeighborsResponse, f64)>`. TTL check compares `now_ms - entry.2 <= ttl_ms`.

## 6. Data flow

### 6.1 Slider tick (steady state)

```
toolbar slider on:input
  → set_fold_threshold(v)
  → fold_threshold signal change
  → Effect-refold fires
  → reads last_response (untracked) → has raw response for current center
  → to_neighborhood(raw, threshold) + populate_orphans         (microseconds)
  → update_graph_state_nodes_only(gs, &nbhd)                   (no viewport reset)
  → nav.retarget(nbhd, now, 150)                                (Active → Animating, or Animating → Animating with interpolated `from`)
  → rAF loop in graph_canvas.rs sees Animating, draws build_interpolated_neighborhood frames until t≥1
  → tick() snaps Animating → Active(new neighborhood)
```

No network. No Loading frame.

### 6.2 Center change (slider untouched)

```
node click / search / breadcrumb
  → set active_request(new_id)
  → Effect-fetch fires
  → last_response.set(None)
  → nav.enter(id, now)  →  NavState::Loading  →  draw_placeholder("Loading…")
  → spawn_local fetch (or prefetch cache hit)
  → on success:
      last_response.set(Some((id, raw)))
      nbhd = to_neighborhood(raw, now, fold_threshold_untracked) + populate_orphans
      seed_graph_state(...)            (full reset including viewport)
      nav.fulfilled(id, name, nbhd)    (NavState::Active, breadcrumb appended)
```

`fold_threshold` did not change → Effect-refold does not fire.

### 6.3 Slider drag during in-flight fetch (race)

```
T0  user clicks B               → Effect-fetch sync prelude: last_response = None, nav.enter(B)
T1  network in flight, screen shows Loading
T2  user drags slider           → Effect-refold fires
T3  Effect-refold: last_response is None → return early; screen stays Loading
T4  fetch returns               → last_response = Some((B, raw)), nav.fulfilled(B)
T5  user drags slider again     → Effect-refold finds last_response, re-folds and retargets normally
```

Slider movements during Loading are intentionally dropped (not queued). The user's final threshold value at the moment fetch resolves is whatever the slider DOM holds; if they keep dragging after fetch returns, the next refold uses that value. This avoids replaying stale intermediate values and matches user expectation that a Loading screen is "in progress, no feedback yet."

## 7. Edge cases and defensive handling

| Case | Handling |
|---|---|
| `fold_threshold` parse fails in toolbar | `unwrap_or(12)` already in `toolbar.rs:37` |
| `fold_threshold` out of expected range | `clamp(1, 1000)` at Effect-refold entry as belt-and-braces |
| `last_response` raw has empty `nodes` (isolated center) | `to_neighborhood` returns Neighborhood with empty one_hop/two_hop; retarget handles it (animates a center-only frame) |
| User's `selected_node` no longer in folded view (was top-K, now in cluster) | `selected_node` retained in GraphState; the detail panel keeps showing it; user can dismiss with Escape. No special handling. |
| Effect-refold runs on creation | `last_response.is_none()` → returns immediately |
| Search / breadcrumb navigation | Writes `active_request`; `fold_threshold` unchanged → only Effect-fetch fires (existing path) |
| Hover on a node whose raw response is already cached | `PrefetchCache::has(id)` short-circuit avoids redundant request |

## 8. Deferred items (registered, not implemented)

The following are intentionally out of scope for this spec. They are tracked here so a future maintainer can find the rationale and the original request.

| ID | Item | Current state | Deferral reason |
|---|---|---|---|
| D1 | `ViewMode` enum (`canvas_engine/types.rs:120-123`) kept as no-op | Enum still exists; no consumer reads it after `LegacyCanvasView` removal | Removing it cascades into `DashboardState`, `UserPrefs`, settings views. Out of scope for a slider bug fix. |
| D2 | `DashboardState::canvas_radial_navigation` kept as no-op | Field present; routing branch already unconditional | Same churn risk as D1; preserve to avoid breaking persisted `UserPrefs` |
| D3 | Deeper `Neighborhood` clone optimization | `retarget` clones the whole `Neighborhood` per slider tick | Single clone < 1 ms with n ≤ ~50 in steady state; not a bottleneck |
| D4 | Breadcrumb "global" home label rename | Label is semantically correct but verbose | Pure copy edit; bundle with next UI text pass |

D1/D2 mean: do not delete, do not reference, do not modify. Downstream readers continue to work; downstream writers (settings UI) continue to write values that are now ignored. This isolates the slider fix from unrelated UI/settings churn.

## 9. Testing strategy

### 9.1 New unit tests in `canvas_engine/navigation.rs`

```rust
#[test] fn retarget_from_active_enters_animating()
#[test] fn retarget_from_animating_snapshots_current_frame()
#[test] fn retarget_from_loading_falls_back_to_active_no_animation()
```

Each builds a minimal `Neighborhood` via the existing `nbhd(id)` helper and asserts on `state` shape after `retarget`. The snapshot test sets `t = 0.5` on a known from/to pair and confirms the new `from_neighborhood.target_positions` for a representative node id matches the lerped value (within an epsilon).

### 9.2 Existing tests

**Retained unchanged**:
- `adapter.rs`'s 4 top-K folding tests (`top_k_fold_keeps_all_when_under_threshold`, `top_k_fold_keeps_top_k_by_weight`, `top_k_fold_remainder_splits_by_category`, `to_neighborhood_basic_shape`)
- `navigation.rs`'s 5 existing tests (Idle start, enter+fulfill breadcrumb, breadcrumb max truncation, animation completes at t=1, breadcrumb_pop_to)
- `interaction.rs` keyboard tests
- `cluster.rs` and `mini_map.rs` test suites
- `prefetch.rs` HoverDebouncer tests (`debounce_fires_after_threshold`, `debounce_resets_on_target_change`)

**`prefetch.rs` cache tests rewritten** to match the id-only key shape:
- `cache_put_then_get` — adapt to new signature `put(id, raw, now)` / `get(id, now)`
- `cache_expires_after_ttl` — adapt to new signature; semantics unchanged
- `cache_evicts_oldest_at_capacity` — adapt to new signature; semantics unchanged
- `cache_miss_when_threshold_differs` — **deleted** (the threshold dimension no longer exists in the cache)
- New: `cache_serves_any_threshold_for_same_id` — verifies that after `put(id, raw, now)`, `get(id, now)` returns the raw response unchanged regardless of any external threshold value

### 9.3 Manual acceptance criteria

1. Load the canvas in the web UI. Drag the Detail slider full sweep 4 → 30 → 4. **No** "Loading…" placeholder appears at any point. Nodes visibly tween between cluster-folded and expanded states.
2. Pan the canvas, zoom in via mouse wheel, drag a node. Then drag the slider. The pan, zoom, and drag offset are preserved across the slider drag.
3. Click a new node to trigger center change. While the Loading placeholder is visible, drag the slider — nothing changes on screen, no crash. After Loading clears, drag the slider again — works normally.
4. Hover over a neighbor for the prefetch dwell duration, then click it. The neighborhood appears without a Loading frame (prefetch hit).

### 9.4 Performance check

`cargo test -p alephcore --lib canvas_engine` completes in the same time envelope as today (within 10%). The new `retarget` tests are pure logic, no async.

## 10. Files changed

```
interfaces/webchat/src/views/canvas/mod.rs               (Effect split, last_response signal, helper fns)
interfaces/webchat/src/canvas_engine/navigation.rs       (retarget method + RETARGET_DURATION_MS const + 3 tests)
interfaces/webchat/src/canvas_engine/tween.rs            (move build_interpolated_neighborhood here, pub(crate))
interfaces/webchat/src/views/canvas/graph_canvas.rs      (use tween::build_interpolated_neighborhood; delete local fn)
interfaces/webchat/src/canvas_engine/prefetch.rs         (PrefetchCache: key id-only, value GraphNeighborsResponse)
```

No other files in `views/canvas/`, `canvas_engine/`, `context.rs`, or `api/settings.rs` are modified.

## 11. Acceptance gate

- `cargo check -p alephcore` clean, zero warnings
- `cargo test -p alephcore --lib` all green, including 3 new retarget tests
- `just build` succeeds (full WASM + server release)
- Manual checklist in §9.3 passes
- Spec's §5–§7 implementation diff matches what the writing-plans plan generates
