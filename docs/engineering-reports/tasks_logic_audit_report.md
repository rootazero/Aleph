# Logic Review Report
**Module**: src/tasks
**Scope**: Full static review of cron/, heartbeat/, shared/ submodules (36 .rs files)
**Date**: 2026-05-22
**Mode**: strict

## Findings

### [Critical] i64 underflow in stale-marker detection (cron catchup)
- **Location**: `src/tasks/cron/service/catchup.rs:93`
- **Trigger condition**: System clock goes backwards (NTP sync, manual adjustment) while a job has `running_at_ms` set
- **Expected behavior**: Stale marker check should gracefully handle negative time deltas
- **Actual behavior**: `now - running_at` panics in debug mode or wraps in release mode when `now < running_at`
- **Suggested fix**: Use `now.saturating_sub(running_at) > stale_threshold`

### [Critical] i64 underflow in stale-marker detection (heartbeat timer)
- **Location**: `src/tasks/heartbeat/service/timer.rs:141`
- **Trigger condition**: System clock goes backwards while a task has `running_at_ms` set
- **Expected behavior**: Stale marker check should gracefully handle negative time deltas
- **Actual behavior**: `now_ms - running_at` panics in debug mode or wraps in release mode
- **Suggested fix**: Use `now_ms.saturating_sub(running_at) > stale_threshold_ms`

### [Critical] i64 underflow in alert cooldown check
- **Location**: `src/tasks/cron/alert.rs:13`
- **Trigger condition**: System clock goes backwards after `last_failure_alert_at_ms` is set
- **Expected behavior**: Cooldown check should handle clock adjustments gracefully
- **Actual behavior**: `now_ms - last_alert` panics in debug mode or wraps in release mode
- **Suggested fix**: Use `now_ms.saturating_sub(last_alert) < alert_config.cooldown_ms`

### [Critical] Heartbeat timer bypasses Clock abstraction
- **Location**: `src/tasks/heartbeat/service/timer.rs:135,167,333`
- **Trigger condition**: Any heartbeat timer tick
- **Expected behavior**: All time-dependent code uses the `Clock` trait for testability
- **Actual behavior**: `clear_stale_running_markers`, `collect_due_tasks`, and `writeback_one` call `chrono::Utc::now().timestamp_millis()` directly, making the code impossible to test deterministically and violating the project's Clock abstraction invariant
- **Suggested fix**: Thread a `Clock` implementation through `HeartbeatServiceState` (mirroring `ServiceState<C: Clock>`) and use `clock.now_ms()` throughout

### [Warning] Cron schedule computation silently fails on out-of-range timestamps
- **Location**: `src/tasks/cron/service/ops.rs:37`
- **Risk**: A job's next_run_at_ms becomes `None` if `now_ms` is out of `DateTime` range, effectively disabling the job without any error logged
- **Current impact**: Low (requires pathological clock values)
- **Suggestion**: Log a warning when `from_timestamp_millis` returns None

### [Warning] Heartbeat L2 status mapping silently swallows unknown statuses
- **Location**: `src/tasks/heartbeat/service/timer.rs:346-353`
- **Risk**: If a new L2 status string is added, it maps to `None` instead of producing an error or default
- **Current impact**: Medium (could hide new status types during development)
- **Suggestion**: Add a tracing::warn! for the `_ => None` branch

### [Warning] Heartbeat history cleanup uses direct system time
- **Location**: `src/tasks/heartbeat/history.rs:142-145`
- **Risk**: Inconsistent with Clock abstraction; makes cleanup functions untestable
- **Current impact**: Low
- **Suggestion**: Accept a `now_ms` parameter instead of reading system time

### [Warning] DedupEngine noop constructor can panic
- **Location**: `src/tasks/heartbeat/dedup.rs:92-99`
- **Risk**: If `Connection::open_in_memory()` fails twice in a row, the code panics
- **Current impact**: Very low (SQLite in-memory open is extremely reliable)
- **Suggestion**: Return a Result or use a static no-op implementation instead of nested unwraps

### [Warning] Potential i64 overflow in heartbeat next_due computation
- **Location**: `src/tasks/heartbeat/service/timer.rs:367-368`
- **Risk**: `now_ms + interval + backoff` could overflow i64 with pathological values
- **Current impact**: Very low
- **Suggestion**: Use `saturating_add` for the computation

### [Warning] Template engine silently ignores invalid context_vars JSON
- **Location**: `src/tasks/cron/template.rs:56-69`
- **Risk**: Malformed `context_vars` JSON is silently ignored; user may not realize their variables aren't being substituted
- **Current impact**: Low
- **Suggestion**: Log a warning when `serde_json::from_str` fails

### [Suggested Test] Clock backward adjustment regression
```rust
#[test]
fn stale_marker_handles_clock_backwards() {
    let clock = FakeClock::new(1_000_000);
    let mut task = make_test_task("stale");
    task.state.running_at_ms = Some(2_000_000); // "running" in the future
    
    // Should not panic or behave incorrectly
    let stale_threshold_ms = 7_200_000;
    let now_ms = clock.now_ms();
    let is_stale = now_ms.saturating_sub(task.state.running_at_ms.unwrap()) > stale_threshold_ms;
    assert!(!is_stale);
}
```

### [Suggested Test] Alert cooldown with clock adjustment
```rust
#[test]
fn alert_cooldown_handles_clock_backwards() {
    let mut job = make_test_job("test");
    job.state.consecutive_errors = 5;
    job.state.last_failure_alert_at_ms = Some(2_000_000);
    
    let config = make_alert_config();
    let now_ms = 1_000_000; // Clock went backwards
    
    // Should not panic
    let should_alert = should_send_alert(&job, &config, now_ms);
    assert!(should_alert.is_some()); // Cooldown check passed due to backwards clock
}
```

## Summary
| Level | Count |
|-------|-------|
| Critical | 4 |
| Warning | 6 |
| Suggested Test | 2 |

## Cross-Module Findings

### [Critical] Time arithmetic underflow pattern across cron and heartbeat
- **Modules**: `cron/service/catchup.rs`, `heartbeat/service/timer.rs`, `cron/alert.rs`
- **Risk**: All three locations use raw `i64` subtraction for time-delta checks without saturation, making them vulnerable to clock backwards adjustments
- **Suggested fix**: Audit all `now - previous` patterns in the tasks module and replace with `saturating_sub`

## Batch Summary
| Module | Critical | Warning | Suggested Test |
|--------|----------|---------|----------------|
| cron | 2 | 2 | 0 |
| heartbeat | 2 | 4 | 0 |
| shared | 0 | 0 | 0 |
| **Cross-module** | **1** | **0** | **0** |
| **Total** | **4** | **6** | **2** |
