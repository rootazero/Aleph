# Cron Module Redesign: Infrastructure-Grade Task Scheduling

**Date**: 2026-03-14
**Status**: Approved
**Approach**: Module rebuild — rewrite internals, preserve external interfaces

---

## Context & Motivation

Aleph's `core/src/cron/` module has a mature skeleton (ScheduleKind, delivery, chain, template, Gateway RPC) but lacks production-grade reliability guarantees. Drawing from OpenClaw's battle-tested planning module, this redesign upgrades the cron system to answer three infrastructure questions reliably:

1. **When does a task start?** — Anchor-aligned scheduling, hash-based stagger, drift-free
2. **Who gets the results?** — Unified delivery with dedup
3. **What happens on failure?** — Retry, alert, crash recovery, restart catchup

This is a greenfield rebuild — the existing code is not in production use.

---

## Key Decisions

| Dimension | Decision | Rationale |
|-----------|----------|-----------|
| Scope | Full coverage (scheduling + concurrency + recovery) in MVP core | User preference |
| Code constraint | Greenfield — can restructure freely | No compatibility burden |
| Persistence | JSON file (job defs + state) + SQLite (execution history) | JSON for atomic write safety; SQLite for query-friendly history via existing state_database |
| Execution paths | Dual: lightweight (main loop injection) + isolated (independent agent session) | Different resource profiles need different strategies |
| Concurrency | Per-type: lightweight unlimited, agent tasks max 2 | Lightweight is <1ms; agent tasks consume LLM tokens/memory |
| Sub-agent followup | Deferred to Phase 2 | Depends on agent execution chain maturity |
| Panel UI | Sync update with backend changes | Existing Leptos UI at apps/panel/ must reflect new fields |

---

## 1. Module Structure

```
core/src/cron/
├── mod.rs                     // Module facade, re-exports
├── config.rs                  // Data types (CronJob, ScheduleKind, JobState, etc.)
├── clock.rs                   // Clock trait + SystemClock + FakeClock
├── schedule.rs                // Pure scheduling computation (replaces scheduler.rs)
├── stagger.rs                 // SHA256 hash-based stagger spreading
├── store.rs                   // JSON atomic persistence (tmp+fsync+rename)
├── alert.rs                   // Failure alerting + cooldown
├── delivery.rs                // Result delivery + dedup (existing, enhanced)
├── chain.rs                   // Job chaining (existing, migrated to JSON store)
├── template.rs                // Prompt templating (existing, Clock-injected)
├── webhook_target.rs          // Webhook targets (existing, unchanged)
├── service/
│   ├── mod.rs                 // Service facade
│   ├── state.rs               // ServiceState<C: Clock> — runtime state container
│   ├── ops.rs                 // Public operations (add/update/remove/list/get/run/toggle)
│   ├── timer.rs               // Core scheduling loop (tick → find due → execute → writeback)
│   ├── concurrency.rs         // Three-phase execution model (mark → execute → merge)
│   └── catchup.rs             // Restart catchup strategy
└── execution/
    ├── mod.rs
    ├── lightweight.rs          // Main-loop event injection
    └── isolated.rs             // Isolated agent session execution
```

### Responsibility Boundaries

| Module | Does | Does Not |
|--------|------|----------|
| `config.rs` | Define all data types | Contain behavior logic |
| `clock.rs` | Provide time | Do scheduling computation |
| `schedule.rs` | Pure-function next-run computation | Access state or persist |
| `stagger.rs` | Pure-function hash offset | Know what a job is |
| `store.rs` | File read/write + migration | Make business decisions |
| `service/state.rs` | Hold store + clock + config | Execute tasks |
| `service/ops.rs` | CRUD operations + manual trigger | Contain timer loop |
| `service/timer.rs` | Timer loop + worker pool dispatch | Do CRUD |
| `service/concurrency.rs` | Three-phase lock protocol | Know execution details |
| `service/catchup.rs` | Restart recovery | Run outside of startup |
| `execution/*` | Actually execute tasks | Operate on store |
| `alert.rs` | Send alerts + enforce cooldown | Modify job state |
| `delivery.rs` | Deliver results + dedup | Modify job state |

