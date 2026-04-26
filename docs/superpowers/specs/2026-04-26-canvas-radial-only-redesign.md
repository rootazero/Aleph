# Canvas Radial-Only Redesign

**Date:** 2026-04-26
**Predecessor:** [`2026-04-25-canvas-radial-navigation-design.md`](./2026-04-25-canvas-radial-navigation-design.md) (Phase 2 implementation)
**Status:** Design

## Problem Statement

Phase 2 of the Canvas Knowledge Graph shipped radial neighborhood navigation alongside the legacy global/local view, gated by a `canvas_radial_navigation` setting. After several iterations of bug-fixing, three persistent issues remained:

1. **Detail slider has no visible folding effect** — dragging the slider only causes "edges to wobble" without any node folding actually occurring.
2. **Global / Local toggle buttons appear confusing** — they only render in legacy mode, with no on-screen explanation, and silently no-op in common situations (e.g., clicking *Local* in Global view without a selected node).
3. **Two coexisting view paradigms create cognitive overhead** — users do not understand when to use which mode, and the Radial/Legacy switch is itself buried in the toolbar.

Root-cause analysis (see Diagnosis section below) shows the slider issue is a semantic mismatch between user intent and the folding algorithm, not a wiring bug — every prior "fix" was working at the data flow level but invisible at the rendering level. The Global/Local issue is a design wart inherited from the pre-radial implementation.

## Diagnosis (root cause of each bug)

### Bug 1: Detail slider only wobbles edges

The `cluster.rs::fold_sector` logic groups 1-hop neighbors **by category before** comparing against the threshold:

```rust
for (category, group) in by_category {
    if group.len() >= threshold {  // per-category subgroup, not total
        // fold this category
    }
}
```

In real vault data, 1-hop neighbors are spread across many categories (concept, reference, topic, project, ...). Even with 30 total neighbors, no single category subgroup reaches `threshold` (default 12), so folding never triggers.

The slider does cause a refetch (Effect 2 already subscribes to `fold_threshold.get()` and Effect 5 forces an additional refetch via `active_request: None → Some(id)`). But because `fold_sector` returns the same set of nodes regardless of the threshold value, the user sees only position jitter from re-running the force layout — interpreted as "edges wobble".

### Bug 2: Global / Local buttons confusing

`toolbar.rs:140` hides the toggle in radial mode by design (radial is always center-focused, so "Global" has no meaning there). However:
- The toggle's purpose is undocumented in the UI.
- In legacy Global mode without a selected node, clicking *Local* only emits `console.warn`, with zero visual feedback.
- Two view modes (Radial vs Legacy) plus two view subtypes (Global vs Local) is one degree of freedom too many.

### Bug 3: Effect 5 and the wobble

`mod.rs::Effect 5` (added in a previous fix attempt) round-trips `active_request: None → Some(id)` to force Effect 2 to re-fire. But Effect 2 already subscribes to `fold_threshold.get()`, so Effect 5 is redundant and causes the duplicate fetch that produces visible jitter.

## Goals

1. **Single coherent paradigm** — only one canvas view, the radial neighborhood navigator. Eliminate Legacy / Global / Local entirely.
2. **Slider produces immediate visible change** — dragging Detail directly controls how many 1-hop nodes are unfolded.
3. **Global awareness without legacy mode** — provide a minimap as the answer to "what's the rest of the graph look like?"

## Non-Goals

- Full minimap zoom/pan
- Drawing edges in the minimap
- Refactoring the force-directed layout
- Backend graph API changes
- Migrating users' `canvas_radial_navigation` setting (the field is preserved but unread)

## Architecture

### Before
```
CanvasView()
  ├── reads radial_signal
  ├── if true → RadialCanvasView
  └── if false → LegacyCanvasView
                 ├── ViewMode { Global, Local }
                 ├── view_mode signal
                 └── Toolbar shows Global/Local toggle

CanvasToolbar
  ├── Radial / Legacy toggle (persists to localStorage)
  ├── Detail slider
  ├── Global / Local toggle (hidden in radial)
  └── Search
```

### After
```
CanvasView()
  └── RadialCanvasView                       // single view, no switching
        ├── Effects 1-4 (entry, active, detail, hover-prefetch)
        ├── all_dtos (already fetched)       // 500-node full graph
        ├── GlobalMiniMap (new UI overlay)
        └── GraphCanvas

CanvasToolbar (simplified)
  ├── Title
  ├── Search
  └── Detail slider with "(N of M)" counter   // direct visual feedback
```

