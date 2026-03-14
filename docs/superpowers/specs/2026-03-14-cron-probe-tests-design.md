# Cron Module Probe Tests: Production-Grade Validation

**Date**: 2026-03-14
**Status**: Approved
**Approach**: Layered probe architecture — CronTestHarness + MockExecutor + scenario files

---

## Context

The cron module redesign produced 150 unit tests, all co-located with source code. These tests validate individual functions and modules in isolation. What's missing: **cross-module integration probes** that verify the complete system works end-to-end, and **fault injection probes** that verify resilience under abnormal conditions.

### Current Coverage Gaps (10 identified)

1. Full CronService lifecycle (new → add → tick → execute → writeback) — never tested end-to-end
2. Three-phase concurrency model — phases tested individually, never as a pipeline
3. Concurrent operations (list while executing, update during run) — untested
4. Crash recovery with real file I/O — stale marker logic tested but not with actual restart
5. Timer loop (`on_timer_tick`) — only worker_pool tested, not the full tick
6. Gateway handler integration — no handler-to-service tests
7. Delivery + alert end-to-end path — tested in isolation, never wired together
8. Job chaining end-to-end — cycle detection tested, but not trigger-after-execution
9. Schedule recomputation with execution duration effects — untested
10. History (SQLite) integration with execution — insert tested, but not wired to execution

---

## Key Decisions

| Dimension | Decision | Rationale |
|-----------|----------|-----------|
| Scope | End-to-end integration + fault injection | Full coverage of gaps |
| Location | `core/tests/cron_probe.rs` + `core/tests/cron_probe/` | Matches existing `real_api_probe.rs` pattern |
| Execution | Mock executor (default) + `#[ignore]` real agent | Fast CI + optional real validation |
| Fault depth | Process-level (fork + kill) for crash tests | True crash recovery validation |

---

## 1. Architecture

```
core/tests/
├── cron_probe.rs              // Main entry, mod declarations
├── cron_probe/
│   ├── harness.rs             // CronTestHarness — core test infrastructure
│   ├── mock_executor.rs       // MockExecutor — configurable execution mock
│   ├── lifecycle.rs           // P1: Full lifecycle probes
│   ├── concurrency.rs         // P2: Concurrent safety probes
│   ├── scheduling.rs          // P3: Scheduling precision probes
│   ├── failure_recovery.rs    // P4: Fault recovery probes (logical)
│   ├── delivery_alert.rs      // P5: Delivery & alert probes
│   ├── chain.rs               // P6: Job chaining probes
│   ├── crash.rs               // P7: Process-level crash recovery probes
│   ├── gateway.rs             // P9: Gateway handler integration probes
│   ├── history_integration.rs // P10: History (SQLite) integration probes
│   └── real_agent.rs          // P8: Real agent execution probes (#[ignore])
```

### Layer Relationships

```
Layer 3 (scenarios):  lifecycle.rs, concurrency.rs, crash.rs, ...
                          │
Layer 2 (harness):    CronTestHarness
                          │   Provides: add_job, advance, tick, assert_*
                          │   Wraps: CronService + FakeClock + MockExecutor + TempDir
                          │
Layer 1 (mock):       MockExecutor
                          │   Provides: configurable results, delays, fault injection
                          │   Records: execution call log
```

Scenarios write only "what to do" and "what to expect" — no setup boilerplate.

---

## 2. Test Infrastructure

### 2.1 MockExecutor

```rust
pub struct MockExecutor {
    results: Arc<Mutex<HashMap<String, MockBehavior>>>,
    call_log: Arc<Mutex<Vec<ExecutionRecord>>>,
    default_delay_ms: u64,
}

pub enum MockBehavior {
    Ok(String),
    Error { message: String, transient: bool },
    Delayed { delay_ms: u64, output: String },
    Panic,
    Hang,
    Custom(Arc<dyn Fn(&JobSnapshot) -> ExecutionResult + Send + Sync>),
}

pub struct ExecutionRecord {
    pub job_id: String,
    pub trigger_source: TriggerSource,
    pub executed_at_ms: i64,
    pub prompt: String,
}
```

API:
- `MockExecutor::new()` — default all jobs return Ok
- `.on_job(id, behavior)` — configure per-job behavior
- `.call_count(id) -> usize`
- `.calls() -> Vec<ExecutionRecord>`
- `.into_executor_fn() -> JobExecutorFn`
- `.reset()`

