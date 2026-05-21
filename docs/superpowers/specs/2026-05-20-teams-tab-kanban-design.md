# Teams Tab + Kanban Visualization — Design Spec

**Date**: 2026-05-20
**Author**: AI co-author (Aleph rootazero)
**Status**: Draft → ready for implementation
**References**:
- hermes-agent (`/Volumes/TBU4/Github/hermes-agent`) — backend kanban inspiration
- hermes-web-ui (`/Volumes/TBU4/Github/hermes-web-ui`) — UI inspiration
- `docs/reference/MULTI_AGENT_SYSTEM.md`
- `docs/reference/GATEWAY.md`
- CLAUDE.md R3 (Core Minimalism), R7 (LLM Sovereignty), R10 (Thin Harness)

---

## 1. Goal

Aleph Panel currently exposes "Teams" as a sub-page of Dashboard (`/dashboard/teams`) with a **collapsible-card team list** view only. The user wants:

1. **Promote Teams to a top-level Panel tab** (parallel to Chat / Dashboard / Memory / Agents / Settings).
2. **Add a Kanban task board** inside the Teams tab visualizing `CoordTask` lifecycle for the selected team.
3. **Migrate the dashboard sidebar's "Teams" entry** to the new tab; remove the now-orphaned route.
4. **Wire the unwired bits** of the multi-agent backend so the kanban can render live data: expose `coord_tasks.*` RPC methods, emit `team.task.*` topics to the `EventBus`, and let the Panel subscribe.
5. **Optimize** the `derive_status()` N+1 dependency query when listing tasks.

Per the user's principles: learn from hermes, don't copy. Fuse hermes' lifecycle insights with Aleph's domain model (5-state CoordTaskStatus + event sourcing + worktree isolation already shipped). No destructive refactors.

---

## 2. Non-Goals

- **No drag-and-drop reordering across columns**. The Aleph state machine is rule-driven (Pending → InProgress via claim/start, etc.); UI-driven re-sequencing of priority is in scope, but UI-driven status mutation is **explicit action** (button in card detail), not drag.
- **No hermes-style claim/heartbeat machinery**. Aleph already has `acquire_lock` / `release_lock` / `release_stale_locks` in `CoordTaskStore`; we reuse it.
- **No new tables / schema migrations**. All new behavior is RPC + event wiring + UI on top of existing storage.
- **No new "team" abstractions**. We do not introduce hermes' `swarm topology` / `blackboard comments` / `task_runs` history table; Aleph's `team_events` + `coord_task_dependencies` already cover the kanban needs.
- **No notification subscriptions to home platforms** (telegram/slack push on task completion). Out of scope here — separate cycle.
- **No archived state**. Aleph's status enum doesn't have it; we use the existing `disband_team` + `delete_team` lifecycle for cleanup.

---

## 3. High-Level Architecture

```
┌────────────────────────────────────────────────────────────────┐
│                       Panel (Leptos/WASM)                       │
│                                                                  │
│   PanelMode::Teams ──► /teams                                   │
│         ▲                                                        │
│         │              ┌────────────────────┐                   │
│         │              │  TeamsTabShell     │                   │
│         │              │  ├ Overview sub    │ (re-org of old   │
│         │              │  │   = list view   │  TeamsView)      │
│         │              │  └ Kanban sub      │ (new)            │
│         │              └────────────────────┘                   │
│         │                                                        │
│         │  RPC: teams.list / teams.get                          │
│         │       teams.list_tasks   (NEW)                        │
│         │       teams.update_task  (NEW)                        │
│         │  WS:  subscribe_topic("team.*")                       │
│         ▼                                                        │
│   DashboardState ── WebSocket ──► Gateway                       │
└──────────────────────────────│─────────────────────────────────┘
                               ▼
┌────────────────────────────────────────────────────────────────┐
│                    aleph-server (Rust)                          │
│                                                                  │
│   gateway::handlers::teams  ◄── new coord_tasks methods         │
│         │                                                        │
│         ▼                                                        │
│   teams::store (SQLiteTeamStore)                                │
│   agents::swarm::tasks::store (SqliteCoordTaskStore)            │
│         │                                                        │
│         ├──► (NEW) emits TopicEvent("team.<id>.task.<verb>")    │
│         │    on create / update / status-change                 │
│         │                                                        │
│         └──► EventBus (gateway::event_bus::TopicEvent)          │
│                  │                                               │
│                  └─► Panel WS subscription                       │
└────────────────────────────────────────────────────────────────┘
```

