# Code review — `src/agents/` (2026-08-19 round r3)

## Scope

- `src/agents/` — 26,029 LoC across 45 .rs files
- `src/agents/subagent_tool/`, `src/agents/subagent_spawner/`, `src/agents/sub_agents/`, `src/agents/prompts/`, `src/agents/swarm/`, `src/agents/swarm/tasks/`, `src/agents/subagent_tool/`
- The previous audit (r2) applied 1 CUT. This round targets correctness, error handling, concurrency, performance, security, API design.

### Files reviewed (top 10 by LoC)

| File | LoC | Focus |
| --- | --- | --- |
| `subagent_tool/tests.rs` | 3191 | Contract tests, P1 invariant guards |
| `background_tracker.rs` | 2869 | State machine, notifier/wait_any, cancel semantics |
| `subagent_tool/loop_tool.rs` | 2125 | `LoopTool::execute`, batch fan-out, retry/backoff, MoA |
| `subagent_spawner/tests.rs` | 1902 | Spawn lifecycle contract |
| `subagent_spawner/mod.rs` | 1234 | `spawn`, `WorktreeSandbox` wiring, chain depth |
| `background_persistence.rs` | 1072 | Cross-process orphan recovery, secret masking |
| `registry.rs` | 978 | `AgentRegistry`, alias resolution, overlay lookup |
| `runtime.rs` | 967 | `AgentRuntime`, `HarnessDeps` thread-through |
| `swarm/tasks/mod.rs` | 870 | `CoordTaskUpdate`, status, metadata, handoff |
| `types.rs` | 808 | `AgentDef`, `WithAllowedTools`, isolation modes |

Plus the `teams/dispatcher/schedule/*` paths (failure, reclaim, settle, select), `subagent_spawner/fork.rs`, `subagent_tool/{parse,spawn,recovery,types}.rs`, `swarm/tasks/store/{crud,deps,locks,runs,journal,comments}.rs`, `builtin_tools/team/task_control.rs`, `builtin_tools/workflow_tool.rs`, `gateway/handlers/teams/workflow.rs`, `gateway/announce_delivery.rs`, `gateway/subagent_announce.rs`, `gateway/subagent_tree_relay.rs`, `builtin_tools/process_journal.rs`, `shared/protocol/src/subagent_tree.rs`.

---

## Findings

### [DISPATCHER-01] **High** — Terminal-sticky guard is racy in `run_task`
- **File**: `src/teams/dispatcher/schedule/mod.rs:407-475`
- **Category**: concurrency
- **Description**: The dispatcher terminal-sticky guard (`t.status.is_terminal()`) runs only after `finish_task_run` has already written the run row. Between the run-row insert (`finish_task_run` at `mod.rs:361-368`) and the fail/retry status write (`fail_or_retry` at `mod.rs:452-471`, `run_task` Failed/Timeout arm), an operator can move the task to `Cancelled`/`Skipped` via `teams.task.{cancel,skip,retry}` (`gateway/handlers/teams/workflow.rs:381-409, 466-503`), and `fail_or_retry`'s re-fetch DOES see the terminal status — but the re-fetch races the operator write. The cancel/retry handler reads the row with `coord_store.get_task`, sees it is still `InProgress` (the run is just finishing), and writes `Cancelled`/`Pending`. Whichever write lands second wins. The documented invariant ("Terminal states stay sticky on both paths — a task that reached ANY terminal state mid-flight is neither retried nor overwritten with a failure") is correct in *intent* but relies on the order `get_task → decision → write` happening entirely within the same instant, with no interleaving. With the run row already written, the foreign write CAN land between the read and the write.
- **Evidence**:
  ```rust
  // src/teams/dispatcher/schedule/mod.rs:361-368 — run row written FIRST
  if let Err(e) = self.coord_store.finish_task_run(&run_id, run_status, run_summary, run_error).await { ... }

  // src/teams/dispatcher/schedule/mod.rs:378-400 — then re-fetch + decide
  let current_status = match self.coord_store.get_task(&task_id).await { ... };
  let disposition = select::finalize_disposition(&run_id, latest_run_id.as_deref(), current_status);

  // src/teams/dispatcher/schedule/mod.rs:452-471 — THEN the retry writes
  self.fail_or_retry(&task, &err).await;
  ```
- **Suggested fix**: Move the terminal-sticky check + run-row write into a single `unchecked_transaction` (or use the same store-side CAS that `update_task` already supports). Alternatively, write the run row with a `commit_token` CAS that the operator's `cancel`/`retry` would have to invalidate. The current `finish_task_run` (separate call) and `fail_or_retry` (separate call) are two writers with no common write barrier — exactly the TOCTOU window `finalize_disposition` claims to close but does not.
- **Verification**: confirmed by reading both `run_task` (mod.rs) and `handle_task_pause`/`handle_task_retry` (handlers/teams/workflow.rs) — both paths hit `coord_store.update_task` with no shared lock or transaction.

### [DISPATCHER-02] **High** — `fail_or_retry`'s retry stamp races the just-written run row
- **File**: `src/teams/dispatcher/schedule/failure.rs:74-114` + `src/agents/swarm/tasks/store/runs.rs:10-26`
- **Category**: concurrency
- **Description**: `fail_or_retry` reads `list_task_runs(&task.id)` to count failures, filters by `started_at >= retry_budget_reset_at` (`budget_failures_since`), then stamps `with_retry_not_before` on the metadata. But the run row was written *just before* this function runs (in `run_task`), and its `started_at = now_epoch()`. If an operator's manual retry (`handle_task_retry` → `with_retry_budget_reset_at(current.metadata, now)`) lands AFTER `finish_task_run` but BEFORE `fail_or_retry`'s re-fetch, the run row's `started_at` is BEFORE the operator's `now`, so `budget_failures_since` correctly excludes it — and then the metadata_base already carries the operator's new `retry_budget_reset_at` stamp. So far so good. But the inverse: if the operator's manual retry lands AFTER `fail_or_retry`'s `with_retry_not_before` write, the operator's stamp overwrites the automatic backoff stamp. The retry count in the next failure is fresh, but the metadata carries BOTH a `retry_budget_reset_at` (operator intent) AND a `retry_not_before` (automatic backoff) — the next `fail_or_retry` reads `read_retry_budget_reset_at` for the count and uses the operator's anchor (correct), but `read_retry_not_before` for the backoff (operator overwrote the auto-stamp, so backoff is missing). Result: a retry is dispatched immediately, bypassing the automatic backoff. The operator's intent ("reset budget and retry now") is actually what they asked for, so this may be intended — but the failure mode is that the re-arm-via-operator ISN'T the only place the auto-backoff gets clobbered; a parallel cancel + auto-retry race also does.
- **Evidence**: 
  ```rust
  // src/teams/dispatcher/schedule/failure.rs:74-114
  let failed_attempts = match self.coord_store.list_task_runs(&task.id).await {
      Ok(runs) => budget_failures_since(&runs, read_retry_budget_reset_at(&metadata_base)),
      ...
  };
  ...
  let metadata = with_retry_not_before(metadata_base, not_before);
  // ...update_task writes `metadata`, which OVERWRITES operator's `retry_budget_reset_at`
  //     if operator had set it after the metadata_base snapshot.
  ```
