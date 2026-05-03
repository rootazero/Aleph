# Canvas Knowledge Graph — Float, Drag-Sync, Orphan-Cluster, Auto-Fit Upgrade

**Date:** 2026-05-03
**Status:** Design (pending implementation plan)
**Predecessors (still in force, do not break):**
- [`2026-04-25-canvas-radial-navigation-design.md`](./2026-04-25-canvas-radial-navigation-design.md) — radial neighborhood paradigm
- [`2026-04-26-canvas-radial-only-redesign.md`](./2026-04-26-canvas-radial-only-redesign.md) — Top-K folding + GlobalMiniMap
- [`2026-04-27-canvas-elastic-node-drag-design.md`](./2026-04-27-canvas-elastic-node-drag-design.md) — `DragState`/`Spring2D` overlay drag (§12 D1: **no force-directed layout**)

**Inspiration (read but not copied):** `/Volumes/TBU4/Github/GitNexus` — Sigma.js + ForceAtlas2 + Leiden community + golden-angle spiral. We borrow the *ideas* (per-node phase animation, type-cluster spread, golden-angle, viewport auto-fit) and re-implement them in pure Rust without a force engine, respecting the elastic-drag spec's no-force-directed constraint.

---

## 1. Problem

The Aleph radial canvas (rendered by `interfaces/webchat/src/views/canvas/` + `interfaces/webchat/src/canvas_engine/`) has four user-reported defects that make the visualization feel rigid and lose nodes off-screen:

### Bug #1 — Nodes have no idle floating animation

`canvas_engine/layout.rs:188-267` defines `ForceLayout::tick()` with explicit "never settle" jitter (`js_sys::Math::random() ... node.velocity += ...`, line 256-263, comment "Never stop animating"), but the radial path in `views/canvas/graph_canvas.rs:198-292` always returns at line 292 before reaching the legacy `state.layout.is_settled` branch at line 300. The drift code is dead. `views/canvas/mod.rs:717` confirms: "no wake — radial uses target_positions, not physics."

In the shipped radial path, `NavState::Active` calls `draw_neighborhood()` with static positions interpolated only during animation. At idle, every pixel is frozen.

### Bug #2 — Dragging a node does not move connected edges

`canvas_engine/drag.rs` (614 LOC) implements the elastic-drag spec via overlay rendering: `DragState` produces a `DragOverlay` describing where the dragged node should *appear*, but `nodes[i].position` itself is never mutated. `canvas_engine/renderer.rs::draw_edges()` reads `nodes[edge.from_idx].position` and `nodes[edge.to_idx].position` directly. Result: the dragged node visually moves; its edges stay anchored at the original layout slot, producing a "broken cable" feel.

### Bug #3 — Unconnected nodes pinned to a stiff outer ring

`canvas_engine/adapter.rs:205-226` lays out orphans via `angle = phase + i·TAU/count; x = cos·550; y = sin·550; node.pinned = true;`, with `ORPHAN_RADIUS = 550` hardcoded in `canvas_engine/types.rs`. Top-K folding (radial-only spec) replaced 1-hop excess with category clusters but did not touch the orphan path, leaving a deterministic, evenly-spaced, motionless ring around the focus.

### Bug #4 — Hardcoded radii cause overflow as the graph grows

`canvas_engine/layout.rs:41-42` hardcodes `R_1 = 180`, `R_2 = 320`. Combined with `ORPHAN_RADIUS = 550`, layout extent is fixed regardless of node count or canvas size. The viewport (`graph_canvas.rs:176-191`) recenters on resize but does not refit content. Dense neighborhoods or small windows lose nodes off-screen.

---

## 2. Goals

