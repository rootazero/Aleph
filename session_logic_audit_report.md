# Logic Review Report
**Module**: src/session
**Scope**: Full static review of all 12 source files (actor, driver, events, in_process, ingress_safety, mod, projection, service, shim, state, store, tool_trace)
**Date**: 2026-05-22
**Mode**: strict

## Findings

### [Critical] Unsafe i64/u64 type coercion in SqliteEventStore
- **Location**: `src/session/store.rs:162, 197-198, 225, 247, 283`
- **Trigger condition**: `EventSeq` (alias for `u64`) values exceed `i64::MAX` (~9.2e18) when cast via `as i64` for SQLite storage
- **Expected behavior**: Graceful error on overflow instead of silent truncation
- **Actual behavior**: `seq as i64` silently truncates large values, corrupting ordering and primary-key uniqueness
- **Suggested fix**: Replace all `as i64` casts with `i64::try_from()` and propagate `SessionError::Storage` on overflow. Similarly, load paths should validate non-negative before casting back to `u64`.
- **Status**: FIXED

### [Critical] Race condition in subscribe slow path
- **Location**: `src/session/in_process.rs:155-164`
- **Trigger condition**: Actor spawned via `spawn_actor` crashes during replay or idle-times out between spawn and `broadcasters.read().await`
- **Expected behavior**: Subscriber receives a valid, live broadcast receiver
- **Actual behavior**: Subscriber may receive a receiver whose sender is permanently dead (actor gone), causing silent event loss
- **Suggested fix**: After `spawn_actor`, verify actor liveness via `sender_for` before handing out the broadcaster.
- **Status**: FIXED

### [Warning] Error context silently discarded on actor communication failure
- **Location**: `src/session/in_process.rs:121-122, 138-139, 193-194`
- **Risk**: Production debugging is impossible when every mpsc/oneshot failure is mapped to the same `ActorShutdown` variant with zero context
- **Current impact**: Medium — observable via incomplete logs, hard to root-cause
- **Suggestion**: Log the underlying `SendError` / `RecvError` at `warn!` level before mapping to `ActorShutdown`
- **Status**: FIXED

### [Warning] Shutdown timeout results discarded without logging
- **Location**: `src/session/in_process.rs:171, 206`
- **Risk**: Actor may still be running when a new one is spawned for the same session, leading to duplicate actors and potential primary-key conflicts on SQLite
- **Current impact**: Medium — `tokio::time::timeout` result completely ignored
- **Suggestion**: Match on `timeout(...).await` and log `warn!` for both timeout and reply-drop cases
- **Status**: FIXED

### [Warning] Unbounded recursion in collect_string_values
- **Location**: `src/session/ingress_safety.rs:267-277`
- **Risk**: Maliciously crafted deeply-nested JSON (e.g. 10,000 nested arrays) causes stack overflow
- **Current impact**: Low — input originates from tool dispatch, but still a DoS vector
- **Suggestion**: Cap recursion depth at a reasonable limit (64) and truncate
- **Status**: FIXED

### [Warning] Session state HashMaps grow without bound
- **Location**: `src/session/in_process.rs:25-27`
- **Risk**: Long-running server with many transient sessions leaks memory because `senders` and `broadcasters` are only cleaned on explicit `wake`/`detach`
- **Current impact**: Low — idle actors self-terminate, but HashMap entries remain forever
- **Suggestion**: Add periodic background sweep or LRU eviction for stale entries
- **Status**: NOT FIXED (architectural concern, deferred to future refactoring)

### [Suggested Test] Concurrent append stress test
```rust
#[tokio::test]
async fn concurrent_appends_produce_unique_seqs() {
    let store = make_store();
    let sid = sample_session_id();
    let mut handles = vec![];
    for i in 0..100 {
        let store = store.clone();
        let sid = sid.clone();
        handles.push(tokio::spawn(async move {
            let e = turn_started(uuid::Uuid::new_v4(), now_ms() + i);
            store.append(&sid, i + 1, &e, now_ms() + i).await
        }));
    }
    for h in handles {
        h.await.unwrap().unwrap();
    }
    let events = store.load_all_events(&sid).await.unwrap();
    let seqs: Vec<u64> = events.iter().map(|r| r.seq).collect();
    assert_eq!(seqs.len(), 100);
    assert_eq!(seqs, (1..=100).collect::<Vec<u64>>());
}
```

### [Suggested Test] Actor idle timeout + reattach
```rust
#[tokio::test]
async fn actor_idle_timeout_allows_clean_reattach() {
    let store = test_store().await;
    let id = sample_id();
    let svc = InProcessActorSessionService::new(store)
        .with_idle_timeout(Duration::from_millis(50));
    svc.attach(id.clone()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    // Actor should have timed out; reattach must create a new one
    let h2 = svc.attach(id.clone()).await.unwrap();
    assert_eq!(h2.head_seq, 0);
}
```

### [Suggested Test] Safety guard deep JSON recursion limit
```rust
#[test]
fn collect_string_values_respects_depth_cap() {
    let guard = SafetyGuard::default_guard();
    let deep_json = (0..200).fold(json!("bottom"), |acc, _| json!([acc]));
    let call = ToolCall {
        name: "test".into(),
        input: deep_json,
    };
    // Should not panic or stack overflow
    let _ = guard.check(&call);
}
```

## Summary
| Level | Count |
|-------|-------|
| Critical | 2 |
| Warning | 4 (1 deferred) |
| Suggested Test | 3 |

## Fixes Applied
1. `store.rs`: Replaced all `as i64`/`as u64` casts with checked `try_from` conversions
2. `in_process.rs`: Added `sender_for` liveness check after `spawn_actor` in `subscribe`
3. `in_process.rs`: Added `tracing::warn!` logs before discarding actor communication errors
4. `in_process.rs`: Added structured logging for `wake`/`detach` shutdown timeouts
5. `ingress_safety.rs`: Added `MAX_DEPTH = 64` cap to `collect_string_values` recursion
