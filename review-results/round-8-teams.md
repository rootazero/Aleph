# Logic Review Report — src/teams
**Module**: src/teams
**Scope**: full module (49 .rs files)
**Date**: 2026-08-29
**Mode**: strict

## Summary

Audit of all 49 Rust files under `src/teams/`. The module implements the team lifecycle (Team/TeamMember/TeamStatus), the autonomous task DAG dispatcher, the group-chat broadcast orchestrator, team messaging, collaborative sessions, snapshots, and the team-template materializer.

This is a critical, large module. Most production code is careful, well-instrumented, and follows the documented invariants, but the audit surfaced one outright security-grade bug in the coord-task ownership gate, plus several warning-grade issues.

## Findings

### [Critical] `task_team_reachable` does not enforce ownership; only existence
- **Location**: `src/teams/scoped.rs:115-122` (function body) and all 9 caller sites in `src/builtin_tools/team/{task_comment,task_submit,task_review,task_control,task_exit_journal,workflow_canvas,workflow_step}.rs`
- **Trigger condition**: A user (Bob) who is not in the room / does not own the team calls one of the coord-task tools (`task_comment`, `task_submit`, `task_review`, `task_control`, `task_exit_journal`, `workflow_canvas`, `workflow_step`) on a `task_id` whose `team_id` belongs to Alice. Because `task_team_reachable` is the documented ownership gate (it is named the "tool-side twin of `gateway::handlers::teams::visibility::gate_task`" in the doc-comment) and the call sites treat its return value as the authorization verdict, Bob's call is allowed: the function only checks `matches!(store.get_team(id).await, Ok(Some(_)))`. `team_visible(t.owner_user_id, t.scope_id)` is never consulted on the `Some(id)` branch.
- **Expected behavior**: Function should return `team_visible(team.owner_user_id, team.scope_id)` for `Some(id)`, mirroring the `None` arm that already calls `team_visible(None, None)`. Today the `Some(id)` arm skips the ownership predicate entirely.
- **Actual behavior**: Any team the caller can name by id is reachable, regardless of `owner_user_id` / `scope_id` visibility. Since the call sites deny access only when this returns false, the only way to deny Bob is for the team to be missing from the store — not for it to be Alice's.
- **Suggested fix**: Fetch the team, then `matches!(t, Some(team) if team_visible(team.owner_user_id.as_deref(), team.scope_id.as_deref()))`. The `Ok(Some(_))` literal has to drop in favor of the visibility check.
```rust
pub async fn task_team_reachable(
    store: Option<&Arc<dyn TeamStore>>,
    team_id: Option<&str>,
) -> bool {
    let Some(store) = store else { return true };
    match team_id {
        None => team_visible(None, None),
        Some(id) => match store.get_team(id).await {
            Ok(Some(t)) => team_visible(t.owner_user_id.as_deref(), t.scope_id.as_deref()),
            Ok(None) => false,
            Err(_) => false, // fail closed: a locked SQLite is a denial, not a pass
        },
    }
}
```
- **Why this is critical**: The `task_review` regression in the module doc-comment ("a fail-closed answer consumed as a value inverts into a permission") and the `task_update`/`task_wait` historical gap the same paragraph names are exactly this class of bug. Returning `true` whenever the row exists, instead of "exists AND visible", inverts the gate into a permission grant. The function is the only thing standing between nine tools and 9+ potential cross-team read/write/control surfaces.

