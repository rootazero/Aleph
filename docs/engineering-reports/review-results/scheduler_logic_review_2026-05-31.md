# Logic Review Report
**Module**: scheduler (session_scheduler + compression_scheduler + memory_producer_scheduler)
**Scope**: Full static review of all scheduler modules under `src/`
**Date**: 2026-05-31
**Mode**: strict

## Findings

### [Critical] SessionScheduler queue memory leak
- **Location**: `src/gateway/session_scheduler.rs:347-374`
- **Trigger condition**: Any session that receives messages will have its `SessionQueue` permanently retained in the `HashMap`, even after all tasks complete and the queue is empty.
- **Expected behavior**: Empty idle queues should be removed to prevent unbounded memory growth.
- **Actual behavior**: `queues.entry().or_insert_with(SessionQueue::new)` creates queues but they are never removed. Over time, especially with many unique session keys, this causes unbounded memory growth.
- **Suggested fix**: In `drain_queue`, after `pop_front()` returns `None` and the queue is idle, remove the entry from the HashMap.

### [Critical] Potential deadlock in SchedulerEventListener::emit
- **Location**: `src/gateway/session_scheduler.rs:405-425`
- **Trigger condition**: If `inner.emit()` acquires any lock (e.g., in `ReplyEmitter`) and then `on_run_finished()` acquires `queues` Mutex, while another code path acquires `queues` first then the same lock — classic lock-order inversion deadlock.
- **Expected behavior**: Terminal event handling should not risk deadlocking with the inner emitter.
- **Actual behavior**: `self.on_run_finished().await` is called synchronously after `inner.emit()`, potentially holding lock ordering that conflicts with other tasks.
- **Suggested fix**: Spawn `drain_queue` into a separate `tokio::task` so it runs outside the `emit()` call stack, eliminating the possibility of lock order inversion with `inner.emit`.

### [Warning] CompressionScheduler u32 overflow in turn counter
- **Location**: `src/memory/compression/scheduler.rs:128-129`
- **Risk**: `fetch_add` wraps around on u32 overflow. After ~4.3 billion increments, `pending_turns` wraps to 0, causing `should_trigger_compression()` to falsely return `None` until enough new turns accumulate.
- **Current impact**: Low (requires ~4 billion turns, unlikely in practice), but violates the "turns only increase" invariant.
- **Suggestion**: Use `fetch_update` with `saturating_add` to clamp at `u32::MAX` instead of wrapping.

### [Warning] MemoryProducerScheduler tick_counter uses Relaxed ordering
- **Location**: `src/memory/extensions/scheduler.rs:59`
- **Risk**: `tick_counter` uses `Ordering::Relaxed` for `fetch_add`. While currently safe (only used as a monotonic counter with no synchronization semantics), future code might rely on it for happens-before relationships.
- **Current impact**: Low
- **Suggestion**: Change to `Ordering::SeqCst` or document the intentional use of Relaxed.

### [Warning] QueueDepthFuture design pattern
- **Location**: `src/gateway/session_scheduler.rs:287-301`
- **Risk**: `queue_depth()` returns a future-like object but `get()` consumes `self`, making it single-use. This is non-idiomatic for Rust async code.
- **Current impact**: Low
- **Suggestion**: Consider making `queue_depth` async or returning a direct value, as the current abstraction adds complexity without clear benefit.

### [Suggested Test] Session queue memory leak regression
```rust
#[tokio::test]
async fn test_queue_cleanup_on_empty() {
    // Create scheduler, enqueue a message, let it complete,
    // then verify the queue is removed from the HashMap.
}
```

### [Suggested Test] SchedulerEventListener deadlock scenario
```rust
#[tokio::test]
async fn test_emit_does_not_deadlock_with_inner_lock() {
    // Mock an EventEmitter that acquires a lock,
    // then verify SchedulerEventListener::emit completes without deadlock
    // when RunComplete is emitted.
}
```

### [Suggested Test] CompressionScheduler saturating overflow
```rust
#[test]
fn test_turn_counter_saturates_at_max() {
    let scheduler = CompressionScheduler::with_defaults();
    scheduler.increment_turns_by(u32::MAX);
    assert_eq!(scheduler.get_pending_turns(), u32::MAX);
    scheduler.increment_turns_by(1);
    assert_eq!(scheduler.get_pending_turns(), u32::MAX); // should saturate, not wrap
}
```

## Summary
| Level | Count |
|-------|-------|
| Critical | 2 |
| Warning | 3 |
| Suggested Test | 3 |

## Fixes Applied
1. **Queue leak**: Added `queues.remove(session_key_str)` when queue is empty and idle.
2. **Deadlock risk**: Replaced synchronous `on_run_finished()` call with `tokio::spawn(drain_queue(...))`.
3. **u32 overflow**: Changed `increment_turns_by` to use `fetch_update` + `saturating_add`.
4. **Dead code**: Removed unused `SchedulerEventListener::on_run_finished` method.

## Cross-Module Findings
- None identified — schedulers are independent subsystems with no shared state or lock hierarchies across module boundaries.
