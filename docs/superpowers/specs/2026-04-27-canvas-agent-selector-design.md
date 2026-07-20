# Canvas Agent Selector — Design Spec

**Date:** 2026-04-27
**Status:** Approved (pending implementation plan)
**Related:**
- `docs/superpowers/specs/2026-04-27-canvas-agent-id-unification-design.md` (DB-side migration that unified existing rows under `"main"`; this spec is the UI-side counterpart)
- `docs/superpowers/specs/2026-04-27-canvas-elastic-node-drag-design.md` (parallel canvas work; no shared code)

---

## 1. Problem

The memory canvas (`interfaces/webchat/src/views/canvas/`) renders memory data scoped to a single agent. Aleph's data model isolates memory **per agent** — every agent has its own independent graph. Today the canvas always shows the `main` agent because:

- The 4 graph JSON-RPC handlers (`graph.query`, `graph.neighbors`, `graph.search`, `graph.node_detail`) hardcode `crate::routing::DEFAULT_AGENT_ID` at 8 call sites in `src/gateway/handlers/graph.rs` (lines 102, 148, 162, 232, 249, 259, 321, 406). One of them carries the explicit comment `// TODO: derive from request when multi-agent is wired`.
- `RadialCanvasView` (`interfaces/webchat/src/views/canvas/mod.rs`) has no UI control to choose an agent and never sends `agent_id` on any RPC call.

The user request: **"canvas 需要以 agent_id 进行选择显示"** — provide a dropdown in the canvas view to switch which agent's memory graph is rendered.

The store layer is already agent-isolated. `MemoryStore::get_graph_data(agent_id, …)`, `get_note_index(node_id, agent_id)`, `get_incoming_links(node_id, agent_id)` etc. all take `agent_id` as a formal parameter. The fix is purely "let the JSON-RPC request pass `agent_id` through to the existing parameter" plus a UI control to set it.

## 2. Goals

- Add an **agent selector dropdown** above the existing canvas toolbar so users can switch between any agent's memory graph.
- Plumb **`agent_id` through 4 graph JSON-RPC endpoints** (`graph.query`, `graph.neighbors`, `graph.search`, `graph.node_detail`) as an optional parameter, falling back to the default agent when omitted.
- **Reset all canvas view state on agent switch** (graph data, selection, breadcrumb, search query, fold threshold, zoom, pan, drag) so different agents never share UI state.
- Preserve **backward compatibility** with existing JSON-RPC clients (CLI, tests, future MCP wrappers) that don't send `agent_id`.

## 3. Non-Goals

| Excluded | Why |
|---|---|
| Create / edit / delete agent UI from the canvas | Already provided in the agent management view; out of scope for "selector" |
| Agent list pagination | Realistic agent count is < 100; flat dropdown is sufficient |
| Cross-page agent state synchronisation | The dashboard's other views (chat, settings, etc.) maintain their own agent context; deliberately not shared |
| Persist last-used agent_id in URL or localStorage | Default agent on next session is acceptable; can revisit if users complain |
| Live updates when an agent is created/deleted in another tab | Manual `↻` refresh button covers the edge case |
| Per-agent permission / authorization | All agents in this profile are owned by the same user |

## 4. Approach

Five-part change, sequenced backend → frontend:

1. **Backend:** add `agent_id: Option<String>` to 4 graph params structs; replace 9 hardcoded `DEFAULT_AGENT_ID` references with `params.agent_id.as_deref().unwrap_or(DEFAULT_AGENT_ID)`. No store-layer changes.
2. **Frontend signals:** introduce `agent_id`, `agents`, `agents_loading`, `agents_error` signals in `RadialCanvasView`. Initial value of `agent_id` is fetched from `agents.list().default_id`.
3. **Frontend component:** new `AgentSelectorBar` placed above `CanvasToolbar`. Renders a `<select>` plus a `↻` refresh button; reports selection changes by writing the `agent_id` signal.
4. **Frontend reset Effect:** new `Effect::new` subscribes to `agent_id`. On change (not first run), it resets every canvas view signal to its default; the existing 4 graph-fetch Effects re-trigger automatically because they now also subscribe to `agent_id`.
5. **Wire `agent_id` into existing GraphApi calls:** `GraphApi::query`, `GraphApi::neighbors`, `GraphApi::search`, `GraphApi::node_detail` each gain an `agent_id: String` parameter that is included in the outgoing JSON params.