### [Warning] `read_member_row` silently downgrades `AcpSession` to `Agent` on parse failure
- **Location**: `src/teams/store.rs:349-359` (`fn read_member_row`); line 351 specifically
- **Trigger condition**: The `kind` column in `team_members` is a `TEXT NOT NULL DEFAULT 'agent'` (added by migration `add_column_if_missing(&conn, "team_members", "kind", "TEXT NOT NULL DEFAULT 'agent'")`). A row that somehow stores a non-`agent` / non-`acp_session` string (corruption, foreign write, future enum extension) makes the `FromSql` impl return an error, and `row.get(4).unwrap_or(TeamMemberKind::Agent)` silently substitutes `Agent`. The downstream `dispatcher::schedule::resolve_dispatch_target` then refuses to construct an `AcpSession` target (returns `Err("ACP member '...' is missing required routing fields (harness_id/cwd)")`) — but only because `acp_harness_id` / `acp_cwd` are also None, not because `kind` was downgraded. A more targeted case: a member row with `kind='acp_session'` but a corrupted string we still want to surface as AcpSession, ends up Agent-routed and falls into the registry, which returns "Agent not found" — a wrong-but-loud failure. Either fail loud (`row.get::<_, TeamMemberKind>(4)?`) or return an error in the `FromSql` impl, do not silently rewrite the discriminant.
- **Current impact**: low (the schema is internally consistent in practice, the FromSql impl would only fail on corruption or future extension)
- **Suggestion**: Replace `row.get(4).unwrap_or(TeamMemberKind::Agent)` with `row.get::<_, TeamMemberKind>(4)?` to surface DB-level corruption. The `?` is safe because every other field in the same struct already uses `?`.

### [Warning] `try_send_first_system_notification_impl` dedup check uses parameter `thread_id`, not the resolved thread of the new message
- **Location**: `src/teams/messages/store.rs:338-419`; the relevant query is at line 363-369
- **Trigger condition**: The function is called by `MessageRouter::maybe_escalate` with `(notification, ttl, team_id, thread_id)` where `thread_id` is the message thread the router is escalating. The check query is
  ```sql
  SELECT EXISTS(SELECT 1 FROM team_messages
   WHERE team_id = ?1 AND thread_id = ?2
     AND msg_type = 'system_notification' AND from_agent = ?3)
  ```
  but the row about to be inserted uses `resolved_thread_id = Self::resolve_thread_id(&conn, &id, &input.reply_to)?` (line 352). Today the only call site sets `reply_to = Some(thread_id)`, so `resolved_thread_id == thread_id` and the check happens to fire against the right thread. A future caller that passes a different `thread_id` from what `resolve_thread_id` produces for the new message (e.g. `reply_to = None` while passing a thread id) would dedup against the wrong thread — a once-only guarantee could then suppress notifications that were never sent.
- **Current impact**: low (no current caller triggers the gap)
- **Suggestion**: Either (a) dedup against the resolved thread of the new message (`params![team_id, resolved_thread_id, input.from_agent]`) or (b) document the invariant that callers must pass the same `thread_id` that `resolve_thread_id` would produce and add a debug_assert.

### [Warning] `notify_settled_workflow_runs` calls `list_tasks(CoordTaskFilter::default())` for the whole DB
- **Location**: `src/teams/dispatcher/schedule/settle.rs:135-142`
- **Trigger condition**: Every `dispatch_once` tick with a wired `channels` cell enumerates the entire `coord_tasks` table, groups by `workflow_run_id`, and checks `is_settled()` for each. With thousands of `coord_tasks` rows this is O(N) every fallback tick (default 60s) — and the channel-fallback in the same file (line 244) also runs the same group-by. The data is on a hot path.
- **Current impact**: low–medium (depends on deployment; tolerable for tens of runs, painful at thousands of tasks)
- **Suggestion**: Add a `team_id` filter (settle sweep only cares about workflow tasks with a team scope) or a `manager` filter that asks `coord_store` for the run ids it has previously seen, instead of every row.

### [Warning] `register_fanout` cap is global, not per-team
- **Location**: `src/teams/broadcast/mod.rs:435-456` (FIFO eviction, cap `MAX_TRACKED_FANOUTS = 4096`)
- **Trigger condition**: A team whose members collectively exceed the global cap (e.g. 50 teams each running 100 fan-outs) evicts a fan-out from a different team. The docstring (line 252-256) explicitly says "fail-closed, the safe direction", so this is documented, but it is worth raising because `team.chat.cancel` will then refuse the rightful owner of the run id with the byte-identical "not yours" error as for a never-minted id. From the user's point of view, their run id silently went away.
- **Current impact**: low
- **Suggestion**: If cancellation must outlive fan-out, persist the run id → team mapping (alongside the snapshot store) instead of holding it in memory.