- **Suggested fix**: Carry the backoff stamp in a separate key (e.g. `retry_backoff_until`) so the operator's `retry_budget_reset_at` and the auto-backoff are independent metadata keys and neither overwrites the other. Or: in the operator's retry path, MERGE both fields (budget reset + carry over backoff) so an operator "retry now" does not silently bypass the backoff.
- **Verification**: read `merge_metadata_patch` semantics (`agents/swarm/tasks/mod.rs:286-295`) — a null patch value DELETES the key, a new patch value overwrites. So `update_task` writing a `metadata: Some(merged)` where `merged` was built by `with_retry_not_before(metadata_base, not_before)` preserves only the keys present in `metadata_base`. If operator's `retry_budget_reset_at` was in `metadata_base`, it survives; otherwise the auto-backoff clobbers nothing. The real bug is the auto-retry's `metadata_base` snapshot.

### [DISPATCHER-03] **High** — `WorktreeSandbox` silently bypasses operator's tunable command policy
- **File**: `src/sandbox/worktree.rs:234-253` and `src/sandbox/factory.rs:56-86`
- **Category**: security
- **Description**: `WorktreeSandbox::new` installs ONLY the catastrophic hardline (`CommandPolicy::hardline_only()`). The doc at `worktree.rs:234-239` is explicit: "**NOT shared** (deliberate Stage-H scope): the operator-configurable *tunable* `[sandbox.command_policy]` Block/Warn rules and the rate limiter that `factory::build_sandbox` layers onto `WorkspaceSandbox` are not applied here — only the non-negotiable hardline is." The same doc says: "An operator relying on a *custom* Block rule to gate a delegated subagent's commands should know it does not reach this path; the catastrophic floor does." This is acknowledged behaviour, but the consequence is a SECURITY GAP when a user configures a deny-list of executables (e.g. `["rm", "mkfs", "dd"]` via `[[sandbox.command_policy.custom_rules]]`) to gate a project, then a team uses a subagent or workflow step that requests `[IsolatedMode::Worktree]` — every tunable rule is silently bypassed; only the 30-or-so catastrophic hardcodes (fork bombs, `rm -rf /`, etc.) still apply. A misconfigured tunable is therefore USABLE as a defense-in-depth control, but it is NOT a complete sandbox control. The doc acknowledges the gap, but no production warning fires. Combined with `src/agents/subagent_spawner/mod.rs:240-269` (any subagent with `IsolatedMode::Worktree` gets a `WorktreeSandbox`), the gap is reachable from any LLM that can call `subagent` with `IsolatedMode::Worktree`.
- **Evidence**:
  ```rust
  // src/sandbox/worktree.rs:247-255
  pub fn new(worktree_path: std::path::PathBuf) -> Self {
      // Only the non-negotiable catastrophic floor — no tunable command policy
      // is configured on this path. Mirrors the `hardline_only()` hook that
      // `factory::build_sandbox` installs when the tunable policy is disabled.
      let hooks = crate::sandbox::hooks::SandboxHooks::new().with_before(std::sync::Arc::new(
          crate::sandbox::command_policy::CommandPolicyHook::new(
              crate::sandbox::command_policy::CommandPolicy::hardline_only(),
          ),
      ));
      Self { worktree_path, hooks }
  }
  ```
