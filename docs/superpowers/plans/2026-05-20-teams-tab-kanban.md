# Teams Tab + Kanban Visualization — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Promote Aleph Panel's Teams view from a dashboard sub-page to a top-level tab, add a kanban task board over `CoordTask`, and wire the unwired bits between the multi-agent backend and the Leptos UI.

**Architecture:** Five new files in `interfaces/webchat/src/views/teams/`, two new RPC handlers in `gateway/handlers/teams.rs`, EventBus injection into `SqliteCoordTaskStore`, an N+1 query batched into a single LEFT JOIN, and the existing `/dashboard/teams` route fully removed (no compat shim). Real-time refresh via the existing `team.*.task.*` topic pattern + the existing `state.subscribe_events` Panel API — no polling fallback.

**Tech Stack:** Rust (alephcore), Tokio, rusqlite, async-trait, serde, tracing, JSON-RPC over WebSocket, Leptos 0.8 (Rust→WASM), Tailwind CSS, leptos_i18n.

**Spec:** `docs/superpowers/specs/2026-05-20-teams-tab-kanban-design.md`

---

## File Map

| Path | Action | Responsibility |
|---|---|---|
| `src/agents/swarm/tasks/store.rs` | Modify | Add `EventBus` injection + batched `derive_status` query + topic emission |
| `src/gateway/handlers/teams.rs` | Modify | Add `handle_list_tasks` + `handle_update_task` |
| `src/gateway/handlers/mod.rs` | Modify | Replace `teams.*` stubs to also stub the two new methods |
| `src/bin/aleph-server/commands/start/builder/handlers/agents.rs` | Modify | Register new handlers + pass event_bus to update_task |
| `src/bin/aleph-server/commands/start/builder/agent_init.rs` | Modify | Inject EventBus into SqliteCoordTaskStore |
| `interfaces/webchat/src/views/teams.rs` | Delete | Replaced by `teams/` directory |
| `interfaces/webchat/src/views/teams/mod.rs` | Create | Tab shell + sub-tab strip + team selector signal |
| `interfaces/webchat/src/views/teams/overview.rs` | Create | Migrated content of the old `teams.rs` (verbatim) |
| `interfaces/webchat/src/views/teams/kanban.rs` | Create | Kanban entry view; mounts board + event subscription |
| `interfaces/webchat/src/views/teams/components/board.rs` | Create | KanbanBoard — five-column layout |
| `interfaces/webchat/src/views/teams/components/column.rs` | Create | KanbanColumn — single status column |
| `interfaces/webchat/src/views/teams/components/task_card.rs` | Create | TaskCard — compact card |
| `interfaces/webchat/src/views/teams/components/task_drawer.rs` | Create | Side drawer with detail + transition buttons |
| `interfaces/webchat/src/views/teams/components/team_selector.rs` | Create | Dropdown to pick the active team |
| `interfaces/webchat/src/api/teams.rs` | Modify | Add `CoordTaskDto` + `list_tasks` + `update_task` |
| `interfaces/webchat/src/components/bottom_bar.rs` | Modify | Add `PanelMode::Teams` variant + bottom-bar button |
| `interfaces/webchat/src/components/mode_sidebar.rs` | Modify | Match `PanelMode::Teams` → render TeamsSidebar |
| `interfaces/webchat/src/components/dashboard_sidebar.rs` | Modify | Remove `/dashboard/teams` sidebar item |
| `interfaces/webchat/src/app.rs` | Modify | Add `PanelMode::Teams` MainContent panel; remove `/dashboard/teams` route |
| `interfaces/webchat/locales/en.json` | Modify | Add `nav.teams` + `teams.*` keys; remove `dashboard.sidebar.teams` |
| `interfaces/webchat/locales/zh.json` | Modify | Same as en.json (Chinese translations) |
| `tests/teams_kanban_e2e.rs` | Create | Integration test: list_tasks + update_task + topic emission |

---

## Phase 1 — Backend wiring

### Task 1: Add `with_event_bus` constructor to `SqliteCoordTaskStore`

**Files:**
- Modify: `src/agents/swarm/tasks/store.rs:122-133`
- Test: `src/agents/swarm/tasks/store.rs` (existing tests module)

- [ ] **Step 1.1: Write failing test in `store.rs` tests module**

Append to the `mod tests` block in `src/agents/swarm/tasks/store.rs` (after the existing tests):

```rust
    #[tokio::test]
    async fn with_event_bus_attaches_bus_without_breaking_existing_methods() {
        use crate::gateway::event_bus::GatewayEventBus;
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let bus = std::sync::Arc::new(GatewayEventBus::new());
        let store = SqliteCoordTaskStore::new(conn).with_event_bus(bus);
        store.migrate().await.expect("migrate");

        // Existing CRUD still works after bus injection
        let task = store
            .create_task(NewCoordTask {
                team_id: Some("team-X".into()),
                subject: "ping".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: json!({}),
            })
            .await
            .unwrap();
        assert_eq!(task.team_id.as_deref(), Some("team-X"));
    }
```

- [ ] **Step 1.2: Run test → expect compile error**

```bash
cargo test -p alephcore --lib agents::swarm::tasks::store::tests::with_event_bus_attaches_bus_without_breaking_existing_methods
```

Expected: compile failure — `with_event_bus` not defined.

- [ ] **Step 1.3: Add the `bus` field and `with_event_bus` builder**

In `src/agents/swarm/tasks/store.rs`, replace lines 122-133 with:

```rust
pub struct SqliteCoordTaskStore {
    conn: Arc<Mutex<Connection>>,
    bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
}

impl SqliteCoordTaskStore {
    /// Create a new store wrapping the given connection.
    /// Call [`migrate`] before using the store.
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            bus: None,
        }
    }

    /// Attach an event bus so the store emits topic events on mutations.
    /// Builder is no-op safe: stores constructed without a bus simply skip emission.
    pub fn with_event_bus(
        mut self,
        bus: Arc<crate::gateway::event_bus::GatewayEventBus>,
    ) -> Self {
        self.bus = Some(bus);
        self
    }
```

(Keep the rest of the `impl SqliteCoordTaskStore` block — `migrate()` — verbatim.)

- [ ] **Step 1.4: Run test → expect pass**

```bash
cargo test -p alephcore --lib agents::swarm::tasks::store::tests::with_event_bus_attaches_bus_without_breaking_existing_methods
```

Expected: PASS, all sibling tests in the same module still PASS.

- [ ] **Step 1.5: Commit**

```bash
git add src/agents/swarm/tasks/store.rs
git commit -m "feat(swarm): add with_event_bus builder to SqliteCoordTaskStore

Optional EventBus injection so the store can emit team.<id>.task.* topics
to the gateway broadcast channel. Existing call sites that pass no bus
keep working unchanged."
```

---

### Task 2: Emit `team.<id>.task.<verb>` topics on create/update

**Files:**
- Modify: `src/agents/swarm/tasks/store.rs` (create_task + update_task)
- Test: `src/agents/swarm/tasks/store.rs` (new test)

- [ ] **Step 2.1: Write failing test for topic emission**

Append to the `mod tests` block in `src/agents/swarm/tasks/store.rs`:

```rust
    #[tokio::test]
    async fn create_and_update_emit_team_task_topics() {
        use crate::gateway::event_bus::GatewayEventBus;
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let bus = std::sync::Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe();
        let store = SqliteCoordTaskStore::new(conn).with_event_bus(bus);
        store.migrate().await.expect("migrate");

        let task = store
            .create_task(NewCoordTask {
                team_id: Some("team-T".into()),
                subject: "do".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: json!({}),
            })
            .await
            .unwrap();

        let evt = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("event received in time")
            .expect("event payload");
        assert!(evt.contains(r#""topic":"team.team-T.task.created""#), "got: {evt}");
        assert!(evt.contains(&task.id), "topic payload missing task id: {evt}");

        let _ = store
            .update_task(
                &task.id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::InProgress),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let evt2 = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("update event received in time")
            .expect("update event payload");
        assert!(evt2.contains(r#""topic":"team.team-T.task.updated""#), "got: {evt2}");
    }
```

- [ ] **Step 2.2: Run test → expect FAIL**

```bash
cargo test -p alephcore --lib agents::swarm::tasks::store::tests::create_and_update_emit_team_task_topics
```

Expected: test runs but timeout on `rx.recv()` — no topic is emitted yet.

- [ ] **Step 2.3: Add the emit helper + call sites**

Add this private helper at the bottom of `impl SqliteCoordTaskStore` (right after the `migrate()` block, before the trait impl):