### [Warning] `MemberRunStatus` class includes no `Abandoned` arm — orphan-recovery path is silent
- **Location**: `src/teams/dispatcher/runner.rs:88-105` (enum), `runner.rs:115-128` (mapping)
- **Trigger condition**: `MemberRunStatus` is `{Completed, Failed, Timeout, Busy, Cancelled}`. The mapping `run_status()` correctly forwards Busy and Cancelled to `TaskRunStatus::Abandoned` so the retry budget is not spent. The orphan-recovery budget counter (`recovery_abandons_since`) is also correctly read in `schedule/reclaim.rs:120-132`. However, `MemberRunStatus` has no `Abandoned` variant of its own — the busy/cancel events are only distinguishable in the `run_status()` projection. A future contributor adding a new error arm to `classify_execution_error` (line 144-148) that maps to a third "non-failure" outcome has no slot to put it and will silently route through `_ => MemberRunStatus::Failed`, paying a retry.
- **Current impact**: low
- **Suggestion**: Add an `Abandoned` variant to `MemberRunStatus` and route through it explicitly. Or document the invariant at the enum.

### [Warning] `SessionCoordinator::finalize` is not atomic — read-then-write race
- **Location**: `src/teams/sessions/coordinator.rs:172-187` (`finalize` method)
- **Trigger condition**: `finalize` fetches the session, checks `participants.contains(agent_id)`, then calls `conclude_session`. Between the fetch and the write, a `cancel` from another path can move the session to `Cancelled`, but `conclude_session` only accepts `active`/`deadlocked`. The write would then silently no-op and return `db_err("cannot conclude session ...")`. This is a benign failure (the operator's cancel wins), but the helper returns `Err` and surfaces a misleading "cannot conclude" to the caller that called `finalize` to record the outcome of an active session.
- **Current impact**: low
- **Suggestion**: Either swallow the "already terminal" error in `finalize` and return `Ok(())`, or check `status != Cancelled` before returning the error.

### [Warning] `ArtifactStore::create_artifact` content cap is 1 MiB
- **Location**: `src/teams/artifacts.rs:77-79` (constant) and `artifacts.rs:434-441` (cap check)
- **Trigger condition**: A member produces a 2 MiB report (e.g. full diff of a large file, generated schema JSON for a big dataset). The cap is enforced at write time with `AlephError::config` (a `ConfigError`). The plan/submission flow then fails the entire `submit_plan` because the artifact was the entire point. The error message names the cap, which is correct, but the constant is `pub(super)` so a 1 MiB ceiling is unconfigurable.
- **Current impact**: low
- **Suggestion**: Move the cap to `ArtifactStore::create_artifact` config or a `CoordinatorConfig` field so operators can raise it for big-analysis templates without recompiling.

### [Warning] `Workflow_canvas::compute_layers` is O(n^3)
- **Location**: `src/teams/workflow_canvas.rs:135-205`
- **Trigger condition**: For each layer, the algorithm walks every task (n) and for each task walks every dependency (≤n). With 200 tasks that is ~8 million comparisons per render. Rendering a 200-task team's canvas blocks the request.
- **Current impact**: low for typical team sizes, but exponential in pathological templates
- **Suggestion**: Maintain an `out_edges: Vec<Vec<usize>>` adjacency list and decrement in-degrees in O(out-degree). O(n+e) per layer, O(n+e) total.

### [Warning] `disband_team` does not invalidate the dispatcher's in-memory state
- **Location**: `src/teams/store.rs:532-561` (disband) and `src/teams/dispatcher/schedule/mod.rs:108-130` (claim path)
- **Trigger condition**: A team is disbanded while the dispatcher holds a `running` entry for one of its tasks and the BackgroundAgentTracker still has a child registration. The next tick re-claims the same task (the team row is updated to `disbanded`, but the task rows are not touched, and the team_store handle is the SCOPED store which still serves the row). A `delete_team` (line 563-583) similarly leaves the in-memory dispatcher state pointing at a team_id that no longer exists. The dispatcher's `cancel_runs_of_terminal_tasks` only triggers on terminal task statuses, not team status.
- **Current impact**: low (the row's `team_id` is still queryable, so the dispatch continues to operate against a phantom team)
- **Suggestion**: Add a `GlobalBus` event (`TeamDisbanded` already exists) to the disband path, and have the dispatcher treat disband as a `cancel_runs` trigger. Currently `TeamDisbanded` exists in `AlephEvent` but the dispatcher's subscription filter does not include it.

### [Suggested Test] Coverage gaps
- **`task_team_reachable` ownership check** (see Critical above): a unit test that creates a team as Alice, switches the ambient actor to Bob, and asserts that `task_team_reachable(Some(&store), Some("alice-team"))` returns `false`. Without this test, the regression returns the day the next person touches the function.
```rust
#[tokio::test]
async fn task_team_reachable_rejects_non_owner_even_when_team_exists() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let store = Arc::new(SqliteTeamStore::new(conn));
    store.migrate().await.unwrap();
    let team = store.create_team(NewTeam {
        name: "secret".into(),
        description: "".into(),
        leader_id: "leader".into(),
    }).await.unwrap();
    let store = ScopedTeamStore::wrap(store);
    // Ambient actor is None (test scope) → unrestricted; the gate should
    // *not* admit a foreign owner. Pin the new check.
    assert!(
        task_team_reachable(Some(&store), Some(&team.id)).await,
        "no ambient actor must be unrestricted"
    );
    // Re-run under a different user: with the fix, this returns false.
    with_scope(
        Some(ScopeAttribution::personal("u-bob")),
        async { task_team_reachable(Some(&store), Some(&team.id)).await }
    ).await,
    false,
    "bob must not reach alice's team via task_team_reachable",
    );
}
```

- **`MemberDispatchTarget::AcpSession` partial routing fields**: `from_member` correctly returns `None` if `acp_harness_id` or `acp_cwd` is `None`, but a row with `acp_session_name: Some(...)` but missing the cwd would still be `None`. Add a test for that shape so a corrupt row is caught at dispatch-claim time, not at run time.
```rust
#[test]
fn dispatch_target_acp_missing_cwd_returns_none() {
    let mut m = acp_member(Some("review-bot"));
    m.acp_cwd = None; // simulate corrupt DB row
    assert!(MemberDispatchTarget::from_member(&m).is_none(),
        "missing cwd must fail-fast at from_member, not at the ACP adapter");
}
```

- **`try_send_first_system_notification_impl` thread mismatch** (see Warning above): pin the invariant that the caller's `thread_id` parameter equals the new message's `resolved_thread_id`, and add a test that exercises the path.
```rust
#[tokio::test]
async fn try_send_first_system_notification_dedups_by_resolved_thread_not_parameter() {
    let store = SqliteMessageStore::new_in_memory().await;
    // reply_to: None → resolved_thread_id = the new message's own id,
    // independent of the thread_id the caller passed.
    let mut n = sample_notification();
    n.reply_to = None;
    let first = store
        .try_send_first_system_notification(n, chrono::Duration::minutes(15), "t", "caller-thread")
        .await.unwrap();
    let mut n2 = sample_notification();
    n2.reply_to = None;
    let second = store
        .try_send_first_system_notification(n2, chrono::Duration::minutes(15), "t", "caller-thread")
        .await.unwrap();
    // Each new message starts its own thread → both insert. The current
    // code dedups on the caller's thread_id and would suppress the second.
    // The expected behavior is to insert both.
    assert!(first.is_some() && second.is_some(),
        "different resolved threads must not dedup against each other");
}
```

- **`Workflow_canvas::compute_layers` cycle orphan behavior** is documented to put stragglers in a final "orphan" column, but there is no test asserting the actual x-coordinate of a cycle straggler. Add a test that constructs `a -> b -> a` and pins that the orphan ends up at the rightmost column.
```rust
#[test]
fn cycle_stragglers_land_in_the_orphan_column() {
    let tasks = vec![
        mk_task("a", vec!["b"], CoordTaskStatus::Pending),
        mk_task("b", vec!["a"], CoordTaskStatus::Pending),
    ];
    let doc = tasks_to_canvas(&tasks);
    // The doc claims orphans go to the rightmost column; pin it.
    let a = doc.nodes.iter().find(|n| n.id() == "a").unwrap().common();
    let b = doc.nodes.iter().find(|n| n.id() == "b").unwrap().common();
    assert_eq!(a.x, b.x, "cycle stragglers share the orphan column");
    assert!(a.x > 0, "orphan column is past the regular layers");
}
```

- **Loom test for `team_work_finished` + `won_claim` race in `notifier.rs:243-303`**: the TOCTOU hardening is documented but never exercised under loom. Spawn two handlers in loom, have the second arrive during the first's `team_work_finished().await` window, and assert the roll-back path. The current real-only test (`derived_statuses_round_trip_through_filter_vocabulary`) does not exercise the claim.

## Cross-Module Findings

1. **teams ↔ dispatcher ↔ sessions ↔ events ↔ messages**
   - All five subsystems are wired correctly: `TeamStore` is the SCOPED handle everywhere except in test code; `MessageStore` and `EventLogStore` are trait objects; `CoordTaskStore` is the only one the dispatcher holds directly.
   - The one cross-module gap is the disband→dispatcher event: `AlephEvent::TeamDisbanded` exists and is broadcast, but the dispatcher's `EventFilter` in `dispatcher/mod.rs:234-244` does not subscribe to it. Disbanded teams still drive the dispatcher until their task rows are touched.

2. **teams ↔ harness / agents**
   - `MemberRunStatus::run_status()` correctly projects Busy/Cancelled to `Abandoned` so the retry budget is not spent; `tasks::retry::budget_failures_since` is the single source of truth.
   - The permission gate (the `agent_admits_user` check in `runner.rs:351-374` and the same predicate in `broadcast/mod.rs:670-680`) is enforced in both team run producers. `broadcast::GroupChatBroadcaster::run_member` and `dispatcher::runner::execute_member_task` both check the same `allowed_users` set; `team_delegate` is documented to be member-open and is not in either path.

3. **teams ↔ workflow (workflow/compile.rs)**
   - `WORKFLOW_STRATEGY_KEY`, `WORKFLOW_SCHEMA_KEY`, `WORKFLOW_NOTIFIED_KEY`, `WORKFLOW_STEP_KEY`, `WORKFLOW_RUN_ID_KEY`, `WORKFLOW_NOTIFIED_BY_KEY` are read by the dispatcher; `WORKFLOW_ORIGIN_KEY` is read by the settle sweep. The schema lives in `workflow/compile.rs`. The compile-time guard is the exhaustive match on `AlephEvent::name` and `event_type`, not a hand-synced table. This is correct.
   - `workflow::clarify::*` is read by `dispatcher::clarify::handle_clarify_task`; the constants `CLARIFY_OWNER`, `CLARIFY_DELIVERED_AT_KEY`, `CLARIFY_DELIVERY_PENDING_KEY` line up.

4. **teams ↔ builtin tools**
   - Nine tools call `task_team_reachable` (see Critical above). The remaining tools that address coord tasks are exempt by virtue of resolving the team through the decorated `TeamStore` (they never address a bare `task_id`).
   - The census test `every_coord_task_tool_answers_the_ownership_question` in `scoped.rs:486-595` walks the source and fails by name. The fix to `task_team_reachable` will not break the census, but the census does not catch the gap in `task_team_reachable` itself.

## Summary

| Level | Count |
|-------|-------|
| Critical | 1 |
| Warning | 8 |
| Suggested Test | 4 |