1. **Idle motion** — visible, gentle node drift while the user is not interacting (Q4=B "breathing" feel: ±5 px amplitude, ~5 s period). Same per-node phase variance as the GitNexus pulse, achieved without a physics engine.
2. **Edges follow the dragged node** — visual continuity across the existing elastic-drag overlay model. Spec D1 still enforced (no force-directed layout).
3. **Orphans organic, not pinned** — replace the rigid ring with type-clustered golden-angle placement, matching the visual language of the 1-hop cluster bubbles.
4. **Layout fits the viewport** — radii adapt to node count and canvas size; `fit_to_content` is invoked when the neighborhood data changes or the window resizes.
5. **Net cleanup** — remove dead code (`ForceLayout`, `ORPHAN_RADIUS`, ring loop) so the upgrade does not add a maintenance burden.

## 3. Non-Goals

| Excluded | Why |
|---|---|
| New `force/` submodule, force-directed simulation, Barnes-Hut quadtree | Conflicts with `2026-04-27-canvas-elastic-node-drag-design.md` §12 D1 (force-directed explicitly rejected) |
| New crate dependencies (`petgraph`, `kiddo`, `fdg`, etc.) | Goals are achievable in pure-renderer changes; honors R3 (Core Minimalism) |
| Replacing `Neighborhood.target_positions: HashMap<String, Vec3>` | It is the radial spec's primary data structure; the `NavController` tween relies on it |
| Touching `DragState`, `Spring2D`, `Tween2D`, or `NavController` logic | Already specced and shipped; only the renderer's *consumption* of drag overlay changes |
| Replacing Top-K folding or `ClusterNode` rendering | Independent code path; outside the four reported bugs |
| Touching `GraphApi::neighbors` / `GraphApi::query` server APIs | Frontend-only fix; backend untouched |
| Web Workers, `wasm-bindgen-rayon`, OffscreenCanvas | YAGNI at current node scale (≤ 2k typical); revisit if measurements justify |
| LOD / frustum culling, edge bundling, mini-map redesign | Out of scope; graph is already paginated by Top-K |
| User-configurable physics constants in settings UI | Defaults sufficient; ship without |
| Animation pause toggle (`prefers-reduced-motion`) | Stub interface only; UI work for follow-up |

## 4. Approach

Four surgical, independent changes, one cleanup pass. Each fix is local to one or two functions; none introduces new modules, types, or dependencies.

| Bug | Fix | Files | Net LOC |
|---|---|---|---|
| #1 — No drift | Per-node sin offset applied at draw time (renderer-side, no physics tick) | `canvas_engine/renderer.rs` | +30 |
| #2 — Edges don't follow drag | Renderer detects `edge.from_id` / `edge.to_id` against active drag overlay, applies offset to that endpoint only | `canvas_engine/renderer.rs` | +15 |
| #3 — Orphan ring | Replace `populate_orphans` deterministic ring with type-cluster + golden-angle placement | `canvas_engine/adapter.rs` | −20 / +60 |
| #4 — Static radii / no fit | Adaptive `R(n, viewport_size)` + new `viewport::fit_to_content` invoked on `NavState::Active` and resize | `canvas_engine/layout.rs`, `canvas_engine/viewport.rs`, `views/canvas/graph_canvas.rs` | +75 |
| Cleanup | Delete unused `ForceLayout` struct, `ORPHAN_RADIUS` constant | `canvas_engine/layout.rs`, `canvas_engine/types.rs` | −151 |

Net: **+180 / −171 ≈ break-even**. Zero new files. Zero new crates.

## 5. Architecture

### 5.1 Module map (no structural change)

```
canvas_engine/
  ├── renderer.rs       MODIFY  + drift offset, drag-aware edge endpoints
  ├── adapter.rs        MODIFY  populate_orphans → type-cluster + golden-angle
  ├── layout.rs         MODIFY  R₁/R₂ adaptive; DELETE ForceLayout (dead)
  ├── viewport.rs       MODIFY  + fit_to_content
  ├── types.rs          MODIFY  DELETE ORPHAN_RADIUS const
  ├── drag.rs           UNCHANGED (overlay model preserved)
  ├── tween.rs          UNCHANGED (Spring2D / Tween2D preserved)
  ├── navigation.rs     UNCHANGED
  ├── cluster.rs        UNCHANGED (Top-K folding)
  ├── mini_map.rs       UNCHANGED (GlobalMiniMap)
  └── prefetch.rs       UNCHANGED

views/canvas/
  ├── graph_canvas.rs   MODIFY  call fit_to_content on Active enter / resize
  ├── mod.rs            UNCHANGED
  └── (others)          UNCHANGED
```