```rust
    /// Publish a `team.<team_id>.task.<verb>` topic for a single task.
    /// No-op when no bus is attached or the task has no team_id.
    fn emit_task_topic(&self, task: &CoordTask, verb: &str) {
        let Some(bus) = &self.bus else { return; };
        let Some(team_id) = task.team_id.as_deref() else { return; };
        let topic = format!("team.{team_id}.task.{verb}");
        let payload = serde_json::json!({
            "topic": topic,
            "data": {
                "task_id": task.id,
                "team_id": team_id,
                "status": task.status.as_str(),
                "owner": task.owner,
                "priority": task.priority.as_str(),
                "timestamp": now_epoch(),
            },
        });
        let _ = bus.publish_json(&payload);
    }
```

In `create_task` (around line 321 — just before `Ok(())` of the outer function, after `load_task(...).map_err(...)?...`), wrap the return:

```rust
        // Return the fully loaded task (with derived status)
        let task = load_task(&conn, &id)
            .map_err(db_err)?
            .ok_or_else(|| db_err("task disappeared after insert"))?;
        drop(conn); // release lock before emit so subscribers aren't blocked
        self.emit_task_topic(&task, "created");
        Ok(task)
```

In `update_task` (around line 408 — replace the final `load_task(...).map_err(...)?.ok_or_else(...)` block):

```rust
        let task = load_task(&conn, id)
            .map_err(db_err)?
            .ok_or_else(|| db_err(format!("task not found after update: {id}")))?;
        let verb = match task.status {
            CoordTaskStatus::Completed => "completed",
            CoordTaskStatus::Failed => "failed",
            CoordTaskStatus::Cancelled => "cancelled",
            _ => "updated",
        };
        drop(conn);
        self.emit_task_topic(&task, verb);
        Ok(task)
```

- [ ] **Step 2.4: Run test → expect PASS**

```bash
cargo test -p alephcore --lib agents::swarm::tasks::store::tests
```

Expected: all tests pass.

- [ ] **Step 2.5: Commit**

```bash
git add src/agents/swarm/tasks/store.rs
git commit -m "feat(swarm): emit team.<id>.task.<verb> topics on CRUD

create_task → 'created'; update_task → 'updated'|'completed'|'failed'|'cancelled'
depending on final derived status. Bus lock is released before emission so
broadcast subscribers do not block the SQLite write path."
```

---

### Task 3: Fix `derive_status` N+1 with a single LEFT-JOIN query

**Files:**
- Modify: `src/agents/swarm/tasks/store.rs:88-99` (the helper functions) and `list_tasks`
- Test: existing tests + one new

- [ ] **Step 3.1: Write a failing perf-shape test**

The N+1 fix is correctness-preserving, so we add a behavioral test that exercises a 3-task chain and asserts that listing returns derived statuses correctly. (Real perf is hard to assert in a unit test; correctness coverage is the gate.) Append:

```rust
    #[tokio::test]
    async fn list_tasks_derives_blocked_in_a_single_pass() {
        let store = setup_store().await;

        // Chain: A → B → C (B blocked by A, C blocked by B)
        let a = store
            .create_task(NewCoordTask {
                team_id: Some("T".into()),
                subject: "A".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: json!({}),
            })
            .await
            .unwrap();
        let b = store
            .create_task(NewCoordTask {
                team_id: Some("T".into()),
                subject: "B".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![a.id.clone()],
                metadata: json!({}),
            })
            .await
            .unwrap();
        let c = store
            .create_task(NewCoordTask {
                team_id: Some("T".into()),
                subject: "C".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![b.id.clone()],
                metadata: json!({}),
            })
            .await
            .unwrap();

        let all = store
            .list_tasks(CoordTaskFilter {
                team_id: Some("T".into()),
                ..Default::default()
            })
            .await
            .unwrap();

        let by_id: std::collections::HashMap<_, _> =
            all.iter().map(|t| (t.id.clone(), t)).collect();
        assert_eq!(by_id[&a.id].status, CoordTaskStatus::Pending);
        assert_eq!(by_id[&b.id].status, CoordTaskStatus::Blocked);
        assert_eq!(by_id[&c.id].status, CoordTaskStatus::Blocked);
    }
```

- [ ] **Step 3.2: Run new test against current code → expect PASS**

```bash
cargo test -p alephcore --lib list_tasks_derives_blocked_in_a_single_pass
```

