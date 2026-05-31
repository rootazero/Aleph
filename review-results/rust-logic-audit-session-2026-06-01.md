# Logic Review Report
**Module**: session
**Scope**: Full static review of `src/session/` (13 files, ~4,087 LOC)
**Date**: 2026-06-01
**Mode**: strict

## Findings

### [Critical] `subscribe` fast path returns stale broadcaster after actor idle-timeout
- **Location**: `src/session/in_process.rs:159-163`
- **Trigger condition**: Actor idle-times out (default 30 min); its `mpsc::Sender` becomes closed, but the `broadcast::Sender` remains in `self.broadcasters`. A subsequent `subscribe` call that hits the fast path returns a `Receiver` on the dead channel — it will never see new events.
- **Expected behavior**: `subscribe` should verify the actor is alive before returning a broadcaster from the cache.
- **Actual behavior**: Fast path only checks `broadcasters.get(id)`, ignoring whether the matching `sender` is closed.
- **Suggested fix**: In the fast path, also read `self.senders` and confirm `!sender.is_closed()` before returning the cached broadcaster.

### [Warning] `collect_string_values` allows unbounded output growth
- **Location**: `src/session/ingress_safety.rs:263-282`
- **Risk**: A malicious or accidentally huge JSON payload (e.g. multi-MB string) causes the `haystack` to grow without limit, leading to excessive memory use during safety checks.
- **Current impact**: medium
- **Suggestion**: Cap total accumulated length (e.g. 10 KB) and truncate.

### [Warning] `collect_string_values` off-by-one depth limit
- **Location**: `src/session/ingress_safety.rs:267`
- **Risk**: `depth > MAX_DEPTH` permits recursion to depth 65 instead of the intended 64, slightly enlarging the attack surface for deeply-nested JSON payloads.
- **Current impact**: low
- **Suggestion**: Change to `depth >= MAX_DEPTH`.

### [Warning] `load_events_range` from-seq silent overflow
- **Location**: `src/session/store.rs:283`
- **Risk**: `i64::try_from(from.unwrap_or(0)).unwrap_or(0)` silently falls back to `0` when `from > i64::MAX`. The query then returns the entire event log instead of the intended slice.
- **Current impact**: low (practically unreachable with normal seq values)
- **Suggestion**: Propagate an explicit error when `from` exceeds `i64::MAX`.

### [Warning] Lock poisoning via `.unwrap()` in test code
- **Location**: `src/session/tool_trace.rs:361`, `402`
- **Risk**: If a test thread panics while holding the `Mutex`, subsequent `.lock().unwrap()` cascades the panic.
- **Current impact**: low (test-only)
- **Suggestion**: Use `.lock().unwrap_or_else(|e| e.into_inner())` to recover the data regardless of poisoning state.

### [Warning] `subscribe` post-spawn sender race
- **Location**: `src/session/in_process.rs:169-171`
- **Risk**: Between `spawn_actor` returning and `sender_for` checking, the actor may crash during replay (e.g. corrupt DB). `subscribe` then returns `ActorShutdown`, which callers may not expect.
- **Current impact**: low
- **Suggestion**: Consider retrying `spawn_actor` once before returning `ActorShutdown`.

## Suggested Tests

### [Suggested Test] Stale-broadcaster skip
```rust
#[tokio::test]
async fn subscribe_skips_stale_broadcaster_after_idle_timeout() {
    let svc = fresh_service().await;
    let id = sample_id("stale-bcast");
    svc.attach(id.clone()).await.unwrap();

    // Force actor death by setting idle timeout to 0 and waiting.
    // (Requires making idle timeout configurable in tests.)
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Actor is dead, but broadcaster may still be in the map.
    let mut rx = svc.subscribe(&id).await.unwrap();

    // Re-attach / emit should work with a fresh actor.
    svc.emit_event(&id, SessionEvent::SessionWoken { at: now_ms(), prior_head: 0 }).await.unwrap();

    let record = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(record.event, SessionEvent::SessionWoken { .. }));
}
```

### [Suggested Test] `collect_string_values` depth and length limits
```rust
#[test]
fn collect_string_values_respects_depth_and_length_limits() {
    let deep = json!({"a": {"a": {"a": {"a": "deep"}}}});
    let mut out = String::new();
    collect_string_values(&deep, &mut out);
    assert!(out.len() <= 10_000 + 64); // rough cap
}
```

## Summary
| Level | Count |
|-------|-------|
| Critical | 1 |
| Warning | 5 |
| Suggested Test | 2 |

## Cross-Module Findings
None identified.