### 5.2 Data-flow invariants (preserved)

- `Neighborhood.target_positions: HashMap<String, Vec3>` remains the canonical post-layout source for tween interpolation. The renderer only **adds** transient offsets (drift, drag) on top of resolved positions; it never writes back.
- `DragState` continues to expose `DragOverlay` snapshots; the renderer reads them, never mutates them.
- `NavController` continues to drive `NavState` transitions; the only new wiring is one `viewport::fit_to_content(nodes)` call on `NavState::Active` entry.

### 5.3 Compliance check

| Constraint | Compliance |
|---|---|
| **R1** Brain-Limb separation | Pure WASM/Canvas2D, no platform API |
| **R2** UI single source of truth in Leptos | All changes inside `interfaces/webchat/` |
| **R3** Core minimalism | Zero new crates; net LOC near zero |
| **R7** One core, many shells | No backend touch |
| **R11** Thin harness, dumb loop | Renderer is dumb (per-frame transform of state → pixels); no new logic, no LLM tax |
| **P1** Low coupling | Each fix touches one function; no new public types |
| **P2** High cohesion | Drift / drag-edge sync stay in `renderer.rs`; orphan placement stays in `adapter.rs`; auto-fit stays in `viewport.rs` |
| **P6** Simplicity (KISS/YAGNI) | No new abstractions; existing types reused |
| **Spec D1** No force-directed layout | Drift is a per-frame visual offset (not a simulation); no inter-node forces; no integration steps |

## 6. Component Designs

### 6.1 Per-node drift (Bug #1, `renderer.rs`)

**Signature:**
```rust
fn drift_offset(t_ms: f64, node_id: &str, amplitude_px: f32, period_ms: f32) -> Vec2 {
    let phase = fnv1a_32(node_id.as_bytes()) as f32 / u32::MAX as f32;  // [0, 1)
    let omega = std::f32::consts::TAU / (period_ms / 1000.0);
    let t = (t_ms as f32) / 1000.0;
    Vec2::new(
        amplitude_px * (omega * t + phase * std::f32::consts::TAU).sin(),
        amplitude_px * (omega * t + (phase + 0.27) * std::f32::consts::TAU).sin(),
    )
}
```

**Defaults:** `AMPLITUDE_PX = 5.0`, `PERIOD_MS = 5000.0`. Centre node receives drift too (the spec does not pin the focus visually beyond Z-layer prominence).

**Wiring:** `Renderer::draw()` already iterates nodes once; for each node, compute the drift, add it to the node's screen position before drawing. Edges read the same drifted position via the helper below so endpoints stay attached.

```rust
fn drifted_screen_pos(node: &CanvasNode, viewport: &Viewport, t_ms: f64) -> Vec2 {
    let world = node.position + drift_offset(t_ms, &node.id, AMPLITUDE_PX, PERIOD_MS);
    viewport.world_to_screen(world)
}
```

**Pinned nodes** (drag pinned, focus pinned) bypass drift via `if !node.pinned`. The dragged node's overlay supersedes drift for that frame.

**Cost:** one `sin/cos` pair per node per frame. At 200 nodes that is ~3 µs total — negligible.

### 6.2 Drag-aware edge endpoints (Bug #2, `renderer.rs`)

**Existing:** `draw_edges()` resolves both endpoints from `nodes[edge.from_idx].position`. **Change:** if the renderer is given a `Some(DragOverlay)`, check whether either endpoint's `node.id == overlay.node_id`; if so, replace that endpoint's screen position with the overlay's drag-displaced position.