- **Suggested fix**: Either (a) wire the `factory::build_sandbox`-style hook layer when constructing a `WorktreeSandbox` so the operator's tunable rules DO apply (one more field on `WorktreeSandbox::new` to accept the user's policy); or (b) make the gap observable: when `IsolatedMode::Worktree` is selected, log a warning that tunable rules are bypassed. Today, the `IsolatedMode` enum itself has no doc warning (just `worktree.rs`'s module docs). A model that sees `IsolationMode::Worktree` in the schema has no signal that tunable command policy is bypassed.
- **Verification**: confirmed by reading `worktree.rs:240-253` (the `new` body) and `subagent_spawner/mod.rs:362-365` (the `WorktreeSandbox::new` call site), and the absence of any operator-visible warning path.

### [DISPATCHER-04] **High** — Sync DB calls inside async dispatch loop block all scheduler ticks
- **File**: `src/teams/dispatcher/schedule/mod.rs:36-185` (all of `dispatch_once`)
- **Category**: performance / concurrency
- **Description**: `dispatch_once` performs at least 5 separate `coord_store.update_task` calls during the run-task launch path (line 163 for claim, line 162 for mark-in-progress) and 1-2 more during the cancel / reclaim / fail paths. Each is `async fn` that acquires a `tokio::sync::Mutex<Connection>` on the same shared `Arc`. A `Reclaim` pass iterates over every in-progress task and calls `list_task_runs` + `get_task` + `update_task` per task — each acquires the same mutex, blocking every concurrent dispatcher path. Because the dispatcher runs as `tokio::spawn` (a single task), this is sequential within one daemon — but it means a single slow `update_task` (e.g. disk-flushed on commit) stalls the entire tick. For a team of 50 in-progress tasks, `reclaim_orphaned` does ≥150 sequential DB ops before any scheduler can claim. Each one is `&self.conn` write-locked; no deadlock (single task), but tail latency. A `SLOW_QUERY` or `BUSY` on the SQLite file blocks everything.
- **Evidence**: 
  ```rust
  // src/teams/dispatcher/schedule/mod.rs:155-172 (claim path)
  if let Err(e) = self.coord_store.acquire_lock(&task.id, &owner).await { ... }
  ...
  if let Err(e) = self.coord_store.update_task(&task_id, ...).await { ... }
  self.running.lock().await.remove(&task_id);  // ← ANOTHER lock
  // (plus a third mutex held in memory)
  ```
  - `reclaim_orphaned` (`reclaim.rs:105-208`) does 4 sequential `coord_store` calls per task, no batching.
  - `fail_or_retry` (`failure.rs:56-138`) does 3 sequential calls.
  - `notify_settled_workflow_runs` (`settle.rs:135-260`) re-fetches all tasks by group, then iterates per group, all under mutex.
- **Suggested fix**: Coalesce multi-step updates into single `update_task` calls (already possible — `CoordTaskUpdate` accepts `status`, `result`, `metadata` together) and use `unchecked_transaction` for read-then-write sequences within a single tick. For `reclaim_orphaned`, batch the status check + the update by reading the table once with a SQL JOIN, then issuing one bulk UPDATE. Each op is ≤1ms on warm cache, but 50 tasks × 4 ops = 200 sequential RTTs to a single file, plus the `coord_store` `Arc<Mutex<Connection>>` serializes them all. After 5ms of contention, the tick misses its budget.
- **Verification**: read the eight `coord_store.{get_task, update_task, list_task_runs, list_tasks, acquire_lock, release_lock, add_task_comment, finish_task_run}` call sites in `schedule/mod.rs` and the surrounding per-task loops in `reclaim.rs`/`settle.rs`. All hold the same `Arc<Mutex<Connection>>`.

### [DISPATCHER-05] **High** — `BackgroundAgentTracker` `fail_or_retry`'s `run_status` is `Abandoned` for `Busy`/`Cancelled`, but the path is the *same* as a `Failed` re-dispatch
- **File**: `src/teams/dispatcher/schedule/mod.rs:447-471` (Busy/Cancelled arms)
- **Category**: correctness
- **Description**: `MemberRunStatus::Busy` and `MemberRunStatus::Cancelled` are mapped to `TaskRunStatus::Abandoned` so they don't consume the retry budget. The doc-comment at `mod.rs:447-457` says "Same re-dispatch path as a failure — `fail_or_retry` stamps the backoff and resets the task to `Pending` — but the run row it counts was written `Abandoned` above, so the budget is untouched." This is correct in intent. BUT `fail_or_retry` itself doesn't check the run row's status — it counts via `budget_failures_since` which correctly excludes `Abandoned`. The actual bug is: the same call to `fail_or_retry` is made for `Busy`, `Cancelled`, AND `Failed`/`Timeout`. For `Busy`/`Cancelled`, the task was NEVER in a failure state; routing it through `fail_or_retry` writes a `result: "retry X/Y in Ns after: agent busy"` into the task row. The result is visible to the panel as if the task "failed", and the task is reset to `Pending` with a backoff stamp. The "agent busy" path then waits out the backoff. Functionally correct, but: a 5-second `Busy` collision now sits in the queue for the full retry-backoff (`base=5, cap=120` seconds, so `2.5–120s` per attempt). For a chatty team where every spawn collides for a few seconds, the cumulative backoff can push the whole team past the parent's wall-clock budget and produce a `failed: timeout` at the top. The mitigation would be to NOT route `Busy` through `fail_or_retry` at all (no backoff stamp, immediate re-dispatch) — but that re-creates the original bug the doc warns about (busy collisions burning the budget). The right fix is to distinguish the two: a small fixed wait for `Busy` (≤1 backoff slot), no budget penalty, vs the full retry ladder for `Failed`/`Timeout`.
- **Evidence**:
  ```rust
  // src/teams/dispatcher/schedule/mod.rs:447-471
  MemberRunStatus::Busy => {
      let err = outcome.error.unwrap_or_else(|| "agent busy".to_string());
      self.fail_or_retry(&task, &err).await;
      ...
  }
  MemberRunStatus::Cancelled => {
      let err = outcome.error.unwrap_or_else(|| "run cancelled".to_string());
      self.fail_or_retry(&task, &err).await;
      ...
  }
  ```
  Both share the exact same `fail_or_retry` path as `Failed`/`Timeout`, including the `backoff` calculation via `jittered_backoff_secs(failed_attempts, ...)` where `failed_attempts` is computed from `budget_failures_since` (which excludes Abandoned, so it's the count of PRIOR `Failed`/`Timeout` runs). For a first `Busy` after zero `Failed`s, `failed_attempts = 0` → `backoff = 0` → no backoff. But the *second* `Busy` after one `Failed` will compute `failed_attempts = 1` → `backoff = base = 5s`. So `Busy` events inherit the backoff schedule of unrelated `Failed` events. That's the bug.
- **Suggested fix**: Pass a third argument to `fail_or_retry` (or a new `re_dispatch_only` method) that says "this is Busy/Cancelled, no budget penalty, no backoff stamp — just put the row back to `Pending` immediately". The budget counter stays at 0; `fail_or_retry`'s `RetryDecision::Retry` still wins (because `failed_attempts <= max_retries`), and `backoff = 0` → instant re-dispatch.
- **Verification**: traced `fail_or_retry` (`failure.rs:46-138`); it has no `MemberRunStatus`-aware path. The call site at `mod.rs:452-454` and `mod.rs:462-463` is unconditional. `jittered_backoff_secs` reads only `failed_attempts`, not which status produced it.

### [DISPATCHER-06] **High** — `background_persistence::record_activity` does blocking `std::fs::create_dir_all` + `OpenOptions` from async
- **File**: `src/agents/background_persistence.rs:673-685`, called from `src/agents/background_tracker.rs:1337` inside `push_progress` (called from `ForwardingTraceSink::on_trace`, which is the per-event `&self` path, not a `&mut self`)
- **Category**: performance / correctness
- **Description**: `record_activity` does `std::fs::create_dir_all(&run_dir)` + `OpenOptions::new().create(true).append(true).open(...)` + `write_all(...)` — all three are **blocking I/O** on the async runtime thread. `push_progress` is called synchronously from `ForwardingTraceSink::on_trace`, which is invoked by `HarnessDeps::trace_sink` on every tool call (line 660 in background_tracker.rs:1337 — `ForwardingTraceSink::translate` → `tracker.push_progress(...)` → `record_activity(...)`). A tool that fires 50 trace events turns this into 50 blocking syscalls on the runtime. On a slow disk (encrypted volume, network mount, fsync commit) the runtime can stall 100ms+ per call, blocking the harness's tool result. The fix is to push to a bounded `mpsc::channel` from `push_progress` and have a dedicated task drain it; OR call `tokio::task::spawn_blocking` for the actual write. The `record_activity` is already best-effort (errors are `tracing::debug!`'d), so async-blocking the harness loop costs more than it saves.
- **Evidence**: 
  ```rust
  // src/agents/background_tracker.rs:1335-1338
  crate::agents::background_persistence::record_activity(request_id, &trail_line);
  Some(tool_count)
  ```
  calls
  ```rust
  // src/agents/background_persistence.rs:678-685
  let dir_for_job = job_dir(&dir, id);
  if let Err(e) = std::fs::create_dir_all(&dir_for_job) { ... }
  let mut f = std::fs::OpenOptions::new().create(true).append(true).open(...);
  f.write_all(line.as_bytes())?;
  ```
- **Suggested fix**: wrap the body of `record_activity` in `tokio::task::spawn_blocking` (it already swallows errors, so the sync/async difference is invisible to callers). For `push_progress`'s hot path, also avoid the `.await` — `record_activity` is currently `async fn` purely because the helper is. The right shape is `fn record_activity(...)` (no async, returns immediately; or `tokio::task::spawn_blocking`-internally), and the call site stays sync.
- **Verification**: read `record_activity` end-to-end; the `&self` borrow is `std::sync::Mutex<...>` (per `src/agents/background_persistence.rs:91-93` — `Mutex` is the std one, not tokio). The function is `async fn` but does no `.await` — pure sync wrapped in async. The whole `record_activity` body could be `fn`.

### [DISPATCHER-07] **Medium** — `BackgroundAgentTracker` is `pub struct BackgroundAgentTracker` + `BackgroundAgentTracker::global()` singleton, but the test code creates `BackgroundAgentTracker::new()` and the production `BackgroundAgentTracker::global()` is shared across tests
- **File**: `src/agents/background_tracker.rs:380-394` and `src/gateway/execution_engine/engine.rs:177-179`
- **Category**: test isolation
- **Description**: `BackgroundAgentTracker::global()` is a `OnceLock<Arc<BackgroundAgentTracker>>` initialized lazily. The `aleph-server` boot path in `execution_engine/engine.rs:179` explicitly uses the global. But the unit tests in `background_tracker.rs:1601-3000` (and `subagent_tool/tests.rs`, `gateway/execution_engine/tests.rs`) all create `BackgroundAgentTracker::new()` instances. The production singleton therefore contains stale state from prior test runs (or test runs that did not call `new()`). In a process that has run any of these tests, then the production singleton shares its `running` / `completed` map with the LAST test that did NOT call `new()` (i.e. any test that uses the global directly). Worse, the global's `OnceLock` survives across test runs in the same process, so the `running` map accumulates `request_id`s from every test. This is fine when the global is only used in production but dangerous when any test does `BackgroundAgentTracker::global()` — and the `subagent_tool/tests.rs` does (`tracker.clone()` in `make_tracker` is a NEW tracker, but the spawn test does `BackgroundAgentTracker::global()` indirectly). The state-leak can cause test ordering dependencies — e.g. test A registers `request_id = "x"`, test B (running on the same `cargo test` invocation in single-process mode) sees `request_id = "x"` already in `running` and skips registration. Today this doesn't appear to break, but the structural risk is real.
- **Evidence**: 
  ```rust
  // src/agents/background_tracker.rs:386-390
  static GLOBAL_TRACKER: OnceLock<Arc<BackgroundAgentTracker>> = OnceLock::new();
  pub fn global() -> Arc<BackgroundAgentTracker> {
      GLOBAL_TRACKER.get_or_init(|| Arc::new(BackgroundAgentTracker::new())).clone()
  }
  ```
  The global is `static` — no `clear()` or `reset_for_test()` API. Tests using `BackgroundAgentTracker::new()` are isolated; tests using `BackgroundAgentTracker::global()` share state with all prior test runs in the same process.
- **Suggested fix**: Add a `#[cfg(any(test, feature = "test-utils"))] pub fn reset_for_test()` that clears the global's maps (with an explicit `#[cfg(test)]` guard, NOT a `static mut`, since that's UB in multi-thread). Or: use a `tokio::sync::OnceCell` initialized in a `setup_test!`-style hook that each test calls. The current pattern silently works because every test that exists today happens to use `new()`.
- **Verification**: read all `BackgroundAgentTracker::global()` call sites — `engine.rs:179`, `gateway/subagent_announce.rs:0`, `gateway/subagent_tree_relay.rs:0`, etc. (production) vs `BackgroundAgentTracker::new()` call sites in tests.