### 2.2 CronTestHarness

**Prerequisite**: `CronService` is currently hardcoded to `SystemClock`. Before the harness can work, a test-only constructor must be added:

```rust
// In core/src/cron/mod.rs — gated behind test-helpers
#[cfg(any(test, feature = "test-helpers"))]
pub fn new_with_clock<C: Clock>(config: CronConfig, clock: Arc<C>) -> Result<Self, String> { ... }
```

```rust
/// The harness wraps ServiceState<FakeClock> directly (not CronService)
/// to enable FakeClock injection. CronService methods are tested through
/// a thin adapter that delegates to the same service::ops functions.
pub struct CronTestHarness {
    state: Arc<ServiceState<FakeClock>>,
    clock: Arc<FakeClock>,
    executor: MockExecutor,
    store_path: PathBuf,
    history_db_path: PathBuf,
    _temp_dir: TempDir,
}
```

API (non-chainable async, avoids borrow checker issues with `&Self` + async):

```rust
// Setup
async fn new() -> Self;
async fn with_config(config: CronConfig) -> Self;

// Job operations (async — locks store internally)
async fn add_every_job(&self, id: &str, interval_ms: i64);
async fn add_cron_job(&self, id: &str, expr: &str);
async fn add_at_job(&self, id: &str, at_ms: i64);
async fn add_job(&self, job: CronJob);
fn configure_executor(&self, id: &str, behavior: MockBehavior);

// Time control (sync — FakeClock is atomic)
fn advance(&self, ms: i64);
fn advance_to(&self, ms: i64);
fn now(&self) -> i64;

// Execution (async)
async fn tick(&self);
async fn tick_n(&self, n: usize);
async fn run_catchup(&self);
async fn manual_run(&self, id: &str);

// Assertions (async — locks store to read)
async fn assert_executed(&self, id: &str);
async fn assert_not_executed(&self, id: &str);
async fn assert_execution_count(&self, id: &str, count: usize);
async fn assert_job_enabled(&self, id: &str, enabled: bool);
async fn assert_consecutive_errors(&self, id: &str, count: u32);
async fn assert_next_run_after(&self, id: &str, after_ms: i64);
async fn assert_running(&self, id: &str, running: bool);
async fn assert_delivery_status(&self, id: &str, status: DeliveryStatus);

// Inspection
fn job_state(&self, id: &str) -> JobStateV2;
fn store_file_content(&self) -> String;
fn executor(&self) -> &MockExecutor;
```

Usage example:
```rust
#[tokio::test]
async fn full_lifecycle() {
    let h = CronTestHarness::new();
    h.add_every_job("report", 60_000);
    h.advance(60_000).tick().await;
    h.assert_executed("report");
    h.assert_execution_count("report", 1);
    h.assert_consecutive_errors("report", 0);
}
```

---

## 3. Probe Scenarios

### P1: Full Lifecycle (`lifecycle.rs`) — 8 scenarios

| Scenario | Description | Verifies |
|----------|-------------|----------|
| `full_lifecycle_every` | Add Every job → advance to due → tick → verify execution → advance second period → tick → verify second execution | Execution count=2, next_run progresses, state persisted to disk |
| `full_lifecycle_cron` | Add Cron job → advance to cron match time → tick → verify | Cron expression parsing, timezone |
| `full_lifecycle_at` | Add At job → advance to due → tick → verify executed, no further scheduling | One-shot execution, delete_after_run |
| `manual_trigger` | Add job (not due) → manual_run → verify immediate execution | TriggerSource::Manual, normal schedule unaffected |
| `disable_prevents_execution` | Add job → disable → advance to due → tick → verify not executed | Disabled jobs skipped |
| `update_reschedules` | Add job → update schedule → verify next_run recalculated | Full recompute triggered |
| `delete_stops_execution` | Add job → tick once → delete → advance → tick → verify not executed again | Deletion is permanent |
| `job_persists_across_reload` | Add job → reload store from disk → verify job intact | JSON persistence roundtrip |

### P2: Concurrent Safety (`concurrency.rs`) — 6 scenarios