---

## 4. Key Design Decisions

### 4.1 Column model — Aleph 5-state, not hermes 7-state

| Hermes column | Aleph mapping | Decision |
|---|---|---|
| triage      | — | Drop. Aleph has no human-triage gate; CoordTask is created via LLM tools. |
| todo        | `Pending`    | Use Aleph name. |
| scheduled   | — | Drop. Aleph does not schedule tasks by time. |
| ready       | `Pending` (deps resolved) | Fold into Pending column; "ready" is a sub-state shown as a badge ("✓ deps met"). |
| running     | `InProgress` | Use Aleph name. |
| blocked     | `Blocked` (derived) | Use Aleph name. Recomputed at list time. |
| done        | `Completed`  | Use Aleph name. |
| archived    | — | Drop. Use team disband/delete instead. |

**Final columns** (left→right): **Pending · Blocked · InProgress · Completed · Failed**.
- `Cancelled` rolls into a "show cancelled" filter toggle (rare state).
- This matches Aleph's existing `CoordTaskStatus` enum 1:1; no new state.

### 4.2 View form — horizontal kanban, no drag-and-drop

Hermes uses **vertical collapsible** columns because their Naive UI dashboard is desktop-first but constrained-width. Aleph Panel is a Leptos app that can host a wide canvas. The user wrote 看板 (kanban) which colloquially implies the **horizontal-columns metaphor**.

- **Wide viewport (≥ 1024px)**: five columns side-by-side, equal width, scrolling vertical inside each column.
- **Narrow viewport (< 1024px)**: columns stack vertically, each collapsible (hermes pattern). Implementation: Tailwind responsive classes — no JS branching.
- **Drag-and-drop** is **not in v1**. Status transitions happen via card detail drawer ("Start" / "Block" / "Complete" / "Fail" buttons) which gate on Aleph's existing CoordTask update path. Reasoning: drag-and-drop on a dependency-DAG without invariant checks is a footgun (e.g., dragging a task with unresolved parents to InProgress would either silently fail or bypass guards). v2 may add it once we have explicit transition rules in `CoordTaskStore::update_task`.

### 4.3 Migration strategy — full move, no compatibility shim

- `PanelMode::Teams` added; sidebar button parallel to Chat/Dashboard/Memory/Agents/Settings.
- Existing `views/teams.rs` → refactored to `views/teams/mod.rs` + `views/teams/overview.rs` + `views/teams/kanban.rs`.
- Dashboard sidebar entry `/dashboard/teams` removed; route removed from `DashboardRouter` in `app.rs`. `dashboard.sidebar.teams` i18n key deleted from both en/zh locale files.
- No deprecation alias. `/dashboard/teams` becomes a 404 (which the existing router falls back to `Home`). Acceptable per "no external callers" — Panel is the only consumer.

### 4.4 Inside the Teams tab — sub-navigation

Two sub-views, switched by an internal tab strip at the top of the Teams page:

| Sub-tab | Purpose | Source |
|---|---|---|
| **Overview** | The existing collapsible team-cards list (renamed but content-preserved). Used for team CRUD: disband, delete, inspect members. | Refactor of current `TeamsView`. |
| **Kanban**   | New 5-column board for the **currently-selected team's** CoordTasks. | New `KanbanBoard` component. |