### Files to Remove

- `scheduler.rs` → functionality moves to `schedule.rs` (computation) + `service/timer.rs` (loop)
- `resource.rs` → CPU-aware concurrency limiting is removed in favor of fixed `max_concurrent_agents=2`. Rationale: Aleph is a personal assistant on a single machine; fixed limits are simpler and sufficient. Adaptive resource management can be revisited in Phase 3 if needed.

### Files to Migrate

- `chain.rs` → currently queries SQLite (`cron_jobs` table) for cycle detection and chain triggering. Must be rewritten to operate against `CronStore` (JSON-backed in-memory state) instead. The chain logic itself (success/failure triggers, cycle detection) is preserved; only the data access layer changes.
- `template.rs` → currently calls `Utc::now()` directly for `{{now}}` template variable. Must accept `&dyn Clock` parameter. Template rendering occurs in Phase 1 (snapshot creation time), so `{{now}}` captures the scheduled execution time, not the actual agent start time.

### ScheduleKind Migration

The existing `CronJob` uses flat fields (`every_ms`, `at_time`, `schedule`, `timezone`, `delete_after_run`) with a separate `schedule_kind` discriminant. The redesign moves to a rich enum `ScheduleKind` with embedded data. Since this is a greenfield rebuild, the Gateway RPC interface adopts the new structure directly. Panel API DTOs (`CronJobInfo`, `CreateCronJob`, `UpdateCronJob`) are updated to match — the `schedule` field becomes a tagged JSON object:

```json
{"kind": "every", "every_ms": 1800000, "anchor_ms": null}
{"kind": "cron", "expr": "0 * * * *", "tz": "Asia/Shanghai", "stagger_ms": 300000}
{"kind": "at", "at": 1710425400000, "delete_after_run": true}
```

---

## 2. Core Data Structures

### 2.1 Clock Trait (`clock.rs`)

```rust
pub trait Clock: Send + Sync + 'static {
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;

#[cfg(any(test, feature = "test-helpers"))]
pub struct FakeClock {
    current: AtomicI64,  // from sync_primitives
}
// Methods: new(ms), advance(ms), set(ms)
```

All time-dependent modules receive `&dyn Clock` or generic `C: Clock`. Zero direct calls to `Utc::now()`.

### 2.2 Enhanced ScheduleKind (`config.rs`)

```rust
pub enum ScheduleKind {
    At {
        at: i64,
        delete_after_run: bool,
    },
    Every {
        every_ms: i64,
        anchor_ms: Option<i64>,     // Anchor point; defaults to created_at_ms
    },
    Cron {
        expr: String,
        tz: Option<String>,
        stagger_ms: Option<i64>,    // Hash-spread window; default 0
    },
}
```

### 2.3 Enhanced JobState (`config.rs`)

```rust
pub struct JobState {
    pub next_run_at_ms: Option<i64>,
    pub running_at_ms: Option<i64>,              // Crash recovery marker
    pub last_run_at_ms: Option<i64>,
    pub last_run_status: Option<RunStatus>,
    pub last_error: Option<String>,
    pub last_error_reason: Option<ErrorReason>,   // Transient vs Permanent
    pub last_duration_ms: Option<i64>,
    pub consecutive_errors: u32,
    pub schedule_error_count: u32,                // Auto-disable threshold
    pub last_failure_alert_at_ms: Option<i64>,    // Alert cooldown
    pub last_delivery_status: Option<DeliveryStatus>,
}

pub enum RunStatus { Ok, Error, Skipped, Timeout }

pub enum ErrorReason {
    Transient(String),   // network, rate_limit, 5xx, timeout → retry
    Permanent(String),   // auth, bad config → don't retry
}

pub enum DeliveryStatus {
    Delivered,
    NotDelivered,
    AlreadySentByAgent,  // Agent used messaging tool during execution
    NotRequested,
}
```