## 5. Component Detail

### 5.1 Backend — `src/gateway/handlers/graph.rs`

Each of `GraphQueryParams`, `GraphNeighborsParams`, `GraphSearchParams`, `GraphNodeDetailParams` gains a single optional field:

```rust
#[derive(Debug, Deserialize)]
pub struct GraphQueryParams {
    pub limit: usize,
    #[serde(default)]
    pub kind_filter: Vec<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
}
```

Inside each `handle_*_impl`, the existing 9 hardcoded references become:

```rust
let agent_id = params
    .agent_id
    .as_deref()
    .unwrap_or(crate::routing::DEFAULT_AGENT_ID);
```

The `// TODO: derive from request when multi-agent is wired` comment at line 249 is removed; the TODO is now resolved.

No changes are needed in `src/memory/store/sqlite/*` — `get_graph_data`, `get_note_index`, `get_incoming_links` already accept `agent_id` as a parameter.

### 5.2 Frontend — `interfaces/webchat/src/views/canvas/agent_selector.rs` (new)

```rust
#[component]
pub fn AgentSelectorBar(
    agent_id: RwSignal<String>,
    agents: RwSignal<Vec<AgentSummary>>,
    default_id: RwSignal<String>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_refresh: Callback<()>,
) -> impl IntoView { … }
```

Render rules:

- **Loading**: skeleton text `Loading agents…`, dropdown disabled.
- **Error**: inline `Failed to load agents · Retry` (Retry click invokes `on_refresh`). Dropdown still shows the current agent_id as a single static option so the canvas remains usable.
- **Loaded**: native `<select>` with one `<option>` per agent. Option text format: `{emoji} {name} ({id})` where `emoji` and `name` fall back to empty string and `id` respectively when absent. The default agent's option appends ` ★` to the visible label.
- **Refresh button**: small `↻` icon button beside the select. Click sets `loading=true`, calls `AgentsApi::list()`, updates `agents` and `default_id` signals; does NOT touch `agent_id` (preserves the current selection unless that agent disappeared, in which case `agent_id` falls back to `default_id`).

The `<select>`'s `on:change` handler reads the new value and writes it to `agent_id`. That is the component's only side effect.

### 5.3 Frontend — `RadialCanvasView` integration in `mod.rs`