A small **team selector** dropdown sits above the kanban (mirrors hermes' board selector). Selected team persists in a `RwSignal<Option<String>>` in the Teams tab's local state; defaults to the first active team. When no teams exist, the kanban shows an empty-state CTA pointing the user to create one via chat (per R8 "Everything is a Tool" — team creation is an LLM tool, not a UI button).

### 4.5 Real-time updates — EventBus topics, no polling fallback (v1)

Hermes implements WebSocket-stream + 15s visibility-aware polling fallback. We **skip polling for v1** because:
- The Aleph Gateway already maintains a stable persistent WS for the Panel (`DashboardState.is_connected` is a single source of truth).
- On reconnect, the Panel re-fetches via `coord_tasks.list`, which is the equivalent of an interval refresh.
- Adding 15s polling for every team page in WASM is wasteful and against R3 (Core Minimalism).

Wiring:

1. **`coord_tasks` mutations** in `SqliteCoordTaskStore::{create_task, update_task}` accept an `EventBus` handle (via `Arc<EventBus>` injection in the constructor) and emit a `TopicEvent` on success.
2. Topic format: `team.<team_id>.task.<verb>` where verb ∈ `{created, updated, completed, failed, cancelled}`.
3. Payload: `{ task_id, team_id, status, owner, priority, timestamp }` — small enough to flood-safe; Panel re-fetches the task body via `coord_tasks.list(team_id)` if it needs richer data.
4. Panel subscribes once on Teams-tab mount with the pattern `team.*.task.*` via existing `state.subscribe_events` API; debounces refresh by 100 ms (mirrors hermes-web-ui).

### 4.6 Bug fix — `derive_status()` N+1 query

Today: `SqliteCoordTaskStore::list_tasks` iterates results and per-row queries `coord_task_dependencies` to derive Blocked status. On a team with 100 tasks each having 2-3 deps, that's 100-300 extra queries.

Fix: **single batched join** in `list_tasks`. Replace the row-by-row dep lookup with one query that left-joins `coord_task_dependencies` and counts unresolved (status ≠ Completed) parents per task. Status becomes `Blocked` iff unresolved-parent-count > 0 AND raw status is `Pending`. One round-trip instead of N+1. Add a regression test in `agents/swarm/tasks/store.rs` that proves the count is correct for a 3-task chain.

### 4.7 Optional polish — task priority badge & timestamps

Each card shows: subject, owner (agent role), priority badge (Low/Normal/High/Critical with color), relative timestamp (`created_at` or `started_at` depending on status), and an "expand" affordance opening a side drawer with full description + result + dependencies graph snippet. This mirrors hermes-web-ui's card density without copying its styling.

---

## 5. Backend Changes (Phase 1)

### 5.1 New RPC methods in `src/gateway/handlers/teams.rs`

```rust
// teams.list_tasks { team_id: String, status?: CoordTaskStatus, owner?: String }
//   -> Vec<CoordTask>
pub async fn handle_list_tasks(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse;

// teams.update_task { task_id: String, status?: ..., priority?: ..., owner?: ..., result?: ... }
//   -> CoordTask
pub async fn handle_update_task(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
    event_bus: Arc<EventBus>,
) -> JsonRpcResponse;
```

Both register through the existing gateway method dispatcher. We **do not** introduce a separate `coord_tasks` namespace — `teams.list_tasks` / `teams.update_task` keep the API surface coherent with the existing `teams.list` / `teams.get` / `teams.disband` / `teams.delete`.

### 5.2 EventBus emission in `agents::swarm::tasks::store`

```rust
pub struct SqliteCoordTaskStore {
    pool: SqlitePool,
    bus: Option<Arc<EventBus>>,   // NEW (Option so existing tests don't break)
}

impl SqliteCoordTaskStore {
    fn emit(&self, team_id: Option<&str>, verb: &str, payload: serde_json::Value) {
        let Some(bus) = &self.bus else { return; };
        let Some(team_id) = team_id else { return; };
        let topic = format!("team.{team_id}.task.{verb}");
        bus.publish(TopicEvent { topic, data: payload, timestamp: now() });
    }
}
```

Bus injection happens in `AppContext` construction; tests pass `None`.

### 5.3 `derive_status()` batched query

Replace the per-row loop in `list_tasks` with:

```sql
SELECT t.*,
       COALESCE(SUM(CASE WHEN p.status != 'Completed' THEN 1 ELSE 0 END), 0) AS unresolved_parents
  FROM coord_tasks t
  LEFT JOIN coord_task_dependencies d ON d.task_id = t.id
  LEFT JOIN coord_tasks p ON p.id = d.depends_on
 WHERE t.team_id = ? -- when filter applied
 GROUP BY t.id
```

`derive_status(raw_status, unresolved_parents)` becomes a pure function — easy to unit-test.

---

## 6. Panel Changes (Phase 2)

### 6.1 PanelMode + routing

| File | Change |
|---|---|
| `interfaces/webchat/src/components/bottom_bar.rs` | Add `PanelMode::Teams`; extend `from_path` to recognize `/teams`; add `BottomBarItem` with a people-cluster icon between Memory and Agents. |
| `interfaces/webchat/src/app.rs` | Add new `<div>` panel for `Teams` mode wrapping `<TeamsView />`. Remove `/dashboard/teams` route. |
| `interfaces/webchat/src/components/dashboard_sidebar.rs` | Remove `<SidebarItem href="/dashboard/teams">`. |
| `interfaces/webchat/src/components/mode_sidebar.rs` | Add Teams mode pattern (lightweight sidebar with sub-tab selector + team selector). |

### 6.2 Views directory reshape

Before:
```
views/
  teams.rs            ← collapsible cards
```

After:
```
views/
  teams/
    mod.rs            ← TeamsView shell with sub-tab strip
    overview.rs       ← extracted from old teams.rs (verbatim move)
    kanban.rs         ← new
    components/
      board.rs        ← KanbanBoard
      column.rs       ← KanbanColumn
      task_card.rs    ← TaskCard
      task_drawer.rs  ← TaskDetailDrawer
      team_selector.rs
```

Sized to honor CLAUDE.md P2 (high cohesion, < 800 lines per file). The current `teams.rs` is ~300 lines and gets split naturally.

### 6.3 RPC client API (`interfaces/webchat/src/api/teams.rs`)

Add:
```rust
impl TeamsApi {
    pub async fn list_tasks(state: &DashboardState, team_id: &str, filter: TaskFilter)
        -> Result<Vec<CoordTaskDto>, String>;
    pub async fn update_task(state: &DashboardState, task_id: &str, patch: TaskPatch)
        -> Result<CoordTaskDto, String>;
}
```

DTO types live in `interfaces/webchat/src/api/teams.rs` (the existing module). They mirror the Rust types in `src/agents/swarm/tasks/mod.rs` but with stringly-typed timestamps for `serde` ergonomics.

### 6.4 Event subscription

In `TeamsView::on_mount`:
```rust
let sub_id = state.subscribe_events(move |evt| {
    if evt.topic.starts_with("team.") && evt.topic.contains(".task.") {
        debounce_refresh(); // 100 ms
    }
});
on_cleanup(move || state.unsubscribe(sub_id));
```

Refresh = re-call `list_tasks(selected_team_id)`.

---

## 7. Data Flow Example

```
User opens Teams tab
  → Panel: PanelMode::Teams active, route /teams
  → Component mount: TeamsView fetches teams.list
  → Sub-tab default = Overview
  → User clicks "Kanban" sub-tab
  → Component mount: KanbanBoard
       1. Read team_selector signal (defaults to first active team)
       2. Call teams.list_tasks(team_id) → server runs the batched query
       3. Group results by CoordTaskStatus → 5 columns
       4. Subscribe to "team.*" topic pattern
  → User clicks a task card → side drawer opens with details
  → User clicks "Start" button
       1. Panel: teams.update_task(id, { status: InProgress })
       2. Server: CoordTaskStore::update_task does its existing transition
       3. Server: emits TopicEvent("team.<id>.task.updated", { … })
       4. EventBus broadcasts → Panel WS handler fires → debounce → refresh
       5. Card now in InProgress column
```

---

## 8. Testing Strategy

### 8.1 Backend

- **Unit** (`agents/swarm/tasks/store.rs`): batched `derive_status` correctness on 3-task chain (Pending without deps; Pending with one Completed parent → Pending; Pending with one InProgress parent → Blocked).
- **Unit** (`gateway/handlers/teams.rs`): `handle_list_tasks` filters honored; `handle_update_task` rejects unknown transitions with a clean RPC error envelope.
- **Integration** (`tests/`): create a team + 2 tasks via store, hit `teams.list_tasks` via JSON-RPC, assert envelope shape. Subscribe to `team.<id>.task.*`, update a task, assert event received within 200 ms.

### 8.2 Frontend

- WASM target is hard to unit-test today; the existing webchat crate has no test harness beyond compile-time check. We rely on:
  - `cargo check -p aleph-panel` after each touched file.
  - Manual `just dev` smoke: open Panel, click Teams tab, click Kanban sub-tab, click Start on a card, verify status flips without a full reload.
  - Documented in spec: not adding new wasm test infrastructure for v1.

### 8.3 i18n

- Add keys to `interfaces/webchat/locales/en.json` and `zh.json`:
  - `nav.teams`
  - `teams.subtab.overview` / `teams.subtab.kanban`
  - `teams.kanban.columns.pending` / `.blocked` / `.in_progress` / `.completed` / `.failed`
  - `teams.kanban.empty_state.no_teams` / `.no_tasks`
  - `teams.kanban.actions.start` / `.block` / `.complete` / `.fail`
- Remove `dashboard.sidebar.teams` from both files (orphan after migration).

---

## 9. Rollout

Single PR via worktree branch `aleph-teams-tab-kanban`. No feature flag — the Teams tab simply appears.

Backout plan: revert the single PR. Storage schema untouched, so no data migration concerns.

---

## 10. Risks

| Risk | Mitigation |
|---|---|
| Aleph CoordTaskStore is consumed by tests that don't pass an EventBus → constructor change breaks them. | New `bus` field is `Option<Arc<EventBus>>` with `None` default; new constructor `with_event_bus()`. Existing tests untouched. |
| Status transition guard logic doesn't exist; users could request invalid transitions via update_task. | `handle_update_task` calls existing `CoordTaskStore::update_task` whose validation is the source of truth. UI buttons only show transitions valid from current status (computed in Leptos). |
| WS event flood on a busy team. | 100 ms debounce on the Panel side + server-side topic is opt-in (clients subscribe to specific patterns). Hermes uses 5 s server-side poll which is much coarser; ours is finer but the cost is acceptable for human-rate task transitions. |
| The `views/teams/` directory restructure conflicts with concurrent main-branch changes. | Worktree based on origin/main HEAD; one merge cycle at end. Memory shows recent main churn is in `harness/` and `teams/dispatcher/`, not in Panel. |

---

## 11. Out of Scope (Future Cycles)

- Drag-and-drop status transitions with rule-driven guards.
- Subtask nesting / hierarchical task view (Aleph deps are flat DAG today).
- Task search and full-text filter.
- Bulk operations (multi-select + batch update).
- Notifier subscriptions: push task completion to telegram/email/slack via existing channel system.
- WebSocket protocol versioning for the new topics.
- Kanban for the "memory canvas" view (different domain).

---

## 12. Approval Note

This spec respects the user's directives:
- **学习 Hermes 又超越它**: keep horizontal kanban metaphor + WS event push, drop unnecessary state-machine bloat (triage / scheduled / archived) and naive polling fallback.
- **不照搬**: reuse Aleph's CoordTask/EventBus/worktree infra; no new tables, no new abstractions.
- **不破坏性重构**: existing TeamsView code becomes the Overview sub-view byte-for-byte (verbatim move); only the file path changes.
- **修复 + 优化 + 增强**: N+1 query fix; live event push; new RPCs; Kanban UI.
- **充分利用现有模块**: TeamStore, CoordTaskStore, EventBus, DashboardState, ui::Card, i18n, all reused.
- **worktree 落地**: implementation occurs in a fresh worktree branch.

Implementation will be planned via the `superpowers:writing-plans` skill in the next step and executed in a worktree.
