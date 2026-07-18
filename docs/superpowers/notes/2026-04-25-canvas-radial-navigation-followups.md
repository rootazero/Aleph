# Canvas Radial Navigation — Phase 2 Follow-ups

**Date:** 2026-04-25
**Feature:** Radial canvas navigation (T1–T23 implementation plan)
**Status:** Implementation complete, feature-gated behind `UserPrefs.canvas_radial_navigation` (default off)

---

## Deferred Items from T22 / T23

### Mini-map click-to-navigate

The mini-map draw routine (T14) correctly renders a thumbnail overview and performs
hit-testing (`MiniMapState::node_at` + `MiniMapState::hit_test_sector`). However, the
click path from `CanvasInteraction::map_intent` through to `NavController::go_to` was
scaffolded but not wired end-to-end inside `RadialCanvasView`. A mini-map click
currently fires `Intent::MiniMapClick(logical_pos)` but the view does not yet call
`self.nav.go_to(target_id)` in response.

**Work needed:**
- In `RadialCanvasView::handle_intent`, match `Intent::MiniMapClick` → look up the
  closest node id from `MiniMapState::node_at` → call `self.nav.go_to(id)`.
- Add integration test: simulate a `MiniMapClick` and assert `NavState` transitions.

### Cluster expansion — topology mutation

`ClusterNode` folding (T9) collapses N same-kind neighbors into a supernode for
display. Clicking a cluster to expand it emits `Intent::ClusterExpand(cluster_id)`.
The intent is recognized but the expansion is not yet implemented: the cluster
supernode must be split back into its constituent `VisualNode` entries, the layout
must re-run a short settling pass, and the breadcrumb stack must not be mutated
(expansion is a display toggle, not a navigation step).

**Work needed:**
- Add `CanvasAdapter::expand_cluster(cluster_id) -> Neighborhood` that rebuilds the
  neighborhood with the cluster's members promoted to full `VisualNode`s.
- Add `NavController::toggle_cluster(cluster_id)` that calls the adapter and triggers
  a layout re-warm.
- Expose an "expanded" flag on `ClusterNode` so the renderer draws an outline instead
  of the folded badge.

### prefers-reduced-motion handling

The RAF render loop (T23) and tween system (T6) run animations unconditionally. On
systems where the user has enabled "Reduce Motion" (macOS Accessibility setting), we
should skip tween interpolation and jump directly to target positions / opacities.

**Work needed:**
- Query `NSWorkspace.shared.accessibilityDisplayShouldReduceMotion` (via Tauri IPC)
  once at startup and expose it as a boolean in the `UserPrefs` snapshot delivered to
  the canvas.
- In `TweenState::step`, when `reduce_motion == true`, clamp `t = 1.0` immediately so
  all animations resolve in a single frame.
- In `LayoutEngine::step`, skip the settling loop (use target positions directly).

---

## Known Data-Model Limitation

### `NoteLinkDto` carries no relation type → all neighbors land in `_default` sector

`graph.neighbors` returns `NoteLinkDto` objects. As of T1–T23, `NoteLinkDto` does not
carry a `relation_type` field (e.g., "parent", "child", "reference", "tag"). The
`CanvasAdapter::to_neighborhood` function therefore assigns every neighbor to the
`"_default"` sector, which means the radial sector layout (T7: deterministic
angle-to-sector assignment) always places all nodes in a single 360° band rather than
distributing them by relationship kind.

**Impact:** The visual distinction between "parent notes", "child notes", and
"references" that the design intended is not expressed in the layout.

**Work needed:**
- Extend `NoteLinkDto` (in `alephcore/src/memory/notes/`) with an optional
  `relation_type: Option<NoteRelationType>` field.
- Update `graph.neighbors` (panel API handler) to populate the field from the edge
  metadata in the note graph.
- Update `CanvasAdapter::to_neighborhood` to map `NoteRelationType` variants to the
  canonical sector names expected by `SectorLayout`.

---

## Pre-existing Follow-ups Surfaced During the Build

### BFS depth ceiling in `get_neighbors` (surfaced in T1)

`src/memory/notes/store.rs` — `get_neighbors` performs a BFS up to `hop_depth` hops
but the graph API caps `hop_depth` at 2 (see `graph(api): tighten hop_depth test
assertion`, commit `a83a856ae`). Notes with very large neighbor counts at depth-1 can
cause the BFS to return hundreds of nodes, overwhelming the layout engine.