| Scenario | Description | Verifies |
|----------|-------------|----------|
| `concurrent_list_during_execution` | Job executing (Phase 2) → concurrent list → verify returns with running_at visible | Reads don't block, running state observable |
| `update_during_execution` | Job executing → concurrent update name → Phase 3 writeback → verify new name + execution results preserved | MVCC merge correct |
| `delete_during_execution` | Job executing → concurrent delete → Phase 3 → verify result discarded, no panic | Graceful deletion handling |
| `multiple_jobs_concurrent` | 5 jobs due simultaneously → tick → verify all executed, worker pool correct | No loss, no duplication |
| `reentrant_tick_skipped` | Tick running → second tick triggered → verify second skipped | AtomicBool re-entrancy guard |
| `concurrent_add_during_tick` | Tick executing → concurrent add_job → verify new job runs in next tick | New job doesn't interfere with current tick |

### P3: Scheduling Precision (`scheduling.rs`) — 6 scenarios

| Scenario | Description | Verifies |
|----------|-------------|----------|
| `anchor_alignment_no_drift` | Anchor=0, interval=30min, execution takes 7min → 10 cycles → all aligned to 30min grid | Zero drift |
| `stagger_spreads_jobs` | 10 jobs same cron expression → verify next_run distributed within stagger window | No thundering herd |
| `min_refire_gap_applied` | Cron returns current-second time → verify actual next_run ≥ ended + 2s | Anti-spin |
| `backoff_after_errors` | 3 consecutive failures → verify next_run delays: 30s, 1m, 5m | Backoff gradient correct |
| `maintenance_recompute_safe` | Job past-due but not executed → tick → verify executed (not skipped by recompute) | Anti-#13992 regression |
| `at_job_fires_once` | At job due → execute → advance → tick → verify not executed again | One-shot strict |

### P4: Fault Recovery (`failure_recovery.rs`) — 7 scenarios

| Scenario | Description | Verifies |
|----------|-------------|----------|
| `transient_error_retries` | Executor returns transient error → verify retry scheduled with backoff, consecutive_errors increments | Transient retry correct |
| `permanent_error_disables` | Executor returns permanent error → verify job immediately disabled | Permanent = no retry |
| `max_retries_then_disable` | Consecutive transient errors exceed max_retries → verify auto-disabled | Retry exhaustion |
| `success_resets_errors` | 3 failures then 1 success → verify consecutive_errors = 0 | Error counter reset |
| `stale_marker_cleared` | Set running_at = 3h ago → catchup → verify running_at cleared | Crash marker recovery |
| `catchup_staggers_missed` | 10 overdue jobs → catchup(max=3) → verify 3 immediate, 7 staggered | Restart no-storm |
| `catchup_then_normal_tick` | Catchup completes → normal tick continues → verify no conflicts | Recovery → normal transition |

### P5: Delivery & Alert (`delivery_alert.rs`) — 6 scenarios

| Scenario | Description | Verifies |
|----------|-------------|----------|
| `delivery_after_execution` | Job with webhook delivery → execute success → verify delivery_status = Delivered | Delivery path works |
| `delivery_dedup_agent_sent` | Executor sets agent_used_messaging_tool=true → verify delivery skipped | Dedup effective |
| `delivery_none_mode` | Delivery mode = None → execute → verify delivery_status = NotRequested | Silent mode |
| `alert_fires_after_threshold` | alert.after=2, 2 consecutive failures → verify alert message generated | Threshold trigger |
| `alert_cooldown_blocks` | Alert sent → 30min later another failure → verify no re-alert | Anti-storm |
| `alert_cooldown_expires` | Alert sent → exceed cooldown → failure → verify new alert | Cooldown recovery |

### P6: Job Chaining (`chain.rs`) — 4 scenarios

| Scenario | Description | Verifies |
|----------|-------------|----------|
| `chain_on_success` | A(on_success=B) → A succeeds → verify B's next_run set to now | Success chain trigger |
| `chain_on_failure` | A(on_failure=C) → A fails → verify C triggered | Failure chain trigger |
| `chain_disabled_target_skipped` | A(on_success=B), B disabled → A succeeds → verify B not triggered | Disabled chain breaks |
| `chain_cycle_rejected` | Create A→B→A cycle → verify rejected | Cycle detection |

### P7: Process-Level Crash (`crash.rs`) — 3 scenarios

