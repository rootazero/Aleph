# Logic Review Report
**Module**: `src/tasks/`
**Scope**: Full review of cron/, heartbeat/, shared/ submodules (36 .rs files, ~15,941 LOC)
**Date**: 2026-08-29
**Branch**: `audit/2026-08-29-tasks`
**Mode**: strict (security-critical)

## Summary
| Level | Count |
|-------|-------|
| Critical | 0 |
| Warning | 4 |
| Suggested Test | 2 |

The module is in good shape: the architectural redlines hold (lock
hierarchy, clock abstraction in time-sensitive scheduler paths, fail-closed
approval markers, typed `TaskError` taxonomy, write-time chain validation
backed by fire-time cycle checks, three-phase concurrency model that keeps
lock-hold time bounded, deterministic at-most-once scheduling). Most of
the prior-audit findings from 2026-05-22 (i64 underflow on clock
adjustment, direct `Utc::now()` in the timer loops, swallowed L2 status
mappings) are already fixed in the current tree.

The remaining warnings cluster around two themes:

1. **Layering leaks at write boundaries** — the service layer lets a
   SQLite `UNIQUE` constraint surface as a generic `Internal` storage
   error when the caller picked a duplicate id. Fixed.
2. **Incomplete Clock injection** — the scheduler uses `Clock` end-to-end,
   but a handful of cleanup/reaper paths still call `chrono::Utc::now()` or
   `std::time::SystemTime::now()` directly. Documented; the reaper wiring
   would need a deeper refactor to thread the clock through.

## Findings

### [Warning] Duplicate job/task id surfaces as a generic storage error
- **Location**: `src/tasks/cron/mod.rs:293-315` (`CronService::add_job`) and
  `src/tasks/heartbeat/mod.rs:124-145` (`HeartbeatService::add_task`)
- **Trigger condition**: A caller submits a job or task with an id that
  already exists in the store (e.g. RPC create with an explicit id, a
  tool surface that did not mint a fresh UUID, an import tool replaying
  payloads).
- **Expected behavior**: The caller sees a classified "already exists"
  error so they can fix the request.
- **Actual behavior**: The store-level `add_job` / `add_task` pushes the
  row onto the in-memory `Vec`; on the next `persist()` the SQLite
  `UNIQUE` constraint on `cron_jobs.id` / `heartbeat_tasks.id` rejects
  the insert. The error bubbles up as `TaskError::Internal("failed to
  insert job 'X': UNIQUE constraint failed: …")` — indistinguishable from
  a real storage fault. The caller cannot tell whether they hit a duplicate
  or a genuine write bug.
- **Suggested fix**: Check for an existing id at the service boundary,
  under the same store lock as the write, and return
  `TaskError::Invalid("job with id 'X' already exists")`. Both
  `add_job` and `add_task` now do this. The check is held under the
  same lock as the subsequent `chain::validate` / `persist` so a
  concurrent delete cannot race the duplicate check.

### [Warning] `DedupEngine::cleanup` bypasses the Clock abstraction
- **Location**: `src/tasks/heartbeat/dedup.rs:172-181`
- **Trigger condition**: The shared task reaper sweeps the
  `heartbeat_dedup` table at a configurable cadence.
- **Expected behavior**: All time-dependent code paths use the `Clock`
  trait so production runs and tests compute cutoffs the same way.
- **Actual behavior**: `cleanup()` calls `chrono::Utc::now()` directly to
  compute the cutoff, so the reaper cannot be exercised with a
  `FakeClock`. The clock IS threaded through `HeartbeatServiceState` and
  the timer loop already uses it for everything else — only the dedup
  reaper path is inconsistent.
- **Suggested fix**: Thread `&dyn Clock` through `DedupEngine::cleanup`
  (and accept it at `DedupHistoryReaper::reap`, or attach the clock to
  the engine at construction). Leave as a tracked follow-up: the reaper
  is built once at boot and the refactor would touch the daemon's wiring
  glue, which is out of scope for this audit's "module only" rule.

### [Warning] `heartbeat::history::prune_old_records` uses `SystemTime::now()`
- **Location**: `src/tasks/heartbeat/history.rs:97-105`
- **Trigger condition**: The shared task reaper sweeps `heartbeat_runs`
  on its interval.
- **Expected behavior**: The cutoff uses `Clock::now_ms()` like every
  other time-dependent scheduler path.
- **Actual behavior**: `prune_old_records` reads `SystemTime::now()`
  directly. The heartbeat timer loop and `compute_next_due` use the
  clock; only the cleanup path does not. Same testability gap as the
  dedup reaper — the reaper is unit-testable only with real wall clock.
- **Suggested fix**: Add a `now_ms: i64` parameter to `prune_old_records`
  and have the reaper call `state.clock.now_ms()`. Tracked as a follow-up
  for the same reason as the dedup path.

