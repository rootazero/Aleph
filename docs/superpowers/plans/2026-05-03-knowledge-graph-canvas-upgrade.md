# Canvas Knowledge Graph Upgrade — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix four shipped canvas defects (no idle drift; edges don't follow drag; orphan ring is rigid; nodes overflow viewport) without introducing a force engine or new dependencies, while deleting the dead `ForceLayout`/`RadialForceLayout`/`R_1`/`R_2`/`ORPHAN_RADIUS` code paths the radial-only redesign left behind.

**Architecture:** Surgical, renderer-side patches plus three new pure helpers (`drift_offset`, `r_one_hop`/`r_two_hop`/`r_orphan`, `Viewport::fit_to_content`). The radial layout, Top-K folding, `DragState`/`Spring2D`, `Neighborhood.target_positions`, and `NavController` machinery all stay intact — we only change how the renderer *consumes* them. The dead "legacy flat-graph" `ForceLayout`-driven rAF branch (unreachable since 2026-04-26) is removed in the same change set so the upgrade leaves the canvas codebase smaller, not larger.

**Tech Stack:** Rust 2021 + `wasm32-unknown-unknown` target. Leptos 0.8 CSR for UI scaffolding. `web-sys` `CanvasRenderingContext2d` for drawing. `js-sys` for `Math::random` and `performance.now()`. No new crates.

**Spec:** [`docs/superpowers/specs/2026-05-03-knowledge-graph-canvas-upgrade-design.md`](../specs/2026-05-03-knowledge-graph-canvas-upgrade-design.md)

**Predecessors that must not break:**
- `2026-04-25-canvas-radial-navigation-design.md`
- `2026-04-26-canvas-radial-only-redesign.md`
- `2026-04-27-canvas-elastic-node-drag-design.md` (§12 D1: no force-directed layout)

---

## File Structure

| File | Role | Change kind |
|---|---|---|
| `interfaces/webchat/src/canvas_engine/renderer.rs` | Canvas2D drawing | New `drift_offset()`; `draw_node` and `endpoints_world_pos` apply drift; `endpoints_world_pos` becomes drag-aware; `draw_orphan_ring` loses the hard-coded hint ring |
| `interfaces/webchat/src/canvas_engine/layout.rs` | Radial geometry | New `r_one_hop()`, `r_two_hop()`, `r_orphan()`; `compute_target_positions` takes `(node_count, viewport_w_px)` and uses the helpers; `R_1`/`R_2`/`ForceLayout`/`RadialForceLayout` deleted |
| `interfaces/webchat/src/canvas_engine/viewport.rs` | World/screen + pan/zoom | New `fit_to_content(&[CanvasNode], padding_pct)` |
| `interfaces/webchat/src/canvas_engine/adapter.rs` | DTO → `Neighborhood` | `populate_orphans()` body rewritten to type-cluster + golden-angle; one new private helper |
| `interfaces/webchat/src/canvas_engine/types.rs` | Shared types/constants | `ORPHAN_RADIUS` deleted |
| `interfaces/webchat/src/views/canvas/graph_canvas.rs` | rAF scheduler / pointer wiring | Resize block calls `fit_to_content`; legacy flat-graph branch and `GraphState::layout` field deleted |
| `interfaces/webchat/src/views/canvas/mod.rs` | Leptos view + effects | `seed_graph_state` calls `fit_to_content` after writing nodes |

No new files. No new crates. No new public types.

---

## Conventions

- Every task ends with running tests and a commit. Commits use the project format `<scope>: <description>` (English, lower-case). Scope is `canvas` for all tasks here.
- Build/test commands the worker needs:
  - `cargo check -p aleph-panel --target wasm32-unknown-unknown` — fastest compile gate
  - `cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::` — unit tests for canvas_engine
  - `cargo clippy -p aleph-panel --target wasm32-unknown-unknown -- -D warnings` — final lint
  - `just dev` — dev server for manual smoke
- The crate name is `aleph-panel` (the `interfaces/webchat/Cargo.toml` package name); confirm with `grep '^name' interfaces/webchat/Cargo.toml` if needed.
- All `#[cfg(test)] mod tests { ... }` blocks live alongside the code they test.
- All new helpers are `pub(crate)` unless they need to cross the canvas_engine boundary.

---

## Task 1 — `drift_offset()` helper

**Goal:** Pure function returning a per-node visual offset based on `(t_ms, node_id, amplitude_px, period_ms)`. No allocations, no global state.

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/renderer.rs`

- [ ] **Step 1.1: Write three failing tests at the bottom of `renderer.rs`**

Add this `#[cfg(test)] mod tests { ... }` block (or extend the existing one if present) at the end of `renderer.rs`:

```rust
#[cfg(test)]
mod drift_tests {
    use super::*;
    use super::types::Vec2;

    #[test]
    fn drift_is_periodic_within_epsilon() {
        let id = "node-abc";
        let amp = 5.0_f32;
        let period = 5000.0_f32;
        let a = drift_offset(0.0, id, amp, period);
        let b = drift_offset(period as f64, id, amp, period);
        assert!((a.x - b.x).abs() < 1e-3, "x not periodic: {} vs {}", a.x, b.x);
        assert!((a.y - b.y).abs() < 1e-3, "y not periodic: {} vs {}", a.y, b.y);
    }

    #[test]
    fn drift_amplitude_is_bounded() {
        let amp = 5.0_f32;
        let period = 5000.0_f32;
        for &id in &["a", "b", "long-id-with-dashes", ""] {
            for step in 0..200 {
                let t_ms = step as f64 * 137.0;
                let v = drift_offset(t_ms, id, amp, period);
                let bound = amp * 1.4143; // sqrt(2) + ε
                assert!(
                    v.x.abs() <= bound && v.y.abs() <= bound,
                    "drift exceeded amp√2 for id={id} t={t_ms}: ({},{})",
                    v.x, v.y
                );
            }
        }
    }

    #[test]
    fn drift_phases_diverge_per_node() {
        let amp = 5.0_f32;
        let period = 5000.0_f32;
        let a = drift_offset(1234.0, "node-a", amp, period);
        let b = drift_offset(1234.0, "node-z-different", amp, period);
        let dx = (a.x - b.x).abs();
        let dy = (a.y - b.y).abs();
        assert!(dx + dy > 0.5, "drift identical for distinct ids: a={a:?} b={b:?}");
    }
}
```

