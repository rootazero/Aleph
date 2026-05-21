# Cron Executor Wiring Design

## Problem

Cron system infrastructure is 90% complete (storage, scheduling, 3-phase concurrency, catchup, chaining, RPC handlers, builtin tool), but jobs never actually execute because:

1. **Timer loop never spawned** — `run_timer_loop()` is never called at server startup
2. **No production `JobExecutorFn`** — no bridge from `JobSnapshot` to `ExecutionEngine`

A user's "9am wake-up reminder" cron job is stored correctly with proper `next_run_at_ms`, but nothing checks or executes it.

## Design Decisions

### Agent Selection

- **Default**: use the agent bound to the channel where the cron was created (agent:channel = 1:1)
- **Override**: user can specify a different `agent_id` at creation time
- When a different agent is specified, it's NOT a sub-agent relationship — the ExecutionEngine simply runs the prompt under that agent's identity
- Results always push back to the `source_channel_id` (the channel where cron was created)

### Result Delivery

- **Channel push + session record** — both
- Push mechanism: agent's `message` builtin tool, triggered via prompt injection (LLM Sovereignty R8)
- Session record: `SessionKey::Task` provides persistent session history

### Session Strategy

- Follows existing `SessionTarget` config per job:
  - `Main` — shared persistent session per cron job (`SessionKey::Task { task_id: job_id }`)
  - `Isolated` — fresh session per execution (`SessionKey::Task { task_id: "{job_id}-{unix_ms}" }`) — task_id must not contain `:` characters (would break SessionKey parsing)

### Failure Handling

- Silent retry with existing `consecutive_errors` tracking
- Notify user via channel only when threshold exceeded (existing alerting config)

## Changes Required

### 1. Data Model — `CronJob` and `JobSnapshot` field additions

**File:** `src/cron/config.rs`

Add `source_channel_id: Option<String>` to both `CronJob` AND `JobSnapshot`. Records which channel the cron was created from, used to:
- Derive default `agent_id` (via channel→agent 1:1 binding)
- Determine where to push execution results

`JobSnapshot` must carry `source_channel_id` so the executor can inject it into the prompt at runtime. Update `phase1_mark_due_jobs()` in `concurrency.rs` to copy this field when constructing snapshots.

Also clean up: `cron/mod.rs` has an older `JobExecutor` type alias (takes `(String, String, String)`) that predates `JobExecutorFn` in `timer.rs`. Remove the old `JobExecutor` to avoid confusion.

No schema migration needed — SQLite `data` column stores JSON, new field auto-serializes with `Option` defaulting to `None`.

### 2. Production Executor — `CronExecutor`

**New file:** `src/cron/executor.rs`

Implements the `JobExecutorFn` callback that bridges cron to agent execution:

```
JobSnapshot (from Phase 1)
    ↓
Build SessionKey::Task { agent_id, task_type: "cron", task_id }
    ↓
Inject cron context into prompt:
  "[Cron Task: {job_name}]
   Execute and send results via message tool to {source_channel_id}.
   Task: {original_prompt}"
    ↓
Construct RunRequest { input, session_key, timeout }
    ↓
ExecutionEngine.execute(run_request)
    ↓
Map RunState to ExecutionResult and return to Phase 3
```

**Dependencies captured via closure at startup:**
- `ExecutionEngine` (or Arc'd trait object) — run the prompt
- `source_channel_id` from `JobSnapshot` — for prompt injection

**No dependency on SubAgentDispatcher** — cron execution is a direct `ExecutionEngine.execute()` call, same as processing an inbound channel message.

### 3. Server Startup — spawn timer loop

**File:** `src/bin/aleph/commands/start/mod.rs`

After existing `CronService::new()` and `register_cron_handlers()`:

```rust
// Construct executor closure capturing ExecutionEngine
let executor_fn = build_cron_executor_fn(execution_engine.clone(), ...);

// Access cron_state from SharedCronService (Arc<Mutex<CronService>>)
// Lock briefly to clone the Arc<ServiceState>, then drop the guard immediately
let cron_state = {
    let guard = cron_service.lock().await;
    guard.state().clone()  // Clone Arc<ServiceState<SystemClock>>
};  // guard dropped here — no lock held during timer loop

// Spawn timer loop as background task
let cron_config = cron_config.clone();
tokio::spawn(async move {
    // Catchup needs (store, clock, max_missed, stagger_ms) — not ServiceState directly
    run_startup_catchup(
        &cron_state.store,
        cron_state.clock.as_ref(),
        cron_config.max_missed_jobs_per_restart,
        cron_config.catchup_stagger_ms,
    ).await;
    run_timer_loop(cron_state, executor_fn).await;
});
```

**Lifecycle:** Timer loop exits via existing `shutdown: AtomicBool` mechanism on server shutdown.

### 4. `cron_manage` tool — auto-capture source_channel_id

**File:** `src/builtin_tools/cron_manage.rs`

On `create` action:
- Auto-populate `source_channel_id` on the new `CronJob`
- If `agent_id` not specified, derive from channel→agent binding

**Implementation note:** `AlephTool::call()` only receives `Args`, not `ToolContext`. To access the calling session's channel info, either:
- (a) Pass `source_channel_id` as an injected field on `CronManageTool` struct at construction time (from the agent's bound channel), or
- (b) Add `source_channel_id` as an optional hidden field in `CronManageArgs` that the system pre-fills before calling the tool

Option (a) is simpler — `CronManageTool` already holds `Arc<CronService>`, adding the agent's channel context at construction is consistent with existing patterns.

No user-facing API change — `source_channel_id` is automatically captured, not user-supplied.

### 5. Snapshot construction — carry `source_channel_id`

**File:** `src/cron/service/concurrency.rs`

In `phase1_mark_due_jobs()`, copy `source_channel_id` from `CronJob` to `JobSnapshot` when constructing snapshots. Minimal change — one field addition to the snapshot builder.

### 6. Cleanup — remove old `JobExecutor` type

**File:** `src/cron/mod.rs`

Remove the old `JobExecutor` type alias that takes `(String, String, String)`. Only `JobExecutorFn` (in `timer.rs`, takes `JobSnapshot`) is used.

## Files NOT Changed

These are all complete and tested:
- `src/cron/schedule.rs` — schedule computation
- `src/cron/store.rs` — SQLite persistence
- `src/cron/service/catchup.rs` — startup recovery
- `src/cron/chain.rs` — job chaining
- `src/gateway/handlers/cron.rs` — RPC handlers
- `tests/cron_probe/mock_executor.rs` — test mocks

## Risk Notes

- `ExecutionEngine` has generic params `<P, R>` — executor closure must capture concrete type or use trait object. Startup code already has concrete types available, so closure capture should work.
- Agent must have `message` tool permission to push results. Default tool set should include it, but verify during implementation.
- Timer loop `check_interval_secs` defaults to 60s — worst case 60s delay from scheduled time. Acceptable for current use cases.

## Concept Clarification (Follow-up)

During design, identified a naming issue: `SubAgentDispatcher` is used for both ephemeral sub-agents AND registered agent-to-agent delegation. These are distinct concepts:
- **Sub-agent**: ephemeral worker created by parent agent, auto-destroyed on completion
- **Agent delegation**: peer-to-peer request between registered agents

This naming cleanup is tracked separately from the cron work.