New signals near the top of the component (the webchat is a separate WASM crate without access to the server-side `DEFAULT_AGENT_ID` constant, so the placeholder is the literal `"main"` — kept in sync with the server's default by convention; if they ever diverge, the worst case is one extra graph fetch on mount because `AgentsApi::list().default_id` overrides the placeholder once it resolves):

```rust
let agent_id = RwSignal::new("main".to_string());
let agents = RwSignal::new(Vec::<AgentSummary>::new());
let default_agent_id = RwSignal::new("main".to_string());
let agents_loading = RwSignal::new(false);
let agents_error = RwSignal::new(None::<String>);
```

`spawn_local` on mount calls `AgentsApi::list()`, populates `agents` and `default_agent_id` from the response, and seeds `agent_id = response.default_id` only when the response value differs from the current signal value (avoids a spurious reset Effect run when the server default already matches the placeholder). Failure path writes `agents_error` and leaves `agent_id` at the placeholder.

`AgentSelectorBar` is inserted as a sibling **before** `CanvasToolbar` in the JSX-equivalent tree.

### 5.4 Reset Effect

```rust
Effect::new(move |prev: Option<String>| {
    let current = agent_id.get();
    if let Some(p) = prev.as_ref() {
        if *p != current {
            nodes.set(vec![]);
            edges.set(vec![]);
            clusters.set(vec![]);
            selected_node.set(None);
            breadcrumb.set(vec![]);
            search_query.set(String::new());
            fold_threshold.set(DEFAULT_FOLD_THRESHOLD);
            zoom.set(DEFAULT_ZOOM);
            pan.set((0.0, 0.0));
            drag_state.set(DragState::Idle);
        }
    }
    current
});
```

Two correctness notes:

- The closure returns `current`, so on first run `prev = None` and no reset happens (avoiding a redundant clear before the initial fetch).
- The 4 existing graph-fetch Effects (`query`, `neighbors`, `search`, `node_detail`) each gain a `let _ = agent_id.get();` subscription so they automatically re-trigger when `agent_id` changes. They include the new value in the outgoing GraphApi calls.

### 5.5 GraphApi signature changes

`interfaces/webchat/src/api/graph.rs` (existing): each method gets an `agent_id: &str` parameter that becomes `agent_id` in the JSON params object. All 4 call sites in `mod.rs` are updated to pass `agent_id.get()` (or `agent_id.get_untracked()` inside event handlers).

## 6. Error Handling

| Failure | Behavior |
|---|---|
| `agents.list` RPC fails on mount | Dropdown shows error inline with retry link. `agent_id` keeps the placeholder `"main"`; canvas still loads main agent's graph. |
| User-selected agent has no memory data | Existing empty-state UI in `RadialCanvasView` already handles `nodes.is_empty()`; no new code needed. |
| User-selected agent was deleted in another tab between list-fetch and selection | `graph.query` will return success with empty data (store filters by agent_id); canvas shows empty state. User can click `↻` to refresh the list and remove the stale option. |
| Network failure on a graph RPC after switching | Existing per-Effect error handling in `mod.rs` shows the error toast; agent_id signal is unaffected. |

## 7. Testing

### 7.1 Backend (`src/gateway/handlers/graph.rs`)

Add 4 unit tests, one per handler, each with two assertions:

1. When the params struct includes `agent_id: Some("alpha".into())`, the store mock's `get_graph_data` (or equivalent) is called with `"alpha"`.
2. When `agent_id: None`, the store mock is called with `DEFAULT_AGENT_ID` (`"main"`).

These follow the existing handler test pattern (mock `MemoryStore`, build params, invoke handler, assert mock recorded args).

### 7.2 Frontend — `agent_selector.rs`

Use Leptos's testing utilities (`mount_to_body` + signal injection) to cover:

- Default agent's option ends with ` ★`; non-default options do not.
- Option without `name` falls back to `id`; option without `emoji` renders without leading emoji.
- `on:change` writes the selected `id` to the `agent_id` signal.
- Refresh button click invokes the `on_refresh` callback.
- Error state renders `Failed to load agents · Retry`.

### 7.3 Frontend — integration in `RadialCanvasView`

One integration test that:

1. Renders `RadialCanvasView` with mock `GraphApi` and `AgentsApi`.
2. Awaits initial mount, verifies `GraphApi::query` was called with the default agent_id from `AgentsApi::list().default_id`.
3. Changes the `agent_id` signal to a different agent.
4. Asserts: nodes/edges/clusters/selected_node/breadcrumb/search_query/fold_threshold/zoom/pan/drag_state are all reset to their defaults; second `GraphApi::query` call carries the new agent_id.

E2E coverage: this feature joins the next webchat manual smoke pass; no automated browser test added.

## 8. Sequencing

1. Backend params + handler threading (§5.1) — landable on its own; existing clients keep working because `agent_id` is `Option`.
2. Frontend GraphApi signature + integration (§5.5, §5.3, §5.4) — add `agent_id` parameter and reset Effect.
3. Frontend `AgentSelectorBar` (§5.2) — visible UI ships last.

Each step is independently testable.

## 9. Risks

- **Risk: agent_id mismatch between list fetch and graph fetch on slow networks.** The user could click a still-loading dropdown. Mitigation: dropdown is disabled while `agents_loading` is true.
- **Risk: state-reset Effect runs before the initial graph-fetch Effect on mount.** Mitigated by the `prev: Option<String>` guard — the reset body only runs when `prev` is `Some` and differs from `current`, so initial mount is a no-op.
- **Risk: `default_id` from server differs from the placeholder `"main"` and triggers an unwanted reset.** Specifically, on mount we set `agent_id = "main"`, then `AgentsApi::list()` resolves and writes `agent_id = response.default_id`. If `default_id ≠ "main"`, the reset Effect fires once. Acceptable: it just clears empty state and re-issues the initial graph fetch with the correct agent. To make this provably correct we use `agent_id.set(response.default_id)` only when it differs from the current value; otherwise skip the write.
- **Risk: A user with zero agents (fresh install).** `AgentsApi::list()` would return an empty list. Dropdown shows "No agents available" disabled state; canvas falls back to `DEFAULT_AGENT_ID` placeholder which yields an empty graph (the existing empty-state path).