### [Warning] `CarryOver::new` and `validate_schedule_kind` ignore Clock
- **Location**: `src/tasks/cron/carryover.rs:88-95`,
  `src/tasks/cron/service/ops.rs:118-120`
- **Trigger condition**: Cron job produces a budget-exhausted partial
  result (writes a carryover record); a write surface validates a
  schedule kind.
- **Expected behavior**: Both produce deterministic timestamps that can
  be tested against a `FakeClock`.
- **Actual behavior**: Both call `chrono::Utc::now()` directly to stamp
  `saved_at_ms` or to pick a reference "now" for the next-occurrence
  validation. The carryover's round-trip tests don't compare to
  `Utc::now()`, so this is not a runtime bug — but the test gap is real
  and the inconsistency with the rest of the scheduler is visible.
- **Suggested fix**: Accept an optional `now_ms` parameter (with
  `Utc::now()` as the default at the existing call site), or thread the
  clock through. Small enough to be a one-line patch; left as a
  follow-up because callers cross module boundaries.

## Suggested Tests

### [Suggested Test] Regression: duplicate-id surface returns `Invalid`
- **Location**: `src/tasks/cron/mod.rs` and `src/tasks/heartbeat/mod.rs`
- Two new tests pin the service-level guard added for the duplicate-id
  finding above: one each in the cron and heartbeat test modules. They
  mint a UUID-bearing job/task via `add_*`, then submit a second
  job/task with the same id and assert the result is
  `TaskError::Invalid` with the original id named in the message.

### [Suggested Test] Test that `phase1_mark_due_jobs` advances a `Every`
schedule's `last_run_at_ms` anchor correctly
- **Location**: `src/tasks/cron/service/concurrency.rs` (test module)
- The current tests cover the at-most-once guarantee for `Cron` and
  `At` jobs. A test for `Every` would close the loop on the
  `compute_next_every` / `apply_min_gap` interaction under repeated
  phases, and would catch any future drift in the
  `last_run_at_ms → anchor` flow.

## Cross-Module Findings

### [Warning] Cron alert dispatcher passes an empty `agent_id` placeholder
- **Modules**: `src/tasks/cron/executor.rs:130-148` (`build_cron_alert_dispatcher_fn`)
  → `src/tasks/shared/delivery.rs:107-118` (`DeliveryPayload.agent_id`)
- **Risk**: `DeliveryPayload.agent_id` is a required `String` (not
  `Option<String>`) so the alert dispatcher has to provide some value.
  Today it sends `String::new()` because no agent is involved in a
  failure alert. This is fine, but it leaks an implementation detail:
  if a future surface starts reading `agent_id` and treating empty as
  "no agent configured", the cron alert would be silently filtered.
- **Suggested fix**: Either make `agent_id` an `Option<String>` in
  `DeliveryPayload`, or document the empty-string contract in the alert
  path. Out of scope here (would touch `shared::delivery` and every
  caller).

## Wiring Audit Summary

- Total pub fns in `src/tasks/`: ~70 (counted across
  `shared/`, `cron/`, `heartbeat/`)
- Verified callers: ~65 (cross-checked against the graph at
  `graphify-out/2026-08-28/graph.json`)
- Orphaned pub fns: 0
  - All `pub fn`s in the timer loops are reachable through
    `service::ops`, `service::state`, or directly through
    `CronService` / `HeartbeatService`.
  - The reaper adapters (`CronHistoryReaper`, `CronSessionReaper`,
    `HeartbeatHistoryReaper`, `DedupHistoryReaper`) all have the
    daemon's wiring glue as their caller (per the graph and the
    `bin/aleph_server/commands/start` edges).
  - The two-tier writer check (`no_write_surface_assigns_the_chain_links_directly`)
    in the cron test module confirms `cron_manage` and the
    `cron.create` / `cron.update` RPCs both go through
    `CronJob::set_chain`.

## What was NOT reviewed

- The `start_server` wiring glue (`bin/aleph_server/commands/start`)
  that builds the timer loops — out of scope for this audit (module-only
  rule).
- The `cron_manage` tool (`src/builtin_tools/cron_manage.rs`) and the
  `cron/real` RPC handler — the source-level census inside cron already
  pins their chain-write paths.
- Loom tests for the reaper / carryover / dedup paths — none exist, and
  the suggested loom template (see the audit skill's Phase 5) would
  apply cleanly to `wake::WakeQueue` and `dedup::DedupEngine::check`'s
  history-load race.
- Performance of `phase3_writeback`'s whole-batch store lock — the
  writeback is bounded by O(jobs × results) and runs detached from the
  timer loop, so the next tick can dispatch fresh snapshots
  concurrently. Not measured; not flagged.