### 2.4 Failure Alert Config

```rust
pub struct FailureAlertConfig {
    pub after: u32,            // Alert after N consecutive failures; default 2
    pub cooldown_ms: i64,      // Cooldown between alerts; default 3600000 (1h)
    pub target: DeliveryTarget,
}
```

### 2.5 Execution Types

```rust
pub struct JobSnapshot {
    pub id: String,
    pub agent_id: Option<String>,
    pub prompt: String,              // Template-rendered prompt (rendered in Phase 1)
    pub model: Option<String>,
    pub timeout_ms: Option<i64>,
    pub delivery: Option<DeliveryConfig>,
    pub session_target: SessionTarget,
    pub marked_at: i64,
}

pub enum SessionTarget { Main, Isolated }

pub struct ExecutionResult {
    pub started_at: i64,
    pub ended_at: i64,
    pub duration_ms: i64,
    pub status: RunStatus,
    pub output: Option<String>,
    pub error: Option<String>,
    pub error_reason: Option<ErrorReason>,
    pub delivery_status: Option<DeliveryStatus>,
}
```

### 2.6 Store File Format

```rust
pub struct CronStoreFile {
    pub version: u32,       // Current = 1
    pub jobs: Vec<CronJob>,
}
// Default path: ~/.config/aleph/cron/jobs.json
```

---

## 3. Scheduling Algorithms

### 3.1 Anchor-Aligned Interval Scheduling

```rust
fn compute_next_every(now_ms: i64, every_ms: i64, anchor_ms: i64, last_run_at_ms: Option<i64>) -> i64 {
    // Guard: future manual trigger
    if let Some(last) = last_run_at_ms {
        if last > now_ms { return last + every_ms; }
    }
    // Guard: future anchor
    if now_ms <= anchor_ms { return anchor_ms; }
    // Guard: invalid interval
    if every_ms <= 0 { return now_ms; } // caller should increment schedule_error_count

    // Core formula: align to anchor grid
    let elapsed = now_ms - anchor_ms;
    let periods = (elapsed + every_ms - 1) / every_ms;  // ceil division (safe: both positive)
    anchor_ms + periods * every_ms
}
// anchor resolved as: explicit_anchor_ms ?? created_at_ms
```

Properties:
- Execution duration does not affect schedule grid alignment
- Example: anchor=10:00, interval=30min, run finishes 10:07 → next is 10:30, not 10:37
- Anchor persists across restarts; schedule never drifts
- `now == anchor` → `ceil(0/interval) = 0` → returns `anchor` (correct: job fires at its anchor point)
- All edge cases (future anchor, future manual trigger, zero interval) have explicit guards before the formula

### 3.2 Cron Hash Stagger

```
offset = SHA256(job_id)[0..4] as u32 % stagger_ms
staggered_next = cron_next + offset
if staggered_next <= now → staggered_next = next_cron_window + offset
```

Properties:
- Deterministic: same job_id always gets same offset
- Uniform distribution across `[0, stagger_ms)`
- Recommended: `stagger_ms = 300_000` (5min) for hourly jobs

### 3.3 Anti-Spin Safety Net

```
MIN_REFIRE_GAP_MS = 2000
actual_next = max(computed_next, last_ended_at + MIN_REFIRE_GAP_MS)
```

Prevents tight loops from cron library edge cases (timezone boundaries, DST transitions).

### 3.4 Two Recompute Modes

| Mode | Called By | Behavior |
|------|-----------|----------|
| **Full recompute** | `add`, `update`, `toggle` | Advance all jobs' `next_run_at_ms` to future |
| **Maintenance recompute** | timer tick, Phase 3 writeback | Only fill `next_run_at_ms == None`; **never modify existing values** |