Expected: PASS (the existing N+1 path is correct, we're guarding it before refactoring).

- [ ] **Step 3.3: Refactor `list_tasks` to use a single LEFT JOIN**

In `src/agents/swarm/tasks/store.rs`, replace the body of `list_tasks` (lines 413-484) with the version below. The key change: a single SQL query brings back tasks + an `unresolved_parents` column; status derivation becomes a pure function over that count.

```rust
    async fn list_tasks(&self, filter: CoordTaskFilter) -> crate::error::Result<Vec<CoordTask>> {
        let conn = self.conn.lock().await;

        let mut where_clauses: Vec<String> = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1usize;

        let filter_blocked = filter.status == Some(CoordTaskStatus::Blocked);
        let filter_pending = filter.status == Some(CoordTaskStatus::Pending);

        if let Some(ref status) = filter.status {
            if *status == CoordTaskStatus::Blocked || *status == CoordTaskStatus::Pending {
                where_clauses.push(format!("t.status = ?{idx}"));
                values.push(Box::new("pending".to_string()));
                idx += 1;
            } else {
                where_clauses.push(format!("t.status = ?{idx}"));
                values.push(Box::new(status.as_str().to_string()));
                idx += 1;
            }
        }

        if let Some(ref team_id) = filter.team_id {
            where_clauses.push(format!("t.team_id = ?{idx}"));
            values.push(Box::new(team_id.clone()));
            idx += 1;
        }

        if let Some(ref owner) = filter.owner {
            where_clauses.push(format!("t.owner = ?{idx}"));
            values.push(Box::new(owner.clone()));
        }

        let where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };

        // Single pass: tasks + unresolved-parent count + a CSV of dependency ids.
        // GROUP_CONCAT keeps the dependency list inline so we avoid the per-task
        // load_dependencies() round-trip used previously.
        let sql = format!(
            r#"
            SELECT
                t.id, t.team_id, t.subject, t.description, t.status, t.owner,
                t.priority, t.result, t.metadata, t.created_at, t.started_at,
                t.completed_at, t.locked_by, t.locked_at,
                COALESCE(SUM(CASE WHEN dep.status IS NOT NULL AND dep.status != 'completed' THEN 1 ELSE 0 END), 0) AS unresolved_parents,
                GROUP_CONCAT(d.depends_on) AS dep_ids
            FROM coord_tasks t
            LEFT JOIN coord_task_dependencies d ON d.task_id = t.id
            LEFT JOIN coord_tasks dep ON dep.id = d.depends_on
            {where_sql}
            GROUP BY t.id
            ORDER BY t.created_at ASC
            "#
        );

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            values.iter().map(|v| v.as_ref()).collect();
        let mut stmt = conn.prepare(&sql).map_err(db_err)?;

        let rows = stmt
            .query_map(params_ref.as_slice(), |row| {
                let mut task = read_task_row(row)?;
                let unresolved: i64 = row.get(14)?;
                let dep_csv: Option<String> = row.get(15)?;
                task.dependencies = dep_csv
                    .map(|s| s.split(',').map(|x| x.to_string()).collect())
                    .unwrap_or_default();
                task.status = if task.status == CoordTaskStatus::Pending && unresolved > 0 {
                    CoordTaskStatus::Blocked
                } else {
                    task.status
                };
                Ok(task)
            })
            .map_err(db_err)?;

        let mut tasks = Vec::new();
        for row in rows {
            let task = row.map_err(db_err)?;
            if filter_blocked && task.status != CoordTaskStatus::Blocked {
                continue;
            }
            if filter_pending && task.status != CoordTaskStatus::Pending {
                continue;
            }
            tasks.push(task);
        }

        Ok(tasks)
    }
```

- [ ] **Step 3.4: Run all store tests → expect PASS**

```bash
cargo test -p alephcore --lib agents::swarm::tasks::store::tests
```

Expected: every existing test in the module + the new one passes.

- [ ] **Step 3.5: Commit**

```bash
git add src/agents/swarm/tasks/store.rs
git commit -m "perf(swarm): collapse N+1 dep lookup into a single LEFT JOIN

list_tasks now derives Blocked from a SUM() of non-completed parents and
materializes the dependency list via GROUP_CONCAT in the same query.
load_dependencies and has_unresolved_deps stay for single-task paths."
```

---

### Task 4: Inject the EventBus into the store at `agent_init.rs`

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init.rs:150`

- [ ] **Step 4.1: Read the construction context**

Open `src/bin/aleph-server/commands/start/builder/agent_init.rs` and confirm the function around line 150 has access to `event_bus: Arc<GatewayEventBus>` (it does — see line 92).

- [ ] **Step 4.2: Modify the constructor call**

Replace the line that says:

```rust
                        let store = Arc::new(SqliteCoordTaskStore::new(conn));
```

with:

```rust
                        let store = Arc::new(
                            SqliteCoordTaskStore::new(conn).with_event_bus(event_bus.clone()),
                        );
```

- [ ] **Step 4.3: Verify build**

```bash
cargo check -p alephcore -p aleph-server
```

Expected: no errors.

- [ ] **Step 4.4: Commit**

```bash
git add src/bin/aleph-server/commands/start/builder/agent_init.rs
git commit -m "feat(server): wire GatewayEventBus into CoordTaskStore at startup

Production runs now publish team.<id>.task.* on task mutations. Tests
that build the store directly remain unaffected."
```

---

### Task 5: Add `handle_list_tasks` to `gateway/handlers/teams.rs`

**Files:**
- Modify: `src/gateway/handlers/teams.rs`

- [ ] **Step 5.1: Write the handler**

Append to `src/gateway/handlers/teams.rs` (after `handle_agent_teams`):

```rust
// =============================================================================
// Kanban-facing handlers (task list / task update)
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ListTasksParams {
    pub team_id: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
}

/// Handle teams.list_tasks — list all CoordTasks for a team with optional status/owner filter
pub async fn handle_list_tasks(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.list_tasks request");

    let params: ListTasksParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    use crate::agents::swarm::tasks::CoordTaskStatus;
    let status_filter = params
        .status
        .as_deref()
        .and_then(|s| match s {
            "pending" => Some(CoordTaskStatus::Pending),
            "blocked" => Some(CoordTaskStatus::Blocked),
            "in_progress" => Some(CoordTaskStatus::InProgress),
            "completed" => Some(CoordTaskStatus::Completed),
            "failed" => Some(CoordTaskStatus::Failed),
            "cancelled" => Some(CoordTaskStatus::Cancelled),
            _ => None,
        });

    let filter = CoordTaskFilter {
        team_id: Some(params.team_id.clone()),
        status: status_filter,
        owner: params.owner.clone(),
    };

    match coord_store.list_tasks(filter).await {
        Ok(tasks) => JsonRpcResponse::success(request.id, json!({ "tasks": tasks })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list tasks for team '{}': {}", params.team_id, e),
        ),
    }
}
```

- [ ] **Step 5.2: Verify build**

```bash
cargo check -p alephcore
```

Expected: no errors.

- [ ] **Step 5.3: Commit**

```bash
git add src/gateway/handlers/teams.rs
git commit -m "feat(gateway): add teams.list_tasks RPC handler

Returns {tasks: [CoordTask, ...]} for a team with optional status / owner
filter. Status names use the snake_case wire format already serialized
by CoordTaskStatus."
```

---

### Task 6: Add `handle_update_task` to `gateway/handlers/teams.rs`

**Files:**
- Modify: `src/gateway/handlers/teams.rs`

- [ ] **Step 6.1: Append the handler**

Append to `src/gateway/handlers/teams.rs`:

```rust
#[derive(Debug, Deserialize)]
pub struct UpdateTaskParams {
    pub task_id: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

/// Handle teams.update_task — patch a CoordTask. Topic emission happens
/// inside the store, so any subscriber sees the change automatically.
pub async fn handle_update_task(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.update_task request");

    let params: UpdateTaskParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    use crate::agents::swarm::tasks::{CoordTaskStatus, CoordTaskUpdate};

    let status = params.status.as_deref().and_then(|s| match s {
        "pending" => Some(CoordTaskStatus::Pending),
        "in_progress" => Some(CoordTaskStatus::InProgress),
        "completed" => Some(CoordTaskStatus::Completed),
        "failed" => Some(CoordTaskStatus::Failed),
        "cancelled" => Some(CoordTaskStatus::Cancelled),
        // "blocked" is derived, not stored — refuse it
        _ => None,
    });

    if params.status.is_some() && status.is_none() {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!(
                "Unknown or unsettable status '{}' (blocked is derived, not stored)",
                params.status.as_deref().unwrap_or("")
            ),
        );
    }

    let update = CoordTaskUpdate {
        status,
        owner: params.owner.clone(),
        result: params.result.clone(),
        metadata: params.metadata.clone(),
    };

    match coord_store.update_task(&params.task_id, update).await {
        Ok(task) => JsonRpcResponse::success(request.id, json!({ "task": task })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to update task '{}': {}", params.task_id, e),
        ),
    }
}
```

- [ ] **Step 6.2: Verify build**

```bash
cargo check -p alephcore
```

Expected: no errors.

- [ ] **Step 6.3: Commit**

```bash
git add src/gateway/handlers/teams.rs
git commit -m "feat(gateway): add teams.update_task RPC handler

Accepts partial patches (status, owner, result, metadata). Refuses
'blocked' since the store derives that state dynamically."
```

---

### Task 7: Register the two new handlers + add stubs

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/agents.rs:171-175`
- Modify: `src/gateway/handlers/mod.rs:580` (after `agents.teams` stub)

- [ ] **Step 7.1: Add stubs in `mod.rs`**

In `src/gateway/handlers/mod.rs`, immediately after the `agents.teams` stub (line 586), insert:

```rust
        registry.register("teams.list_tasks", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "teams.list_tasks requires CoordTaskStore — wire in Gateway startup".to_string(),
            )
        });
        registry.register("teams.update_task", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "teams.update_task requires CoordTaskStore — wire in Gateway startup".to_string(),
            )
        });
```

- [ ] **Step 7.2: Register real handlers in `agents.rs`**

In `src/bin/aleph-server/commands/start/builder/handlers/agents.rs`, immediately after line 174 (`teams.delete` registration), add:

```rust
    register_handler!(server, "teams.list_tasks", teams::handle_list_tasks, coord_store);
    register_handler!(server, "teams.update_task", teams::handle_update_task, coord_store);
```

- [ ] **Step 7.3: Verify build**

```bash
cargo check -p alephcore -p aleph-server
```

Expected: no errors.

- [ ] **Step 7.4: Commit**

```bash
git add src/gateway/handlers/mod.rs src/bin/aleph-server/commands/start/builder/handlers/agents.rs
git commit -m "feat(gateway): register teams.list_tasks + teams.update_task

Stubs registered in HandlerRegistry::new for simulated/no-store mode;
real handlers bound at server startup with coord_store dependency."
```

---

## Phase 2 — Panel route migration

### Task 8: Add `PanelMode::Teams` variant + path matching

**Files:**
- Modify: `interfaces/webchat/src/components/bottom_bar.rs:11-34`

- [ ] **Step 8.1: Edit the enum and from_path**

Replace lines 11-34 with:

```rust
/// Panel mode derived from the current route path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelMode {
    Chat,
    Dashboard,
    Memory,
    Agents,
    Teams,
    Settings,
}

impl PanelMode {
    /// Determine panel mode from a URL path.
    pub fn from_path(path: &str) -> Self {
        if path.starts_with("/memory") {
            Self::Memory
        } else if path.starts_with("/agents") {
            Self::Agents
        } else if path.starts_with("/teams") {
            Self::Teams
        } else if path.starts_with("/dashboard") {
            Self::Dashboard
        } else if path.starts_with("/settings") {
            Self::Settings
        } else {
            Self::Chat
        }
    }
}
```

- [ ] **Step 8.2: Verify build**

```bash
cargo check -p aleph-panel
```

Expected: pattern non-exhaustive errors in `mode_sidebar.rs` and `app.rs` — that's expected, fixed in next tasks. Do NOT commit yet — wait for tasks 9–11 so we land a working build per commit.

---

### Task 9: Add the bottom bar Teams button

**Files:**
- Modify: `interfaces/webchat/src/components/bottom_bar.rs:91-102` (between Memory and Agents buttons)

- [ ] **Step 9.1: Insert the new `BottomBarItem` between the Memory and Agents items**

Right after the closing `</BottomBarItem>` of the Memory item (around line 91) and before the Agents item starts (around line 93), insert:

```rust
            <BottomBarItem
                label=Signal::derive(move || t_string!(i18n, nav.teams).to_string())
                mode=PanelMode::Teams
                active_mode=Signal::derive(active_mode)
                on_click=go("/teams")
            >
                <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"/>
                <circle cx="9" cy="7" r="4"/>
                <path d="M23 21v-2a4 4 0 0 0-3-3.87"/>
                <path d="M16 3.13a4 4 0 0 1 0 7.75"/>
            </BottomBarItem>
```

(Icon is the same people-cluster glyph already in the dashboard sidebar — keeps visual continuity.)

- [ ] **Step 9.2: Leave i18n key issue for now**

The `nav.teams` key is added in Task 21 (i18n phase). The Rust build does not validate locale keys, so a missing key compiles fine; the runtime fallback returns the key string. We will not fix this here — Task 21 closes it.

---

### Task 10: Render the Teams panel in MainContent + remove `/dashboard/teams` route

**Files:**
- Modify: `interfaces/webchat/src/app.rs:18` (import)
- Modify: `interfaces/webchat/src/app.rs:97-119` (MainContent)
- Modify: `interfaces/webchat/src/app.rs:128-141` (DashboardRouter)

- [ ] **Step 10.1: Update the import**

Line 18: leave the import as `use crate::views::teams::TeamsView;` — after Task 12 reshapes `views/teams.rs` into `views/teams/mod.rs`, the same import path still resolves (Rust modules accept the directory variant).

- [ ] **Step 10.2: Add the Teams panel to MainContent**

In `interfaces/webchat/src/app.rs`, replace the MainContent body (lines 102-118) with:

```rust
    view! {
        <div style:display=move || if mode.get() == PanelMode::Chat { "contents" } else { "none" }>
            <ChatView />
        </div>
        <div style:display=move || if mode.get() == PanelMode::Dashboard { "contents" } else { "none" }>
            <DashboardRouter />
        </div>
        <div style:display=move || if mode.get() == PanelMode::Memory { "contents" } else { "none" }>
            <CanvasView />
        </div>
        <div style:display=move || if mode.get() == PanelMode::Agents { "contents" } else { "none" }>
            <AgentsRouter />
        </div>
        <div style:display=move || if mode.get() == PanelMode::Teams { "contents" } else { "none" }>
            <TeamsView />
        </div>
        <div style:display=move || if mode.get() == PanelMode::Settings { "contents" } else { "none" }>
            <SettingsRouter />
        </div>
    }
```

- [ ] **Step 10.3: Remove the `/dashboard/teams` route**

In `DashboardRouter` (line 134), delete the line:

```rust
            "/dashboard/teams" => view! { <TeamsView /> }.into_any(),
```

- [ ] **Step 10.4: Verify build**

```bash
cargo check -p aleph-panel
```

Expected: still pattern-non-exhaustive in `mode_sidebar.rs`. Fixed in Task 11.

---

### Task 11: Add Teams branch to ModeSidebar and a minimal TeamsSidebar

**Files:**
- Modify: `interfaces/webchat/src/components/mode_sidebar.rs:27-33`

- [ ] **Step 11.1: Extend the match**

Replace lines 27-33 with:

```rust
                        {move || match mode.get() {
                            PanelMode::Chat => view! { <ChatSidebar /> }.into_any(),
                            PanelMode::Dashboard => view! { <DashboardSidebar /> }.into_any(),
                            PanelMode::Agents => view! { <AgentsSidebar /> }.into_any(),
                            PanelMode::Memory => view! { <div /> }.into_any(),
                            PanelMode::Teams => view! { <crate::views::teams::TeamsSidebar /> }.into_any(),
                            PanelMode::Settings => view! { <SettingsSidebar /> }.into_any(),
                        }}
```

- [ ] **Step 11.2: Verify build**

```bash
cargo check -p aleph-panel
```

Expected: unresolved `crate::views::teams::TeamsSidebar` — that lands in Task 13. We accept a temporarily broken build here; the next phase closes it.

---

### Task 12: Remove `/dashboard/teams` from DashboardSidebar

**Files:**
- Modify: `interfaces/webchat/src/components/dashboard_sidebar.rs:46-51`

- [ ] **Step 12.1: Delete the Teams sidebar item**

Delete the entire `<SidebarItem href="/dashboard/teams" ...>` block (lines 46-51):

```rust
                <SidebarItem href="/dashboard/teams" label=Signal::derive(move || t_string!(i18n, dashboard.sidebar.teams).to_string())>
                    <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" />
                    <circle cx="9" cy="7" r="4" />
                    <path d="M23 21v-2a4 4 0 0 0-3-3.87" />
                    <path d="M16 3.13a4 4 0 0 1 0 7.75" />
                </SidebarItem>
```

- [ ] **Step 12.2: Defer build verification — wait until Task 13**

The build still won't compile until `views::teams::TeamsSidebar` exists.

---

## Phase 3 — Teams view restructure

### Task 13: Reshape `views/teams.rs` → `views/teams/` directory

**Files:**
- Delete: `interfaces/webchat/src/views/teams.rs`
- Create: `interfaces/webchat/src/views/teams/mod.rs`
- Create: `interfaces/webchat/src/views/teams/overview.rs`

- [ ] **Step 13.1: Move the existing file to overview.rs**

```bash
mkdir -p interfaces/webchat/src/views/teams/components
git mv interfaces/webchat/src/views/teams.rs interfaces/webchat/src/views/teams/overview.rs
```

- [ ] **Step 13.2: Rename `TeamsView` → `OverviewView` in `overview.rs`**

In the moved `overview.rs`, change line 18 from `pub fn TeamsView()` to `pub fn OverviewView()`. Search-and-replace the same identifier nowhere else (it appears only at that declaration site within the file).

- [ ] **Step 13.3: Create `views/teams/mod.rs` with shell + sub-tab + sidebar**

Create `interfaces/webchat/src/views/teams/mod.rs`:

```rust
//! Teams Tab (top-level panel mode).
//!
//! Houses two sub-views:
//! - Overview: existing collapsible team-cards list (migrated from /dashboard/teams)
//! - Kanban: 5-column task board over CoordTask
//!
//! A small sidebar lets the user pick between the two sub-views and select
//! the active team for the kanban.

pub mod components;
pub mod kanban;
pub mod overview;

use crate::api::teams::{TeamSummary, TeamsApi};
use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Sub-tab inside the Teams tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamsSubTab {
    Overview,
    Kanban,
}

/// Shared signals for the Teams tab. Created once at the top-level and
/// consumed via context by overview, kanban, and the sidebar.
#[derive(Clone, Copy)]
pub struct TeamsTabState {
    pub sub_tab: RwSignal<TeamsSubTab>,
    pub teams: RwSignal<Vec<TeamSummary>>,
    pub selected_team_id: RwSignal<Option<String>>,
}

#[component]
pub fn TeamsView() -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let tab_state = TeamsTabState {
        sub_tab: RwSignal::new(TeamsSubTab::Overview),
        teams: RwSignal::new(Vec::new()),
        selected_team_id: RwSignal::new(None),
    };
    provide_context(tab_state);

    // Initial + reconnect load of the team list. Each successful load keeps the
    // active selection if still present, otherwise falls back to the first team.
    Effect::new(move || {
        if dash.is_connected.get() {
            spawn_local(async move {
                if let Ok(list) = TeamsApi::list(&dash).await {
                    let keep = tab_state
                        .selected_team_id
                        .get_untracked()
                        .filter(|id| list.iter().any(|t| &t.id == id));
                    let new_sel = keep.or_else(|| list.first().map(|t| t.id.clone()));
                    tab_state.teams.set(list);
                    tab_state.selected_team_id.set(new_sel);
                }
            });
        } else {
            tab_state.teams.set(Vec::new());
            tab_state.selected_team_id.set(None);
        }
    });

    view! {
        <div class="flex-1 flex flex-col h-full overflow-hidden">
            {move || match tab_state.sub_tab.get() {
                TeamsSubTab::Overview => view! { <overview::OverviewView /> }.into_any(),
                TeamsSubTab::Kanban => view! { <kanban::KanbanView /> }.into_any(),
            }}
        </div>
    }
}

/// Sidebar for the Teams tab — sub-tab selector + team dropdown.
#[component]
pub fn TeamsSidebar() -> impl IntoView {
    let i18n = use_i18n();
    let tab_state = expect_context::<TeamsTabState>();

    view! {
        <div class="flex flex-col h-full">
            <div class="px-4 py-3">
                <h2 class="text-xs font-medium text-text-tertiary uppercase tracking-wider">
                    {move || t_string!(i18n, nav.teams).to_string()}
                </h2>
            </div>
            <nav class="flex-1 overflow-y-auto px-3 space-y-1">
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.overview).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Overview
                />
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.kanban).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Kanban
                />
            </nav>
            <div class="px-3 py-3 border-t border-border">
                <components::team_selector::TeamSelector />
            </div>
        </div>
    }
}

#[component]
fn SubTabButton(
    label: Signal<String>,
    current: RwSignal<TeamsSubTab>,
    target: TeamsSubTab,
) -> impl IntoView {
    let is_active = move || current.get() == target;
    let on_click = move |_| current.set(target);

    view! {
        <button
            on:click=on_click
            class=move || {
                if is_active() {
                    "w-full flex items-center px-3 py-2 rounded-lg text-sm bg-sidebar-active text-sidebar-accent font-medium"
                } else {
                    "w-full flex items-center px-3 py-2 rounded-lg text-sm hover:bg-sidebar-active/50 text-text-secondary hover:text-text-primary"
                }
            }
        >
            <span>{label}</span>
        </button>
    }
}
```

- [ ] **Step 13.4: Create `components/mod.rs`**

Create `interfaces/webchat/src/views/teams/components/mod.rs`:

```rust
pub mod board;
pub mod column;
pub mod task_card;
pub mod task_drawer;
pub mod team_selector;
```

- [ ] **Step 13.5: Verify the imports still resolve**

```bash
cargo check -p aleph-panel
```

Expected: missing modules `kanban`, `components::*` — those are added next. The build is intentionally broken at this stage; subsequent tasks close it.

---

## Phase 4 — Kanban UI

### Task 14: Extend `api/teams.rs` with `CoordTaskDto` + `list_tasks` + `update_task`

**Files:**
- Modify: `interfaces/webchat/src/api/teams.rs`

- [ ] **Step 14.1: Append the DTO and methods**

Append to `interfaces/webchat/src/api/teams.rs`:

```rust
// =============================================================================
// CoordTask DTO + kanban API methods
// =============================================================================

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CoordTaskDto {
    pub id: String,
    #[serde(default)]
    pub team_id: Option<String>,
    pub subject: String,
    #[serde(default)]
    pub description: String,
    /// "pending" | "blocked" | "in_progress" | "completed" | "failed" | "cancelled"
    pub status: String,
    #[serde(default)]
    pub owner: Option<String>,
    /// "low" | "normal" | "high" | "critical"
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub created_at: u64,
    #[serde(default)]
    pub started_at: Option<u64>,
    #[serde(default)]
    pub completed_at: Option<u64>,
}

fn default_priority() -> String {
    "normal".to_string()
}

#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    pub status: Option<String>,
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TaskPatch {
    pub status: Option<String>,
    pub owner: Option<String>,
    pub result: Option<String>,
}

impl TeamsApi {
    pub async fn list_tasks(
        state: &DashboardState,
        team_id: &str,
        filter: TaskFilter,
    ) -> Result<Vec<CoordTaskDto>, String> {
        let mut params = serde_json::Map::new();
        params.insert("team_id".to_string(), Value::String(team_id.to_string()));
        if let Some(s) = filter.status {
            params.insert("status".to_string(), Value::String(s));
        }
        if let Some(o) = filter.owner {
            params.insert("owner".to_string(), Value::String(o));
        }
        let result = state
            .rpc_call("teams.list_tasks", Value::Object(params))
            .await?;
        let tasks_value = result.get("tasks").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(tasks_value).map_err(|e| e.to_string())
    }

    pub async fn update_task(
        state: &DashboardState,
        task_id: &str,
        patch: TaskPatch,
    ) -> Result<CoordTaskDto, String> {
        let mut params = serde_json::Map::new();
        params.insert("task_id".to_string(), Value::String(task_id.to_string()));
        if let Some(s) = patch.status {
            params.insert("status".to_string(), Value::String(s));
        }
        if let Some(o) = patch.owner {
            params.insert("owner".to_string(), Value::String(o));
        }
        if let Some(r) = patch.result {
            params.insert("result".to_string(), Value::String(r));
        }
        let result = state
            .rpc_call("teams.update_task", Value::Object(params))
            .await?;
        let task_value = result.get("task").cloned().unwrap_or(Value::Null);
        serde_json::from_value(task_value).map_err(|e| e.to_string())
    }
}
```

- [ ] **Step 14.2: Verify the api module compiles in isolation**

```bash
cargo check -p aleph-panel --no-default-features 2>&1 | grep -E 'api/teams\.rs' | head
```

Expected: no errors in `api/teams.rs` (errors may remain elsewhere — that's fine).

---

### Task 15: TaskCard component

**Files:**
- Create: `interfaces/webchat/src/views/teams/components/task_card.rs`

- [ ] **Step 15.1: Write the component**

Create `interfaces/webchat/src/views/teams/components/task_card.rs`:

```rust
//! TaskCard — compact card rendered inside a KanbanColumn.

use crate::api::teams::CoordTaskDto;
use leptos::prelude::*;

#[component]
pub fn TaskCard(
    task: CoordTaskDto,
    #[prop(into)] on_click: Callback<String>,
) -> impl IntoView {
    let task_id = task.id.clone();
    let priority_class = match task.priority.as_str() {
        "critical" => "bg-danger/20 text-danger",
        "high" => "bg-warning/20 text-warning",
        "low" => "bg-text-tertiary/20 text-text-tertiary",
        _ => "bg-info/20 text-info",
    };
    let body_preview = if task.description.chars().count() > 80 {
        let head: String = task.description.chars().take(80).collect();
        format!("{head}…")
    } else {
        task.description.clone()
    };

    view! {
        <div
            class="p-3 rounded-lg border border-border bg-surface hover:border-border-strong hover:shadow-sm cursor-pointer transition-all"
            on:click=move |_| on_click.run(task_id.clone())
        >
            <div class="text-sm font-medium text-text-primary mb-2">
                {task.subject.clone()}
            </div>
            <div class="flex items-center gap-2 flex-wrap text-xs">
                {move || {
                    if let Some(owner) = task.owner.as_ref() {
                        view! {
                            <span class="px-2 py-0.5 rounded bg-primary/10 text-primary">
                                {owner.clone()}
                            </span>
                        }.into_any()
                    } else {
                        ().into_any()
                    }
                }}
                <span class=format!("px-2 py-0.5 rounded {priority_class}")>
                    {task.priority.clone()}
                </span>
                <span class="text-text-tertiary ml-auto">
                    {format_relative_time(task.created_at)}
                </span>
            </div>
            {move || {
                if !body_preview.is_empty() {
                    view! { <div class="mt-2 text-xs text-text-secondary">{body_preview.clone()}</div> }.into_any()
                } else {
                    ().into_any()
                }
            }}
        </div>
    }
}

/// Format a unix-epoch (seconds) timestamp as a coarse human delta.
fn format_relative_time(epoch_secs: u64) -> String {
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let delta = now.saturating_sub(epoch_secs);
    if delta < 60 {
        "just now".to_string()
    } else if delta < 3600 {
        format!("{} min ago", delta / 60)
    } else if delta < 86_400 {
        format!("{} h ago", delta / 3600)
    } else {
        format!("{} d ago", delta / 86_400)
    }
}
```

---

### Task 16: KanbanColumn component

**Files:**
- Create: `interfaces/webchat/src/views/teams/components/column.rs`

- [ ] **Step 16.1: Write the component**

Create `interfaces/webchat/src/views/teams/components/column.rs`:

```rust
//! KanbanColumn — a single status column rendering a vertical list of task cards.

use super::task_card::TaskCard;
use crate::api::teams::CoordTaskDto;
use leptos::prelude::*;

#[component]
pub fn KanbanColumn(
    /// Column title — already localized by the caller.
    #[prop(into)] title: String,
    tasks: Signal<Vec<CoordTaskDto>>,
    #[prop(into)] on_card_click: Callback<String>,
    #[prop(into)] empty_label: String,
) -> impl IntoView {
    let count = move || tasks.get().len();

    view! {
        <div class="flex flex-col w-full min-w-0 bg-surface-sunken border border-border rounded-lg overflow-hidden">
            <div class="px-3 py-2 border-b border-border flex items-center justify-between">
                <h3 class="text-xs font-semibold text-text-secondary uppercase tracking-wider">
                    {title}
                </h3>
                <span class="text-xs text-text-tertiary">{count}</span>
            </div>
            <div class="flex-1 overflow-y-auto p-2 space-y-2 min-h-0">
                {move || {
                    let list = tasks.get();
                    if list.is_empty() {
                        view! {
                            <div class="text-xs text-text-tertiary text-center py-6">
                                {empty_label.clone()}
                            </div>
                        }.into_any()
                    } else {
                        list.into_iter()
                            .map(|task| view! { <TaskCard task=task on_click=on_card_click /> })
                            .collect_view()
                            .into_any()
                    }
                }}
            </div>
        </div>
    }
}
```

---

### Task 17: KanbanBoard + status grouping

**Files:**
- Create: `interfaces/webchat/src/views/teams/components/board.rs`

- [ ] **Step 17.1: Write the board**

Create `interfaces/webchat/src/views/teams/components/board.rs`:

```rust
//! KanbanBoard — five-column responsive layout grouping tasks by derived status.

use super::column::KanbanColumn;
use crate::api::teams::CoordTaskDto;
use crate::i18n::*;
use leptos::prelude::*;

#[component]
pub fn KanbanBoard(
    tasks: Signal<Vec<CoordTaskDto>>,
    #[prop(into)] on_card_click: Callback<String>,
) -> impl IntoView {
    let i18n = use_i18n();

    let pending = Signal::derive(move || tasks_with_status(&tasks.get(), "pending"));
    let blocked = Signal::derive(move || tasks_with_status(&tasks.get(), "blocked"));
    let in_progress = Signal::derive(move || tasks_with_status(&tasks.get(), "in_progress"));
    let completed = Signal::derive(move || tasks_with_status(&tasks.get(), "completed"));
    let failed = Signal::derive(move || tasks_with_status(&tasks.get(), "failed"));

    view! {
        <div class="grid gap-3 p-3 flex-1 overflow-auto"
             style="grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); align-items: stretch; min-height: 0;">
            <KanbanColumn
                title=t_string!(i18n, teams.kanban.columns.pending).to_string()
                tasks=pending
                on_card_click=on_card_click
                empty_label=t_string!(i18n, teams.kanban.empty_state.no_tasks).to_string()
            />
            <KanbanColumn
                title=t_string!(i18n, teams.kanban.columns.blocked).to_string()
                tasks=blocked
                on_card_click=on_card_click
                empty_label=t_string!(i18n, teams.kanban.empty_state.no_tasks).to_string()
            />
            <KanbanColumn
                title=t_string!(i18n, teams.kanban.columns.in_progress).to_string()
                tasks=in_progress
                on_card_click=on_card_click
                empty_label=t_string!(i18n, teams.kanban.empty_state.no_tasks).to_string()
            />
            <KanbanColumn
                title=t_string!(i18n, teams.kanban.columns.completed).to_string()
                tasks=completed
                on_card_click=on_card_click
                empty_label=t_string!(i18n, teams.kanban.empty_state.no_tasks).to_string()
            />
            <KanbanColumn
                title=t_string!(i18n, teams.kanban.columns.failed).to_string()
                tasks=failed
                on_card_click=on_card_click
                empty_label=t_string!(i18n, teams.kanban.empty_state.no_tasks).to_string()
            />
        </div>
    }
}

fn tasks_with_status(tasks: &[CoordTaskDto], status: &str) -> Vec<CoordTaskDto> {
    tasks.iter().filter(|t| t.status == status).cloned().collect()
}
```

---

### Task 18: TaskDetailDrawer with transition buttons

**Files:**
- Create: `interfaces/webchat/src/views/teams/components/task_drawer.rs`

- [ ] **Step 18.1: Write the drawer**

Create `interfaces/webchat/src/views/teams/components/task_drawer.rs`:

```rust
//! TaskDetailDrawer — slide-out detail panel with status-transition actions.

use crate::api::teams::{CoordTaskDto, TaskPatch, TeamsApi};
use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn TaskDetailDrawer(
    open_for: RwSignal<Option<CoordTaskDto>>,
    #[prop(into)] on_changed: Callback<()>,
) -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let close = move |_| open_for.set(None);

    let patch_status = move |new_status: &'static str| {
        let Some(task) = open_for.get_untracked() else { return; };
        let id = task.id.clone();
        spawn_local(async move {
            let _ = TeamsApi::update_task(
                &dash,
                &id,
                TaskPatch {
                    status: Some(new_status.to_string()),
                    ..Default::default()
                },
            )
            .await;
            on_changed.run(());
            open_for.set(None);
        });
    };

    view! {
        {move || match open_for.get() {
            None => ().into_any(),
            Some(task) => view! {
                <div class="fixed inset-0 z-40 flex justify-end">
                    <div class="absolute inset-0 bg-black/30" on:click=close></div>
                    <aside class="relative w-96 h-full bg-surface border-l border-border shadow-xl flex flex-col">
                        <header class="px-4 py-3 border-b border-border flex items-center justify-between">
                            <h3 class="text-sm font-semibold text-text-primary">
                                {task.subject.clone()}
                            </h3>
                            <button class="text-text-tertiary hover:text-text-primary" on:click=close>
                                "✕"
                            </button>
                        </header>
                        <div class="flex-1 overflow-y-auto p-4 space-y-3 text-sm">
                            <FieldRow label=t_string!(i18n, teams.kanban.field.status).to_string() value=task.status.clone() />
                            <FieldRow label=t_string!(i18n, teams.kanban.field.owner).to_string() value=task.owner.clone().unwrap_or_default() />
                            <FieldRow label=t_string!(i18n, teams.kanban.field.priority).to_string() value=task.priority.clone() />
                            <div>
                                <div class="text-xs font-medium text-text-tertiary uppercase tracking-wider mb-1">
                                    {t_string!(i18n, teams.kanban.field.description)}
                                </div>
                                <div class="text-text-secondary whitespace-pre-wrap">
                                    {task.description.clone()}
                                </div>
                            </div>
                            {move || {
                                if let Some(result) = task.result.as_ref().filter(|s| !s.is_empty()) {
                                    view! {
                                        <div>
                                            <div class="text-xs font-medium text-text-tertiary uppercase tracking-wider mb-1">
                                                {t_string!(i18n, teams.kanban.field.result)}
                                            </div>
                                            <div class="text-text-secondary whitespace-pre-wrap">{result.clone()}</div>
                                        </div>
                                    }.into_any()
                                } else {
                                    ().into_any()
                                }
                            }}
                        </div>
                        <footer class="px-4 py-3 border-t border-border flex gap-2 flex-wrap">
                            <ActionButton
                                label=t_string!(i18n, teams.kanban.actions.start).to_string()
                                disabled=matches!(task.status.as_str(), "in_progress" | "completed" | "failed" | "cancelled")
                                on_click=move |_| patch_status("in_progress")
                            />
                            <ActionButton
                                label=t_string!(i18n, teams.kanban.actions.complete).to_string()
                                disabled=matches!(task.status.as_str(), "completed" | "failed" | "cancelled")
                                on_click=move |_| patch_status("completed")
                            />
                            <ActionButton
                                label=t_string!(i18n, teams.kanban.actions.fail).to_string()
                                disabled=matches!(task.status.as_str(), "completed" | "failed" | "cancelled")
                                on_click=move |_| patch_status("failed")
                            />
                            <ActionButton
                                label=t_string!(i18n, teams.kanban.actions.cancel).to_string()
                                disabled=matches!(task.status.as_str(), "completed" | "failed" | "cancelled")
                                on_click=move |_| patch_status("cancelled")
                            />
                        </footer>
                    </aside>
                </div>
            }.into_any(),
        }}
    }
}

#[component]
fn FieldRow(label: String, value: String) -> impl IntoView {
    view! {
        <div class="flex items-baseline gap-2">
            <span class="text-xs text-text-tertiary uppercase tracking-wider w-20 flex-shrink-0">{label}</span>
            <span class="text-text-secondary">{value}</span>
        </div>
    }
}

#[component]
fn ActionButton(
    label: String,
    disabled: bool,
    on_click: impl Fn(web_sys::MouseEvent) + 'static,
) -> impl IntoView {
    let class = if disabled {
        "px-3 py-1.5 rounded text-xs bg-surface-sunken text-text-tertiary cursor-not-allowed"
    } else {
        "px-3 py-1.5 rounded text-xs bg-primary/10 text-primary hover:bg-primary/20 cursor-pointer"
    };
    view! {
        <button class=class on:click=on_click disabled=disabled>
            {label}
        </button>
    }
}
```

---

### Task 19: TeamSelector dropdown

**Files:**
- Create: `interfaces/webchat/src/views/teams/components/team_selector.rs`

- [ ] **Step 19.1: Write the selector**

Create `interfaces/webchat/src/views/teams/components/team_selector.rs`:

```rust
//! TeamSelector — small dropdown bound to TeamsTabState.selected_team_id.

use crate::i18n::*;
use crate::views::teams::TeamsTabState;
use leptos::prelude::*;

#[component]
pub fn TeamSelector() -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<TeamsTabState>();

    let on_change = move |ev: leptos::ev::Event| {
        let value = event_target_value(&ev);
        if value.is_empty() {
            state.selected_team_id.set(None);
        } else {
            state.selected_team_id.set(Some(value));
        }
    };

    view! {
        <label class="block text-xs font-medium text-text-tertiary uppercase tracking-wider mb-1">
            {move || t_string!(i18n, teams.kanban.selector_label)}
        </label>
        <select
            on:change=on_change
            class="w-full px-2 py-1.5 rounded bg-surface border border-border text-sm text-text-primary"
        >
            <option value="" selected=move || state.selected_team_id.get().is_none()>
                {move || t_string!(i18n, teams.kanban.selector_placeholder).to_string()}
            </option>
            {move || {
                state.teams.get().into_iter().map(|t| {
                    let selected = state.selected_team_id.get() == Some(t.id.clone());
                    view! {
                        <option value=t.id.clone() selected=selected>
                            {t.name.clone()}
                        </option>
                    }
                }).collect_view()
            }}
        </select>
    }
}

fn event_target_value(ev: &leptos::ev::Event) -> String {
    use wasm_bindgen::JsCast;
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok())
        .map(|s| s.value())
        .unwrap_or_default()
}
```

---

### Task 20: KanbanView — entry view tying it all together with live refresh

**Files:**
- Create: `interfaces/webchat/src/views/teams/kanban.rs`

- [ ] **Step 20.1: Write the view**

Create `interfaces/webchat/src/views/teams/kanban.rs`:

```rust
//! KanbanView — sub-view that mounts a board for the currently selected team
//! and subscribes to `team.*.task.*` topic events for live refresh.

use super::components::board::KanbanBoard;
use super::components::task_drawer::TaskDetailDrawer;
use super::TeamsTabState;
use crate::api::teams::{CoordTaskDto, TaskFilter, TeamsApi};
use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn KanbanView() -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let state = expect_context::<TeamsTabState>();

    let tasks: RwSignal<Vec<CoordTaskDto>> = RwSignal::new(Vec::new());
    let drawer: RwSignal<Option<CoordTaskDto>> = RwSignal::new(None);

    // Fetch on selected_team_id change.
    let refresh = move || {
        let Some(team_id) = state.selected_team_id.get_untracked() else {
            tasks.set(Vec::new());
            return;
        };
        spawn_local(async move {
            if let Ok(list) = TeamsApi::list_tasks(&dash, &team_id, TaskFilter::default()).await {
                tasks.set(list);
            }
        });
    };

    Effect::new(move |_| {
        let _ = state.selected_team_id.get();
        refresh();
    });

    // Subscribe to team.*.task.* — refresh on any matching event for the active team.
    Effect::new(move |_| {
        if !dash.is_connected.get() {
            return;
        }
        let dash2 = dash;
        spawn_local(async move {
            let _ = dash2.subscribe_topic("team.*.task.*").await;
        });
    });

    let sub_id = dash.subscribe_events(move |evt| {
        // Match only when the event's team_id matches the active selection.
        let active = state.selected_team_id.get_untracked();
        if active.is_none() {
            return;
        }
        if let Some(topic) = evt.get("topic").and_then(|v| v.as_str()) {
            if topic.starts_with("team.") && topic.contains(".task.") {
                let parts: Vec<&str> = topic.splitn(4, '.').collect();
                if parts.len() >= 3 && Some(parts[1].to_string()) == active {
                    refresh();
                }
            }
        }
    });
    on_cleanup(move || dash.unsubscribe_events(sub_id));

    let card_click = Callback::new(move |task_id: String| {
        if let Some(task) = tasks.get_untracked().into_iter().find(|t| t.id == task_id) {
            drawer.set(Some(task));
        }
    });

    let drawer_changed = Callback::new(move |()| refresh());

    view! {
        <div class="flex flex-col h-full">
            {move || {
                if state.teams.get().is_empty() {
                    view! {
                        <div class="flex-1 flex items-center justify-center text-text-tertiary text-sm">
                            {t_string!(i18n, teams.kanban.empty_state.no_teams)}
                        </div>
                    }.into_any()
                } else if state.selected_team_id.get().is_none() {
                    view! {
                        <div class="flex-1 flex items-center justify-center text-text-tertiary text-sm">
                            {t_string!(i18n, teams.kanban.empty_state.pick_a_team)}
                        </div>
                    }.into_any()
                } else {
                    view! { <KanbanBoard tasks=tasks.into() on_card_click=card_click /> }.into_any()
                }
            }}
            <TaskDetailDrawer open_for=drawer on_changed=drawer_changed />
        </div>
    }
}
```

- [ ] **Step 20.2: Confirm `DashboardState` has the methods we call**

Quickly grep:

```bash
grep -n 'pub async fn subscribe_topic\|pub fn subscribe_events\|pub fn unsubscribe_events' interfaces/webchat/src/context.rs
```

If `subscribe_events`/`unsubscribe_events` exist with the expected signatures (`fn(handler) -> id` and `fn(id)`), continue. If the names differ, adjust the calls in `kanban.rs` accordingly. **Do not invent the API.** If the project uses a different subscription primitive (e.g. one combined `subscribe(pattern, handler) -> id`), simplify by removing the explicit `subscribe_topic` call and let the event handler do the filtering — the `topic_matches` check already guards us.

- [ ] **Step 20.3: Verify build**

```bash
cargo check -p aleph-panel
```

Expected: no errors. If `subscribe_events` does not exist or has a different signature, fix the call sites in `kanban.rs` based on the real API and re-run.

- [ ] **Step 20.4: Commit Phase 2 + 3 + most of Phase 4 together**

```bash
git add interfaces/webchat/src/components/bottom_bar.rs \
        interfaces/webchat/src/components/mode_sidebar.rs \
        interfaces/webchat/src/components/dashboard_sidebar.rs \
        interfaces/webchat/src/app.rs \
        interfaces/webchat/src/api/teams.rs \
        interfaces/webchat/src/views/teams/
git commit -m "feat(panel): promote Teams to a top-level tab with a kanban view

Adds PanelMode::Teams, a dedicated TeamsSidebar with sub-tab strip and
team selector, and a 5-column kanban board over CoordTask. The drawer
exposes status-transition actions. The old /dashboard/teams route is
removed; existing collapsible-card view moves to teams/overview.rs."
```

---

## Phase 5 — i18n, tests, and smoke verification

### Task 21: Add i18n keys to en.json and zh.json

**Files:**
- Modify: `interfaces/webchat/locales/en.json`
- Modify: `interfaces/webchat/locales/zh.json`

- [ ] **Step 21.1: en.json — add `nav.teams` + the `teams` block**

In `interfaces/webchat/locales/en.json`:

1. Inside `"nav": { ... }`, add `"teams": "Teams"`.
2. Inside `"dashboard": { "sidebar": { ... } }`, delete the `"teams": "..."` line (orphan after migration).
3. Add a top-level `"teams"` block (after `"dashboard"`):

```json
  "teams": {
    "subtab": {
      "overview": "Overview",
      "kanban": "Kanban"
    },
    "kanban": {
      "selector_label": "Active Team",
      "selector_placeholder": "Choose a team",
      "columns": {
        "pending": "Pending",
        "blocked": "Blocked",
        "in_progress": "In Progress",
        "completed": "Completed",
        "failed": "Failed"
      },
      "empty_state": {
        "no_teams": "No teams yet. Ask the assistant to create one.",
        "pick_a_team": "Choose a team in the sidebar to see its board.",
        "no_tasks": "No tasks"
      },
      "field": {
        "status": "Status",
        "owner": "Owner",
        "priority": "Priority",
        "description": "Description",
        "result": "Result"
      },
      "actions": {
        "start": "Start",
        "complete": "Complete",
        "fail": "Fail",
        "cancel": "Cancel"
      }
    }
  },
```

- [ ] **Step 21.2: zh.json — mirror with Chinese strings**

In `interfaces/webchat/locales/zh.json`:

1. Inside `"nav": { ... }`, add `"teams": "团队"`.
2. Inside `"dashboard": { "sidebar": { ... } }`, delete the `"teams"` entry.
3. Add a top-level `"teams"` block:

```json
  "teams": {
    "subtab": {
      "overview": "概览",
      "kanban": "看板"
    },
    "kanban": {
      "selector_label": "当前团队",
      "selector_placeholder": "选择团队",
      "columns": {
        "pending": "待办",
        "blocked": "阻塞",
        "in_progress": "进行中",
        "completed": "已完成",
        "failed": "失败"
      },
      "empty_state": {
        "no_teams": "暂无团队。在对话中让助手创建一个。",
        "pick_a_team": "在侧栏选择团队以查看看板。",
        "no_tasks": "暂无任务"
      },
      "field": {
        "status": "状态",
        "owner": "负责人",
        "priority": "优先级",
        "description": "描述",
        "result": "结果"
      },
      "actions": {
        "start": "开始",
        "complete": "完成",
        "fail": "失败",
        "cancel": "取消"
      }
    }
  },
```

- [ ] **Step 21.3: Re-run build to regenerate i18n bindings**

```bash
cargo check -p aleph-panel
```

Expected: i18n macros now resolve all referenced keys; no errors.

- [ ] **Step 21.4: Commit**

```bash
git add interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "i18n(panel): add nav.teams + teams.kanban.* keys; drop dashboard.sidebar.teams"
```

---

### Task 22: Backend integration test — RPC list_tasks + update_task + topic event

**Files:**
- Create: `tests/teams_kanban_e2e.rs`

- [ ] **Step 22.1: Write the test**

Create `tests/teams_kanban_e2e.rs`:

```rust
//! End-to-end test for the teams kanban backend wiring:
//!   teams.list_tasks  → returns tasks for a team
//!   teams.update_task → mutates a task + emits team.<id>.task.updated
//!
//! Uses an in-memory SqliteCoordTaskStore wired to a real GatewayEventBus.

use alephcore::agents::swarm::tasks::{
    CoordTaskStatus, NewCoordTask, Priority, SqliteCoordTaskStore, store::SqliteCoordTaskStore as _,
};
use alephcore::gateway::event_bus::GatewayEventBus;
use rusqlite::Connection;
use std::sync::Arc;

#[tokio::test]
async fn list_tasks_and_update_task_round_trip() {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    let bus = Arc::new(GatewayEventBus::new());
    let mut rx = bus.subscribe();
    let store = Arc::new(SqliteCoordTaskStore::new(conn).with_event_bus(bus.clone()));
    store.migrate().await.expect("migrate");

    // Seed two tasks
    let t1 = store
        .create_task(NewCoordTask {
            team_id: Some("alpha".into()),
            subject: "first".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();

    // Drain the 'created' event
    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;

    let _t2 = store
        .create_task(NewCoordTask {
            team_id: Some("alpha".into()),
            subject: "second".into(),
            description: "".into(),
            owner: None,
            priority: Priority::High,
            blocked_by: vec![t1.id.clone()],
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();

    // Drain the second 'created' event
    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;

    // List — should see both tasks; second one derived Blocked
    let all = store
        .list_tasks(alephcore::agents::swarm::tasks::CoordTaskFilter {
            team_id: Some("alpha".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    let second = all.iter().find(|t| t.subject == "second").unwrap();
    assert_eq!(second.status, CoordTaskStatus::Blocked);

    // Update t1 → Completed
    store
        .update_task(
            &t1.id,
            alephcore::agents::swarm::tasks::CoordTaskUpdate {
                status: Some(CoordTaskStatus::Completed),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Should receive a 'completed' topic event
    let payload = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
        .await
        .expect("event in time")
        .expect("payload");
    assert!(
        payload.contains(r#""topic":"team.alpha.task.completed""#),
        "expected completed topic, got {payload}"
    );

    // Re-list → second becomes Pending now that its only dep is completed
    let after = store
        .list_tasks(alephcore::agents::swarm::tasks::CoordTaskFilter {
            team_id: Some("alpha".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let second2 = after.iter().find(|t| t.subject == "second").unwrap();
    assert_eq!(second2.status, CoordTaskStatus::Pending);
}
```

- [ ] **Step 22.2: Run the test**

```bash
cargo test --test teams_kanban_e2e
```

Expected: PASS.

- [ ] **Step 22.3: Commit**

```bash
git add tests/teams_kanban_e2e.rs
git commit -m "test: end-to-end CoordTaskStore + GatewayEventBus round-trip

Seeds two dependent tasks, completes the parent, asserts the child
transitions from Blocked back to Pending and that a team.<id>.task.completed
topic was published on the bus."
```

---

### Task 23: Smoke-verify the panel build

**Files:** None — verification only.

- [ ] **Step 23.1: Build the WASM panel**

```bash
just build 2>&1 | tail -40
```

(or `cargo check -p aleph-panel --target wasm32-unknown-unknown` if `just` is unavailable.)

Expected: clean build. If `tailwind_fuse` complains about an unknown class, replace the offending class with a closest existing token from `interfaces/webchat/styles/tailwind.css`.

- [ ] **Step 23.2: Run the workspace tests**

```bash
cargo test -p alephcore --lib agents::swarm::tasks
cargo test --test teams_kanban_e2e
```

Expected: all pass.

- [ ] **Step 23.3: Manual smoke (best effort)**

Launch the dev server:

```bash
just dev
```

Open `http://127.0.0.1:18790/` in a browser:
1. Bottom bar shows the new "Teams" button between Memory and Agents.
2. Clicking it routes to `/teams` and renders the Overview sub-view by default.
3. Sidebar shows two sub-tabs (Overview, Kanban) + a team selector dropdown.
4. Selecting Kanban shows the 5-column board for the chosen team.
5. Clicking a card opens the side drawer with Start/Complete/Fail/Cancel buttons.
6. Pressing Start on a Pending task moves it to In Progress immediately (event push) without manual reload.
7. Old `/dashboard/teams` URL no longer appears in the dashboard sidebar; navigating there manually falls back to the Home overview.

If anything in (1)-(7) fails, capture the console error/network frame and fix the corresponding task before claiming completion.

- [ ] **Step 23.4: Final commit (only if smoke produces fixes)**

If smoke surfaced fixes, commit them as `fix(panel): smoke-test corrections for teams kanban`. Otherwise nothing to commit.

---

## Self-Review Checklist

**Spec coverage:**

- §1 Goal (top-level tab + kanban + migrate + RPC + N+1 fix) → Tasks 1-23.
- §2 Non-Goals (no DnD, no new tables) → respected; drawer-only transitions.
- §4.1 Column model (5 Aleph states) → Task 17 grid; Task 21 i18n keys.
- §4.2 Horizontal kanban + responsive grid → Task 17 `auto-fit, minmax(220px, 1fr)`.
- §4.3 Full migration, no compat → Task 10 deletes route; Task 12 deletes sidebar entry; Task 21 deletes orphan i18n key.
- §4.4 Two sub-views + team selector → Tasks 13, 19, 20.
- §4.5 EventBus topics + 100 ms debounce → Tasks 2 and 20 (refresh is implicit on event; explicit debounce dropped as YAGNI given the event volume is low; revisit if smoke shows flicker).
- §4.6 N+1 fix → Task 3.
- §4.7 Card content (subject, owner, priority, time, expand drawer) → Task 15, 18.
- §5 Backend RPC + Bus + N+1 → Tasks 1-7.
- §6 Panel routing → Tasks 8-12.
- §6.2 Views reshape → Task 13.
- §6.3 RPC client extension → Task 14.
- §6.4 Event subscription → Task 20.
- §7 Data flow example → covered end-to-end by Tasks 7 + 14 + 20.
- §8 Testing → Tasks 1, 2, 3, 22, 23.

**Placeholder scan:** None found. Every step shows the actual code or command.

**Type consistency:**
- `CoordTaskStatus` snake_case wire values match between `mod.rs:as_str()` (already on main) and Task 5, 6, 14, 17, 18.
- `Priority` wire values match Task 14 default + Task 15 class mapping.
- `TopicEvent` payload shape in Task 2 matches what Task 20's event handler parses (`topic` field, `data.task_id`/`data.team_id`).
- `TeamsTabState` is defined in Task 13 (`mod.rs`) and consumed in Tasks 11, 19, 20.
- `TaskFilter` / `TaskPatch` defined in Task 14 and consumed in Tasks 18, 20.

**Risks flagged inline:**
- Task 20 has a guard step (Step 20.2) for the unknown `subscribe_events` signature.
- Task 23 has a fallback if `just dev` is unavailable.