- [ ] **Step 1.2: Run tests — expect failure (function does not exist)**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::renderer::drift_tests
```

Expected: `error[E0425]: cannot find function 'drift_offset' in this scope`.

- [ ] **Step 1.3: Implement `drift_offset` and the constants**

Just below the `use ...` block at the top of `renderer.rs`, add the constants:

```rust
/// Idle drift amplitude (pixels). Each node oscillates within this radius around its
/// resolved (target) position. Spec §6.1, Q4 = "B / breathing" feel.
pub(crate) const DRIFT_AMPLITUDE_PX: f32 = 5.0;
/// Idle drift period in ms (one full oscillation).
pub(crate) const DRIFT_PERIOD_MS: f32 = 5000.0;
```

Then, immediately after the `Renderer` struct's `impl` block (around line 200, but anywhere private at module scope is fine), add the helper:

```rust
/// FNV-1a 32-bit. Stable, allocation-free; used for per-node phase derivation.
fn fnv1a_32_drift(bytes: &[u8]) -> u32 {
    let mut h = 0x811c9dc5_u32;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Pure visual drift for a node. Two independent sine components (x, y) using
/// the same period but different phases derived from `node_id` so adjacent nodes
/// move out of sync. Returns an offset to add to the resolved world position
/// before drawing. Never writes to node state.
pub(crate) fn drift_offset(t_ms: f64, node_id: &str, amplitude_px: f32, period_ms: f32) -> Vec2 {
    let phase = fnv1a_32_drift(node_id.as_bytes()) as f32 / u32::MAX as f32; // [0, 1)
    let omega = std::f32::consts::TAU / (period_ms / 1000.0);
    let t = (t_ms as f32) / 1000.0;
    let x = amplitude_px * (omega * t + phase * std::f32::consts::TAU).sin();
    // Use a different phase (+0.27 of full revolution) for y so the motion is not a straight line.
    let y = amplitude_px * (omega * t + (phase + 0.27) * std::f32::consts::TAU).sin();
    Vec2::new(x as f64, y as f64)
}
```

(`Vec2` already exists in `super::types`. The `f64` cast on the way out matches `Vec2`'s field type.)

- [ ] **Step 1.4: Run tests — expect pass**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::renderer::drift_tests
```

Expected: 3 passed.

- [ ] **Step 1.5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/renderer.rs
git commit -m "canvas: add drift_offset helper for per-node idle motion"
```

---

## Task 2 — `r_one_hop` / `r_two_hop` / `r_orphan` adaptive radius helpers

**Goal:** Three pure functions returning radii that grow with neighborhood size and clamp to viewport width. They replace the current `pub const R_1` / `pub const R_2` / `pub const ORPHAN_RADIUS`.

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/layout.rs`

- [ ] **Step 2.1: Write failing tests at the bottom of `layout.rs`**

Locate the existing `#[cfg(test)] mod radial_tests { ... }` block (starts around line 280). Inside it, append:

```rust
#[test]
fn r_one_hop_grows_with_node_count() {
    let small = r_one_hop(20, 800.0);
    let big = r_one_hop(500, 800.0);
    assert!(big > small * 1.5, "expected bigger n to grow R, got small={small} big={big}");
}

#[test]
fn r_one_hop_clamps_viewport() {
    // Below the lower bound (clamped to 0.6) and above the upper (1.4) should both pin.
    let narrow_low  = r_one_hop(50, 100.0);
    let narrow_min  = r_one_hop(50, 480.0);  // 480/800 = 0.6 exactly
    let wide_max    = r_one_hop(50, 1120.0); // 1120/800 = 1.4
    let wide_high   = r_one_hop(50, 4000.0);
    let eps = 1e-3;
    assert!((narrow_low - narrow_min).abs() < eps,
        "lower clamp: {narrow_low} vs {narrow_min}");
    assert!((wide_high - wide_max).abs() < eps,
        "upper clamp: {wide_high} vs {wide_max}");
}

#[test]
fn r_two_hop_outside_one_hop() {
    let one = r_one_hop(50, 800.0);
    let two = r_two_hop(50, 800.0);
    assert!(two > one, "R₂ must exceed R₁: one={one} two={two}");
}

#[test]
fn r_orphan_outside_two_hop() {
    let two = r_two_hop(50, 800.0);
    let orphan = r_orphan(50, 800.0);
    assert!(orphan > two, "R_orphan must exceed R₂: two={two} orphan={orphan}");
}
```

- [ ] **Step 2.2: Run tests — expect failure**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::layout
```

Expected: `cannot find function 'r_one_hop'` (and the others).

- [ ] **Step 2.3: Implement helpers, near the existing constants (around line 40-46)**

Replace the `pub const R_1` / `pub const R_2` lines (currently lines 41-42) with the helpers below. Leave the other `Z_*` constants as-is. The constants stay public for one task more so that intermediate compiles still work; they are deleted in Task 14 once all callers are migrated.

Insert *after* the existing `pub const Z_TWO_HOP` line:

```rust
/// Adaptive radius for a hop layer.
///
/// Grows ~ √n so the ring widens as neighborhoods densify, then is multiplied
/// by `hop` (1 for one-hop, 2 for two-hop, 2.4 for orphans) and a viewport
/// scale factor so small windows shrink and large windows expand within
/// reasonable bounds.
fn r_for_hop(hop_factor: f32, n: usize, viewport_w_px: f32) -> f32 {
    let base = 100.0_f32;
    let count_factor = (1.0 + (n as f32) / 16.0).sqrt();
    let vw_factor = (viewport_w_px / 800.0).clamp(0.6, 1.4);
    base * count_factor * vw_factor * hop_factor
}

pub fn r_one_hop(n: usize, viewport_w_px: f32) -> f32 {
    r_for_hop(1.0, n, viewport_w_px)
}

pub fn r_two_hop(n: usize, viewport_w_px: f32) -> f32 {
    r_for_hop(2.0, n, viewport_w_px)
}

pub fn r_orphan(n: usize, viewport_w_px: f32) -> f32 {
    r_for_hop(2.4, n, viewport_w_px)
}
```

- [ ] **Step 2.4: Run tests — expect pass**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::layout
```

Expected: 4 new tests passing alongside the existing radial tests.

- [ ] **Step 2.5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/layout.rs
git commit -m "canvas: add adaptive r_one_hop/r_two_hop/r_orphan helpers"
```

---

## Task 3 — `Viewport::fit_to_content`

**Goal:** Method on `Viewport` that adjusts `scale` and `offset` so a given set of `CanvasNode`s fits inside the canvas with `padding_pct` margin.

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/viewport.rs`

- [ ] **Step 3.1: Write failing tests at the bottom of `viewport.rs`**

The file already has a `#[cfg(test)] mod tests` block (`parallax_offset_proportional_to_drag` is in it). Inside that block, append:

```rust
fn make_node(x: f64, y: f64) -> CanvasNode {
    CanvasNode {
        id: "n".into(),
        name: "n".into(),
        category: "".into(),
        color: super::super::types::Color { r: 0, g: 0, b: 0 },
        radius: 6.0,
        position: Vec2::new(x, y),
        velocity: Vec2::zero(),
        pinned: false,
        z: 0.0,
        hop: 1,
        decay_score: 1.0,
        edge_count: 0,
    }
}

#[test]
fn fit_to_content_empty_is_no_op() {
    let mut v = Viewport::new(800.0, 600.0);
    let before = (v.scale, v.offset.x, v.offset.y);
    v.fit_to_content(&[], 0.10);
    assert_eq!((v.scale, v.offset.x, v.offset.y), before);
}

#[test]
fn fit_to_content_centres_single_node() {
    let mut v = Viewport::new(800.0, 600.0);
    v.fit_to_content(&[make_node(123.0, -45.0)], 0.10);
    // Single node has zero bbox → falls back to scale 1, offset centred on node.
    let cx_world = (v.width / 2.0 - v.offset.x) / v.scale;
    let cy_world = (v.height / 2.0 - v.offset.y) / v.scale;
    assert!((cx_world - 123.0).abs() < 1.0);
    assert!((cy_world + 45.0).abs() < 1.0);
}

#[test]
fn fit_to_content_padding_respected() {
    let mut v = Viewport::new(800.0, 600.0);
    let nodes = vec![make_node(-100.0, -100.0), make_node(100.0, 100.0)];
    v.fit_to_content(&nodes, 0.10);
    // bbox = 200×200, padded to 220×220 → scale ≤ min(800/220, 600/220) ≈ 2.727.
    assert!(v.scale <= 2.728, "scale too large: {}", v.scale);
    assert!(v.scale >= 0.2);
}

#[test]
fn fit_to_content_clamps_scale() {
    let mut v = Viewport::new(800.0, 600.0);
    // Tiny bbox → would compute very high scale; must clamp at 3.0.
    v.fit_to_content(&[make_node(0.0, 0.0), make_node(0.5, 0.5)], 0.10);
    assert!(v.scale <= 3.0 + 1e-6, "expected scale ≤ 3.0, got {}", v.scale);

    // Massive bbox → would compute very low scale; must clamp at 0.2.
    let mut v2 = Viewport::new(800.0, 600.0);
    v2.fit_to_content(
        &[make_node(-100_000.0, -100_000.0), make_node(100_000.0, 100_000.0)],
        0.10,
    );
    assert!(v2.scale >= 0.2 - 1e-6, "expected scale ≥ 0.2, got {}", v2.scale);
}
```

(The `make_node` helper imports `CanvasNode` from `super::super::types`. If `Color` is not pub there, swap it for whatever the existing tests construct nodes from. The existing parallax test does not build a `CanvasNode`, so this helper is new — adjust imports if compile complains.)

- [ ] **Step 3.2: Run tests — expect failure**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::viewport
```

Expected: `no method named 'fit_to_content'`.

- [ ] **Step 3.3: Implement `fit_to_content`**

Inside `impl Viewport { ... }` (the existing block), add:

```rust
/// Scale + recentre so all `nodes` fit inside the canvas with `padding_pct`
/// extra margin on every side. No-op for an empty slice. `padding_pct` is a
/// fraction (0.10 = 10 %).
///
/// Scale is clamped to `[0.2, 3.0]` so degenerate inputs (single point, vast
/// outliers) cannot pin the user at unusable zoom levels.
pub fn fit_to_content(&mut self, nodes: &[CanvasNode], padding_pct: f32) {
    if nodes.is_empty() {
        return;
    }
    let (mut min_x, mut min_y) = (f64::MAX, f64::MAX);
    let (mut max_x, mut max_y) = (f64::MIN, f64::MIN);
    for n in nodes {
        min_x = min_x.min(n.position.x);
        min_y = min_y.min(n.position.y);
        max_x = max_x.max(n.position.x);
        max_y = max_y.max(n.position.y);
    }
    // Avoid div-by-zero for degenerate (single-node) bboxes.
    let bbox_w = (max_x - min_x).max(1.0);
    let bbox_h = (max_y - min_y).max(1.0);
    let pad = 1.0 + padding_pct as f64;
    let scale_x = self.width / (bbox_w * pad);
    let scale_y = self.height / (bbox_h * pad);
    self.scale = scale_x.min(scale_y).clamp(0.2, 3.0);
    let cx = (min_x + max_x) * 0.5;
    let cy = (min_y + max_y) * 0.5;
    self.offset.x = self.width / 2.0 - cx * self.scale;
    self.offset.y = self.height / 2.0 - cy * self.scale;
}
```

- [ ] **Step 3.4: Run tests — expect pass**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::viewport
```

Expected: 4 new tests passing alongside `parallax_offset_proportional_to_drag`.

- [ ] **Step 3.5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/viewport.rs
git commit -m "canvas: add Viewport::fit_to_content for auto-fit on data load"
```

---

## Task 4 — Migrate `compute_target_positions` to adaptive radii

**Goal:** Change `compute_target_positions`'s signature to accept node count and viewport width, route through `r_one_hop` / `r_two_hop`, and update every call site (including tests). The legacy `pub const R_1` / `R_2` are still in the file but become unused after this task; they get deleted in Task 14.

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/layout.rs`
- Modify: `interfaces/webchat/src/canvas_engine/adapter.rs`

- [ ] **Step 4.1: Add the new signature alongside the old**

In `layout.rs`, find the existing `pub fn compute_target_positions(active, one_hop, two_hop, clusters, edges) -> HashMap<...>`. We will:
1. Rename it to `compute_target_positions_internal` and make it `pub(crate)` for the moment.
2. Add a new `pub fn compute_target_positions(active, one_hop, two_hop, clusters, edges, viewport_w_px: f32) -> HashMap<...>` that calls the internal with the same logic but uses the new helpers.

Concretely, replace the existing function body's `let r1 = R_1;` line (currently around line 76) and the `R_1` / `R_2` references in the 2-hop block (lines 116, 121, 122) so the function takes `viewport_w_px: f32` as a new last argument and computes:

```rust
pub fn compute_target_positions(
    active: &CanvasNode,
    one_hop: &[CanvasNode],
    two_hop: &[CanvasNode],
    clusters: &[ClusterNode],
    edges: &[CanvasEdge],
    viewport_w_px: f32,
) -> HashMap<String, Vec3> {
    // ... existing setup unchanged through the by_relation / sector_centers section ...

    // Replace this line:
    //     let r1 = R_1;
    // with:
    let total_visible = one_hop.len() + clusters.len() + two_hop.len();
    let r1 = r_one_hop(total_visible, viewport_w_px);
    let r2 = r_two_hop(total_visible, viewport_w_px);

    // ... existing one-hop placement loop using r1 unchanged ...

    // In the two-hop loop, replace the `(R_1, 0.0)` fallback and the two
    // `R_2 * theta.cos()` / `R_2 * theta.sin()` references with `r1` and `r2`:
    let (px, py) = match parent_pos {
        Some(p) => (p.x, p.y),
        None => (r1, 0.0),
    };
    let parent_angle = py.atan2(px);
    let jitter = (fnv1a_32(n.id.as_bytes()) as f32 / u32::MAX as f32 - 0.5) * 0.6;
    let theta = parent_angle + jitter;
    let x = r2 * theta.cos();
    let y = r2 * theta.sin();
    out.insert(n.id.clone(), Vec3::new(x, y, Z_TWO_HOP));
    // ... unchanged tail ...
}
```

- [ ] **Step 4.2: Update the production caller in `adapter.rs`**

The only production call site is `interfaces/webchat/src/canvas_engine/adapter.rs:128-129`:

```rust
let target_positions =
    compute_target_positions(&center, &filtered_one_hop, &two_hop, &clusters, &edges);
```

Change to:

```rust
let viewport_w_px = 800.0_f32; // adapter has no viewport at this layer; pass nominal default,
                               // which is the centre of the clamp range. Real fit happens via
                               // viewport::fit_to_content after seeding (Task 9).
let target_positions = compute_target_positions(
    &center, &filtered_one_hop, &two_hop, &clusters, &edges, viewport_w_px,
);
```

(`adapter.rs` does not import `Viewport`. Passing the nominal 800 is intentional — the *initial* target positions only need to be plausible; the real visible scale comes from `Viewport::fit_to_content` invoked from the canvas-level code after `seed_graph_state`.)

- [ ] **Step 4.3: Update test call sites in `layout.rs`**

There are five test-only call sites in `layout.rs` (currently lines 317, 328, 343, 423, 442). For each, append `, 800.0` as the new argument. Example:

```rust
// Before:
let targets = compute_target_positions(&active, &one_hop, &[], &[], &edges);
// After:
let targets = compute_target_positions(&active, &one_hop, &[], &[], &edges, 800.0);
```

Run `grep -n "compute_target_positions(" interfaces/webchat/src/canvas_engine/layout.rs` to find every call and verify they all pass six arguments.

- [ ] **Step 4.4: Run tests — expect pass**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::
```

Expected: existing radial tests still green; no new failures introduced. If a test asserts an exact `R_1` value, weaken the assertion to `(r_one_hop(n, 800.0) - 1.0)..=(r_one_hop(n, 800.0) + 1.0)` or compute the expected radius via the helper.

- [ ] **Step 4.5: Run a wide build to catch any other caller**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
```

Expected: clean compile.

- [ ] **Step 4.6: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/layout.rs interfaces/webchat/src/canvas_engine/adapter.rs
git commit -m "canvas: thread viewport-aware adaptive radii through compute_target_positions"
```

---

## Task 5 — Apply drift in `draw_node`

**Goal:** Every drawn node visibly drifts at idle. Active centre included (per spec §6.1). Drag overlay path is unaffected (the dragged node skips this code path via the existing `if node_drag.map(|o| o.node_id == n.id).unwrap_or(false) { continue; }` guards in `draw_neighborhood` at renderer.rs:301 and 307).

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/renderer.rs`

- [ ] **Step 5.1: Locate `draw_node` and inject drift**

`draw_node` starts at `renderer.rs:397`. The relevant lines are:

```rust
let off = crate::canvas_engine::viewport::parallax_offset(n.z, drag.0, drag.1);
let cx = n.position.x as f32 + off.0;
let cy = n.position.y as f32 + off.1;
```

Replace with:

```rust
let off = crate::canvas_engine::viewport::parallax_offset(n.z, drag.0, drag.1);
let drift = drift_offset(
    now_ms_in_seconds() * 1000.0,
    &n.id,
    DRIFT_AMPLITUDE_PX,
    DRIFT_PERIOD_MS,
);
let cx = n.position.x as f32 + off.0 + drift.x as f32;
let cy = n.position.y as f32 + off.1 + drift.y as f32;
```

(`now_ms_in_seconds` already exists at `renderer.rs:702`, returning seconds — multiply by 1000 to get ms for `drift_offset`.)

- [ ] **Step 5.2: Same change in `draw_orphan_ring` for orphan dots**

`draw_orphan_ring` at `renderer.rs:337` draws each orphan as a dot. The dot position calc currently is:

```rust
let off = crate::canvas_engine::viewport::parallax_offset(n.z, drag.0, drag.1);
let cx = n.position.x + off.0 as f64;
let cy = n.position.y + off.1 as f64;
```

Replace with:

```rust
let off = crate::canvas_engine::viewport::parallax_offset(n.z, drag.0, drag.1);
let drift = drift_offset(
    now_ms_in_seconds() * 1000.0,
    &n.id,
    DRIFT_AMPLITUDE_PX,
    DRIFT_PERIOD_MS,
);
let cx = n.position.x + off.0 as f64 + drift.x;
let cy = n.position.y + off.1 as f64 + drift.y;
```

- [ ] **Step 5.3: Compile + manual smoke**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
just dev
```

In a browser at the `/memory` route, observe: at idle, every node visibly drifts ~5 px in a slow oscillation; phase varies per node. Hovering and selecting still highlight correctly. Active centre also drifts (centre is no longer perfectly still — confirm this is the intended look from the spec).

- [ ] **Step 5.4: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/renderer.rs
git commit -m "canvas: apply per-node drift in draw_node and draw_orphan_ring (bug #1)"
```

---

## Task 6 — Apply drift in `endpoints_world_pos`

**Goal:** Edges' endpoints follow the same drift, so the line continues to attach visibly to its node. We extend `endpoints_world_pos` to take `t_ms: f64` and apply `drift_offset` to each endpoint. Drag-aware override comes in Task 7.

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/renderer.rs`

- [ ] **Step 6.1: Locate the function**

`endpoints_world_pos` is at `renderer.rs:737`. Current signature:

```rust
fn endpoints_world_pos(
    e: &CanvasEdge,
    nbhd: &Neighborhood,
    drag: (f32, f32),
) -> Option<((f32, f32), (f32, f32), f32, f32)> { ... }
```

Inside it the `resolve` closure returns `Option<Vec3>` from `nbhd.target_positions`. We need access to the resolved id too so we can compute drift.

- [ ] **Step 6.2: Refactor + add `t_ms` parameter**

Replace the function with:

```rust
fn endpoints_world_pos(
    e: &CanvasEdge,
    nbhd: &Neighborhood,
    drag: (f32, f32),
    t_ms: f64,
) -> Option<((f32, f32), (f32, f32), f32, f32)> {
    let resolve = |idx: usize| -> Option<(&str, Vec3)> {
        if idx == 0 {
            let id = nbhd.center.id.as_str();
            nbhd.target_positions.get(id).copied().map(|p| (id, p))
        } else if idx <= nbhd.one_hop.len() {
            let n = &nbhd.one_hop[idx - 1];
            nbhd.target_positions
                .get(n.id.as_str())
                .copied()
                .map(|p| (n.id.as_str(), p))
        } else {
            let off = idx - 1 - nbhd.one_hop.len();
            let n = nbhd.two_hop.get(off)?;
            nbhd.target_positions
                .get(n.id.as_str())
                .copied()
                .map(|p| (n.id.as_str(), p))
        }
    };
    let (id1, p1) = resolve(e.from_idx)?;
    let (id2, p2) = resolve(e.to_idx)?;
    let off1 = crate::canvas_engine::viewport::parallax_offset(p1.z, drag.0, drag.1);
    let off2 = crate::canvas_engine::viewport::parallax_offset(p2.z, drag.0, drag.1);
    let d1 = drift_offset(t_ms, id1, DRIFT_AMPLITUDE_PX, DRIFT_PERIOD_MS);
    let d2 = drift_offset(t_ms, id2, DRIFT_AMPLITUDE_PX, DRIFT_PERIOD_MS);
    Some((
        (p1.x + off1.0 + d1.x as f32, p1.y + off1.1 + d1.y as f32),
        (p2.x + off2.0 + d2.x as f32, p2.y + off2.1 + d2.y as f32),
        p1.z,
        p2.z,
    ))
}
```

- [ ] **Step 6.3: Update the single caller**

`draw_edges_for_node` at `renderer.rs:601` calls `endpoints_world_pos(e, nbhd, drag)`. Update to pass current ms:

```rust
let endpoints = endpoints_world_pos(e, nbhd, drag, now_ms_in_seconds() * 1000.0);
```

- [ ] **Step 6.4: Compile + smoke**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
just dev
```

Watch idle: edges' endpoints follow the drift of their nodes. No "broken cable" visual.

- [ ] **Step 6.5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/renderer.rs
git commit -m "canvas: drift edge endpoints in sync with their nodes"
```

---

## Task 7 — Drag-aware edge endpoints

**Goal:** When `DragOverlay::Some(o)` is in flight, any edge whose endpoint id matches `o.node_id` substitutes `o.position` for that endpoint (instead of the drifted target position). Bug #2 fix.

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/renderer.rs`

- [ ] **Step 7.1: Extend `endpoints_world_pos` signature**

Add an `node_drag: Option<&DragOverlay>` parameter. Replace the function from Task 6 with:

```rust
fn endpoints_world_pos(
    e: &CanvasEdge,
    nbhd: &Neighborhood,
    drag: (f32, f32),
    node_drag: Option<&DragOverlay>,
    t_ms: f64,
) -> Option<((f32, f32), (f32, f32), f32, f32)> {
    let resolve = |idx: usize| -> Option<(&str, Vec3)> {
        if idx == 0 {
            let id = nbhd.center.id.as_str();
            nbhd.target_positions.get(id).copied().map(|p| (id, p))
        } else if idx <= nbhd.one_hop.len() {
            let n = &nbhd.one_hop[idx - 1];
            nbhd.target_positions.get(n.id.as_str()).copied().map(|p| (n.id.as_str(), p))
        } else {
            let off = idx - 1 - nbhd.one_hop.len();
            let n = nbhd.two_hop.get(off)?;
            nbhd.target_positions.get(n.id.as_str()).copied().map(|p| (n.id.as_str(), p))
        }
    };
    let (id1, p1) = resolve(e.from_idx)?;
    let (id2, p2) = resolve(e.to_idx)?;

    let resolve_endpoint = |id: &str, p: Vec3| -> (f32, f32, f32) {
        if let Some(o) = node_drag {
            if o.node_id == id {
                return (o.position.x as f32, o.position.y as f32, p.z);
            }
        }
        let off = crate::canvas_engine::viewport::parallax_offset(p.z, drag.0, drag.1);
        let d = drift_offset(t_ms, id, DRIFT_AMPLITUDE_PX, DRIFT_PERIOD_MS);
        (p.x + off.0 + d.x as f32, p.y + off.1 + d.y as f32, p.z)
    };

    let (x1, y1, z1) = resolve_endpoint(id1, p1);
    let (x2, y2, z2) = resolve_endpoint(id2, p2);
    Some(((x1, y1), (x2, y2), z1, z2))
}
```

- [ ] **Step 7.2: Update `draw_edges_for_node` signature and body**

`draw_edges_for_node` at `renderer.rs:601`. Change signature to accept the overlay and propagate to `endpoints_world_pos`:

```rust
fn draw_edges_for_node(
    ctx: &CanvasRenderingContext2d,
    n: &CanvasNode,
    nbhd: &Neighborhood,
    drag: (f32, f32),
    node_drag: Option<&DragOverlay>,
) {
    for e in &nbhd.edges {
        let endpoints = endpoints_world_pos(e, nbhd, drag, node_drag, now_ms_in_seconds() * 1000.0);
        // ... unchanged tail ...
```

- [ ] **Step 7.3: Update the two callers in `draw_neighborhood`**

In `draw_neighborhood` at `renderer.rs:267`, lines 289 and 304 currently call `draw_edges_for_node(ctx, n, nbhd, drag)`. Change both to pass the overlay:

```rust
draw_edges_for_node(ctx, n, nbhd, drag, node_drag);
```

Also remove the now-redundant guard:

```rust
// Skip edges for the dragged node — we'll draw a stretched edge instead.
if node_drag.map(|o| o.node_id == n.id).unwrap_or(false) {
    continue;
}
draw_edges_for_node(ctx, n, nbhd, drag);
```

(currently lines 300-304). The new behaviour is: edges *to* the dragged node should follow the overlay (Bug #2 is fixed). The "stretched edge from centre" still rendered separately by `draw_dragged_node` at line 766; that remains as-is to indicate promote intent.

After removal, the loop becomes simply:

```rust
for n in &nbhd.one_hop {
    draw_edges_for_node(ctx, n, nbhd, drag, node_drag);
}
```

- [ ] **Step 7.4: Compile + smoke**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
just dev
```

Manually: drag a 1-hop neighbor. Confirm:
1. The connecting edge from the centre to that node visibly tracks the cursor — no broken-cable gap.
2. Any other edges *between* that node and 2-hop neighbours (if such edges exist in the displayed neighborhood) likewise track.
3. Releasing into spring-back: the edge follows the spring smoothly back to rest.
4. Promote-on-drag: the edge tweens with the node into the centre; navigation triggers.

- [ ] **Step 7.5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/renderer.rs
git commit -m "canvas: edge endpoints follow drag overlay (bug #2)"
```

---

## Task 8 — Type-clustered orphan placement

**Goal:** Replace the rigid `R = ORPHAN_RADIUS` ring with kind-grouped golden-angle discs. Orphans no longer set `pinned = true`, so they participate in idle drift like any other node.

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/adapter.rs`

- [ ] **Step 8.1: Write failing tests**

In the existing `#[cfg(test)] mod tests { ... }` block of `adapter.rs` (starts around line 315), append:

```rust
fn dto_kind(id: &str, kind: &str) -> NoteNodeDto {
    NoteNodeDto {
        id: id.to_string(),
        name: id.to_string(),
        path: format!("{id}.md"),
        category: kind.to_string(),
        tags: vec![],
        link_count: 1,
    }
}

#[test]
fn populate_orphans_groups_by_kind() {
    use crate::canvas_engine::types::Vec2;
    let mut nbhd = Neighborhood {
        center: note_dto_to_canvas(&dto_kind("c", "centre"), 0),
        one_hop: vec![],
        two_hop: vec![],
        orphans: vec![],
        clusters: vec![],
        edges: vec![],
        target_positions: Default::default(),
        fetched_at_ms: 0.0,
    };
    let all = vec![
        dto_kind("a1", "concept"),
        dto_kind("a2", "concept"),
        dto_kind("b1", "person"),
        dto_kind("b2", "person"),
        dto_kind("b3", "person"),
        dto_kind("c1", "tool"),
    ];
    populate_orphans(&mut nbhd, &all);

    // All orphans returned, no pinning.
    assert_eq!(nbhd.orphans.len(), 6);
    for o in &nbhd.orphans {
        assert!(!o.pinned, "orphan {} is pinned", o.id);
    }

    // Same-kind orphans are tightly grouped (within ~ √n·16 + ε).
    let group = |kind: &str| -> Vec<Vec2> {
        nbhd.orphans
            .iter()
            .filter(|o| o.category == kind)
            .map(|o| o.position)
            .collect()
    };
    let person = group("person");
    assert_eq!(person.len(), 3);
    let cx = person.iter().map(|p| p.x).sum::<f64>() / 3.0;
    let cy = person.iter().map(|p| p.y).sum::<f64>() / 3.0;
    for p in &person {
        let d = ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt();
        // Golden-angle disc with j∈{0,1,2}, radius = √(j+1)·16 → max ≈ √3·16 ≈ 27.7.
        assert!(d < 60.0, "person orphan {} too far from cluster centre: d={d}", p.x);
    }
}

#[test]
fn populate_orphans_distinct_kinds_separated() {
    let mut nbhd = Neighborhood {
        center: note_dto_to_canvas(&dto_kind("c", "centre"), 0),
        one_hop: vec![],
        two_hop: vec![],
        orphans: vec![],
        clusters: vec![],
        edges: vec![],
        target_positions: Default::default(),
        fetched_at_ms: 0.0,
    };
    let all = vec![
        dto_kind("a", "kind-a"),
        dto_kind("b", "kind-b"),
        dto_kind("c1", "kind-c"),
    ];
    populate_orphans(&mut nbhd, &all);

    // For 3 kinds the angular separation between consecutive cluster centres
    // (sorted by angle) should be ≥ TAU/(2·K) = ~60°.
    let mut angles: Vec<f64> = nbhd
        .orphans
        .iter()
        .map(|o| o.position.y.atan2(o.position.x))
        .collect();
    angles.sort_by(|a, b| a.partial_cmp(b).unwrap());
    for w in angles.windows(2) {
        let gap = (w[1] - w[0]).abs();
        assert!(gap > std::f64::consts::PI / 3.0 - 0.1, "clusters too close: gap={gap}");
    }
}

#[test]
fn populate_orphans_writes_target_positions() {
    let mut nbhd = Neighborhood {
        center: note_dto_to_canvas(&dto_kind("c", "centre"), 0),
        one_hop: vec![],
        two_hop: vec![],
        orphans: vec![],
        clusters: vec![],
        edges: vec![],
        target_positions: Default::default(),
        fetched_at_ms: 0.0,
    };
    let all = vec![dto_kind("only", "kind")];
    populate_orphans(&mut nbhd, &all);
    assert!(nbhd.target_positions.contains_key("only"));
}
```

- [ ] **Step 8.2: Run tests — expect failure**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::adapter
```

Expected: assertions about pinning / clustering fail (current ring layout is not type-grouped and pins everything).

- [ ] **Step 8.3: Rewrite `populate_orphans` body**

Replace the body of `populate_orphans` (currently `adapter.rs:176-227`) with the type-cluster placement. The function signature stays the same:

```rust
pub fn populate_orphans(nbhd: &mut Neighborhood, all_dtos: &[NoteNodeDto]) {
    use std::collections::{BTreeMap, HashSet};

    let mut in_view: HashSet<&str> = HashSet::new();
    in_view.insert(nbhd.center.id.as_str());
    for n in &nbhd.one_hop { in_view.insert(n.id.as_str()); }
    for n in &nbhd.two_hop { in_view.insert(n.id.as_str()); }
    for c in &nbhd.clusters {
        for id in &c.member_ids { in_view.insert(id.as_str()); }
    }

    let orphan_dtos: Vec<&NoteNodeDto> = all_dtos
        .iter()
        .filter(|d| !in_view.contains(d.id.as_str()))
        .collect();

    if orphan_dtos.is_empty() {
        nbhd.orphans = Vec::new();
        return;
    }

    // Bucket by kind (== `category`). BTreeMap gives stable iteration order.
    let mut by_kind: BTreeMap<String, Vec<&NoteNodeDto>> = BTreeMap::new();
    for d in &orphan_dtos {
        by_kind.entry(d.category.clone()).or_default().push(d);
    }

    let kind_count = by_kind.len().max(1) as f64;
    // Adaptive orphan ring radius (consistent with hop layout). The adapter has no
    // viewport at this layer; pass nominal 800 — viewport::fit_to_content corrects
    // the visible scale after seeding.
    let orphan_count = orphan_dtos.len();
    let r_ring = crate::canvas_engine::layout::r_orphan(orphan_count, 800.0) as f64;

    let golden = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt()); // ≈ 137.5°

    let mut orphans = Vec::with_capacity(orphan_count);

    for (i, (kind, members)) in by_kind.into_iter().enumerate() {
        // Cluster-centre angle: even slots, slightly perturbed by hash so the
        // pattern differs across kinds. Stable across sessions.
        let h = fnv1a_32(kind.as_bytes()) as f64 / u32::MAX as f64;
        let center_angle =
            (i as f64 / kind_count) * std::f64::consts::TAU + h * 0.5;
        let cx = r_ring * center_angle.cos();
        let cy = r_ring * center_angle.sin();

        for (j, dto) in members.into_iter().enumerate() {
            let angle = j as f64 * golden;
            // √(j+1)·16: golden-angle disc with denser centre.
            let radius = (j as f64 + 1.0).sqrt() * 16.0;
            let x = cx + radius * angle.cos();
            let y = cy + radius * angle.sin();

            let mut node = note_dto_to_canvas(dto, ORPHAN_HOP_SENTINEL);
            node.position = Vec2::new(x, y);
            node.z = ORPHAN_Z;
            // No `pinned = true` — orphans drift like everyone else.
            node.radius = 4.5;
            orphans.push(node);

            nbhd.target_positions
                .insert(dto.id.clone(), Vec3::new(x as f32, y as f32, ORPHAN_Z));
        }
    }

    nbhd.orphans = orphans;
}
```

`fnv1a_32` is already in `layout.rs` and used by `compute_target_positions`. Either re-export it (`pub(crate)` in `layout.rs`) or copy the 6-line function locally as `fnv1a_32` in `adapter.rs`. The simpler path is `pub(crate) fn fnv1a_32(...)` in `layout.rs` and `use crate::canvas_engine::layout::fnv1a_32;` here.

- [ ] **Step 8.4: Run tests — expect pass**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::adapter
```

Expected: all three new tests green; existing tests unaffected.

- [ ] **Step 8.5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/adapter.rs interfaces/webchat/src/canvas_engine/layout.rs
git commit -m "canvas: type-cluster + golden-angle orphan placement (bug #3)"
```

---

## Task 9 — Wire `fit_to_content` into `seed_graph_state`

**Goal:** When a fresh `Neighborhood` is loaded, the viewport scales to fit it. `seed_graph_state` is the single function that writes nodes into `GraphState` after a fetch (called from three Effects in `views/canvas/mod.rs`); adding `fit_to_content` here covers entry-load, click-navigate, and breadcrumb-nav uniformly.

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/mod.rs`

- [ ] **Step 9.1: Find and read `seed_graph_state`**

It is at `interfaces/webchat/src/views/canvas/mod.rs:675`. Open and read it; near the end of the function it writes `state.nodes`/`state.edges`. After those writes is where the new call goes.

- [ ] **Step 9.2: Append `fit_to_content` at end of `seed_graph_state`**

Just before the function returns (after the line that finalises `state.edges`), add:

```rust
// Auto-fit viewport to the freshly seeded layout so we don't lose nodes off-screen.
// Consider all visible nodes (centre + one_hop + two_hop + cluster bubbles + orphans).
{
    let mut to_fit: Vec<CanvasNode> = Vec::new();
    to_fit.push(state.nodes[0].clone()); // centre is index 0 in seed order
    to_fit.extend(state.nodes.iter().skip(1).cloned());
    state.viewport.fit_to_content(&to_fit, 0.10);
}
```

If `seed_graph_state`'s exact symbol map differs (e.g., it builds nodes incrementally), keep the *intent*: after the function has finished mutating the GraphState, call `state.viewport.fit_to_content(&state.nodes, 0.10)`. Use the simplest form that satisfies the borrow checker.

- [ ] **Step 9.3: Compile + smoke**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
just dev
```

Open `/memory`. Confirm:
- Initial load: viewport fits all visible nodes with ~10 % padding.
- Click a 1-hop neighbour to promote: viewport refits to the new neighborhood after the tween settles.
- Resize window mid-session: layout still appears centred (the resize-block fit comes in Task 10).

- [ ] **Step 9.4: Commit**

```bash
git add interfaces/webchat/src/views/canvas/mod.rs
git commit -m "canvas: fit viewport to neighborhood on seed (bug #4 part 1/2)"
```

---

## Task 10 — Wire `fit_to_content` into resize block

**Goal:** When the canvas's parent element resizes, after `state.viewport.{width,height,offset}` are updated, refit content so the user does not need to reload to see the full graph.

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/graph_canvas.rs`

- [ ] **Step 10.1: Update the resize block**

The block is at `graph_canvas.rs:179-192`. Currently:

```rust
if pw > 1.0 && ph > 1.0 {
    let cur_w = canvas_for_resize.width() as f64 / dpr;
    let cur_h = canvas_for_resize.height() as f64 / dpr;
    if (pw - cur_w).abs() > 1.0 || (ph - cur_h).abs() > 1.0 {
        canvas_for_resize.set_width((pw * dpr) as u32);
        canvas_for_resize.set_height((ph * dpr) as u32);
        let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
        state.viewport.width = pw;
        state.viewport.height = ph;
        state.viewport.offset.x = pw / 2.0;
        state.viewport.offset.y = ph / 2.0;
    }
}
```

Replace with (only the `if (pw - cur_w).abs() ...` branch needs updating):

```rust
if pw > 1.0 && ph > 1.0 {
    let cur_w = canvas_for_resize.width() as f64 / dpr;
    let cur_h = canvas_for_resize.height() as f64 / dpr;
    if (pw - cur_w).abs() > 1.0 || (ph - cur_h).abs() > 1.0 {
        canvas_for_resize.set_width((pw * dpr) as u32);
        canvas_for_resize.set_height((ph * dpr) as u32);
        let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
        state.viewport.width = pw;
        state.viewport.height = ph;
        state.viewport.offset.x = pw / 2.0;
        state.viewport.offset.y = ph / 2.0;
        // Refit content to the new canvas size. Only when the radial branch is
        // active and a neighborhood is loaded (otherwise nodes is empty).
        if !state.nodes.is_empty() {
            state.viewport.fit_to_content(&state.nodes, 0.10);
        }
    }
}
```

- [ ] **Step 10.2: Compile + smoke**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
just dev
```

Open `/memory`. Resize the browser window narrow then wide; confirm content refits in both directions.

- [ ] **Step 10.3: Commit**

```bash
git add interfaces/webchat/src/views/canvas/graph_canvas.rs
git commit -m "canvas: fit viewport on resize (bug #4 part 2/2)"
```

---

## Task 11 — Delete `ForceLayout` and `RadialForceLayout`

**Goal:** Remove the 200+ lines of unreachable force-layout code. The radial path returns early at `graph_canvas.rs:292`; `ForceLayout::tick` is never called by shipped code. `RadialForceLayout` is referenced only by `layout.rs`'s own tests.

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/layout.rs`

- [ ] **Step 11.1: Confirm zero callers in production**

```bash
grep -rn "ForceLayout\|RadialForceLayout" interfaces/webchat/src/
```

Expected hits:
- `canvas_engine/layout.rs` (definitions + their own tests)
- `views/canvas/graph_canvas.rs` (the `GraphState::layout: ForceLayout` field — that goes in Task 12)

If any other file references them, stop and reassess — the spec assumes none do.

- [ ] **Step 11.2: Delete the structs and their impls**

In `layout.rs`, delete:
- `LayoutConfig` struct + `Default` impl (currently around lines 166-186; only used by `ForceLayout`)
- `pub struct ForceLayout { ... }` (line 188-191)
- `impl ForceLayout { ... }` (lines 193-272)
- `impl Default for ForceLayout { ... }` (lines 274-278)
- `pub struct RadialForceLayout { ... }` and its `impl` (lines 475-578)
- Any test inside `mod radial_tests` that uses `RadialForceLayout` (the test starting at line 442 with `let mut layout = RadialForceLayout::new(...)`)

Check the existing test names with `grep -n "fn " layout.rs` and remove any test that no longer compiles after the structs go away. Tests that only use `compute_target_positions` stay.

- [ ] **Step 11.3: Compile**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
```

Expected: error pointing at `views/canvas/graph_canvas.rs:13` (`use crate::canvas_engine::layout::ForceLayout;`) and `:25` / `:45` (the `GraphState::layout` field). That is fixed in the next task.

- [ ] **Step 11.4: Defer compile success — commit a WIP**

The deletion is most cleanly atomic when paired with Task 12. Do *not* commit yet; keep the changes staged and proceed straight to Task 12. (If your worker prefers atomic commits over compiling green at every step, that's the call here. Commit text below combines both.)

---

## Task 12 — Delete `GraphState::layout` field and the legacy flat-graph rAF branch

**Goal:** Remove the `ForceLayout`-typed field on `GraphState` and the unreachable rAF branch that uses it. After this, `cargo check` is green again.

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/graph_canvas.rs`

- [ ] **Step 12.1: Delete the import and field**

`graph_canvas.rs:13`:
```rust
use crate::canvas_engine::layout::ForceLayout;
```
Delete.

`graph_canvas.rs:25` (inside `pub struct GraphState`):
```rust
pub layout: ForceLayout,
```
Delete.

`graph_canvas.rs:45` (inside `GraphState::new`):
```rust
layout: ForceLayout::new(),
```
Delete.

- [ ] **Step 12.2: Delete the legacy rAF branch**

After the `// Radial nav branch: NavState-aware rendering` block returns at `graph_canvas.rs:292`, the code falls through to a `// Legacy flat-graph branch (no nav controller)` block starting around line 295. Find that comment and delete from it down to the next `// Schedule next frame` block (the one that follows the legacy render). Verify no other code in the closure depends on what was in that block.

The legacy branch is the only code path that calls `state.layout.tick(...)` and the legacy `Renderer::draw(...)`. Both are dead in shipped code (the radial branch always wins). Use `grep -n "Renderer::draw\b" graph_canvas.rs` afterwards — should return zero hits. The `Renderer` impl in `renderer.rs` itself stays for now (its `draw_edges` / `draw_nodes` methods are unused but only `pub(crate)` visible; clippy will warn, which we silence in Step 12.4).

- [ ] **Step 12.3: Verify**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
```

Expected: clean. `cargo clippy ... -- -D warnings` may now complain about unused `Renderer::draw` / `Renderer::draw_edges` / `Renderer::draw_nodes` (they were callers of the legacy branch). If so, delete the `impl Renderer` block in `renderer.rs` lines 13-265 (everything inside `impl Renderer`). The `pub struct Renderer;` declaration becomes useless too — delete it as well. The radial path uses standalone `pub fn draw_neighborhood` (line 267), which stays. Update the import at the top of `graph_canvas.rs` if it imports `Renderer`.

- [ ] **Step 12.4: Confirm clippy clean**

```bash
cargo clippy -p aleph-panel --target wasm32-unknown-unknown -- -D warnings
```

Expected: clean.

- [ ] **Step 12.5: Commit Tasks 11 + 12 atomically**

```bash
git add interfaces/webchat/src/canvas_engine/layout.rs interfaces/webchat/src/views/canvas/graph_canvas.rs interfaces/webchat/src/canvas_engine/renderer.rs
git commit -m "canvas: delete dead ForceLayout/RadialForceLayout and legacy rAF branch"
```

---

## Task 13 — Delete `R_1` / `R_2` constants and the orphan hint ring

**Goal:** Remove the now-unused `pub const R_1` / `pub const R_2` from `layout.rs`. Also remove the hard-coded hint ring at `R = ORPHAN_RADIUS` in `draw_orphan_ring`, which is meaningless once orphans are clustered (Task 8).

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/layout.rs`
- Modify: `interfaces/webchat/src/canvas_engine/renderer.rs`

- [ ] **Step 13.1: Delete `R_1` and `R_2` constants**

In `layout.rs`, find:

```rust
pub const R_1: f32 = 180.0;
pub const R_2: f32 = 320.0;
```

Delete both lines. Run:

```bash
grep -n "R_1\|R_2" interfaces/webchat/src/
```

Expected: zero hits in `interfaces/webchat/src/` (the helpers are `r_one_hop` / `r_two_hop`).

- [ ] **Step 13.2: Delete the orphan hint ring**

In `renderer.rs::draw_orphan_ring`, find the block that draws the hint ring at `R = ORPHAN_RADIUS` (currently lines 351-355):

```rust
ctx.set_stroke_style_str("rgba(167,139,250,0.06)");
ctx.set_line_width(1.0);
ctx.begin_path();
let _ = ctx.arc(0.0, 0.0, ORPHAN_RADIUS as f64, 0.0, TAU);
ctx.stroke();
```

Delete this block. Keep the dot-rendering loop that follows.

Also remove `ORPHAN_RADIUS` from the `use super::types::*;` imports if it was named explicitly. If only the `*` glob is used, no change needed there — the unused-name will simply stop being referenced.

- [ ] **Step 13.3: Compile + clippy**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
cargo clippy -p aleph-panel --target wasm32-unknown-unknown -- -D warnings
```

Expected: clean. Manual smoke at `/memory` — orphans appear in clusters, no faint hint ring at R=550.

- [ ] **Step 13.4: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/layout.rs interfaces/webchat/src/canvas_engine/renderer.rs
git commit -m "canvas: drop unused R_1/R_2 constants and orphan hint ring"
```

---

## Task 14 — Delete `ORPHAN_RADIUS` constant

**Goal:** Final cleanup — remove the orphan ring radius from `types.rs`. After Task 13 it has zero callers.

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/types.rs`

- [ ] **Step 14.1: Confirm zero callers**

```bash
grep -rn "ORPHAN_RADIUS" interfaces/webchat/src/
```

Expected: only the definition line in `types.rs`. If anything else hits, fix that first.

- [ ] **Step 14.2: Delete the constant**

In `types.rs`, find:

```rust
pub const ORPHAN_RADIUS: f32 = 550.0;
```

Delete the line (and any doc-comment block above it that referred only to it).

`ORPHAN_HOP_SENTINEL` and `ORPHAN_Z` stay — they have other consumers.

- [ ] **Step 14.3: Compile + clippy**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
cargo clippy -p aleph-panel --target wasm32-unknown-unknown -- -D warnings
```

Expected: clean.

- [ ] **Step 14.4: Update doc-comment on `populate_orphans`**

The doc-comment above `populate_orphans` (lines 169-175) mentions "outer ring at `ORPHAN_RADIUS`". Rewrite to reflect type-cluster reality:

```rust
/// Populate `nbhd.orphans` with all nodes from `all_dtos` that are not already
/// present in the neighborhood (centre, one_hop, two_hop, or cluster members).
///
/// Orphans are grouped by `category` (kind), each group placed at a stable
/// angular sector around an orphan ring radius (see `layout::r_orphan`), and
/// laid out within their group via golden-angle spiral. They are tagged with
/// `hop = ORPHAN_HOP_SENTINEL` and `z = ORPHAN_Z`, and they participate in
/// idle drift (no `pinned`).
```

Also update the comment in `types.rs` that referenced the orphan ring (the doc-comment above `Neighborhood::orphans`):

```rust
/// Nodes outside the current connected component, drawn in dim type-grouped
/// clusters around the canvas. Click to re-center. Tagged with
/// `hop = ORPHAN_HOP_SENTINEL`.
pub orphans: Vec<CanvasNode>,
```

- [ ] **Step 14.5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/types.rs interfaces/webchat/src/canvas_engine/adapter.rs
git commit -m "canvas: drop ORPHAN_RADIUS const, refresh orphan doc-comments"
```

---

## Task 15 — Final integration and DoD verification

**Goal:** Run all builds, lints, tests, and the manual smoke checklist from spec §10. Land any remaining fixes; sign off the plan.

**Files:** none (verification only)

- [ ] **Step 15.1: Full unit test run**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib
```

Expected: all tests green. Note any failures — they are likely in test sites that referenced deleted constants.

- [ ] **Step 15.2: Clippy clean**

```bash
cargo clippy -p aleph-panel --target wasm32-unknown-unknown -- -D warnings
```

Expected: zero warnings.

- [ ] **Step 15.3: Verify DoD #4 grep is empty**

```bash
grep -rn "ForceLayout\|RadialForceLayout\|ORPHAN_RADIUS\|pub const R_1\|pub const R_2" interfaces/webchat/src/
```

Expected: zero hits.

- [ ] **Step 15.4: Manual smoke at `/memory`**

```bash
just dev
```

Then in browser at `http://127.0.0.1:18790/memory`:

1. **Idle drift** — load any neighbourhood; without touching the mouse, observe nodes drifting ~5 px in a slow oscillation. Edges follow.
2. **Drag → edge follows** — drag a 1-hop neighbor; the edge from centre tracks the dragged node continuously (no broken cable). Spring-back retracts the edge with the node.
3. **Promote-on-drag** — drag a node into the centre hot zone; release; tween into centre + navigation triggers.
4. **Orphan clusters** — load a vault that has orphans; orphans appear in kind-grouped clusters around the canvas, not a single ring; orphans drift.
5. **Auto-fit on load** — viewport fits all nodes with ~10 % padding on entry, on click-promote, and on breadcrumb-back.
6. **Auto-fit on resize** — narrow the window; layout shrinks. Wide it; layout grows.
7. **Top-K slider, hover dimming, click-promote, minimap navigation** — unchanged from before.

- [ ] **Step 15.5: Verify net diff is reasonable**

```bash
git diff --stat $(git log --grep='canvas: design upgrade' -n1 --format='%H')..HEAD -- interfaces/webchat/
```

Expected: roughly +180 / −180 lines across the touched files; net change near zero. If the delete tasks land much less than +150 net (i.e., the upgrade *adds* hundreds of lines), something is unaccounted for — investigate.

- [ ] **Step 15.6: Push the branch (or hand off for review)**

(If the worker is on the `main` branch per Aleph convention, just leave the commits in place; the developer pushes when ready.)

---

## Self-Review Notes

Before declaring the plan complete, the writer ran four checks against the spec:

1. **Spec coverage:** Each of the 4 bugs and the cleanup pass maps to at least one task — Bug #1 → Tasks 1, 5, 6; Bug #2 → Task 7; Bug #3 → Task 8; Bug #4 → Tasks 2, 3, 4, 9, 10; cleanup → Tasks 11, 12, 13, 14. The spec's §6.5 cleanup table maps line-for-line into Tasks 11-14.
2. **Placeholder scan:** No "TBD"/"TODO"/"as appropriate" remain. Every code block is concrete, every command has expected output. The one judgement call ("If clippy complains about unused `Renderer::draw`...") is made explicit as "delete the impl Renderer block".
3. **Type consistency:** `drift_offset` returns `Vec2` (consistent across Tasks 1, 5, 6, 7); `r_one_hop` / `r_two_hop` / `r_orphan` return `f32` (consistent across Tasks 2, 4, 8); `Viewport::fit_to_content` takes `(&[CanvasNode], f32)` (consistent across Tasks 3, 9, 10). The `populate_orphans` signature is preserved; only its body changes.
4. **Ambiguity:** `compute_target_positions`'s `viewport_w_px` defaulted to 800.0 in adapter callers is called out explicitly with the rationale that `Viewport::fit_to_content` is the *real* viewport adaptation and the layout pass only needs plausible relative geometry.

---

**Plan complete and saved to** `docs/superpowers/plans/2026-05-03-knowledge-graph-canvas-upgrade.md`.