```rust
let (from_screen, to_screen) = endpoints_with_drag(
    &nodes[edge.from_idx], &nodes[edge.to_idx], viewport, drag_overlay, t_ms,
);
```

`endpoints_with_drag` is a pure helper:
- if `from.id == overlay.node_id` → `from_screen = overlay.screen_pos`
- otherwise → `from_screen = drifted_screen_pos(from, viewport, t_ms)`
- same for `to`

This makes Bug #2 disappear without touching `DragState` or its event loop. SpringBack/Promoting work automatically — `DragOverlay` carries the spring's current position, edges follow it.

**Cost:** one extra id comparison per edge endpoint. O(E).

### 6.3 Type-cluster orphan placement (Bug #3, `adapter.rs::populate_orphans`)

The shipped function signature is `pub fn populate_orphans(nbhd: &mut Neighborhood, all_dtos: &[NoteNodeDto])` (declared at `adapter.rs:176`); the ring loop lives inside that function around lines 205-226. The sketch below uses a small helper to keep the type-cluster math readable; the helper is private to `adapter.rs` and the public signature is preserved.

**Replace** the ring loop:
```rust
// DELETED:
let r = ORPHAN_RADIUS as f64;        // = 550.0
let phase = TAU / 8.0;
for (i, dto) in orphan_dtos.into_iter().enumerate() {
    let angle = phase + (i as f64) * TAU / (count as f64);
    let x = angle.cos() * r;
    let y = angle.sin() * r;
    ...
    node.pinned = true;
}
```

**With** type-cluster + golden-angle:

```rust
// Helper, private to adapter.rs. The public populate_orphans keeps its
// existing &mut Neighborhood signature and pushes the returned nodes
// into nbhd.nodes (and corresponding entries into nbhd.target_positions
// for tween consistency, just like the ring loop did).
fn place_orphans_clustered(orphan_dtos: Vec<NoteNodeDto>, r_orphan: f32) -> Vec<CanvasNode> {
    let mut by_kind: BTreeMap<String, Vec<NoteNodeDto>> = BTreeMap::new();
    for dto in orphan_dtos {
        by_kind.entry(dto.kind.clone()).or_default().push(dto);
    }

    let kind_count = by_kind.len().max(1) as f32;
    let mut nodes = Vec::new();

    for (i, (kind, members)) in by_kind.into_iter().enumerate() {
        // Cluster centre angle: hash-based for stable layout across sessions,
        // then evenly redistributed so adjacent clusters do not overlap.
        let center_angle = (i as f32 / kind_count) * std::f32::consts::TAU
            + (fnv1a_32(kind.as_bytes()) as f32 / u32::MAX as f32) * 0.5;
        let cx = r_orphan * center_angle.cos();
        let cy = r_orphan * center_angle.sin();

        let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());  // ≈ 137.5°
        for (j, dto) in members.into_iter().enumerate() {
            let angle = j as f32 * golden;
            let radius = (j as f32 + 1.0).sqrt() * 16.0;  // golden-angle disc, dense centre
            let x = cx + radius * angle.cos();
            let y = cy + radius * angle.sin();
            let mut node = note_dto_to_canvas(dto, ORPHAN_HOP_SENTINEL);
            node.position = Vec2::new(x, y);
            // pinned = false → orphan participates in drift via §6.1
            nodes.push(node);
        }
    }

    nodes
}
```

`r_orphan` is supplied by the caller (`to_neighborhood`) using the adaptive formula in §6.4.

**Effect:** orphans cluster by `kind` (Note / Skill / Tool / ...), each cluster a small golden-angle disc; clusters distributed around the focus at the adaptive orphan radius; everything drifts at idle (no pinning). Visual continuity with the 1-hop `ClusterNode` bubbles.

### 6.4 Adaptive radii + fit_to_content (Bug #4)

**`canvas_engine/layout.rs`** — replace constants with functions:

```rust
// Adaptive radius for hop layer. n = total visible nodes, vw = viewport width px.
fn r_for_hop(hop: u8, n: usize, vw: f32) -> f32 {
    let base = 100.0_f32;
    let count_factor = (1.0 + (n as f32) / 16.0).sqrt();          // grows ~ √n
    let vw_factor = (vw / 800.0).clamp(0.6, 1.4);                  // small windows shrink
    base * count_factor * vw_factor * (hop as f32)
}

pub fn r_one_hop(n: usize, vw: f32) -> f32  { r_for_hop(1, n, vw) }
pub fn r_two_hop(n: usize, vw: f32) -> f32  { r_for_hop(2, n, vw) }
pub fn r_orphan(n: usize, vw: f32) -> f32   { r_one_hop(n, vw) * 2.4 }
```

`compute_target_positions()` takes `n` and `vw` (or a `LayoutMetrics` struct) and uses the helpers instead of `R_1` / `R_2` constants. The current call site in `adapter.rs:128` passes a freshly-counted `n` and the current viewport width.

**`canvas_engine/viewport.rs`** — new method:

```rust
impl Viewport {
    pub fn fit_to_content(&mut self, nodes: &[CanvasNode], padding_pct: f32) {
        if nodes.is_empty() { return; }
        let (mut min_x, mut min_y, mut max_x, mut max_y) =
            (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for n in nodes {
            min_x = min_x.min(n.position.x);
            min_y = min_y.min(n.position.y);
            max_x = max_x.max(n.position.x);
            max_y = max_y.max(n.position.y);
        }
        let bbox_w = (max_x - min_x).max(1.0);
        let bbox_h = (max_y - min_y).max(1.0);
        let pad = 1.0 + padding_pct;
        let scale_x = self.width / (bbox_w * pad);
        let scale_y = self.height / (bbox_h * pad);
        self.scale = scale_x.min(scale_y).clamp(0.2, 3.0);
        let cx = (min_x + max_x) * 0.5;
        let cy = (min_y + max_y) * 0.5;
        self.offset.x = self.width  / 2.0 - cx * self.scale;
        self.offset.y = self.height / 2.0 - cy * self.scale;
    }
}
```

**`views/canvas/graph_canvas.rs`** — invoke `fit_to_content` once per neighborhood load and on resize:

| Trigger | Detection | Where |
|---|---|---|
| `NavState` transitions `Loading → Active` (fresh data) | compare current `nav_rc.borrow().state` discriminant against the previous frame's snapshot held in a `Cell<NavStateKind>` near the rAF closure | inside the radial rAF branch (`graph_canvas.rs:198-292`), immediately after `nav_rc.borrow_mut().tick(now)` |
| `NavState` transitions `Animating → Active` (after focus switch) | same discriminant check | same site |
| Canvas parent resize (`pw`/`ph` change) | the existing resize-detect block (currently `graph_canvas.rs:176-191`) already centres the viewport; append `viewport.fit_to_content(&nodes_for_current_state, 0.10)` after centring | `graph_canvas.rs:176-191` |
| User pan / wheel zoom | **not** triggered (respect user's manual view) | n/a |

`fit_to_content` does not run every frame — only on the events above.

### 6.5 Cleanup

| Target | File | Action |
|---|---|---|
| `ForceLayout` struct + `tick()` + `wake()` + `Default` impl | `canvas_engine/layout.rs:188-278` | Delete (~90 lines) |
| `RadialForceLayout` struct + impl | `canvas_engine/layout.rs:475-578` | Delete (~104 lines) |
| `GraphState::layout: ForceLayout` field + use sites | `views/canvas/graph_canvas.rs:13, 25, 45, 300-308` | Delete (~12 lines) |
| `R_1` / `R_2` constants | `canvas_engine/layout.rs:41-42` | Delete (replaced by `r_one_hop` / `r_two_hop`) |
| `ORPHAN_RADIUS` constant | `canvas_engine/types.rs` | Delete |
| Legacy "flat-graph" rAF branch | `views/canvas/graph_canvas.rs` from the `// Legacy flat-graph branch (no nav controller)` comment near line 295 down to the next `// Schedule next frame` block | Delete the dead `if !state.layout.is_settled` branch and its rendering tail; the radial branch always returns at line 292, so this code is unreachable |
| Layout tests referencing `RadialForceLayout` | `canvas_engine/layout.rs` test mod | Delete or migrate to `compute_target_positions` |

Total removed: ~150 lines across layout / graph_canvas. The grep `target_positions` is **kept** (still used by `Neighborhood`).

## 7. Compatibility With Prior Specs

| Prior spec | Constraint | This spec |
|---|---|---|
| `2026-04-25 canvas-radial-navigation` | Active-centric radial paradigm; FNV-hashed sectors; tween between neighborhoods | Preserved unchanged. Adaptive R only changes the magnitude, not the structure. |
| `2026-04-26 canvas-radial-only-redesign` | Single radial view, Top-K folding, GlobalMiniMap | Preserved unchanged. Orphan placement is a sibling code path; folding/cluster logic untouched. |
| `2026-04-27 canvas-elastic-node-drag` | `DragState`, `Spring2D`, overlay-based drag; **D1: no force-directed layout** | Preserved unchanged. This spec adds *consumption* of `DragOverlay` in the renderer, not new physics. Drift is a closed-form sin offset, not a simulation. |
| `2026-04-26 canvas-detail-slider-fix` | Effect-fetch / Effect-refold split | Untouched. |

## 8. Testing

### 8.1 Unit tests (additions)

| Test | Module | Asserts |
|---|---|---|
| `drift_offset_is_periodic` | `renderer.rs` | `drift(t) == drift(t + period)` (within ε) for several node IDs |
| `drift_offset_amplitude_bounded` | `renderer.rs` | `drift(t).length() ≤ AMPLITUDE_PX·√2` for 100 random `t` and IDs |
| `drift_offset_phase_diverges` | `renderer.rs` | drift for two different IDs differs by ≥ 0.5 px on at least one of 10 sampled t-values |
| `endpoints_with_drag_no_overlay_returns_drifted` | `renderer.rs` | with overlay = `None`, returns `drifted_screen_pos` for both endpoints |
| `endpoints_with_drag_from_match_uses_overlay` | `renderer.rs` | with overlay matching `from.id`, `from_screen == overlay.screen_pos` |
| `populate_orphans_clusters_by_kind` | `adapter.rs` | orphans of same `kind` placed within disc of radius ≤ `√n·16 + ε` of cluster centre |
| `populate_orphans_distinct_kinds_separated` | `adapter.rs` | for ≥ 2 kinds, cluster centres are ≥ π/(2K) radians apart |
| `populate_orphans_no_pinned` | `adapter.rs` | no orphan returned has `node.pinned == true` |
| `r_for_hop_grows_with_n` | `layout.rs` | `r_one_hop(50, 800) < r_one_hop(500, 800)` |
| `r_for_hop_clamps_viewport` | `layout.rs` | `r_one_hop(100, 100) == r_one_hop(100, 480)` (lower clamp) and likewise upper clamp |
| `fit_to_content_single_node` | `viewport.rs` | bbox of one node → scale = 1, centred |
| `fit_to_content_padding_respected` | `viewport.rs` | content occupies ≤ `(1 - padding)` of viewport |
| `fit_to_content_clamps_scale` | `viewport.rs` | extreme bbox produces `scale ∈ [0.2, 3.0]` |
| `fit_to_content_empty_no_op` | `viewport.rs` | empty `nodes` does not modify viewport |

All tests live in `#[cfg(test)] mod tests {}` blocks in their respective files. No new test infrastructure.

### 8.2 Integration / smoke (manual)

On `http://127.0.0.1:18790/memory`:

1. Load any neighborhood → nodes visibly drift (~5 px breathing) at idle.
2. Drag a 1-hop neighbor → connected edges follow continuously; SpringBack release → edges retract with the node.
3. Vault with orphans → orphans appear in clusters by kind, not on a single ring.
4. Resize browser window narrow → layout shrinks to fit; widen → expands.
5. Switch focus across nodes with varying neighbor counts → radii visibly adapt; nothing escapes viewport.
6. Existing flows (Top-K slider, click-promote, minimap navigation, breadcrumb) work unchanged.

### 8.3 Performance smoke

- Idle CPU at 200 nodes: target ≤ baseline + 0.5 % single-core (drift sin/cos pair per node per frame is ~3 µs).
- Drag at 200 nodes: target identical to baseline (only one extra id-eq check per edge endpoint).

Measured locally with macOS Activity Monitor; not gated in CI.

## 9. Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Drift makes hover/click hit-tests miss | Hit-test uses `node.position` (un-drifted); the visual offset is purely cosmetic. Pointer-down on the visual location maps back to the underlying node via `viewport::screen_to_world` plus a small tolerance (already 6 px for click target). |
| Drift causes label readability issues | Labels are rendered at the drifted position, so they move *with* the node, not against it. ±5 px is well below text height. |
| `fit_to_content` fights the user during drag | Only triggered on `NavState::Active` entry and resize, never during drag or pan. |
| Adaptive R changes break user spatial memory | Same neighborhood always gets same radii (deterministic on `n`, `vw`); switching focus already reflows in the existing tween. |
| Type-cluster orphan placement collapses when only one kind exists | Single-cluster path falls back to a centred golden-angle disc (the loop runs once with the entire orphan set). |
| Removing `ForceLayout` breaks the legacy flat-graph rAF branch | The branch is already unreachable (radial path returns at line 292). Deleting the branch and the field is safe; tests covering it (if any) are deleted in the same commit. |
| Hit-test on dragged-edge endpoints | Edges are not interactive today; no regression. |

## 10. Definition of Done

1. All §8.1 unit tests pass (`cargo test -p webchat --target wasm32-unknown-unknown` + native unit tests).
2. `cargo clippy -p webchat --target wasm32-unknown-unknown -- -D warnings` clean.
3. `just dev` builds; manual §8.2 smoke checklist passes.
4. Within `interfaces/webchat/src/`, `grep -rn "ForceLayout\|RadialForceLayout\|ORPHAN_RADIUS\|pub const R_1\|pub const R_2"` returns zero production hits (only the new helpers `r_one_hop`, `r_two_hop`, `r_orphan` exist).
5. Existing canvas behaviours unchanged: Top-K slider, drag spring-back/promote, minimap click, breadcrumb, hover prefetch.
6. No new entries in `Cargo.toml`.

## 11. Out of Scope (formal record)

- **OS-1.** Replacing `Neighborhood.target_positions` with reactive signals — the current HashMap is sufficient.
- **OS-2.** Force-directed simulation in any form — see Spec D1; reaffirmed here.
- **OS-3.** Web Worker / `wasm-bindgen-rayon` for layout — current scale does not justify.
- **OS-4.** Edge gradient / hue per relation type (GitNexus has it; pure polish, not a bug).
- **OS-5.** `prefers-reduced-motion` honoring — interface is forward-compatible (a `calm_mode` flag on `Viewport` could later disable drift), but no UI is shipped this round.
- **OS-6.** Mini-map redesign — the existing `GlobalMiniMap` (radial-only spec) is sufficient; orphans appear in the mini-map via the same union-find that already runs on `all_dtos`.
- **OS-7.** `compute_target_positions` algorithmic rework — only the magnitude (R) changes; sector logic unchanged.

## 12. Open Questions

None at design time. Parameter values (`AMPLITUDE_PX = 5`, `PERIOD_MS = 5000`, golden-angle disc spread `√(j+1)·16`, viewport clamp `[0.6, 1.4]`, scale clamp `[0.2, 3.0]`) are defaults and may be tuned during implementation if measurement justifies; tuning is non-blocking for ship.