| Scenario | Description | Verifies |
|----------|-------------|----------|
| `crash_mid_execution_recovers` | Manually call `phase1_mark_due_jobs` (persists running markers) → drop everything (simulate crash) → new ServiceState from same store path → `run_startup_catchup` → verify markers cleared, jobs rescheduled | True crash recovery — deterministic, no spawn/abort non-determinism |
| `crash_after_phase1_before_phase2` | Call `phase1_mark_due_jobs` → do NOT call executor or phase3 → drop ServiceState → reload store → verify running_at_ms persisted to disk → new ServiceState + catchup → verify markers cleared | Phase 1 crash safety |
| `crash_preserves_store_integrity` | Write store with multiple jobs → call `atomic_write` directly → verify file is either old or new (never partial). Also: `#[ignore]`-gated fork+kill test for true SIGKILL validation | Atomic write verification |

Implementation approach:
- **Primary (CI)**: Manual phase-call + drop pattern — deterministic, no spawn/abort non-determinism. Call `phase1_mark_due_jobs` to persist running markers, then simply drop the ServiceState. Create a new ServiceState from the same store path and run catchup. This is the correct and reliable way to test crash boundaries.
- **`#[ignore]`-gated**: `std::process::Command` fork + `child.kill()` for true SIGKILL tests. The child process loads a store, marks a job running, persists, then hangs. Parent kills it and verifies store integrity on reload.
- The primary approach tests the same logical invariants without process management complexity. The fork+kill approach validates the OS-level atomicity of `rename(2)`.

### P9: Gateway Handler Integration (`gateway.rs`) — 3 scenarios

| Scenario | Description | Verifies |
|----------|-------------|----------|
| `handler_create_and_list` | Call `handle_create` with mock JSON-RPC request containing tagged ScheduleKind → call `handle_list` → verify job appears in response JSON with correct fields | Handler-to-service path, ScheduleKind serialization |
| `handler_manual_run` | Create job via handler → call `handle_run` → verify execution triggered (mock executor called) | Manual trigger via RPC |
| `handler_error_responses` | Call `handle_get` with invalid job ID → verify error JSON-RPC response | Error handling in handlers |

### P10: History Integration (`history_integration.rs`) — 3 scenarios

| Scenario | Description | Verifies |
|----------|-------------|----------|
| `execution_produces_history_record` | Add job → tick → verify `get_cron_runs(job_id)` returns 1 record with correct status, duration, trigger_source | End-to-end history recording |
| `multiple_runs_accumulate` | Execute job 3 times → verify history has 3 records ordered by time | History accumulation |
| `history_cleanup_respects_retention` | Insert old records → `cleanup_old_cron_runs(retention=1)` → verify old records deleted, recent kept | Retention policy |

### P8: Real Agent Execution (`real_agent.rs`) — 2 scenarios

| Scenario | Description | Verifies |
|----------|-------------|----------|
| `real_agent_execution` | `#[ignore]` — Real agent_loop with simple prompt → verify non-empty output | End-to-end real path |
| `real_agent_timeout` | `#[ignore]` — Very short timeout → verify timeout detected | Timeout mechanism |

Require `ALEPH_TEST_AGENT=true` environment variable.

---

## 4. Execution Strategy

### Run Commands

```bash
# All probes (CI — excludes #[ignore])
cargo test -p alephcore --test cron_probe

# Include crash probes
cargo test -p alephcore --test cron_probe -- --include-ignored

# Real agent probes only
ALEPH_TEST_AGENT=true cargo test -p alephcore --test cron_probe real_agent -- --ignored
```

### Test Tiers

| Tier | Scenarios | Time per test | CI Policy |
|------|-----------|---------------|-----------|
| L1 Fast | P1, P3, P5, P6, P9, P10 | < 1s | Always run |
| L2 Medium | P2, P4 | < 5s | Always run |
| L3 Heavy | P7 crash | < 10s | Primary (phase-call+drop) always; fork+kill `#[ignore]` |
| L4 Real | P8 real_agent | Variable | `#[ignore]`, manual trigger |

### Test Isolation

Each test function uses independent `TempDir` — no shared filesystem state, safe for parallel execution, auto-cleanup on drop.

---

## 5. Total Scope

| Category | Scenario Count |
|----------|---------------|
| P1 Lifecycle | 8 |
| P2 Concurrency | 6 |
| P3 Scheduling | 6 |
| P4 Fault Recovery | 7 |
| P5 Delivery & Alert | 6 |
| P6 Chain | 4 |
| P7 Crash | 3 |
| P8 Real Agent | 2 |
| P9 Gateway Handlers | 3 |
| P10 History Integration | 3 |
| **Total** | **48** |