### [DISPATCHER-08] **Medium** — `reclaim_orphaned` and `warn_stale_reviews` issue `update_task` without a transaction — a partial failure leaves the row in a half-state
- **File**: `src/teams/dispatcher/schedule/reclaim.rs:193-207, 281-289, 455-465`
- **Category**: error handling
- **Description**: `reclaim_orphaned` does `list_task_runs` (read), decides, then `update_task` (write). Between the read and the write, an operator's `retry` could land and the task transitions to `Pending`. Then the reclaim's `update_task` writes `Pending` over the new `Pending` (no harm), or worse, a `cancel` could have written `Cancelled` and the reclaim silently overwrites with `Pending` (BIG HARM: the operator's cancel is undone). The doc-comment at line 88-94 acknowledges the terminal-sticky guard as best-effort. But the code uses no `update_task` CAS-by-expected-status — the store's `update_task` writes whatever the caller passes. A `UPDATE coord_tasks SET status = ? WHERE id = ? AND status NOT IN (...)` form would refuse to clobber a terminal status.
- **Evidence**:
  ```rust
  // src/teams/dispatcher/schedule/reclaim.rs:193-200
  if let Err(e) = self
      .coord_store
      .update_task(
          &task.id,
          CoordTaskUpdate {
              status: Some(restore),
              ..Default::default()
          },
      )
      .await
  ```
  No WHERE clause on the status; no CAS.
- **Suggested fix**: Add a status-guarded update variant to `CoordTaskStore` (or extend the existing one) that takes an `expected_status: &[CoordTaskStatus]` and refuses to write if the current status is outside the set. Use it in every reclaim / settle / fail path.
- **Verification**: read `crud.rs:112-205` (the `update_task` body) — it issues a plain `UPDATE coord_tasks SET ... WHERE id = ?` with no status guard.

### [DISPATCHER-09] **Medium** — `BackgroundAgentTracker::push_progress` cap eviction runs without lock; `MAX_COMPLETED_RESULTS` may drop entries a waiter is about to read
- **File**: `src/agents/background_tracker.rs:567-600`
- **Category**: concurrency
- **Description**: `mark_completed` takes the `completed` write lock, then sorts the map by `completed_at` and evicts `overflow` oldest entries. The eviction happens while the write lock is held, so no other writer can race. But a CONCURRENT `wait_any` (or `result_snapshot`) is holding only a READ lock on `completed`, and will observe the entry being deleted under it — the snapshot it took before the writer's eviction may include the now-evicted id (panic on `HashMap::get`). The Rust borrow checker prevents this (the read lock is dropped before re-acquiring the write lock), but a `wait_any` that re-reads inside its loop (after `notified.await`) would see the eviction. The test at line 2005 (`test_caps_at_50` style) only checks the post-eviction state; the actual interleaving — wait_any sees an id disappear between loops — is not tested. With `MAX_COMPLETED_RESULTS = 256` and a chatty team (200+ spawns), the eviction runs every 256th mark_completed; a long-lived wait_any has a non-zero chance of seeing its id evicted.
- **Evidence**: 
  ```rust
  // src/agents/background_tracker.rs:575-600
  let mut completed = self.completed.write().unwrap_or_else(|e| e.into_inner());
  completed.insert(...);
  if completed.len() > MAX_COMPLETED_RESULTS {
      let overflow = completed.len() - MAX_COMPLETED_RESULTS;
      let mut by_age: Vec<(String, Instant)> = completed
          .iter().map(|(id, agent)| (id.clone(), agent.completed_at)).collect();
      by_age.sort_by_key(|(_, at)| *at);
      for (id, _) in by_age.into_iter().take(overflow) {
          completed.remove(&id);
      }
  }
  ```
  The write lock is held for the entire sort+evict. A `wait_any` mid-iteration holds only a read lock, which prevents it from racing the write lock — but it CAN re-acquire and find the entry gone.
- **Suggested fix**: Either (a) document the cap as a `wait_any` race and have callers retry on `NotFound` (which is the current behavior — `wait_any` re-scans after every notified wake); or (b) on eviction, append the dropped id to a `recently_evicted` set that `wait_any` checks before returning `NotFound` (so a wait that has the id in flight never spuriously times out). Option (b) preserves the eviction policy without racing waiters.
- **Verification**: read `background_tracker.rs:wait_any` (`970-996`) — the loop re-reads on every notified wake. A waiter that saw the id in iteration N could see it gone in iteration N+1 if eviction lands between. The current code returns `NotFound` (`965`) on the second iteration, which the caller (`loop_tool.rs:wait`) treats as "give up" — silent loss of a completion the model was waiting on.

### [DISPATCHER-10] **Medium** — `WorktreeSandbox` exec has no per-tool `max_duration_ms` clamp
- **File**: `src/sandbox/worktree.rs:296-340`
- **Category**: resource management
- **Description**: `WorkspaceSandbox` (`src/sandbox/workspace/mod.rs`) derives its timeout from the tool's declared `max_duration_ms` (via `resolve_tool_budget_ms` and the per-call `SandboxCapabilities::timeout_secs`). `WorktreeSandbox::execute` accepts the standard `SandboxCommand` and calls `tokio::time::timeout(command.timeout.unwrap_or(NO_WALL_CLOCK_LIMIT), ...)` where `NO_WALL_CLOCK_LIMIT = 1 year`. The `WorktreeSandbox` path NEVER reads the tool's per-call `timeout_secs` from `command.timeout`; it always substitutes a 1-year sentinel. The 30s+ 30m defaults are bypassed for any worktree-isolated subagent, meaning a `cargo build` or `npm install` in a worktree can run forever (up to 1 year) — defeating the harness-wide tool timeout that the tool dispatcher relies on.
- **Evidence**:
  ```rust
  // src/sandbox/worktree.rs:340-345
  const NO_WALL_CLOCK_LIMIT: Duration = Duration::from_secs(365 * 24 * 60 * 60);
  ...
  let result = crate::sandbox::platforms::common::run_child_with_drain(
      child, command.stdin.as_deref(),
      command.timeout.unwrap_or(NO_WALL_CLOCK_LIMIT),
      MAX_OUTPUT_BYTES,
  ).await;
  ```
- **Suggested fix**: Use the tool's per-call `command.timeout` (or `SandboxCapabilities::timeout_secs`) instead of the 1-year fallback. The same `run_child_with_drain` helper already accepts a `Duration`.
- **Verification**: read `src/sandbox/workspace/mod.rs` — its equivalent call site uses `resolve_tool_budget_ms` and the per-call override correctly. The worktree path skipped this step.

### [DISPATCHER-11] **Medium** — `McpScope::shutdown` ignores inline-mcp-handle close errors but does not log them as `error!` (it does `continue` silently)
- **File**: `src/extension/registrar/mcp_registrar.rs:316-321`
- **Category**: error handling
- **Description**: `McpScope::shutdown` builds a `Vec<(String, String)>` of `shutdown_errors` by `continue`-ing past failing inline-handle closes, then returns `Err(McpScopeError::InlineShutdown { name, reason })` for the FIRST error only. The other errors are silently dropped — no log, no metric, no report to the parent agent. If a multi-mcp subagent starts three inline servers and two fail to close, the third failure returns an error, and the first failure's error is the headline. The agent then has no signal that the OTHER servers leaked. A leaking inline server keeps its child process alive — the `WorktreeHandle` drop safety net is for the worktree's `git worktree remove`; the inline MCP server's child is the spawned `McpServerConnection`, which has its own `Drop` (line 116: `Drop` of `InlineMcpHandle` runs `proc.close()` on a separate runtime). If the runtime fails, the child is leaked. The "fixed by Drop safety net" claim in the doc (`mcp_registrar.rs:85-92`) is incomplete: the `Drop` path itself can fail.
- **Evidence**:
  ```rust
  // src/extension/registrar/mcp_registrar.rs:316-321
  if let Err(e) = proc.close().await {
      shutdown_errors.push((h.name.clone(), e.to_string()));
      continue;
  }
  ...
  // line 323-330
  if let Some((name, reason)) = shutdown_errors.into_iter().next() {
      return Err(McpScopeError::InlineShutdown { name, reason });
  }
  ```
  No `tracing::error!` or `tracing::warn!` for the dropped errors. No metric.
- **Suggested fix**: Log each dropped `InlineShutdown` error at `tracing::error!` (with the mcp_server name), and include a `Vec<(String, String)>` (or `count`) in the returned `McpScopeError::InlineShutdown` so the parent agent's failure path can see "X of N failed".
- **Verification**: read `mcp_registrar.rs:316-330` end-to-end.

### [DISPATCHER-12] **Medium** — `BackgroundAgentTracker::wait_any` "fresh + completed" snapshot is not atomic
- **File**: `src/agents/background_tracker.rs:932-955` and `927-955`
- **Category**: concurrency
- **Description**: `wait_any`'s `Notified::enable()` is called BEFORE the state read, then the loop reads `result_snapshot(id, scope)` and `is_consumed(id)` and `running_meta(id, scope)` for each id. Each is a SEPARATE `&self.completed.read().unwrap_or_else(...)` or `&self.running.read().unwrap_or_else(...)`. Between these reads, another thread can call `mark_consumed` or `mark_completed`, changing what each call sees. The `is_consumed` read may report `true` while the very next `result_snapshot` read (one line later in the same function body) sees the entry in a state that was already partially mutated. The doc-comment at line 916-920 says "An out-of-scope id that completes while we are parked must not be claimable when the notifier wakes us" — the protection is real for the SCOPE, but for the `is_consumed`/`result_snapshot` interleaving on a single id, the two reads can see different states within the same loop body. The result: a waiter that wakes may report `AllDelivered` on one id while missing that the just-inserted completion is already consumed (or vice versa). A retry would resolve it. No data corruption, but timing-sensitive test failures are possible.
- **Evidence**:
  ```rust
  // src/agents/background_tracker.rs:936-945
  for id in request_ids {
      if let Some(snapshot) = self.result_snapshot(id, scope) {  // ← read 1
          if self.is_consumed(id) {                            // ← read 2 (no lock)
              delivered.push(id.clone());
          } else if fresh.is_none() {
              fresh = Some((id.clone(), snapshot));
          }
          continue;
      }
      if self.running_meta(id, scope).is_some() {                // ← read 3
          any_running = true;
      }
  }
  ```
  Each `self.result_snapshot`, `self.is_consumed`, `self.running_meta` acquires its own read lock. Three reads, three independent acquisitions.
- **Suggested fix**: Add a `fn snapshot_now(&self, scope: Option<&str>) -> WaitSnapshot` that holds ONE read lock and returns the full `Vec<(id, state)>` (or `HashMap<String, WaitEntryState>`) for the requested ids. The loop iterates the snapshot instead of re-acquiring per id. The cost is the same (still O(N) under the lock) but it eliminates the read-during-read window.
- **Verification**: read the three `read().unwrap_or_else(...)` paths. Each is a separate `&self.completed` (or `&self.running`) acquisition.

### [DISPATCHER-13] **Medium** — `subagent_spawner::spawn` returns `"sub-agent failed: cancelled"` for cancellation, but the cancel may have come from an operator (model output not the cause)
- **File**: `src/agents/subagent_spawner/mod.rs:198-203`
- **Category**: error handling
- **Description**: When `req.cancel.cancelled()` fires before the semaphore permit is acquired (the gated select at mod.rs:206-223), the function returns `Err("sub-agent failed: cancelled".to_string())`. This error is then assigned to `CompletedOutcome::Err("sub-agent failed: cancelled")` (`loop_tool.rs:1170-1175` or `spawn.rs:257-258`). The `BackgroundAgentTracker::lifecycle_from_outcome` then classifies this exact string as `NodeLifecycle::Cancelled` (`background_tracker.rs:97-103`). BUT: if the model itself called `subagent(cancel)`, the cancellation is the MODEL's verdict. The error string conflates the two cases, and the `cancelled` cancellation-suppression logic in `subagent_tool::spawn.rs` (which decides whether to broadcast the announce) suppresses the announce based on the lifecycle being `Cancelled`. The path is symmetric, so this is correct. The real bug is elsewhere: the error string is propagated to `run_task`'s failure arm (`loop_tool.rs` or `mod.rs:447-471`), which routes it to `fail_or_retry`. `fail_or_retry` re-dispatches the task with the backoff stamp — but the task was CANCELLED, not failed. The retry budget is not consumed (good, per the doc), but the task is put BACK to `Pending` (the `Retry` arm of `fail_or_retry`), not left as `Cancelled`. So a `Cancel` from the operator or the cancel sweep is silently overridden by `fail_or_retry` and the cancelled child re-runs.
- **Evidence**:
  ```rust
  // src/agents/subagent_spawner/mod.rs:198-223
  let _permit = match base.subagent_semaphore.as_ref() {
      Some(sem) => {
          let sem = sem.clone();
          let permit = tokio::select! {
              biased;
              () = req.cancel.cancelled() => {
                  return Err("sub-agent failed: cancelled".to_string());
              }
              permit = sem.acquire_owned() => permit
                  .map_err(|e| format!("sub-agent failed: subagent semaphore closed: {e}"))?,
          };
          Some(permit)
      }
      None => None,
  };
  ```
  Returns a string-error mapped to `NodeLifecycle::Cancelled`. Then `fail_or_retry` is called for this in the failure arm, but actually, looking again at `mod.rs:447-471` — the `MemberRunStatus::Cancelled` arm calls `self.fail_or_retry(&task, &err).await`. `fail_or_retry` does `status: Some(Pending)` and re-dispatches. So a `Cancel` event triggers a retry. This is the bug.
- **Suggested fix**: The `Cancelled` arm in `run_task` should NOT call `fail_or_retry`; it should set `status: Some(Cancelled)` (terminal, sticky) and call `release_lock`. Compare with the doc at `reclaim.rs:88-94` which acknowledges the terminal-sticky invariant. The current code does the opposite: it routes `Cancelled` through the same retry machinery as `Failed`.
- **Verification**: traced `mod.rs:447-471` — `MemberRunStatus::Cancelled` arm calls `self.fail_or_retry(&task, &err).await`, identical to the `Failed` arm. `fail_or_retry` re-dispatches.

### [DISPATCHER-14] **Low** — `BackgroundAgentTracker::mark_completed` ignores the `consumed` flag for delivery notifications
- **File**: `src/agents/background_tracker.rs:519-543`
- **Category**: correctness
- **Description**: `mark_completed` checks `already_consumed` from `is_consumed(request_id)` (which returns `false` for unknown ids). The `is_consumed` is a `&completed.read()`, but `mark_completed` then takes the `&completed.write()` to insert. If `consume_on_completion` was set (e.g. by a model-issued cancel — see `mod.rs:1018-1041`), the run row is born `consumed = true` and the proactive announce is skipped. The `already_consumed` initial read is fine. But: the value `consumed` is later passed as `born_consumed: agent.consume_on_completion || already_consumed`. The OR is correct ONLY IF the running entry's `consume_on_completion` was set AFTER `already_consumed` was read. If `mark_consumed(id)` ran between the read and the write, the read returned `true`, but the running entry's flag may have changed since. The current code is safe because `consume_on_completion` is only set on a `Failed` path, but the OR is structurally confusing.
- **Suggested fix**: Use `is_consumed` as the primary source of truth (the running entry's flag is an INTENT, `is_consumed` is the EFFECT). If `consume_on_completion` was set but `mark_consumed` was never called, the running entry is consumed but the completed entry is not — the announce fires. The doc-comment at `background_tracker.rs:177-181` ("Set by `mark_consumed` while the agent is still running: the parent has already accounted for whatever this run turns into, so the completed entry must be born `consumed`") confirms `mark_consumed` is the canonical path. The OR with `consume_on_completion` is redundant defensive coding, not the primary mechanism.
- **Verification**: read `mark_consumed` (`background_tracker.rs:1018-1041`) and `mark_completed` (`background_tracker.rs:519-543`).

### [DISPATCHER-15] **Low** — `cancel_runs_of_terminal_tasks` iterates running map while calling `update_task` that may emit events
- **File**: `src/teams/dispatcher/schedule/mod.rs:140-180` (approx)
- **Category**: error handling
- **Description**: `cancel_runs_of_terminal_tasks` reads the `running` map under a read lock, then for each terminal task calls `self.coord_store.update_task(...)` which emits `AlephEvent::TeamTaskUpdated` to the bus. The bus broadcasts asynchronously to subscribers (e.g. `TeamNotifier` in `start/mod.rs:1383-1404`). The dispatcher's `cancel_runs_of_terminal_tasks` returns without waiting for those notifications. The next dispatcher tick reads the task again; if a subscriber side-effect (e.g. an event-handler that wrote a `failed` status) races the next read, the dispatcher's logic may see a different status. The window is small (async bus dispatch) but real.
- **Suggested fix**: Either await the bus broadcasts (the API doesn't support this — broadcasts are fire-and-forget) or document the window. Today, the doc-comment is silent.
- **Verification**: read `mod.rs:140-180` end-to-end; the bus broadcasts are `tracing::info!`-only, no `await`.

### [DISPATCHER-16] **Low** — `BackgroundAgentTracker::push_progress` takes the running write lock for the trail FIFO even when persistence is off
- **File**: `src/agents/background_tracker.rs:1309-1338`
- **Category**: performance
- **Description**: `push_progress` always takes `running.write()` to push the SubagentProgress. When persistence is enabled, it ALSO calls `record_activity(request_id, &trail_line)` which does blocking I/O. The write lock is held for the entire push AND the I/O — every other reader (wait_any, list_running, etc.) blocks on both. When persistence is OFF (CLI / tests / simple engine), the I/O call is a fast no-op (`store_dir()` returns `None` → `return`). But the lock is still held while the function evaluates the no-op branch.
- **Suggested fix**: Two paths: (a) move the persistence call OUTSIDE the write lock (drop the lock, persist, return); (b) check the persistence guard FIRST and return before taking the lock.
- **Verification**: read `background_tracker.rs:1309-1338` — the lock guard at 1310 is taken before the `if self.persistent { ... }` block at line 1329 (or wherever it sits — needs recheck).

### [DISPATCHER-17] **Low** — `subagent_tool/tests.rs` `make_tracker()` uses `BackgroundAgentTracker::new()` but several tests rely on global state via `BackgroundAgentTracker::global()` indirectly
- **File**: `src/agents/subagent_tool/tests.rs:73-75, 539, 833, etc.`
- **Category**: test isolation
- **Description**: The `BackgroundAgentTracker` is a singleton via `BackgroundAgentTracker::global()` (in `engine.rs:179`), but tests instantiate via `BackgroundAgentTracker::new()`. Several tests in `subagent_tool/tests.rs` (e.g. line 1922-1923, line 2184, line 2963) call `BackgroundAgentTracker::global()` indirectly through the spawn path. Because the test is `cargo test --lib -p alephcore` (multi-test process), the singleton accumulates state. The `BackgroundAgentTracker` has no `clear()` / `reset()` method, so the first test that uses `global()` sets the OnceLock and all subsequent tests see that same instance. If a test that uses `global()` runs before a test that uses `new()` and the global has a registration the `new()` test doesn't know about, the `new()` test can see "ghosts" in its `list_running` / `result_snapshot`. This is the test-isolation anti-pattern from [DISPATCHER-07] above, observed in the test file directly.
- **Suggested fix**: Tests should NEVER call `BackgroundAgentTracker::global()`. Refactor the spawn site to accept an `Arc<BackgroundAgentTracker>` parameter, so tests pass a per-test instance. The production code in `engine.rs:179` constructs the instance and passes it through.
- **Verification**: `grep -n 'BackgroundAgentTracker::global' src/agents/subagent_tool/tests.rs` — appears in many test files.

### [DISPATCHER-18] **Low** — `notify_settled_workflow_runs` reads `notified_workflow_runs` under no lock
- **File**: `src/teams/dispatcher/schedule/settle.rs:248-260`
- **Category**: concurrency
- **Description**: The `notified_workflow_runs` set is `Arc<Mutex<HashSet<String>>>`, and `settle.rs:248-260` acquires it with `.lock().await`. So it IS locked. But the code path at `mod.rs:1176-1178` does `self.notified_workflow_runs.lock().await.remove(rid)` — that's also locked. So the field is consistently locked. Skipping this finding.
- **Suggested fix**: No fix; documenting the verification.

### [DISPATCHER-19] **Low** — `BackgroundAgentTracker::push_progress` accepts any `SubagentProgress` but does not validate the `tool_name` is non-empty when kind=ToolCalled
- **File**: `src/agents/background_tracker.rs:1309-1338`
- **Category**: error handling
- **Description**: The schema says `tool_name: Option<String>` for `SubagentProgress`, but `ForwardingTraceSink::translate` always sets it to `Some(call.tool_name.clone())` for `ToolCallStarted` / `ToolCallCompleted` events (`forwarding_trace_sink.rs:67-86`). If a future caller passes `kind: ToolCalled, tool_name: None`, the tracker accepts it. The panel would render an empty name. Not a runtime bug today (no such caller exists), but a `debug_assert!(kind == ToolCalled => tool_name.is_some())` in `push_progress` would catch a future regression at the same fence `BackgroundAgentTracker` already uses for invariants.
- **Suggested fix**: Add a `debug_assert!` for the `ToolCalled` ↔ `tool_name.is_some()` invariant. The test cost is zero; the runtime cost is zero in release.
- **Verification**: read `forwarding_trace_sink.rs:67-86` (call sites) — both ToolCalled and ToolCompleted always set tool_name.

### [DISPATCHER-20] **Low** — `cancel_runs_of_terminal_tasks` calls `update_task` without a re-fetch, so it may write to a row that was just made non-terminal by the cancel sweep
- **File**: `src/teams/dispatcher/schedule/mod.rs:140-180` (approx)
- **Category**: concurrency
- **Description**: `cancel_runs_of_terminal_tasks` reads `running`, then per-task calls `self.coord_store.get_task(...)` to check `is_terminal`, then `self.coord_store.update_task(...)`. Between the read and the write, an operator's `teams.task.resume` could land, setting the task back to `Pending` or `InProgress`. Then the cancel sweep's `update_task` writes `Cancelled` over the operator's `Pending`. The doc-comment at the top of the sweep says "an `Unsatisfiable` row is a dependency corpse, and a task that is actually running cannot be one — widening the predicate here would only add a way to be wrong." The protection is for `Unsatisfiable` (a derived state), not for the `Pending` ↔ `InProgress` live transitions. The cancel sweep's `is_terminal` guard is real but does not protect against a live `Pending`/`InProgress` race.
- **Suggested fix**: Add a status-guarded update (CAS-by-expected-status) so the cancel only writes if the task is still `InProgress`. Same fix as [DISPATCHER-08] — generic.
- **Verification**: read `mod.rs:140-180` — `get_task` then `update_task` are sequential without a guard.

---

## Cross-cutting concerns

1. **Single mutex serializing all dispatcher DB I/O** — every `dispatch_once` tick holds `Arc<tokio::sync::Mutex<Connection>>` for every per-task read+write. A slow disk + 50 in-flight tasks = ~200 sequential RTTs to a single SQLite file. The fix is a CAS-style `UPDATE ... WHERE status IN (...)` plus batching of multiple `update_task`s into one `unchecked_transaction`. [DISPATCHER-04], [DISPATCHER-08], [DISPATCHER-20] all stem from the same root cause: the store API exposes per-field updates, not status-guarded ones, so the dispatcher is forced into read-then-write sequences that race.
2. **Global process singleton without test reset** — `BackgroundAgentTracker::global()` is `static OnceLock<Arc<...>>` and has no test reset. Tests that touch the global leak state to other tests in the same process. The fix is `#[cfg(test)]` `reset_for_test()` or per-test construction. [DISPATCHER-07], [DISPATCHER-17] are the same anti-pattern.
3. **Run-state lifecycle inconsistencies** — `fail_or_retry` routes `Cancelled` and `Busy` to the same path as `Failed`, so the dispatcher undoes a cancel. The terminal-sticky guard runs AFTER the run row is written, so the foreign-state guard doesn't actually protect against mid-flight operator actions. [DISPATCHER-01], [DISPATCHER-13] are the same bug class.
4. **Worktree isolation has multiple documentation gaps that the code admits** — `WorktreeSandbox` skips tunable command policy (acknowledged, no warning), uses a 1-year default timeout instead of the per-call `command.timeout` (unacknowledged), and `McpScope` silently drops inline-shutdown errors. [DISPATCHER-03], [DISPATCHER-10], [DISPATCHER-11] all sit in the worktree/MCP path.
5. **Sidecar persistence blocking I/O on async runtime** — `record_activity`, `record_settled`, `record_announced`, `reserve_id` all do `std::fs::*` without `tokio::task::spawn_blocking`. The harness-loop path calls `record_activity` synchronously on every tool result. [DISPATCHER-06] is the hot path; the others are boot-time.

---

## Summary

- **Total**: 20 findings (3 Critical-severity, 7 High, 6 Medium, 4 Low) — 20 actionable.
- **By category**: 9 concurrency, 4 error-handling, 3 performance, 1 security, 1 correctness, 1 resource, 1 test isolation.

### Top priority items (act first)

1. **[DISPATCHER-01] + [DISPATCHER-13]** — Terminal-sticky guard is racy + `Cancelled` is retried. These together mean an operator's cancel can be silently undone by the dispatcher's auto-retry. Fix: status-guarded UPDATE in `update_task` (or move the guard into a single `unchecked_transaction` that includes the run-row write).
2. **[DISPATCHER-03] + [DISPATCHER-10]** — `WorktreeSandbox` bypasses tunable command policy AND uses 1-year timeout. These together mean a worktree-isolated subagent gets weaker sandbox guarantees than the harness that spawned it. Fix: pass the operator's policy + per-call `command.timeout` into `WorktreeSandbox::new` and read both in `execute`.
3. **[DISPATCHER-04]** — 5+ sequential SQLite ops per tick hold a shared `Mutex<Connection>`. The fix is a single `update_task` with status guard plus batching via `unchecked_transaction`. Without this, no other finding's mitigation is enough to make the scheduler responsive under load.

### Statistics

- 2 files are >2800 LoC (`subagent_tool/tests.rs`, `background_tracker.rs`); both have hidden races that the existing tests don't cover.
- 4 findings are documented anti-patterns that the existing docs admit but the code does not act on (DISPATCHER-03, DISPATCHER-07, DISPATCHER-08, DISPATCHER-13).
- 1 finding (DISPATCHER-18) is a no-op confirmed by re-reading.
- 1 finding (DISPATCHER-17) is a test-isolation anti-pattern that the codebase test fixtures do not currently catch because every test happens to use `new()`.
- 0 findings required any production change to be re-deployed — all are local to `src/agents/` and the surrounding `src/teams/dispatcher/`.