Critical invariant: maintenance mode prevents silently skipping past-due jobs during timer ticks. (OpenClaw Bug #13992)

---

## 4. Concurrency Control

### 4.1 Three-Phase Execution Model

```
Phase 1 (locked, <1ms):       Phase 2 (unlocked, s~min):    Phase 3 (locked, <1ms):
┌──────────────────┐          ┌──────────────────┐          ┌──────────────────┐
│ Check runnable   │          │ Execute agent    │          │ reload_from_disk │
│ Set running_at   │  ───►    │ Collect output   │  ───►    │ Merge results    │
│ Clone snapshot   │          │ Deliver results  │          │ Maintenance      │
│ Persist marker   │          │ (no store access)│          │   recompute      │
│ Release lock     │          │                  │          │ Persist          │
└──────────────────┘          └──────────────────┘          └──────────────────┘
```

Lock choice: `tokio::sync::Mutex` (async lock) because Phase 1/3 include `persist()` which requires `.await`.

### 4.2 MVCC-Style Merge Strategy

Phase 3 writeback applies different merge rules by field category:

| Field Category | Source | Rule |
|---------------|--------|------|
| Config (name, schedule, delivery, prompt...) | Disk (latest) | Preserved from reload; never overwritten |
| Execution results (last_run_*, duration, error...) | ExecutionResult | Always written |
| Counters (consecutive_errors) | Computed | Increment or reset based on result.status |
| Schedule state (next_run_at_ms) | Maintenance recompute | Only fill if None |
| Running marker (running_at_ms) | Fixed | Unconditionally set to None |

Special case — job deleted during execution: if the job is absent from the reloaded file in Phase 3, discard the execution result and log a warning. The execution history record in SQLite is still written (it's a factual record of what happened).

Effect: user edits during execution are preserved; execution results are always recorded; no conflicts.

### 4.3 Read Operations — Zero Side Effects

```rust
// Correct: pure read
pub fn list_jobs(store: &CronStore) -> Vec<CronJobView> { ... }
pub fn get_job(store: &CronStore, id: &str) -> Option<CronJobView> { ... }

// CronJobView is a read-only projection type — no &mut exposure
// Compiler prevents state modification on read paths
```

### 4.4 Stale Running Marker Recovery

On startup, clear any `running_at_ms` older than the stale threshold:
```
STALE_THRESHOLD = max(7_200_000, job_timeout_ms * 2)  // at least 2h, or 2x the job's timeout
for job where running_at_ms < now - STALE_THRESHOLD:
    running_at_ms = None
    warn!("cleared stale running marker")
```

The threshold is tied to `job_timeout_ms` to avoid prematurely clearing markers for long-running jobs with high timeout configs.

---

## 5. Execution Architecture

### 5.1 Timer Loop

```
every check_interval_secs (default 15s, max 60s):
  1. if is_running (AtomicBool) → skip (re-entrancy guard)
  2. set is_running = true
  3. Phase 1: lock → reload → collect due jobs → mark → persist → unlock
  4. Split by SessionTarget:
     ├─ Main jobs → tokio::spawn each (unlimited concurrency)
     └─ Isolated jobs → push to shared queue
  5. Spawn N workers (N = min(due_count, max_concurrent_agents=2))
     each worker: loop { pop from queue → execute → collect result }
  6. Await all workers + all main tasks
  7. Phase 3: lock → reload → merge results → maintenance recompute → persist → unlock
  8. Check failure alerts (cooldown-aware)
  9. set is_running = false
  10. Arm next tick
```

### 5.2 Dual Execution Paths

**Lightweight (`lightweight.rs`)** — SessionTarget::Main:
- Wrap prompt as SystemEvent, inject into main agent session via Gateway event channel
- <1ms, no LLM tokens, unlimited concurrency
- Use case: timed reminders, system notifications, built-in tool triggers

**Isolated (`isolated.rs`)** — SessionTarget::Isolated:
- Create independent session (`cron:{job_id}:{run_uuid}`)
- Resolve model (job override > config default)
- Call `agent_loop::run_turn()` with timeout
- Collect output, clean up session
- Seconds to minutes, LLM token cost, limited to `max_concurrent_agents=2`
- Use case: scheduled reports, data analysis, proactive monitoring

### 5.3 Result Delivery

```
ExecutionResult
  → mode == None? → NotRequested
  → dedup: agent already sent via messaging tool? → AlreadySentByAgent
  → for each target:
      Announce → gateway.send_message(channel, output)
      Webhook → HTTP POST { job_id, status, output, timestamp }
      Memory → memory_store.insert(output, metadata)
  → Delivered / NotDelivered
```

### 5.4 Failure Chain

```
On Error/Timeout:
  → consecutive_errors += 1
  → classify: Transient / Permanent

  Transient && consecutive_errors <= max_retries (default 3):
    → next_run_at_ms = max(natural_next, ended_at + backoff[n])
    → backoff tiers: [30s, 1m, 5m, 15m, 1h]

  Transient && consecutive_errors > max_retries:
    → enabled = false (auto-disable)

  Permanent:
    → enabled = false (immediate disable)

  Then alert check:
    → consecutive_errors >= alert.after (default 2)?
    → now - last_failure_alert_at_ms > cooldown (default 1h)?
    → Both true → send alert → update last_failure_alert_at_ms
```

---

## 6. Restart Recovery

### Startup Catchup Sequence

```
Step 1: Clear stale running markers (>2h old)
Step 2: Collect missed jobs (enabled, not running, next_run_at_ms <= now)
        Sort by next_run_at_ms ASC (most overdue first)
Step 3: Split:
        immediate = first 5 → keep next_run_at_ms (timer picks up on first tick)
        deferred = rest → next_run_at_ms = now + (i+1) * 5000ms
Step 4: Persist → start timer loop
```

Configurable: `max_missed_jobs_per_restart` (default 5), `catchup_stagger_ms` (default 5000ms).

Effect: 20 overdue jobs after 2h downtime → 5 execute on first tick, remaining 15 spread over ~75s.

---

## 7. Persistence

### 7.1 Atomic File Write

```
atomic_write(path, data):
  1. tmp = "{path}.{pid}.{rand:08x}.tmp"
  2. write(tmp, data)
  3. fsync(tmp)
  4. rename(path, path.bak)     // backup, best-effort
  5. rename(tmp, path)          // atomic swap
```

At any moment, `path` contains either the old complete file or the new complete file.

### 7.2 Startup Load with Recovery

```
load_store(path):
  if path exists → parse
  elif path.bak exists → warn, rename bak→path, parse
  else → empty store (first run)

  apply idempotent migrations if version < CURRENT
```

### 7.3 Reload & Dirty Tracking

```rust
pub struct CronStore {
    path: PathBuf,
    file: CronStoreFile,
    last_mtime: Option<SystemTime>,  // skip reload if unchanged
    dirty: bool,                      // skip persist if clean
}
```

### 7.4 Execution History in SQLite

```sql
CREATE TABLE cron_job_runs (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL,
    trigger_source TEXT NOT NULL,    -- "schedule"|"manual"|"catchup"|"chain"
    status TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    duration_ms INTEGER,
    error TEXT,
    error_reason TEXT,
    output_summary TEXT,             -- first 500 chars
    delivery_status TEXT,
    created_at INTEGER NOT NULL
);
```

Retention: `history_retention_days` (default 30), cleaned hourly by timer loop.

Referential integrity: execution history references `job_id` but has no foreign key to the JSON store. Orphaned history records (job deleted but history remains) are acceptable — they are cleaned up by time-based retention. Deleting a job does NOT eagerly purge its history; this avoids coupling the two persistence layers.

---

## 8. Panel UI Sync

### 8.1 API DTO Changes (`apps/panel/src/api/cron.rs`)

CronJobInfo adds: `anchor_ms`, `stagger_ms`, `running_at_ms`, `consecutive_errors`, `last_error_reason`, `last_delivery_status`, `failure_alert`.

CreateCronJob/UpdateCronJob add: `anchor_ms`, `stagger_ms`, `failure_alert`.

### 8.2 JobList Enhancement

Three-state indicator:
- Blue pulse — running (`running_at_ms.is_some()`)
- Green solid — enabled, idle
- Gray solid — disabled
- Red badge overlay — `consecutive_errors > 0`

### 8.3 JobEditor Form Enhancement

Schedule Type = `every`: add optional Anchor input field.
Schedule Type = `cron`: add optional Stagger window input field.
New collapsible section: Failure Alert config (after, cooldown, target).

### 8.4 RunHistory Enhancement

Add Delivery column: `sent` / `by agent` / `skip` / `n/a`.
Error column shows classification prefix: `transient:` / `permanent:`.

---

## 9. Testing Strategy

### 9.1 Unit Test Matrix

| Module | Tests | Method |
|--------|-------|--------|
| `schedule.rs` | Anchor alignment no-drift, edge cases, DST, MIN_REFIRE_GAP, maintenance vs full recompute | Pure functions + FakeClock |
| `stagger.rs` | Determinism, uniform distribution, range validity | Pure functions |
| `store.rs` | Atomic write, crash simulation, bak recovery, idempotent migration, mtime-based reload | tempdir |
| `concurrency.rs` | Full three-phase flow, config-changed-during-execution merge, job-deleted-during-execution | FakeClock + mock executor |
| `timer.rs` | Multi-job concurrent dispatch, re-entrancy guard | FakeClock + mock executor |
| `catchup.rs` | Stale marker clearing, stagger distribution | FakeClock |
| `ops.rs` | Read operations zero side effects (snapshot comparison) | Store snapshot |
| `alert.rs` | Cooldown enforcement, threshold check | FakeClock |
| `delivery.rs` | Agent-send dedup | Mock |
| Backoff | Tier correctness, transient retry / permanent disable | Pure functions |

### 9.2 Regression Tests (from OpenClaw production bugs)

- **#13992**: maintenance recompute must not advance past-due jobs
- **#17821**: MIN_REFIRE_GAP prevents spin loops
- **#17554**: stale running markers cleared on startup
- **#18892**: startup catchup respects max_missed limit

### 9.3 Integration Tests

`MockExecutor` (under `test-helpers` feature) with configurable results, call logging, and delay simulation. End-to-end scenarios: create→advance time→verify execution→verify persistence.

---

## 10. Phased Delivery

### MVP (Phase 1) — Scheduling Reliability Foundation

All items in this section:
- `clock.rs`: Clock trait + FakeClock
- `schedule.rs`: anchor-aligned, cron-next, MIN_REFIRE_GAP, two recompute modes
- `stagger.rs`: hash-based stagger
- `store.rs`: atomic write, load, reload, dirty tracking
- `config.rs`: enhanced ScheduleKind, JobState, new enums
- `service/state.rs`: ServiceState<C>
- `service/ops.rs`: CRUD with zero-side-effect reads
- `service/concurrency.rs`: three-phase model
- `service/timer.rs`: timer loop + worker pool
- `service/catchup.rs`: restart recovery
- `execution/lightweight.rs`: main-loop injection
- `execution/isolated.rs`: isolated agent execution
- `alert.rs`: failure alert + cooldown
- `delivery.rs`: enhanced with dedup
- SQLite schema for execution history
- Gateway handler updates
- Panel UI sync (all changes in Section 8)
- Full test suite (unit + regression + integration)

### Phase 2 — Advanced Features (Future)

- Sub-agent detection and followup (`execution/subagent.rs`)
- Persistent sessions for isolated jobs
- Job chaining enhancement (conditional chains based on output content)
- Execution telemetry (token usage, model, provider)

### Phase 3 — Operational Excellence (Future)

- Store format migration framework (version 1→2→...)
- Execution history analytics (success rate trends, duration percentiles)
- Job dependency graph (DAG scheduling)
- External trigger API (webhook-triggered jobs)