**Recommendation:** Add a `max_nodes: usize` parameter (default 50) to `get_neighbors`
and truncate the BFS result, returning the closest nodes by BFS layer first. The
canvas adapter should surface a `truncated: bool` flag so the UI can show a "showing
N of M neighbors" indicator.

### Layout engine settling convergence (surfaced in T13)

`LayoutEngine::step` runs up to 60 iterations to converge (test:
`force_step_converges_within_60_iterations`). For dense neighborhoods (>30 nodes) the
repulsion forces may not fully converge within that budget, leaving nodes overlapping.

**Recommendation:** Add an adaptive iteration budget: start at 60, but if the maximum
node displacement in the final step is above a threshold (e.g., 2.0 units), extend by
30 more iterations (capped at 150). Log a warning if the cap is hit — it indicates a
degenerate layout scenario worth investigating.

---

## Manual QA Checklist

The following items require human verification with `just dev` + a browser. All items
are **unchecked** — none have been visually confirmed by an automated test.

> **To run:** `just dev` → navigate to the Aleph panel → open a note with ≥3
> neighbors → enable Canvas Radial Navigation in Settings → open the canvas view.

### Initial render

- [ ] Canvas loads without blank screen on first open
- [ ] Center node rendered at canvas center with glow halo (depth 0 brightness)
- [ ] 1-hop neighbors rendered at correct radial distance, dimmer than center
- [ ] 2-hop neighbors rendered at outer ring, further dimmed
- [ ] Bezier edges connect nodes with gradient stroke (solid for normal links, dashed for wikilinks)

### Navigation

- [ ] Clicking a 1-hop neighbor transitions it to the center position (smooth tween)
- [ ] Breadcrumb bar updates after navigation step
- [ ] Clicking a breadcrumb item navigates back to that node
- [ ] Breadcrumb ellipsis (`…`) appears when history exceeds display width
- [ ] Double-clicking the center node opens the note detail panel

### Cluster folding

- [ ] Notes with ≥3 same-kind neighbors show a cluster supernode badge
- [ ] Cluster badge displays the member count
- [ ] Single-member groups are NOT folded into a cluster

### Local / Global toggle + fold threshold slider

- [ ] Toolbar renders Local/Global toggle buttons
- [ ] FOLD_THRESHOLD slider is present and draggable
- [ ] Changing FOLD_THRESHOLD re-renders cluster groupings without full reload

### Hover + keyboard

- [ ] Hovering a node for >150ms triggers prefetch (visible in network tab)
- [ ] `←` / `→` arrow keys step through breadcrumb history
- [ ] `Escape` key returns to previous node

### Mini-map

- [ ] Mini-map thumbnail renders in the corner of the canvas
- [ ] Mini-map viewport indicator tracks pan/zoom (if panning is implemented)

### Performance

- [ ] Frame rate remains above 30 fps during a navigation transition on a note with 20+ neighbors (check via browser dev tools Performance panel)
- [ ] No visible jank when rapidly clicking between nodes

### Accessibility

- [ ] With macOS "Reduce Motion" enabled, transitions snap to final position without animation (pending prefers-reduced-motion implementation — will fail until that deferred item is completed)

---

## Summary

| Area | Status |
|------|--------|
| Core layout + physics | Complete (T4–T5) |
| Cluster folding | Display complete; expansion not wired (T9) |
| Renderer (nodes, edges, depth) | Complete (T10–T12) |
| Viewport + parallax | Complete (T13) |
| Mini-map draw + hit-test | Draw complete; click-to-navigate not wired (T14) |
| Navigation state machine | Complete (T8) |
| Breadcrumb bar | Complete (T16) |
| Interaction + keyboard | Complete (T15) |
| Prefetch / LRU cache | Complete (T7) |
| Tween / easing | Complete (T6) |
| RAF render loop | Complete (T23) |
| Feature flag (`canvas_radial_navigation`) | Complete (T3) |
| Relation-type sector routing | Blocked on `NoteLinkDto` schema gap |
| prefers-reduced-motion | Deferred |
| Manual visual QA | Pending human walkthrough |
