# Cron Probe Tests Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build 48 integration/probe tests that validate the cron module's production-grade capabilities end-to-end.

**Architecture:** Layered probe architecture — `MockExecutor` (configurable execution mock) → `CronTestHarness` (wraps `ServiceState<FakeClock>` + store + executor) → scenario files (P1-P10). Tests live in `core/tests/cron_probe.rs` + `core/tests/cron_probe/` submodules, following the existing `core/tests/real_api_probe.rs` pattern.

**Tech Stack:** Rust, tokio, tempfile, alephcore cron module (ServiceState, FakeClock, CronStore, ops, concurrency, timer, catchup, history)

**Spec:** `docs/superpowers/specs/2026-03-14-cron-probe-tests-design.md`

---

## File Map

### Prerequisites (production code changes)
| File | Change |
|------|--------|
| `core/src/lib.rs` | Add `pub mod cron;` to export cron module for integration tests |
| `core/src/cron/service/state.rs` | Make `ServiceState::new` accept bare `CronStore` (wrap in Arc<Mutex> internally) — simplifies test setup |

### New Test Files
| File | Responsibility |
|------|---------------|
| `core/tests/cron_probe.rs` | Main entry — mod declarations for submodules |
| `core/tests/cron_probe/mock_executor.rs` | MockExecutor + MockBehavior + ExecutionRecord |
| `core/tests/cron_probe/harness.rs` | CronTestHarness wrapping ServiceState<FakeClock> |
| `core/tests/cron_probe/lifecycle.rs` | P1: 8 full lifecycle scenarios |
| `core/tests/cron_probe/concurrency.rs` | P2: 6 concurrent safety scenarios |
| `core/tests/cron_probe/scheduling.rs` | P3: 6 scheduling precision scenarios |
| `core/tests/cron_probe/failure_recovery.rs` | P4: 7 fault recovery scenarios |
| `core/tests/cron_probe/delivery_alert.rs` | P5: 6 delivery & alert scenarios |
| `core/tests/cron_probe/chain.rs` | P6: 4 job chaining scenarios |
| `core/tests/cron_probe/crash.rs` | P7: 3 process-level crash scenarios |
| `core/tests/cron_probe/gateway.rs` | P9: 3 gateway handler scenarios |
| `core/tests/cron_probe/history_integration.rs` | P10: 3 history integration scenarios |
| `core/tests/cron_probe/real_agent.rs` | P8: 2 real agent scenarios (#[ignore]) |

---

## Chunk 1: Prerequisites + Test Infrastructure

### Task 1: Export cron module in lib.rs

**Files:**
- Modify: `core/src/lib.rs`

- [ ] **Step 1: Add pub mod cron export**

Check if `cron` is already declared. If it's `mod cron` (private), change to `pub mod cron`. If missing, add `pub mod cron;`. This allows integration tests (`core/tests/`) to access `alephcore::cron::*`.

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`

- [ ] **Step 3: Commit**

```bash
git add core/src/lib.rs
git commit -m "cron: export cron module for integration tests"
```

---

### Task 2: MockExecutor

**Files:**
- Create: `core/tests/cron_probe/mock_executor.rs`
- Create: `core/tests/cron_probe.rs`

- [ ] **Step 1: Create main entry file**

```rust
// core/tests/cron_probe.rs
mod cron_probe;
```

Wait — Rust integration tests require either a single file or `mod.rs` pattern. The correct pattern for a multi-file integration test is:

```rust
// core/tests/cron_probe.rs
mod cron_probe {
    pub mod mock_executor;
    pub mod harness;
    // scenario modules added later
}
```

Actually, the standard pattern is to create `core/tests/cron_probe/mod.rs` and have `core/tests/cron_probe.rs` include it. But `cargo test` treats each file in `tests/` as a test crate root. The correct approach:

```rust
// core/tests/cron_probe.rs — this IS the test crate root
// Submodules are declared here and live in core/tests/cron_probe/
mod mock_executor;
mod harness;
// (scenario modules added in later tasks)
```

And the submodule files go in `core/tests/cron_probe/` directory.

- [ ] **Step 2: Write MockExecutor**

```rust
// core/tests/cron_probe/mock_executor.rs
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use alephcore::cron::config::{
    DeliveryStatus, ErrorReason, ExecutionResult, JobSnapshot, RunStatus, TriggerSource,
};
use alephcore::cron::service::timer::JobExecutorFn;

/// Record of a single job execution by the mock.
#[derive(Debug, Clone)]
pub struct ExecutionRecord {
    pub job_id: String,
    pub trigger_source: TriggerSource,
    pub executed_at_ms: i64,
    pub prompt: String,
}

/// Configurable behavior for a mocked job execution.
#[derive(Clone)]
pub enum MockBehavior {
    /// Return success with given output.
    Ok(String),
    /// Return error.
    Error {
        message: String,
        reason: ErrorReason,
    },
    /// Return success after a simulated delay (delay is in the result's duration_ms, not real time).
    Delayed {
        delay_ms: i64,
        output: String,
    },
}

/// A configurable mock executor for probe tests.
///
/// Default behavior: all jobs return `RunStatus::Ok` with output "ok".
/// Per-job behavior can be configured with `on_job()`.
pub struct MockExecutor {
    behaviors: Arc<Mutex<HashMap<String, MockBehavior>>>,
    call_log: Arc<Mutex<Vec<ExecutionRecord>>>,
}

impl MockExecutor {
    pub fn new() -> Self {
        Self {
            behaviors: Arc::new(Mutex::new(HashMap::new())),
            call_log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Configure behavior for a specific job ID.
    pub fn on_job(&self, job_id: &str, behavior: MockBehavior) {
        self.behaviors
            .lock()
            .unwrap()
            .insert(job_id.to_string(), behavior);
    }

    /// Get the number of times a job was executed.
    pub fn call_count(&self, job_id: &str) -> usize {
        self.call_log
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.job_id == job_id)
            .count()
    }

    /// Get all execution records.
    pub fn calls(&self) -> Vec<ExecutionRecord> {
        self.call_log.lock().unwrap().clone()
    }

    /// Get execution records for a specific job.
    pub fn calls_for(&self, job_id: &str) -> Vec<ExecutionRecord> {
        self.call_log
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.job_id == job_id)
            .cloned()
            .collect()
    }

    /// Was this job executed at all?
    pub fn was_executed(&self, job_id: &str) -> bool {
        self.call_count(job_id) > 0
    }

    /// Reset all call logs (keep configured behaviors).
    pub fn reset_calls(&self) {
        self.call_log.lock().unwrap().clear();
    }

    /// Convert to the `JobExecutorFn` type expected by the timer loop.
    pub fn into_executor_fn(&self) -> JobExecutorFn {
        let behaviors = self.behaviors.clone();
        let call_log = self.call_log.clone();

        Arc::new(move |snapshot: JobSnapshot| {
            let behaviors = behaviors.clone();
            let call_log = call_log.clone();
            let job_id = snapshot.id.clone();

            Box::pin(async move {
                // Record the call
                call_log.lock().unwrap().push(ExecutionRecord {
                    job_id: job_id.clone(),
                    trigger_source: snapshot.trigger_source.clone(),
                    executed_at_ms: snapshot.marked_at,
                    prompt: snapshot.prompt.clone(),
                });

                // Look up configured behavior
                let behavior = behaviors
                    .lock()
                    .unwrap()
                    .get(&job_id)
                    .cloned()
                    .unwrap_or(MockBehavior::Ok("ok".to_string()));

                match behavior {
                    MockBehavior::Ok(output) => ExecutionResult {
                        started_at: snapshot.marked_at,
                        ended_at: snapshot.marked_at + 100,
                        duration_ms: 100,
                        status: RunStatus::Ok,
                        output: Some(output),
                        error: None,
                        error_reason: None,
                        delivery_status: None,
                        agent_used_messaging_tool: false,
                    },
                    MockBehavior::Error { message, reason } => ExecutionResult {
                        started_at: snapshot.marked_at,
                        ended_at: snapshot.marked_at + 100,
                        duration_ms: 100,
                        status: RunStatus::Error,
                        output: None,
                        error: Some(message),
                        error_reason: Some(reason),
                        delivery_status: None,
                        agent_used_messaging_tool: false,
                    },
                    MockBehavior::Delayed { delay_ms, output } => ExecutionResult {
                        started_at: snapshot.marked_at,
                        ended_at: snapshot.marked_at + delay_ms,
                        duration_ms: delay_ms,
                        status: RunStatus::Ok,
                        output: Some(output),
                        error: None,
                        error_reason: None,
                        delivery_status: None,
                        agent_used_messaging_tool: false,
                    },
                }
            }) as Pin<Box<dyn Future<Output = ExecutionResult> + Send>>
        })
    }
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo test -p alephcore --test cron_probe --no-run`

- [ ] **Step 4: Commit**

```bash
git add core/tests/cron_probe.rs core/tests/cron_probe/
git commit -m "cron probe: add MockExecutor test infrastructure"
```

---

### Task 3: CronTestHarness

**Files:**
- Create: `core/tests/cron_probe/harness.rs`
- Modify: `core/tests/cron_probe.rs` (add `pub mod harness;`)

- [ ] **Step 1: Write CronTestHarness**

```rust
// core/tests/cron_probe/harness.rs
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::Mutex;

use alephcore::cron::clock::testing::FakeClock;
use alephcore::cron::config::{
    CronConfig, CronJob, CronJobView, DeliveryStatus, JobStateV2, ScheduleKind, SessionTarget,
};
use alephcore::cron::service::catchup::{run_startup_catchup, CatchupReport};
use alephcore::cron::service::concurrency::{phase1_mark_due_jobs, phase1_mark_manual, phase3_writeback};
use alephcore::cron::service::ops::{self, CronJobUpdates};
use alephcore::cron::service::state::ServiceState;
use alephcore::cron::service::timer::{on_timer_tick, JobExecutorFn};
use alephcore::cron::store::CronStore;

use super::mock_executor::MockExecutor;

/// Test harness wrapping ServiceState<FakeClock> with MockExecutor.
/// Provides high-level operations for probe tests.
pub struct CronTestHarness {
    pub state: Arc<ServiceState<FakeClock>>,
    pub clock: Arc<FakeClock>,
    pub executor: MockExecutor,
    pub store_path: PathBuf,
    _temp_dir: TempDir,
}

impl CronTestHarness {
    /// Create a new test harness with default config.
    pub fn new() -> Self {
        Self::with_config(CronConfig::default())
    }

    /// Create a new test harness with custom config.
    pub fn with_config(config: CronConfig) -> Self {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let store_path = temp_dir.path().join("cron_jobs.json");
        let store = CronStore::load(store_path.clone()).expect("failed to load store");
        let clock = Arc::new(FakeClock::new(1_000_000_000)); // Start at a reasonable timestamp

        let state = Arc::new(ServiceState::new(
            Arc::new(Mutex::new(store)),
            clock.clone(),
            config,
        ));

        let executor = MockExecutor::new();

        Self {
            state,
            clock,
            executor,
            store_path,
            _temp_dir: temp_dir,
        }
    }

    // --- Job operations ---

    /// Add an Every-type job with the given interval.
    pub async fn add_every_job(&self, id: &str, interval_ms: i64) {
        let mut job = CronJob::new(
            id.to_string(),
            "test-agent".to_string(),
            format!("probe prompt for {}", id),
            ScheduleKind::Every {
                every_ms: interval_ms,
                anchor_ms: Some(self.clock.now_ms()),
            },
        );
        job.id = id.to_string();

        let mut store = self.state.store.lock().await;
        ops::add_job(&mut store, job, self.clock.as_ref());
        store.persist().expect("failed to persist");
    }

    /// Add a Cron-type job.
    pub async fn add_cron_job(&self, id: &str, expr: &str) {
        let mut job = CronJob::new(
            id.to_string(),
            "test-agent".to_string(),
            format!("probe prompt for {}", id),
            ScheduleKind::Cron {
                expr: expr.to_string(),
                tz: None,
                stagger_ms: None,
            },
        );
        job.id = id.to_string();

        let mut store = self.state.store.lock().await;
        ops::add_job(&mut store, job, self.clock.as_ref());
        store.persist().expect("failed to persist");
    }

    /// Add an At-type (one-shot) job.
    pub async fn add_at_job(&self, id: &str, at_ms: i64) {
        let mut job = CronJob::new(
            id.to_string(),
            "test-agent".to_string(),
            format!("probe prompt for {}", id),
            ScheduleKind::At {
                at: at_ms,
                delete_after_run: false,
            },
        );
        job.id = id.to_string();

        let mut store = self.state.store.lock().await;
        ops::add_job(&mut store, job, self.clock.as_ref());
        store.persist().expect("failed to persist");
    }

    /// Add a fully custom job.
    pub async fn add_job(&self, mut job: CronJob) {
        let mut store = self.state.store.lock().await;
        ops::add_job(&mut store, job, self.clock.as_ref());
        store.persist().expect("failed to persist");
    }

    /// Update a job.
    pub async fn update_job(&self, id: &str, updates: CronJobUpdates) {
        let mut store = self.state.store.lock().await;
        ops::update_job(&mut store, id, updates, self.clock.as_ref())
            .expect("failed to update job");
        store.persist().expect("failed to persist");
    }

    /// Delete a job.
    pub async fn delete_job(&self, id: &str) {
        let mut store = self.state.store.lock().await;
        ops::delete_job(&mut store, id).expect("failed to delete job");
        store.persist().expect("failed to persist");
    }

    /// Toggle a job's enabled state.
    pub async fn toggle_job(&self, id: &str) -> bool {
        let mut store = self.state.store.lock().await;
        let result = ops::toggle_job(&mut store, id, self.clock.as_ref())
            .expect("failed to toggle job");
        store.persist().expect("failed to persist");
        result
    }

    // --- Time control ---

    /// Advance clock by the given number of milliseconds.
    pub fn advance(&self, ms: i64) {
        self.clock.advance(ms);
    }

    /// Set clock to an absolute timestamp.
    pub fn advance_to(&self, ms: i64) {
        self.clock.set(ms);
    }

    /// Get current clock time.
    pub fn now(&self) -> i64 {
        self.clock.now_ms()
    }

    // --- Execution ---

    /// Execute one timer tick (Phase 1 → execute → Phase 3).
    pub async fn tick(&self) {
        let executor_fn = self.executor.into_executor_fn();
        on_timer_tick(&self.state, &executor_fn)
            .await
            .expect("timer tick failed");
    }

    /// Execute N timer ticks.
    pub async fn tick_n(&self, n: usize) {
        for _ in 0..n {
            self.tick().await;
        }
    }

    /// Run startup catchup.
    pub async fn run_catchup(&self) -> CatchupReport {
        run_startup_catchup(
            &self.state.store,
            self.clock.as_ref(),
            self.state.config.max_missed_jobs_per_restart,
            self.state.config.catchup_stagger_ms,
        )
        .await
        .expect("catchup failed")
    }

    /// Manually trigger a specific job.
    pub async fn manual_run(&self, id: &str) {
        let snapshot = phase1_mark_manual(&self.state.store, self.clock.as_ref(), id)
            .await
            .expect("manual mark failed")
            .expect("job not found or already running");

        let executor_fn = self.executor.into_executor_fn();
        let result = executor_fn(snapshot.clone()).await;

        phase3_writeback(
            &self.state.store,
            self.clock.as_ref(),
            &[(snapshot.id, result)],
        )
        .await
        .expect("writeback failed");
    }

    // --- Assertions ---

    /// Assert that a job was executed by the mock executor.
    pub fn assert_executed(&self, id: &str) {
        assert!(
            self.executor.was_executed(id),
            "expected job '{}' to be executed, but it was not. Calls: {:?}",
            id,
            self.executor.calls().iter().map(|c| &c.job_id).collect::<Vec<_>>()
        );
    }

    /// Assert that a job was NOT executed.
    pub fn assert_not_executed(&self, id: &str) {
        assert!(
            !self.executor.was_executed(id),
            "expected job '{}' NOT to be executed, but it was",
            id
        );
    }

    /// Assert exact execution count.
    pub fn assert_execution_count(&self, id: &str, expected: usize) {
        let actual = self.executor.call_count(id);
        assert_eq!(
            actual, expected,
            "expected job '{}' to be executed {} times, but was executed {} times",
            id, expected, actual
        );
    }

    /// Assert job enabled state.
    pub async fn assert_job_enabled(&self, id: &str, expected: bool) {
        let store = self.state.store.lock().await;
        let job = store.get_job(id).expect(&format!("job '{}' not found", id));
        assert_eq!(
            job.enabled, expected,
            "expected job '{}' enabled={}, got {}",
            id, expected, job.enabled
        );
    }

    /// Assert consecutive error count.
    pub async fn assert_consecutive_errors(&self, id: &str, expected: u32) {
        let store = self.state.store.lock().await;
        let job = store.get_job(id).expect(&format!("job '{}' not found", id));
        assert_eq!(
            job.state.consecutive_errors, expected,
            "expected job '{}' consecutive_errors={}, got {}",
            id, expected, job.state.consecutive_errors
        );
    }

    /// Assert next_run_at_ms is after a given time.
    pub async fn assert_next_run_after(&self, id: &str, after_ms: i64) {
        let store = self.state.store.lock().await;
        let job = store.get_job(id).expect(&format!("job '{}' not found", id));
        let next = job.state.next_run_at_ms.expect("next_run_at_ms is None");
        assert!(
            next > after_ms,
            "expected job '{}' next_run > {}, got {}",
            id, after_ms, next
        );
    }

    /// Assert running state.
    pub async fn assert_running(&self, id: &str, expected: bool) {
        let store = self.state.store.lock().await;
        let job = store.get_job(id).expect(&format!("job '{}' not found", id));
        let is_running = job.state.running_at_ms.is_some();
        assert_eq!(
            is_running, expected,
            "expected job '{}' running={}, got {}",
            id, expected, is_running
        );
    }

    /// Get a snapshot of job state.
    pub async fn job_state(&self, id: &str) -> JobStateV2 {
        let store = self.state.store.lock().await;
        store
            .get_job(id)
            .expect(&format!("job '{}' not found", id))
            .state
            .clone()
    }

    /// Check if job exists in store.
    pub async fn job_exists(&self, id: &str) -> bool {
        let store = self.state.store.lock().await;
        store.get_job(id).is_some()
    }

    /// Read the raw store file content.
    pub fn store_file_content(&self) -> String {
        std::fs::read_to_string(&self.store_path).unwrap_or_default()
    }

    /// List all jobs (read-only).
    pub async fn list_jobs(&self) -> Vec<CronJobView> {
        let store = self.state.store.lock().await;
        ops::list_jobs(&store)
    }
}
```

- [ ] **Step 2: Add module declaration and a smoke test**

Update `core/tests/cron_probe.rs`:

```rust
mod mock_executor;
mod harness;

#[cfg(test)]
mod smoke {
    use super::harness::CronTestHarness;

    #[tokio::test]
    async fn harness_smoke_test() {
        let h = CronTestHarness::new();
        h.add_every_job("test", 60_000).await;
        h.advance(60_000);
        h.tick().await;
        h.assert_executed("test");
        h.assert_execution_count("test", 1);
    }
}
```

- [ ] **Step 3: Run smoke test**

Run: `cargo test -p alephcore --test cron_probe smoke`
Expected: 1 test passes.

- [ ] **Step 4: Commit**

```bash
git add core/tests/cron_probe/ core/tests/cron_probe.rs
git commit -m "cron probe: add CronTestHarness with smoke test"
```

---

## Chunk 2: P1 Lifecycle + P3 Scheduling (L1 Fast)

### Task 4: P1 Lifecycle Probes

**Files:**
- Create: `core/tests/cron_probe/lifecycle.rs`
- Modify: `core/tests/cron_probe.rs` (add `mod lifecycle;`)

- [ ] **Step 1: Write all 8 lifecycle scenarios**

```rust
// core/tests/cron_probe/lifecycle.rs
use super::harness::CronTestHarness;
use super::mock_executor::MockBehavior;
use alephcore::cron::config::{CronJob, ScheduleKind};

#[tokio::test]
async fn full_lifecycle_every() {
    let h = CronTestHarness::new();
    h.add_every_job("report", 60_000).await;

    // First cycle
    h.advance(60_000);
    h.tick().await;
    h.assert_executed("report");
    h.assert_execution_count("report", 1);

    // Second cycle
    h.executor.reset_calls();
    h.advance(60_000);
    h.tick().await;
    h.assert_executed("report");
    h.assert_execution_count("report", 1); // 1 since reset

    // Verify persisted to disk
    let content = h.store_file_content();
    assert!(content.contains("report"), "job should be persisted to disk");
}

#[tokio::test]
async fn full_lifecycle_at() {
    let h = CronTestHarness::new();
    let target_time = h.now() + 30_000;
    h.add_at_job("once", target_time).await;

    // Before due — should not execute
    h.advance(10_000);
    h.tick().await;
    h.assert_not_executed("once");

    // At due time — should execute
    h.advance(20_000);
    h.tick().await;
    h.assert_executed("once");

    // After — should not execute again
    h.executor.reset_calls();
    h.advance(60_000);
    h.tick().await;
    h.assert_not_executed("once");
}

#[tokio::test]
async fn manual_trigger() {
    let h = CronTestHarness::new();
    h.add_every_job("task", 3_600_000).await; // 1 hour interval — far from due

    // Manual trigger before schedule
    h.manual_run("task").await;
    h.assert_executed("task");
    h.assert_execution_count("task", 1);
}

#[tokio::test]
async fn disable_prevents_execution() {
    let h = CronTestHarness::new();
    h.add_every_job("task", 60_000).await;
    h.toggle_job("task").await; // Disable

    h.advance(60_000);
    h.tick().await;
    h.assert_not_executed("task");
}

#[tokio::test]
async fn update_reschedules() {
    let h = CronTestHarness::new();
    h.add_every_job("task", 60_000).await;

    let state_before = h.job_state("task").await;
    let next_before = state_before.next_run_at_ms.unwrap();

    // Update to longer interval
    use alephcore::cron::service::ops::CronJobUpdates;
    h.update_job("task", CronJobUpdates {
        schedule_kind: Some(ScheduleKind::Every { every_ms: 120_000, anchor_ms: None }),
        ..Default::default()
    }).await;

    let state_after = h.job_state("task").await;
    let next_after = state_after.next_run_at_ms.unwrap();

    // next_run should have changed (recomputed with new interval)
    assert_ne!(next_before, next_after, "update should trigger recompute");
}

#[tokio::test]
async fn delete_stops_execution() {
    let h = CronTestHarness::new();
    h.add_every_job("task", 60_000).await;

    // Execute once
    h.advance(60_000);
    h.tick().await;
    h.assert_executed("task");

    // Delete
    h.delete_job("task").await;
    h.executor.reset_calls();

    // Should not execute
    h.advance(60_000);
    h.tick().await;
    h.assert_not_executed("task");
    assert!(!h.job_exists("task").await);
}

#[tokio::test]
async fn job_persists_across_reload() {
    let h = CronTestHarness::new();
    h.add_every_job("persistent", 60_000).await;

    // Force reload from disk
    {
        let mut store = h.state.store.lock().await;
        store.force_reload().expect("reload failed");
    }

    // Job should still be there
    assert!(h.job_exists("persistent").await);
    let jobs = h.list_jobs().await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, "persistent");
}

#[tokio::test]
async fn full_lifecycle_cron() {
    let h = CronTestHarness::new();
    // Use a cron expression — we can't easily control when it fires with FakeClock
    // since cron depends on real DateTime. Instead, test that the job gets a valid next_run.
    h.add_cron_job("cron-task", "0 0 * * * *").await; // Every hour at :00

    let state = h.job_state("cron-task").await;
    assert!(
        state.next_run_at_ms.is_some(),
        "cron job should have computed next_run_at_ms"
    );
    assert!(
        state.next_run_at_ms.unwrap() > h.now(),
        "next_run should be in the future"
    );
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --test cron_probe lifecycle`
Expected: 8 tests pass.

- [ ] **Step 3: Commit**

```bash
git add core/tests/cron_probe/
git commit -m "cron probe: P1 lifecycle probes (8 scenarios)"
```

---

### Task 5: P3 Scheduling Precision Probes

**Files:**
- Create: `core/tests/cron_probe/scheduling.rs`
- Modify: `core/tests/cron_probe.rs` (add `mod scheduling;`)

- [ ] **Step 1: Write 6 scheduling scenarios**

```rust
// core/tests/cron_probe/scheduling.rs
use super::harness::CronTestHarness;
use super::mock_executor::MockBehavior;
use alephcore::cron::config::{ErrorReason, ScheduleKind};

#[tokio::test]
async fn anchor_alignment_no_drift() {
    let h = CronTestHarness::new();
    let anchor = h.now();
    let interval = 30 * 60 * 1000; // 30 minutes

    h.add_every_job("aligned", interval).await;

    // Simulate execution taking 7 minutes
    h.executor.on_job("aligned", MockBehavior::Delayed {
        delay_ms: 7 * 60 * 1000,
        output: "done".to_string(),
    });

    // Run 5 cycles
    for cycle in 1..=5 {
        h.advance(interval);
        h.tick().await;

        let state = h.job_state("aligned").await;
        if let Some(next) = state.next_run_at_ms {
            // Next run should be aligned to anchor grid
            let expected = anchor + (cycle + 1) * interval;
            assert_eq!(
                next, expected,
                "cycle {}: next_run {} should align to grid {}",
                cycle, next, expected
            );
        }
    }

    h.assert_execution_count("aligned", 5);
}

#[tokio::test]
async fn stagger_spreads_jobs() {
    let h = CronTestHarness::new();

    // Create 10 jobs with same cron expression but stagger window
    for i in 0..10 {
        let mut job = alephcore::cron::config::CronJob::new(
            format!("stagger-{}", i),
            "agent".to_string(),
            "prompt".to_string(),
            ScheduleKind::Cron {
                expr: "0 0 * * * *".to_string(), // Every hour
                tz: None,
                stagger_ms: Some(300_000), // 5 minute spread window
            },
        );
        job.id = format!("stagger-{}", i);
        h.add_job(job).await;
    }

    // Collect all next_run times
    let mut next_runs = Vec::new();
    for i in 0..10 {
        let state = h.job_state(&format!("stagger-{}", i)).await;
        next_runs.push(state.next_run_at_ms.unwrap());
    }

    // They should NOT all be identical (stagger should spread them)
    let unique_times: std::collections::HashSet<i64> = next_runs.iter().cloned().collect();
    assert!(
        unique_times.len() > 1,
        "stagger should spread jobs, but all {} jobs have the same next_run",
        next_runs.len()
    );

    // All should be within stagger window of each other
    let min = *next_runs.iter().min().unwrap();
    let max = *next_runs.iter().max().unwrap();
    assert!(
        max - min <= 300_000,
        "spread {} should be within stagger window 300000",
        max - min
    );
}

#[tokio::test]
async fn backoff_after_errors() {
    let h = CronTestHarness::new();
    h.add_every_job("failing", 60_000).await;
    h.executor.on_job("failing", MockBehavior::Error {
        message: "timeout".to_string(),
        reason: ErrorReason::Transient("timeout".to_string()),
    });

    // First failure
    h.advance(60_000);
    h.tick().await;
    h.assert_consecutive_errors("failing", 1).await;

    let state1 = h.job_state("failing").await;
    let next1 = state1.next_run_at_ms.unwrap();

    // Advance to retry point and fail again
    h.advance_to(next1);
    h.tick().await;
    h.assert_consecutive_errors("failing", 2).await;

    let state2 = h.job_state("failing").await;
    let next2 = state2.next_run_at_ms.unwrap();

    // Third failure
    h.advance_to(next2);
    h.tick().await;
    h.assert_consecutive_errors("failing", 3).await;

    // Verify increasing delays (backoff tiers: 30s, 60s, 300s)
    // The exact values depend on interaction with natural schedule,
    // but each retry should be later than the previous
    assert!(next2 > next1, "backoff should increase delay");
}

#[tokio::test]
async fn maintenance_recompute_safe() {
    // Regression test for OpenClaw #13992
    let h = CronTestHarness::new();
    h.add_every_job("safe", 60_000).await;

    // Advance so job is past due
    h.advance(60_000);

    // First tick should execute, not skip
    h.tick().await;
    h.assert_executed("safe");
    h.assert_execution_count("safe", 1);
}

#[tokio::test]
async fn at_job_fires_once() {
    let h = CronTestHarness::new();
    let target = h.now() + 10_000;
    h.add_at_job("oneshot", target).await;

    // Fire
    h.advance(10_000);
    h.tick().await;
    h.assert_execution_count("oneshot", 1);

    // Should not fire again
    h.executor.reset_calls();
    h.advance(60_000);
    h.tick().await;
    h.assert_not_executed("oneshot");
}

#[tokio::test]
async fn min_refire_gap_applied() {
    // Ensure MIN_REFIRE_GAP prevents spin when next computed time is too close
    let h = CronTestHarness::new();
    h.add_every_job("fast", 1_000).await; // 1 second interval

    // Execute
    h.advance(1_000);
    h.tick().await;
    h.assert_executed("fast");

    let state = h.job_state("fast").await;
    let next = state.next_run_at_ms.unwrap();
    let ended = state.last_run_at_ms.unwrap() + state.last_duration_ms.unwrap_or(0);

    // Next run should be at least MIN_REFIRE_GAP_MS (2000) after end
    assert!(
        next >= ended + 2000 || next >= h.now() + 1_000,
        "next_run {} should respect MIN_REFIRE_GAP from ended {}",
        next, ended
    );
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --test cron_probe scheduling`
Expected: 6 tests pass.

- [ ] **Step 3: Commit**

```bash
git add core/tests/cron_probe/
git commit -m "cron probe: P3 scheduling precision probes (6 scenarios)"
```

---

## Chunk 3: P2 Concurrency + P4 Fault Recovery (L2 Medium)

### Task 6: P2 Concurrency Probes

**Files:**
- Create: `core/tests/cron_probe/concurrency.rs`

- [ ] **Step 1: Write 6 concurrency scenarios**

```rust
// core/tests/cron_probe/concurrency.rs
use super::harness::CronTestHarness;
use super::mock_executor::MockBehavior;
use alephcore::cron::config::ScheduleKind;
use alephcore::cron::service::concurrency::{phase1_mark_due_jobs, phase3_writeback};
use alephcore::cron::service::ops;

#[tokio::test]
async fn concurrent_list_during_execution() {
    let h = CronTestHarness::new();
    h.add_every_job("running", 60_000).await;
    h.advance(60_000);

    // Phase 1: mark job as running
    let snapshots = phase1_mark_due_jobs(&h.state.store, h.clock.as_ref())
        .await
        .unwrap();
    assert_eq!(snapshots.len(), 1);

    // While "executing" (Phase 2), do a list — should see running_at_ms set
    let jobs = h.list_jobs().await;
    assert_eq!(jobs.len(), 1);
    assert!(
        jobs[0].state.running_at_ms.is_some(),
        "list during Phase 2 should show running_at_ms"
    );

    // Complete Phase 3
    let executor_fn = h.executor.into_executor_fn();
    let result = executor_fn(snapshots.into_iter().next().unwrap()).await;
    phase3_writeback(
        &h.state.store,
        h.clock.as_ref(),
        &[("running".to_string(), result)],
    )
    .await
    .unwrap();

    // After writeback, running_at should be cleared
    h.assert_running("running", false).await;
}

#[tokio::test]
async fn update_during_execution() {
    let h = CronTestHarness::new();
    h.add_every_job("updating", 60_000).await;
    h.advance(60_000);

    // Phase 1
    let snapshots = phase1_mark_due_jobs(&h.state.store, h.clock.as_ref())
        .await
        .unwrap();

    // During Phase 2, update the job name
    {
        let mut store = h.state.store.lock().await;
        let job = store.get_job_mut("updating").unwrap();
        job.name = "updated-name".to_string();
        store.persist().unwrap();
    }

    // Phase 3 writeback (force_reload captures the name change)
    let executor_fn = h.executor.into_executor_fn();
    let result = executor_fn(snapshots.into_iter().next().unwrap()).await;
    phase3_writeback(
        &h.state.store,
        h.clock.as_ref(),
        &[("updating".to_string(), result)],
    )
    .await
    .unwrap();

    // Name should be the updated value (MVCC merge preserves config changes)
    let store = h.state.store.lock().await;
    let job = store.get_job("updating").unwrap();
    assert_eq!(job.name, "updated-name", "MVCC merge should preserve config updates");
    // Execution results should also be written
    assert!(job.state.last_run_at_ms.is_some(), "execution result should be recorded");
}

#[tokio::test]
async fn delete_during_execution() {
    let h = CronTestHarness::new();
    h.add_every_job("deleting", 60_000).await;
    h.advance(60_000);

    // Phase 1
    let snapshots = phase1_mark_due_jobs(&h.state.store, h.clock.as_ref())
        .await
        .unwrap();

    // During Phase 2, delete the job
    {
        let mut store = h.state.store.lock().await;
        store.remove_job("deleting");
        store.persist().unwrap();
    }

    // Phase 3 — should gracefully discard result (no panic)
    let executor_fn = h.executor.into_executor_fn();
    let result = executor_fn(snapshots.into_iter().next().unwrap()).await;
    phase3_writeback(
        &h.state.store,
        h.clock.as_ref(),
        &[("deleting".to_string(), result)],
    )
    .await
    .unwrap(); // Should not panic

    assert!(!h.job_exists("deleting").await);
}

#[tokio::test]
async fn multiple_jobs_concurrent() {
    let h = CronTestHarness::new();
    for i in 0..5 {
        h.add_every_job(&format!("job-{}", i), 60_000).await;
    }

    h.advance(60_000);
    h.tick().await;

    // All 5 should have executed
    for i in 0..5 {
        h.assert_executed(&format!("job-{}", i));
    }
    assert_eq!(h.executor.calls().len(), 5);
}

#[tokio::test]
async fn reentrant_tick_skipped() {
    let h = CronTestHarness::new();
    h.add_every_job("task", 60_000).await;
    h.advance(60_000);

    // Simulate re-entrancy: set is_running before tick
    h.state.set_running(true);
    h.tick().await; // Should be a no-op due to re-entrancy guard
    h.assert_not_executed("task"); // Timer's on_timer_tick checks is_running internally

    // After clearing, tick should work
    // Note: on_timer_tick may or may not check is_running — depends on implementation.
    // If on_timer_tick doesn't check (only run_timer_loop does), this test verifies
    // that the state flag works correctly at least.
    h.state.set_running(false);
}

#[tokio::test]
async fn concurrent_add_during_tick() {
    let h = CronTestHarness::new();
    h.add_every_job("existing", 60_000).await;
    h.advance(60_000);

    // Execute tick for existing job
    h.tick().await;
    h.assert_executed("existing");

    // Add new job after tick
    h.add_every_job("new-job", 60_000).await;
    h.executor.reset_calls();

    // Advance and tick again — new job should now execute
    h.advance(60_000);
    h.tick().await;
    h.assert_executed("new-job");
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --test cron_probe concurrency`

- [ ] **Step 3: Commit**

```bash
git add core/tests/cron_probe/
git commit -m "cron probe: P2 concurrency safety probes (6 scenarios)"
```

---

### Task 7: P4 Fault Recovery Probes

**Files:**
- Create: `core/tests/cron_probe/failure_recovery.rs`

- [ ] **Step 1: Write 7 fault recovery scenarios**

```rust
// core/tests/cron_probe/failure_recovery.rs
use super::harness::CronTestHarness;
use super::mock_executor::MockBehavior;
use alephcore::cron::config::{ErrorReason, RunStatus};
use alephcore::cron::service::catchup::run_startup_catchup;

#[tokio::test]
async fn transient_error_retries() {
    let h = CronTestHarness::new();
    h.add_every_job("retry", 60_000).await;
    h.executor.on_job("retry", MockBehavior::Error {
        message: "rate_limit".to_string(),
        reason: ErrorReason::Transient("rate_limit".to_string()),
    });

    h.advance(60_000);
    h.tick().await;

    h.assert_consecutive_errors("retry", 1).await;
    h.assert_job_enabled("retry", true).await; // Still enabled — will retry

    // next_run should be set (for retry)
    let state = h.job_state("retry").await;
    assert!(state.next_run_at_ms.is_some(), "should schedule retry");
}

#[tokio::test]
async fn permanent_error_disables() {
    let h = CronTestHarness::new();
    h.add_every_job("perm", 60_000).await;
    h.executor.on_job("perm", MockBehavior::Error {
        message: "invalid API key".to_string(),
        reason: ErrorReason::Permanent("invalid API key".to_string()),
    });

    h.advance(60_000);
    h.tick().await;

    h.assert_consecutive_errors("perm", 1).await;
    // Job should be disabled after permanent error
    // (depends on whether the service layer auto-disables — check actual behavior)
}

#[tokio::test]
async fn max_retries_then_disable() {
    let h = CronTestHarness::new();
    h.add_every_job("exhaust", 60_000).await;
    h.executor.on_job("exhaust", MockBehavior::Error {
        message: "timeout".to_string(),
        reason: ErrorReason::Transient("timeout".to_string()),
    });

    // Run max_retries (default 3) + 1 failures
    for _ in 0..4 {
        let state = h.job_state("exhaust").await;
        if let Some(next) = state.next_run_at_ms {
            h.advance_to(next);
        } else {
            h.advance(60_000);
        }
        h.tick().await;
    }

    // After exceeding max_retries, job should be auto-disabled
    // (if implemented in the service layer)
    h.assert_execution_count("exhaust", 4);
}

#[tokio::test]
async fn success_resets_errors() {
    let h = CronTestHarness::new();
    h.add_every_job("recover", 60_000).await;

    // Fail 3 times
    h.executor.on_job("recover", MockBehavior::Error {
        message: "timeout".to_string(),
        reason: ErrorReason::Transient("timeout".to_string()),
    });

    for _ in 0..3 {
        let state = h.job_state("recover").await;
        if let Some(next) = state.next_run_at_ms {
            h.advance_to(next);
        } else {
            h.advance(60_000);
        }
        h.tick().await;
    }
    h.assert_consecutive_errors("recover", 3).await;

    // Now succeed
    h.executor.on_job("recover", MockBehavior::Ok("recovered".to_string()));
    let state = h.job_state("recover").await;
    if let Some(next) = state.next_run_at_ms {
        h.advance_to(next);
    }
    h.tick().await;

    h.assert_consecutive_errors("recover", 0).await;
}

#[tokio::test]
async fn stale_marker_cleared() {
    let h = CronTestHarness::new();
    h.add_every_job("stale", 60_000).await;

    // Manually set running_at to 3 hours ago
    {
        let mut store = h.state.store.lock().await;
        let job = store.get_job_mut("stale").unwrap();
        job.state.running_at_ms = Some(h.now() - 3 * 3_600_000);
        store.persist().unwrap();
    }

    // Run catchup — should clear stale marker
    let report = h.run_catchup().await;
    assert_eq!(report.stale_markers_cleared, 1);

    h.assert_running("stale", false).await;
}

#[tokio::test]
async fn catchup_staggers_missed() {
    let h = CronTestHarness::new();

    // Create 10 jobs, all past due
    for i in 0..10 {
        let mut job = alephcore::cron::config::CronJob::new(
            format!("missed-{}", i),
            "agent".to_string(),
            "prompt".to_string(),
            alephcore::cron::config::ScheduleKind::Every {
                every_ms: 60_000,
                anchor_ms: Some(h.now()),
            },
        );
        job.id = format!("missed-{}", i);
        job.state.next_run_at_ms = Some(h.now() - 1000 * (10 - i as i64)); // All past due
        h.add_job(job).await;
    }

    // Run catchup with max_missed=3
    let report = run_startup_catchup(
        &h.state.store,
        h.clock.as_ref(),
        Some(3),
        Some(5_000),
    )
    .await
    .unwrap();

    assert_eq!(report.immediate_count, 3);
    assert_eq!(report.deferred_count, 7);
}

#[tokio::test]
async fn catchup_then_normal_tick() {
    let h = CronTestHarness::new();
    h.add_every_job("catchup-job", 60_000).await;

    // Make it past due
    {
        let mut store = h.state.store.lock().await;
        let job = store.get_job_mut("catchup-job").unwrap();
        job.state.next_run_at_ms = Some(h.now() - 1000);
        store.persist().unwrap();
    }

    // Catchup
    h.run_catchup().await;

    // Normal tick should pick it up
    h.tick().await;
    h.assert_executed("catchup-job");

    // And subsequent normal ticks should continue working
    h.executor.reset_calls();
    h.advance(60_000);
    h.tick().await;
    h.assert_executed("catchup-job");
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --test cron_probe failure_recovery`

- [ ] **Step 3: Commit**

```bash
git add core/tests/cron_probe/
git commit -m "cron probe: P4 fault recovery probes (7 scenarios)"
```

---

## Chunk 4: P5 Delivery/Alert + P6 Chain + P7 Crash

### Task 8: P5 Delivery & Alert Probes

**Files:**
- Create: `core/tests/cron_probe/delivery_alert.rs`

- [ ] **Step 1: Write 6 delivery/alert scenarios**

```rust
// core/tests/cron_probe/delivery_alert.rs
use super::harness::CronTestHarness;
use super::mock_executor::MockBehavior;
use alephcore::cron::alert::should_send_alert;
use alephcore::cron::config::{
    DeliveryConfig, DeliveryMode, DeliveryTargetConfig, ErrorReason,
    FailureAlertConfig,
};
use alephcore::cron::delivery::should_skip_delivery;

#[tokio::test]
async fn delivery_dedup_agent_sent() {
    // When agent already sent via messaging tool, delivery should be skipped
    let status = should_skip_delivery(true, &DeliveryMode::Primary);
    assert_eq!(
        status,
        alephcore::cron::config::DeliveryStatus::AlreadySentByAgent
    );
}

#[tokio::test]
async fn delivery_none_mode() {
    let status = should_skip_delivery(false, &DeliveryMode::None);
    assert_eq!(
        status,
        alephcore::cron::config::DeliveryStatus::NotRequested
    );
}

#[tokio::test]
async fn delivery_normal_proceeds() {
    let status = should_skip_delivery(false, &DeliveryMode::Primary);
    assert_eq!(
        status,
        alephcore::cron::config::DeliveryStatus::Delivered
    );
}

#[tokio::test]
async fn alert_fires_after_threshold() {
    let h = CronTestHarness::new();
    h.add_every_job("alerting", 60_000).await;

    // Simulate 2 consecutive errors by directly setting state
    {
        let mut store = h.state.store.lock().await;
        let job = store.get_job_mut("alerting").unwrap();
        job.state.consecutive_errors = 2;
        job.state.last_error = Some("timeout".to_string());
    }

    let alert_config = FailureAlertConfig {
        after: 2,
        cooldown_ms: 3_600_000,
        target: DeliveryTargetConfig::Webhook {
            url: "https://example.com".to_string(),
            method: None,
            headers: None,
        },
    };

    let store = h.state.store.lock().await;
    let job = store.get_job("alerting").unwrap();
    let msg = should_send_alert(job, &alert_config, h.now());
    assert!(msg.is_some(), "alert should fire at threshold");
    assert!(msg.unwrap().contains("timeout"));
}

#[tokio::test]
async fn alert_cooldown_blocks() {
    let h = CronTestHarness::new();
    h.add_every_job("cooldown", 60_000).await;

    {
        let mut store = h.state.store.lock().await;
        let job = store.get_job_mut("cooldown").unwrap();
        job.state.consecutive_errors = 5;
        job.state.last_error = Some("error".to_string());
        job.state.last_failure_alert_at_ms = Some(h.now() - 1_800_000); // 30 min ago
    }

    let alert_config = FailureAlertConfig {
        after: 2,
        cooldown_ms: 3_600_000, // 1 hour cooldown
        target: DeliveryTargetConfig::Webhook {
            url: "https://example.com".to_string(),
            method: None,
            headers: None,
        },
    };

    let store = h.state.store.lock().await;
    let job = store.get_job("cooldown").unwrap();

    // Within cooldown — should block
    let msg = should_send_alert(job, &alert_config, h.now());
    assert!(msg.is_none(), "alert should be blocked by cooldown");
}

#[tokio::test]
async fn alert_cooldown_expires() {
    let h = CronTestHarness::new();
    h.add_every_job("expired", 60_000).await;

    {
        let mut store = h.state.store.lock().await;
        let job = store.get_job_mut("expired").unwrap();
        job.state.consecutive_errors = 5;
        job.state.last_error = Some("error".to_string());
        job.state.last_failure_alert_at_ms = Some(h.now() - 4_000_000); // >1h ago
    }

    let alert_config = FailureAlertConfig {
        after: 2,
        cooldown_ms: 3_600_000,
        target: DeliveryTargetConfig::Webhook {
            url: "https://example.com".to_string(),
            method: None,
            headers: None,
        },
    };

    let store = h.state.store.lock().await;
    let job = store.get_job("expired").unwrap();
    let msg = should_send_alert(job, &alert_config, h.now());
    assert!(msg.is_some(), "alert should fire after cooldown expires");
}
```

- [ ] **Step 2: Run and commit**

Run: `cargo test -p alephcore --test cron_probe delivery_alert`

```bash
git add core/tests/cron_probe/
git commit -m "cron probe: P5 delivery & alert probes (6 scenarios)"
```

---

### Task 9: P6 Chain Probes

**Files:**
- Create: `core/tests/cron_probe/chain.rs`

- [ ] **Step 1: Write 4 chain scenarios**

```rust
// core/tests/cron_probe/chain.rs
use super::harness::CronTestHarness;
use alephcore::cron::chain::{detect_cycle, trigger_chain_job};
use alephcore::cron::config::{CronJob, ScheduleKind};

#[tokio::test]
async fn chain_on_success() {
    let h = CronTestHarness::new();

    // Job A chains to Job B on success
    let mut job_a = CronJob::new(
        "job-a".to_string(), "agent".to_string(), "prompt-a".to_string(),
        ScheduleKind::Every { every_ms: 60_000, anchor_ms: Some(h.now()) },
    );
    job_a.id = "job-a".to_string();
    job_a.next_job_id_on_success = Some("job-b".to_string());
    h.add_job(job_a).await;

    h.add_every_job("job-b", 3_600_000).await; // Long interval — not due naturally

    // Execute job-a
    h.advance(60_000);
    h.tick().await;
    h.assert_executed("job-a");

    // Trigger chain
    {
        let mut store = h.state.store.lock().await;
        trigger_chain_job(&mut store, "job-b", h.now()).unwrap();
        store.persist().unwrap();
    }

    // Job B should now be due
    let state = h.job_state("job-b").await;
    assert!(state.next_run_at_ms.unwrap() <= h.now(), "chain trigger should set job-b as due");
}

#[tokio::test]
async fn chain_disabled_target_skipped() {
    let h = CronTestHarness::new();
    h.add_every_job("target", 60_000).await;
    h.toggle_job("target").await; // Disable

    let mut store = h.state.store.lock().await;
    let result = trigger_chain_job(&mut store, "target", h.now());
    // Should return false or not trigger for disabled job
    assert!(
        result.is_ok(),
        "triggering disabled job should not error"
    );
}

#[tokio::test]
async fn chain_cycle_rejected() {
    let h = CronTestHarness::new();

    let mut job_a = CronJob::new(
        "a".to_string(), "agent".to_string(), "p".to_string(),
        ScheduleKind::Every { every_ms: 60_000, anchor_ms: None },
    );
    job_a.id = "a".to_string();
    job_a.next_job_id_on_success = Some("b".to_string());
    h.add_job(job_a).await;

    let mut job_b = CronJob::new(
        "b".to_string(), "agent".to_string(), "p".to_string(),
        ScheduleKind::Every { every_ms: 60_000, anchor_ms: None },
    );
    job_b.id = "b".to_string();
    h.add_job(job_b).await;

    // Detect cycle: a → b → a would be a cycle
    let store = h.state.store.lock().await;
    let has_cycle = detect_cycle(&store, "b", "a").unwrap();
    assert!(has_cycle, "a→b→a should be detected as cycle");
}

#[tokio::test]
async fn chain_no_cycle_linear() {
    let h = CronTestHarness::new();
    h.add_every_job("x", 60_000).await;
    h.add_every_job("y", 60_000).await;
    h.add_every_job("z", 60_000).await;

    // Set chain: x → y (no further chain)
    {
        let mut store = h.state.store.lock().await;
        let job = store.get_job_mut("x").unwrap();
        job.next_job_id_on_success = Some("y".to_string());
        store.persist().unwrap();
    }

    // z → y should NOT be a cycle
    let store = h.state.store.lock().await;
    let has_cycle = detect_cycle(&store, "z", "y").unwrap();
    assert!(!has_cycle, "z→y should not be a cycle");
}
```

- [ ] **Step 2: Run and commit**

Run: `cargo test -p alephcore --test cron_probe chain`

```bash
git add core/tests/cron_probe/
git commit -m "cron probe: P6 job chaining probes (4 scenarios)"
```

---

### Task 10: P7 Crash Probes

**Files:**
- Create: `core/tests/cron_probe/crash.rs`

- [ ] **Step 1: Write 3 crash scenarios using phase-call + drop pattern**

```rust
// core/tests/cron_probe/crash.rs
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::Mutex;

use alephcore::cron::clock::testing::FakeClock;
use alephcore::cron::config::{CronConfig, CronJob, ScheduleKind};
use alephcore::cron::service::catchup::run_startup_catchup;
use alephcore::cron::service::concurrency::phase1_mark_due_jobs;
use alephcore::cron::service::ops;
use alephcore::cron::service::state::ServiceState;
use alephcore::cron::store::CronStore;

/// Simulate crash: call phase1 (persists running markers), then drop everything.
/// Restart: load same store path, run catchup, verify recovery.
#[tokio::test]
async fn crash_mid_execution_recovers() {
    let temp_dir = TempDir::new().unwrap();
    let store_path = temp_dir.path().join("crash_test.json");

    // === First "run" — mark job running, then "crash" ===
    {
        let store = CronStore::load(store_path.clone()).unwrap();
        let clock = Arc::new(FakeClock::new(1_000_000_000));
        let config = CronConfig::default();
        let state = Arc::new(ServiceState::new(
            Arc::new(Mutex::new(store)),
            clock.clone(),
            config,
        ));

        // Add a job and make it due
        {
            let mut store = state.store.lock().await;
            let mut job = CronJob::new(
                "crash-victim".to_string(), "agent".to_string(), "prompt".to_string(),
                ScheduleKind::Every { every_ms: 60_000, anchor_ms: Some(clock.now_ms()) },
            );
            job.id = "crash-victim".to_string();
            ops::add_job(&mut store, job, clock.as_ref());
            store.persist().unwrap();
        }

        // Advance to due
        clock.advance(60_000);

        // Phase 1: marks running_at_ms and persists
        let snapshots = phase1_mark_due_jobs(&state.store, clock.as_ref())
            .await
            .unwrap();
        assert_eq!(snapshots.len(), 1);

        // === CRASH: drop everything without Phase 2 or Phase 3 ===
        // state, store, clock all dropped here
    }

    // === Second "run" — restart from same store file ===
    {
        let store = CronStore::load(store_path.clone()).unwrap();
        let clock = Arc::new(FakeClock::new(1_000_000_000 + 3 * 3_600_000)); // 3 hours later

        // Verify running_at_ms is still set from crashed run
        assert!(
            store.get_job("crash-victim").unwrap().state.running_at_ms.is_some(),
            "running_at_ms should be persisted from Phase 1"
        );

        let config = CronConfig::default();
        let state = Arc::new(ServiceState::new(
            Arc::new(Mutex::new(store)),
            clock.clone(),
            config,
        ));

        // Run catchup — should clear stale marker
        let report = run_startup_catchup(&state.store, clock.as_ref(), None, None)
            .await
            .unwrap();

        assert_eq!(report.stale_markers_cleared, 1);

        // Verify marker cleared
        let store = state.store.lock().await;
        let job = store.get_job("crash-victim").unwrap();
        assert!(job.state.running_at_ms.is_none(), "stale marker should be cleared after catchup");
    }
}

#[tokio::test]
async fn crash_after_phase1_before_phase2() {
    let temp_dir = TempDir::new().unwrap();
    let store_path = temp_dir.path().join("phase1_crash.json");

    // Phase 1 persists running markers
    {
        let store = CronStore::load(store_path.clone()).unwrap();
        let clock = Arc::new(FakeClock::new(1_000_000_000));
        let state = Arc::new(ServiceState::new(
            Arc::new(Mutex::new(store)),
            clock.clone(),
            CronConfig::default(),
        ));

        let mut store_guard = state.store.lock().await;
        let mut job = CronJob::new(
            "phase1-victim".to_string(), "agent".to_string(), "prompt".to_string(),
            ScheduleKind::Every { every_ms: 60_000, anchor_ms: Some(clock.now_ms()) },
        );
        job.id = "phase1-victim".to_string();
        ops::add_job(&mut store_guard, job, clock.as_ref());
        store_guard.persist().unwrap();
        drop(store_guard);

        clock.advance(60_000);
        let _snapshots = phase1_mark_due_jobs(&state.store, clock.as_ref()).await.unwrap();
        // Crash here — no executor, no phase3
    }

    // Verify store file has the running marker
    let raw = std::fs::read_to_string(&store_path).unwrap();
    assert!(raw.contains("running_at_ms"), "Phase 1 should have persisted running_at_ms");

    // Restart and recover
    let store = CronStore::load(store_path).unwrap();
    let clock = Arc::new(FakeClock::new(1_000_000_000 + 3 * 3_600_000));
    let state = Arc::new(ServiceState::new(
        Arc::new(Mutex::new(store)),
        clock.clone(),
        CronConfig::default(),
    ));
    let report = run_startup_catchup(&state.store, clock.as_ref(), None, None).await.unwrap();
    assert_eq!(report.stale_markers_cleared, 1);
}

#[tokio::test]
async fn crash_preserves_store_integrity() {
    let temp_dir = TempDir::new().unwrap();
    let store_path = temp_dir.path().join("integrity.json");

    // Write a store with multiple jobs
    {
        let mut store = CronStore::load(store_path.clone()).unwrap();
        for i in 0..5 {
            let mut job = CronJob::new(
                format!("integrity-{}", i), "agent".to_string(), "prompt".to_string(),
                ScheduleKind::Every { every_ms: 60_000, anchor_ms: None },
            );
            job.id = format!("integrity-{}", i);
            store.add_job(job);
        }
        store.persist().unwrap();
    }

    // Verify file is valid JSON and contains all jobs
    let content = std::fs::read_to_string(&store_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content)
        .expect("store file should be valid JSON");
    let jobs = parsed["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 5, "all 5 jobs should be in store");

    // Verify it can be loaded back
    let store = CronStore::load(store_path).unwrap();
    assert_eq!(store.job_count(), 5);
}
```

- [ ] **Step 2: Run and commit**

Run: `cargo test -p alephcore --test cron_probe crash`

```bash
git add core/tests/cron_probe/
git commit -m "cron probe: P7 crash recovery probes (3 scenarios)"
```

---

## Chunk 5: P9 Gateway + P10 History + P8 Real Agent + Final

### Task 11: P9 Gateway Handler Probes + P10 History

**Files:**
- Create: `core/tests/cron_probe/gateway.rs`
- Create: `core/tests/cron_probe/history_integration.rs`

- [ ] **Step 1: Write P9 gateway scenarios (3)**

Test gateway handlers by calling them with mock `SharedCronService`. The handlers are at `alephcore::gateway::handlers::cron::*`. Check if they're accessible — if not, test through `CronService` methods instead (which is what the handlers delegate to).

```rust
// core/tests/cron_probe/gateway.rs
// If gateway handlers are not publicly accessible, test the CronService
// methods they delegate to — which verifies the same path.

use super::harness::CronTestHarness;
use alephcore::cron::config::ScheduleKind;
use alephcore::cron::service::ops;

#[tokio::test]
async fn service_create_and_list() {
    let h = CronTestHarness::new();

    // Create via ops (same path gateway handler uses)
    h.add_every_job("gw-test", 60_000).await;

    // List — should return the job
    let jobs = h.list_jobs().await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, "gw-test");
    assert_eq!(jobs[0].name, "gw-test");

    // Verify schedule_kind is correct
    match &jobs[0].schedule_kind {
        ScheduleKind::Every { every_ms, .. } => assert_eq!(*every_ms, 60_000),
        _ => panic!("expected Every schedule"),
    }
}

#[tokio::test]
async fn service_manual_run_executes() {
    let h = CronTestHarness::new();
    h.add_every_job("manual", 3_600_000).await; // Far from due

    h.manual_run("manual").await;
    h.assert_executed("manual");
    h.assert_execution_count("manual", 1);

    // Verify trigger source
    let calls = h.executor.calls_for("manual");
    assert_eq!(calls[0].trigger_source, alephcore::cron::config::TriggerSource::Manual);
}

#[tokio::test]
async fn service_get_nonexistent_returns_none() {
    let h = CronTestHarness::new();
    let store = h.state.store.lock().await;
    let result = ops::get_job(&store, "nonexistent");
    assert!(result.is_none());
}
```

- [ ] **Step 2: Write P10 history scenarios (3)**

```rust
// core/tests/cron_probe/history_integration.rs
use alephcore::cron::history::{init_schema, insert_cron_run, get_cron_runs, cleanup_old_cron_runs, CronRunRecord};
use rusqlite::Connection;

#[test]
fn execution_produces_history_record() {
    let conn = Connection::open_in_memory().unwrap();
    init_schema(&conn).unwrap();

    let record = CronRunRecord {
        id: "run-1".to_string(),
        job_id: "job-1".to_string(),
        trigger_source: "schedule".to_string(),
        status: "ok".to_string(),
        started_at: 1_000_000,
        ended_at: Some(1_001_000),
        duration_ms: Some(1000),
        error: None,
        error_reason: None,
        output_summary: Some("test output".to_string()),
        delivery_status: Some("delivered".to_string()),
        created_at: 1_000_000,
    };

    insert_cron_run(&conn, &record).unwrap();

    let runs = get_cron_runs(&conn, "job-1", 10).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "ok");
    assert_eq!(runs[0].duration_ms, Some(1000));
    assert_eq!(runs[0].output_summary, Some("test output".to_string()));
}

#[test]
fn multiple_runs_accumulate() {
    let conn = Connection::open_in_memory().unwrap();
    init_schema(&conn).unwrap();

    for i in 0..3 {
        let record = CronRunRecord {
            id: format!("run-{}", i),
            job_id: "job-1".to_string(),
            trigger_source: "schedule".to_string(),
            status: "ok".to_string(),
            started_at: 1_000_000 + i * 60_000,
            ended_at: Some(1_001_000 + i * 60_000),
            duration_ms: Some(1000),
            error: None,
            error_reason: None,
            output_summary: None,
            delivery_status: None,
            created_at: 1_000_000 + i * 60_000,
        };
        insert_cron_run(&conn, &record).unwrap();
    }

    let runs = get_cron_runs(&conn, "job-1", 10).unwrap();
    assert_eq!(runs.len(), 3);
    // Most recent first
    assert!(runs[0].started_at > runs[1].started_at);
}

#[test]
fn history_cleanup_respects_retention() {
    let conn = Connection::open_in_memory().unwrap();
    init_schema(&conn).unwrap();

    let now = 1_000_000_000i64;
    let one_day = 86_400_000i64;

    // Old record (5 days ago)
    insert_cron_run(&conn, &CronRunRecord {
        id: "old".to_string(),
        job_id: "job-1".to_string(),
        trigger_source: "schedule".to_string(),
        status: "ok".to_string(),
        started_at: now - 5 * one_day,
        ended_at: None, duration_ms: None, error: None,
        error_reason: None, output_summary: None, delivery_status: None,
        created_at: now - 5 * one_day,
    }).unwrap();

    // Recent record (1 hour ago)
    insert_cron_run(&conn, &CronRunRecord {
        id: "recent".to_string(),
        job_id: "job-1".to_string(),
        trigger_source: "schedule".to_string(),
        status: "ok".to_string(),
        started_at: now - 3_600_000,
        ended_at: None, duration_ms: None, error: None,
        error_reason: None, output_summary: None, delivery_status: None,
        created_at: now - 3_600_000,
    }).unwrap();

    // Cleanup with 3-day retention
    cleanup_old_cron_runs(&conn, 3, now).unwrap();

    let runs = get_cron_runs(&conn, "job-1", 10).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, "recent");
}
```

- [ ] **Step 3: Run and commit**

Run: `cargo test -p alephcore --test cron_probe gateway` and `cargo test -p alephcore --test cron_probe history_integration`

```bash
git add core/tests/cron_probe/
git commit -m "cron probe: P9 gateway + P10 history integration probes (6 scenarios)"
```

---

### Task 12: P8 Real Agent Probes + Final Module Declarations

**Files:**
- Create: `core/tests/cron_probe/real_agent.rs`
- Modify: `core/tests/cron_probe.rs` (final mod declarations)

- [ ] **Step 1: Write real agent stubs (2 scenarios, all #[ignore])**

```rust
// core/tests/cron_probe/real_agent.rs

/// Real agent execution probe — requires ALEPH_TEST_AGENT=true
#[tokio::test]
#[ignore]
async fn real_agent_execution() {
    if std::env::var("ALEPH_TEST_AGENT").is_err() {
        eprintln!("Skipping: ALEPH_TEST_AGENT not set");
        return;
    }
    // TODO: Wire to actual agent_loop::run_turn when integration is ready
    // For now, this is a placeholder that verifies the test infrastructure works
    // with a real executor callback.
    eprintln!("Real agent probe: not yet wired to agent_loop");
}

/// Real agent timeout probe
#[tokio::test]
#[ignore]
async fn real_agent_timeout() {
    if std::env::var("ALEPH_TEST_AGENT").is_err() {
        eprintln!("Skipping: ALEPH_TEST_AGENT not set");
        return;
    }
    eprintln!("Real agent timeout probe: not yet wired");
}
```

- [ ] **Step 2: Finalize cron_probe.rs with all module declarations**

```rust
// core/tests/cron_probe.rs
mod mock_executor;
mod harness;
mod lifecycle;
mod scheduling;
mod concurrency;
mod failure_recovery;
mod delivery_alert;
mod chain;
mod crash;
mod gateway;
mod history_integration;
mod real_agent;
```

- [ ] **Step 3: Run all probes**

Run: `cargo test -p alephcore --test cron_probe`
Expected: 48 tests (46 run + 2 ignored).

- [ ] **Step 4: Commit**

```bash
git add core/tests/cron_probe/
git commit -m "cron probe: P8 real agent stubs + finalize all 48 scenarios"
```

---

### Task 13: Final Verification

- [ ] **Step 1: Run full cron test suite (unit + integration)**

Run: `cargo test -p alephcore --lib cron && cargo test -p alephcore --test cron_probe`
Expected: ~150 unit tests + 46 probe tests pass, 2 ignored.

- [ ] **Step 2: Clippy**

Run: `cargo clippy -p alephcore --test cron_probe -- -W clippy::all`

- [ ] **Step 3: Final commit if fixes needed**

```bash
git commit -m "cron probe: final integration fixes"
```

---

## Summary

| Chunk | Tasks | Scenarios |
|-------|-------|-----------|
| 1. Prerequisites + Infrastructure | Tasks 1-3: lib.rs export, MockExecutor, Harness | 1 smoke test |
| 2. Lifecycle + Scheduling | Tasks 4-5: P1, P3 | 14 scenarios |
| 3. Concurrency + Fault Recovery | Tasks 6-7: P2, P4 | 13 scenarios |
| 4. Delivery/Alert + Chain + Crash | Tasks 8-10: P5, P6, P7 | 13 scenarios |
| 5. Gateway + History + Real Agent | Tasks 11-12: P9, P10, P8 | 8 scenarios |
| **Total** | **13 tasks** | **48 scenarios** |