`ViewMode` enum: deleted (no longer used).
`view_mode`/`set_view_mode` signals: deleted.
`Effect 5` (forced refetch): deleted (redundant with Effect 2's reactive subscription).
`radial_signal` and `save_canvas_radial_navigation`: no longer read by canvas; the `DashboardState` field stays for one release as a no-op for compatibility, then deleted.

## Component Designs

### GlobalMiniMap

**Location & size:** Bottom-right overlay on the canvas, fixed 200×200 px (140×140 below 1280px viewport, hidden below 960px). Rounded corners, semi-transparent background `bg-surface-raised/80`, 16px from canvas edges.

**Layout algorithm:** Deterministic circular projection — no force simulation, so node positions never jitter.

```rust
for node in dtos:
    let angle = hash(node.id) * TAU
    let radius = sqrt(hash2(node.id)) * R_minimap   // sqrt → uniform area distribution
    let component = connected_component_id(node)
    let hue = component_hue(component)
    points.push(MiniPoint { id, pos, hue })
```

**Render layers (bottom to top):**
1. Background circle outline (subtle gray)
2. Node points (2px, colored by connected component)
3. Current focus highlight (semi-transparent disk covering the 2-hop equivalent of the current Radial center)
4. Center marker (4px filled, white outline)

**No edges drawn.**

**Interactions:**
- Hover: tooltip `{name} · {category}`
- Click: `active_request.set(Some(id))` → triggers Effect 2 → tween to new center
- No pan/drag

**Performance:** 500 nodes × `arc()` = ~25ms total per repaint. Render is `Effect`-driven (on `all_dtos` change or `focus_id` change), not RAF, since most ticks are static.

**Module API:**
```rust
pub struct GlobalMiniMap {
    size_px: f32,
    points: Vec<MiniPoint>,
    component_colors: HashMap<u32, Color>,
}

pub struct MiniPoint {
    pub id: String,
    pub pos: Vec2,                   // local minimap coords (0..size, 0..size)
    pub component: u32,
}

impl GlobalMiniMap {
    pub fn build(dtos: &[NoteNodeDto], edges: &[NoteLinkDto], size_px: f32) -> Self;
    pub fn pick_at(&self, mx: f32, my: f32, hit_radius: f32) -> Option<&str>;
    pub fn render(
        &self,
        ctx: &CanvasRenderingContext2d,
        focus_id: Option<&str>,
        focus_neighbor_ids: &[String],
    );
}
```

The existing `mini_map::MiniMap` struct (currently dead code at `mod.rs:84`) is removed; `GlobalMiniMap` is the only minimap type.

### Top-K Folding (replaces fold_sector)

**Old semantics:** "Fold a category subgroup when its size ≥ threshold."
**New semantics:** "Show at most K = threshold most-important 1-hop nodes; fold the rest into category-named clusters."

```rust
// adapter.rs::to_neighborhood
let one_hop_count = one_hop.len();
let (filtered_one_hop, clusters) = if one_hop_count <= threshold {
    (one_hop, vec![])
} else {
    let mut sorted = one_hop;
    sorted.sort_by(|a, b| {
        let wa = a.decay_score * a.edge_count as f32;
        let wb = b.decay_score * b.edge_count as f32;
        wb.partial_cmp(&wa).unwrap_or(Ordering::Equal)
    });
    let kept: Vec<_> = sorted.drain(..threshold).collect();
    let folded_clusters = group_by_category_into_clusters(sorted, &resp.center.id);
    (kept, folded_clusters)
};
```

`group_by_category_into_clusters`: groups remaining nodes by `category`, emits one `ClusterNode` per category with `kind = category`, `representative_names = top 3 by weight`, `radius = cluster_radius(member_count)`.

The folded count always equals `max(0, one_hop_count - threshold)`, so the slider has direct, monotonic visual effect.

**Slider range:** `4..=30` default `12` (was `6..=20`). Wider range accommodates both sparse and dense vaults.

### Toolbar with Counter

```
┌──────────────────────────────────────────────────────────────────┐
│ 🔵 Knowledge Graph    [search…]    Detail [──●──] 12 (12 of 27)  │
└──────────────────────────────────────────────────────────────────┘
```

The `(K of N)` text reads from a `RwSignal<(usize, usize)>` updated whenever a neighborhood loads. `K = filtered_one_hop.len()`, `N = one_hop_count`.

Removed from toolbar: Radial/Legacy toggle, Global/Local toggle, the `is_radial` prop, the `view_mode`/`on_toggle_mode` props.

### Cache Key with Threshold

`PrefetchCache` is now keyed by `(id, threshold)` instead of `id` alone. This makes threshold changes naturally invalidate stale entries without an explicit `clear()` call.

```rust
pub fn put(&mut self, id: String, threshold: usize, nbhd: Neighborhood);
pub fn get(&self, id: &str, threshold: usize, now_ms: f64) -> Option<&Neighborhood>;
```

`clear()` is removed (its only caller was Effect 5, which is also being removed).

## Data Flow

### Slider drag
```
slider input → fold_threshold signal updates
            → Effect 2 re-fires (subscribes to fold_threshold.get())
            → cache miss (different threshold key)
            → GraphApi::neighbors() fetch
            → to_neighborhood(threshold) applies top-K fold
            → seed_graph_state writes new positions
            → tween animates to new layout
            → toolbar counter updates "(K of N)"
```

### MiniMap click
```
click on minimap → pick_at returns node_id
                → active_request.set(Some(node_id))
                → Effect 2 fetches neighborhood of new center
                → graph_state updates → tween → re-render
                → minimap focus_id updates → repaint
```

### Initial load
```
Effect 1: GraphApi::query(500) → all_dtos
        → GlobalMiniMap::build(all_dtos, edges) → cached
        → entry pick → GraphApi::neighbors → seed graph_state
```

## Testing

### Unit tests (additions)

`adapter.rs::tests`
- `top_k_fold_keeps_highest_weight` — 30 nodes, threshold=12 → 12 unfolded (highest `decay_score * edge_count`) + 18 in clusters
- `top_k_fold_no_op_when_under_threshold` — 8 nodes, threshold=12 → all 8 unfolded, 0 clusters
- `top_k_fold_clusters_split_by_category` — folded remainder splits by `category`, one ClusterNode per distinct category

`mini_map.rs`
- `global_minimap_deterministic_layout` — `build` with same input twice produces identical `MiniPoint::pos` for every node
- `global_minimap_pick_at_finds_node` — `pick_at` returns the node whose center is within `hit_radius`
- `global_minimap_pick_at_misses_outside_radius` — returns `None` when click is far from any node
- `global_minimap_component_coloring` — nodes in the same connected component receive the same hue

`prefetch.rs::tests`
- `cache_keyed_by_id_and_threshold` — `put(id, 12, ...)` then `get(id, 6, ...)` returns `None`
- Existing tests updated to pass `threshold` parameter

### Tests removed

- `cluster.rs::fold_at_threshold_creates_cluster` and related (old by-category logic deleted)
- Any tests asserting Effect 5 behavior (Effect 5 deleted)

### Integration verification (manual + chrome-devtools-mcp)

1. Load `/memory` → minimap visible bottom-right, ~500 node points colored by component.
2. Drag Detail slider 4 → 30 → main canvas node count changes monotonically; "(N of M)" updates in real time.
3. Click a minimap node → Radial center transitions to that node; focus highlight in minimap follows.
4. Hover prefetch still works (hover a 1-hop node for 150ms, then click → no fetch latency).
5. Search → click result → breadcrumb updates → minimap focus follows.

## Files Changed

| File | Change |
|---|---|
| `views/canvas/mod.rs` | Delete `LegacyCanvasView`, `Effect 5`, `view_mode` signal; root component returns `RadialCanvasView` directly; wire minimap render Effect |
| `views/canvas/toolbar.rs` | Delete Radial/Legacy and Global/Local toggles; add `(K of N)` counter; drop `is_radial`/`view_mode`/`on_toggle_mode` props |
| `canvas_engine/adapter.rs` | Rewrite folding in `to_neighborhood` to top-K; add `group_by_category_into_clusters` helper; drop `_default` relation grouping |
| `canvas_engine/cluster.rs` | Delete `fold_sector`, `fallback_fold`, `FOLD_THRESHOLD`; keep `cluster_radius` |
| `canvas_engine/mini_map.rs` | Delete existing `MiniMap` struct; add `GlobalMiniMap` per spec |
| `canvas_engine/prefetch.rs` | Cache key becomes `(id, threshold)`; delete `clear()` |
| `canvas_engine/types.rs` | No change (keep `ViewMode` enum unused for one release to avoid downstream churn) |
| `context.rs` | No change (keep `canvas_radial_navigation` field, stop reading it in canvas) |
| `api/settings.rs` | No change |

Net change: approximately -300 lines deleted, +150 lines added. Mental-model count: 2 → 1.

## Migration & Compatibility

- Users with `canvas_radial_navigation = false` in localStorage will see the Radial view on next load (the field is no longer consulted). No data migration required.
- Backend `GraphApi::query` and `GraphApi::neighbors` endpoints are unchanged. Legacy code paths simply stop calling `query` (it is still called by Effect 1 for the minimap).
- Saved breadcrumbs, selected node ids, etc., remain compatible — they were always indexed by node id.

## Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Users relied on Legacy Global view to see the entire graph at once | GlobalMiniMap exposes the full topology in the bottom-right; Radial center can jump anywhere via minimap click |
| 500-node minimap repaints cause visible cost on slow devices | Render only on `all_dtos`/`focus_id` change, not RAF; `arc()` calls are cheap (<25ms total measured budget) |
| Category-based clustering loses meaning in vaults with no category diversity | Top-K folding still works (single fat cluster); slider still controls visible count |
| Removing `fold_sector` breaks downstream consumers we missed | Grep confirms `fold_sector` and `fallback_fold` are called only from `adapter.rs::to_neighborhood`; no public API exposure |

## Open Questions

None — all major decisions confirmed during brainstorming on 2026-04-26.
