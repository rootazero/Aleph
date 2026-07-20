# Memory Hub Unification — Design Spec

**Date:** 2026-06-20
**Scope:** Aleph Panel (Leptos/WASM frontend) — direction #2 of the memory-panel roadmap.
**Status:** Approved design, ready for implementation plan.

> Direction #1 (Vault table deep-refactor) is done and merged to `main`.
> Direction #3 (backend RPC for missing pillars + retrieval-score transparency) is a **separate** subsystem and gets its own spec/plan cycle after #2.

---

## Goal

Fuse the two currently-disjoint memory surfaces — the Canvas radial graph (`/memory`) and the Vault table (`/dashboard/memory`) — into a single "Memory Hub" with an in-place graph⇄table view toggle, shared agent selection, shared search, and bidirectional selection linking. **Pure frontend, zero backend RPC changes, zero new dependencies.**

## Constraints (Global)

- **Frontend only.** No changes to `src/gateway/handlers/` or any Rust core. Reuse existing JSON-RPC verbatim (`graph.*`, `memory.*`).
- **No new dependencies.**
- **No `cargo check` / `cargo build` after implementation** — per standing resource-governance constraint, commit directly. Correctness is ensured by-construction (reuse typed APIs, mirror existing patterns) plus pure-logic unit tests that need no WASM toolchain.
- **Branch isolation.** All work in a new git worktree branch; never touch `main` directly during implementation.
- **Entropy reduction.** Remove the duplicated agent selectors and the Vault-local search signals that this refactor obsoletes.
- Reply language: Chinese; code comments: English.

---

## Architecture

Add a thin host component `MemoryHub` that owns the shared toolbar and switches between the two existing views via **CSS `display` toggling** (the same keep-alive pattern `MainContent` already uses), so neither view re-mounts on toggle. All shared state lives in the existing `MemoryState` (already `#[derive(Clone, Copy)]`). The two inner views (`CanvasView`, `Memory`/Vault) keep their internals; they are adapted only to read shared signals instead of local/duplicated ones.

```
┌─ Memory mode (/memory) ───────────────────────────────────┐
│ ┌─ Top toolbar (NEW toolbar.rs, shared) ──────────────────┐ │
│ │  [🕸 Graph | ▤ Table]   🔍 search…   ▾ Agent select    │ │  → writes MemoryState
│ └─────────────────────────────────────────────────────────┘ │
│ ┌─ View host (display toggle, both views stay mounted) ───┐ │
│ │  display:graph → <CanvasView/>   (radial graph, kept)    │ │
│ │  display:table → <Memory/>       (Vault table, kept)     │ │
│ └─────────────────────────────────────────────────────────┘ │
└───────────────────────────────────────────────────────────┘
Sidebar MemorySidebar becomes view-aware:
  graph → fold slider + node detail ; table → facet info
```

## Routing

- `PanelMode::Memory` renders `<MemoryHub/>` (instead of rendering `CanvasView` directly).
- `/dashboard/memory` → **redirect to `/memory?view=table`** (back-compat); remove its entry from `DashboardSidebar` (entropy reduction).
- On load, `MemoryHub` reads the `?view=` query param to pick the initial view; default `graph` (preserves current behavior when entering via `/memory`).

## Shared State (`state/memory.rs`)

Extend `MemoryState` (stays Copy):

- **NEW** `memory_view: RwSignal<MemoryView>` — current view (Graph/Table).
- **NEW** `highlight_note_id: RwSignal<Option<String>>` — reverse-link target (graph→table).
- **REUSE** `agent_id`, `agents`, `search_query`, `selected_node`, `focus_id`.

## Search (single box, dual semantics)

The toolbar search box two-way-binds `MemoryState.search_query`.

- **Table view:** submitting `search_query` drives server-side `memory.search` (Raw facet, paginated) — existing Vault logic, now reading the shared signal.
- **Graph view:** the same `search_query` drives client-side node highlight/focus — existing Canvas logic (Canvas already reads `mem.search_query`).
- Switching views preserves the query text; each view re-interprets it. **No backend change.**

## Cross-View Selection Linking (bidirectional)

**Forward (table → graph)** — robust:
A table row gains a "🕸 View in graph" action → writes `selected_node + focus_id`, sets `memory_view = Graph`. Canvas already reacts to these signals and centers on the node.

**Reverse (graph → table)** — with an honest boundary:
`NodeDetailPanel` gains a "▤ View in list" action → sets `memory_view = Table` and `highlight_note_id`. The Vault view watches `highlight_note_id`:
- If the note id is within the already-loaded 1000-note window → switch to that note's facet, jump to the page containing it, highlight the row, open the detail drawer.
- If the note is outside the window → switch to table view and show an inline notice: "This note is outside the current window; use search to locate it."
- **Why the boundary:** `NotesTable` is a client-side faceted window, not a server-side by-id lookup. Cross-window by-id location would require a new backend RPC (that belongs to direction #3). #2 stays strictly zero-backend.

## Error Handling

- Forward link: graph `neighbors` fetch failure → Canvas's existing error state.
- Reverse link not-found: inline notice (never panic, never silent).
- Search failure: existing Vault/Canvas error states.

## File Structure

```
views/memory_hub/
  mod.rs        — MemoryHub() host: toolbar + display-toggle of both views
                  + read ?view= + dispatch reverse-link (highlight_note_id)
  toolbar.rs    — shared toolbar: view toggle / search box / agent selector
  view_kind.rs  — pure logic: MemoryView enum + parse_view_param()
                  + locate_note(window, id) -> Option<(MemoryFacet, u32 page)>  [unit-tested]

state/memory.rs                  — + memory_view, + highlight_note_id
app.rs                           — Memory mode → MemoryHub ; /dashboard/memory → redirect
components/mode_sidebar.rs       — MemorySidebar drops agent selector (moved to toolbar);
                                   keeps fold slider + node detail
components/dashboard_sidebar.rs  — remove /dashboard/memory link (entropy reduction)
views/memory/{mod,facets}.rs     — delete AgentFilter; read mem.search_query;
                                   row "View in graph" action; watch highlight_note_id
views/canvas/mod.rs              — drop sidebar agent-selector dependency;
                                   NodeDetailPanel "View in list" action
```

## Testing

`view_kind.rs` carries pure-logic unit tests (no WASM, no `cargo check` required at author time, runnable later via `cargo test`):

- `parse_view_param`: `"table"`→Table, `"graph"`→Graph, missing/unknown→default Graph.
- `locate_note`: hit (returns correct facet + page), miss (returns None), pagination-boundary math.

This continues the `data.rs` testable-pure-logic pattern established in direction #1.

## Out of Scope (explicit)

- Any backend RPC change (deferred to #3).
- Cross-window by-id note lookup (needs new RPC → #3).
- Unifying the two detail surfaces (`NodeDetailPanel` sidebar vs `DetailDrawer` right-panel) into one — large, risky, separate refactor. Both are kept; linking is via shared `selected_node` / `highlight_note_id`, not by merging the panels.
- Retrieval-score transparency / dream insights / corrections listing (all #3).
