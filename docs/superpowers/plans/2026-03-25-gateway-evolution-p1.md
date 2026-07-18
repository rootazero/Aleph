# Gateway Evolution P1: IdempotencyGuard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add lock-free RPC idempotency guard to prevent duplicate execution when clients reconnect and resend requests.

**Architecture:** A `DashMap`-based guard with in-flight tracking sits between rate limiting and lane dispatch in the handler pipeline. Only Execute/Mutate/System lane methods are guarded. The idempotency key is an optional field extracted from the RPC request params.

**Tech Stack:** `dashmap`, `tokio::sync::watch`, `serde_json::Value`, `std::time::Instant`

**Spec:** `docs/superpowers/specs/2026-03-25-gateway-evolution-design.md` (Phase 1)

---

## File Map

| Action | File | Responsibility |
|--------|------|---------------|
| Create | `src/gateway/idempotency.rs` | IdempotencyGuard struct, CacheEntry enum, try_acquire/complete/prune |
| Modify | `src/gateway/mod.rs` | Add `pub mod idempotency;` |
| Modify | `src/gateway/server/mod.rs` | Add `idempotency_guard` field to GatewaySharedState |
| Modify | `src/gateway/server/handler.rs` | Insert idempotency check before lane dispatch |

---

### Task 1: Create IdempotencyGuard with tests

**Files:**
- Create: `src/gateway/idempotency.rs`

- [ ] **Step 1: Write the failing test skeleton**

Create the file with tests first, then the minimal struct to make them compile:

```rust
//! RPC idempotency guard for preventing duplicate request execution.
//!
//! Uses DashMap for lock-free concurrent access. Tracks both completed
//! results (with TTL) and in-flight requests (via watch channels).

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// Cache entry: in-flight or completed.
enum CacheEntry {
    /// Request is being processed; duplicates subscribe to this receiver.
    InFlight(watch::Sender<Option<Value>>),
    /// Result cached until TTL expires.
    Complete(Value, Instant),
}

/// Result of trying to acquire an idempotency slot.
pub enum AcquireResult {
    /// First request — caller should execute and call `complete()`.
    /// Wraps a drop guard that auto-discards if not explicitly completed.
    Proceed(IdempotencySlot),
    /// Cached result available — return immediately.
    Cached(Value),
    /// Another request with this key is in-flight — await the receiver.
    Waiting(watch::Receiver<Option<Value>>),
}

/// RAII guard for an acquired idempotency slot.
/// If dropped without calling `complete()`, automatically discards
/// the entry so the next request can retry (P7: Defensive Design).
pub struct IdempotencySlot {
    key: String,
    guard: Option<std::sync::Arc<DashMap<String, CacheEntry>>>,
}

impl IdempotencySlot {
    /// Mark this slot as completed with a result. Consumes the guard.
    pub fn complete(mut self, result: Value) {
        if let Some(cache) = self.guard.take() {
            // Atomically remove InFlight and notify waiters, then insert Complete
            if let Some((_, old)) = cache.remove(&self.key) {
                if let CacheEntry::InFlight(tx) = old {
                    let _ = tx.send(Some(result.clone()));
                }
            }
            cache.insert(self.key.clone(), CacheEntry::Complete(result, Instant::now()));
        }
    }

    /// Explicitly discard this slot (on error). Consumes the guard.
    pub fn discard(mut self) {
        self.do_discard();
    }

    fn do_discard(&mut self) {
        if let Some(cache) = self.guard.take() {
            if let Some((_, entry)) = cache.remove(&self.key) {
                if let CacheEntry::InFlight(tx) = entry {
                    let _ = tx.send(None);
                }
            }
        }
    }
}

impl Drop for IdempotencySlot {
    fn drop(&mut self) {
        // Auto-discard if not explicitly completed (e.g., panic)
        self.do_discard();
    }
}

/// Lock-free RPC idempotency guard with TTL-based expiry.
pub struct IdempotencyGuard {
    cache: std::sync::Arc<DashMap<String, CacheEntry>>,
    ttl: Duration,
}

impl IdempotencyGuard {
    /// Create a new guard with the given TTL for cached results.
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: std::sync::Arc::new(DashMap::new()),
            ttl,
        }
    }

    /// Try to acquire an idempotency slot for the given key.
    ///
    /// Uses `DashMap::entry()` for atomic check-and-insert to prevent
    /// TOCTOU race conditions between concurrent requests.
    pub fn try_acquire(&self, key: &str) -> AcquireResult {
        // First, check for existing non-expired entries without holding an entry lock
        // (entry() on an occupied key would block other readers of the same key)
        if let Some(entry) = self.cache.get(key) {
            match entry.value() {
                CacheEntry::Complete(value, inserted_at) => {
                    if inserted_at.elapsed() < self.ttl {
                        return AcquireResult::Cached(value.clone());
                    }
                    // Expired — drop ref, fall through to entry() below
                }
                CacheEntry::InFlight(tx) => {
                    return AcquireResult::Waiting(tx.subscribe());
                }
            }
            drop(entry); // Explicitly drop before entry() to avoid deadlock
        }

        // Atomic check-and-insert via entry() API
        match self.cache.entry(key.to_string()) {
            Entry::Occupied(e) => {
                // Another thread beat us — re-check the entry
                match e.get() {
                    CacheEntry::Complete(value, inserted_at) => {
                        if inserted_at.elapsed() < self.ttl {
                            return AcquireResult::Cached(value.clone());
                        }
                        // Still expired — replace with InFlight
                        drop(e);
                        let (tx, _rx) = watch::channel(None);
                        self.cache.insert(key.to_string(), CacheEntry::InFlight(tx));
                        AcquireResult::Proceed(IdempotencySlot {
                            key: key.to_string(),
                            guard: Some(self.cache.clone()),
                        })
                    }
                    CacheEntry::InFlight(tx) => {
                        AcquireResult::Waiting(tx.subscribe())
                    }
                }
            }
            Entry::Vacant(e) => {
                let (tx, _rx) = watch::channel(None);
                e.insert(CacheEntry::InFlight(tx));
                AcquireResult::Proceed(IdempotencySlot {
                    key: key.to_string(),
                    guard: Some(self.cache.clone()),
                })
            }
        }
    }

    /// Remove expired entries. Returns number of entries pruned.
    pub fn prune(&self) -> usize {
        let mut pruned = 0;
        self.cache.retain(|_, entry| {
            match entry {
                CacheEntry::Complete(_, inserted_at) => {
                    if inserted_at.elapsed() >= self.ttl {
                        pruned += 1;
                        false
                    } else {
                        true
                    }
                }
                CacheEntry::InFlight(_) => true, // Keep in-flight entries
            }
        });
        pruned
    }

    /// Number of entries currently in the cache.
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_request_proceeds() {
        let guard = IdempotencyGuard::new(Duration::from_secs(300));
        match guard.try_acquire("key-1") {
            AcquireResult::Proceed(_slot) => {} // expected
            _ => panic!("First request should proceed"),
        }
    }

    #[test]
    fn test_duplicate_gets_waiting() {
        let guard = IdempotencyGuard::new(Duration::from_secs(300));

        // First request — hold the slot to keep it InFlight
        let _slot = match guard.try_acquire("key-1") {
            AcquireResult::Proceed(slot) => slot,
            _ => panic!("First request should proceed"),
        };

        // Second request with same key — should wait
        match guard.try_acquire("key-1") {
            AcquireResult::Waiting(_) => {} // expected
            _ => panic!("Duplicate request should wait"),
        }
    }

    #[test]
    fn test_completed_returns_cached() {
        let guard = IdempotencyGuard::new(Duration::from_secs(300));

        // First request proceeds
        let slot = match guard.try_acquire("key-1") {
            AcquireResult::Proceed(slot) => slot,
            _ => panic!("First request should proceed"),
        };

        // Complete it via the slot
        slot.complete(serde_json::json!({"result": "ok"}));

        // Next request with same key — should get cached
        match guard.try_acquire("key-1") {
            AcquireResult::Cached(val) => {
                assert_eq!(val, serde_json::json!({"result": "ok"}));
            }
            _ => panic!("Should return cached result"),
        }
    }

    #[tokio::test]
    async fn test_waiter_receives_result() {
        let guard = std::sync::Arc::new(IdempotencyGuard::new(Duration::from_secs(300)));

        // First request proceeds
        let slot = match guard.try_acquire("key-1") {
            AcquireResult::Proceed(slot) => slot,
            _ => panic!("First request should proceed"),
        };

        // Second request waits
        let mut rx = match guard.try_acquire("key-1") {
            AcquireResult::Waiting(rx) => rx,
            _ => panic!("Should be waiting"),
        };

        // Complete from another "thread" via the slot
        tokio::spawn(async move {
            slot.complete(serde_json::json!(42));
        });

        // Waiter should receive the result
        rx.changed().await.unwrap();
        let val = rx.borrow().clone();
        assert_eq!(val, Some(serde_json::json!(42)));
    }

    #[test]
    fn test_discard_allows_retry() {
        let guard = IdempotencyGuard::new(Duration::from_secs(300));

        let slot = match guard.try_acquire("key-1") {
            AcquireResult::Proceed(slot) => slot,
            _ => panic!("Should proceed"),
        };

        // Explicitly discard (simulating an error)
        slot.discard();

        // Next request should proceed (not wait or return cached)
        match guard.try_acquire("key-1") {
            AcquireResult::Proceed(_) => {} // expected — can retry
            _ => panic!("After discard, should be able to proceed"),
        }
    }

    #[test]
    fn test_drop_guard_auto_discards() {
        let guard = IdempotencyGuard::new(Duration::from_secs(300));

        {
            let _slot = match guard.try_acquire("key-1") {
                AcquireResult::Proceed(slot) => slot,
                _ => panic!("Should proceed"),
            };
            // _slot dropped here without complete() — auto-discards
        }

        // Next request should proceed (auto-discarded)
        match guard.try_acquire("key-1") {
            AcquireResult::Proceed(_) => {} // expected
            _ => panic!("After auto-discard, should be able to proceed"),
        }
    }

    #[test]
    fn test_prune_removes_expired() {
        let guard = IdempotencyGuard::new(Duration::from_millis(1));

        let slot = match guard.try_acquire("key-1") {
            AcquireResult::Proceed(slot) => slot,
            _ => panic!("Should proceed"),
        };
        slot.complete(serde_json::json!("done"));

        // Wait for TTL to expire
        std::thread::sleep(Duration::from_millis(10));

        let pruned = guard.prune();
        assert_eq!(pruned, 1);
        assert!(guard.is_empty());
    }

    #[test]
    fn test_expired_entry_allows_new_request() {
        let guard = IdempotencyGuard::new(Duration::from_millis(1));

        let slot = match guard.try_acquire("key-1") {
            AcquireResult::Proceed(slot) => slot,
            _ => panic!("Should proceed"),
        };
        slot.complete(serde_json::json!("old"));

        // Wait for TTL to expire
        std::thread::sleep(Duration::from_millis(10));

        // New request should proceed (expired)
        match guard.try_acquire("key-1") {
            AcquireResult::Proceed(_) => {} // expected — expired entry replaced
            _ => panic!("Expired entry should allow new request"),
        }
    }

    #[test]
    fn test_different_keys_independent() {
        let guard = IdempotencyGuard::new(Duration::from_secs(300));

        match guard.try_acquire("key-1") {
            AcquireResult::Proceed(_) => {}
            _ => panic!("key-1 should proceed"),
        }

        match guard.try_acquire("key-2") {
            AcquireResult::Proceed(_) => {} // different key, independent
            _ => panic!("key-2 should proceed independently"),
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib idempotency -- --nocapture`
Expected: All 8 tests PASS

- [ ] **Step 3: Commit**

```bash
git add src/gateway/idempotency.rs
git commit -m "gateway: add IdempotencyGuard with lock-free in-flight tracking"
```

---

### Task 2: Register module and add to GatewaySharedState

**Files:**
- Modify: `src/gateway/mod.rs` — add `pub mod idempotency;`
- Modify: `src/gateway/server/mod.rs` — add field to GatewaySharedState

- [ ] **Step 1: Add module declaration**

In `src/gateway/mod.rs`, after `pub mod lane;` (line 75), add:

```rust
pub mod idempotency;
```

- [ ] **Step 2: Add field to GatewaySharedState**

In `src/gateway/server/mod.rs`, in `GatewaySharedState` struct (around line 83-97), add after `lane_manager`:

```rust
    pub idempotency_guard: Arc<crate::gateway::idempotency::IdempotencyGuard>,
```

- [ ] **Step 3: Add field to GatewayServer struct**

In `src/gateway/server/mod.rs`, in the `GatewayServer` struct (around line 138-166), add a field:

```rust
    pub idempotency_guard: Arc<crate::gateway::idempotency::IdempotencyGuard>,
```

- [ ] **Step 4: Initialize in both constructor paths**

In `GatewayServer::new()` and `GatewayServer::with_config()`, where `GatewayServer` struct is built (around lines 180-195 and 210-225), add:

```rust
            idempotency_guard: Arc::new(crate::gateway::idempotency::IdempotencyGuard::new(
                std::time::Duration::from_secs(300), // 5 minute TTL
            )),
```

Also in `build_router()` (around line 272-286), where `GatewaySharedState` is constructed from `self.*` fields, add:

```rust
            idempotency_guard: self.idempotency_guard.clone(),
```

- [ ] **Step 5: Add to ConnectionContext in handler.rs**

In `src/gateway/server/handler.rs`, add to `ConnectionContext` struct (around line 33-45):

```rust
    idempotency_guard: Arc<crate::gateway::idempotency::IdempotencyGuard>,
```

And in `ws_upgrade_handler` where `ConnectionContext` is constructed (around line 61-73):

```rust
            idempotency_guard: state.idempotency_guard.clone(),
```

- [ ] **Step 6: Add background prune task**

In `src/gateway/server/mod.rs`, in `spawn_background_tasks()` (around line 316), add after the rate limiter prune task:

```rust
        // Background: prune stale idempotency entries every 60s
        let ig = self.idempotency_guard.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                let pruned = ig.prune();
                if pruned > 0 {
                    debug!("Pruned {} expired idempotency entries", pruned);
                }
            }
        });
```

- [ ] **Step 7: Compile check**

Run: `cargo check -p alephcore`
Expected: Compiles without errors

- [ ] **Step 8: Commit**

```bash
git add src/gateway/mod.rs src/gateway/server/mod.rs src/gateway/server/handler.rs
git commit -m "gateway: wire IdempotencyGuard into GatewaySharedState and background tasks"
```

---

### Task 3: Integrate idempotency check into handler dispatch

**Files:**
- Modify: `src/gateway/server/handler.rs` — add check before lane dispatch
- Modify: `src/gateway/lane.rs` — add `needs_idempotency()` helper

- [ ] **Step 1: Add `needs_idempotency()` helper to Lane**

In `src/gateway/lane.rs`, add method to `Lane` impl (after `for_method()`):

```rust
    /// Whether this lane's methods should be idempotency-guarded.
    /// Query lane is read-only and doesn't need protection.
    pub fn needs_idempotency(&self) -> bool {
        !matches!(self, Lane::Query)
    }
```

- [ ] **Step 2: Add test for needs_idempotency**

In the test module of `lane.rs`:

```rust
    #[test]
    fn test_needs_idempotency() {
        assert!(!Lane::Query.needs_idempotency());
        assert!(Lane::Execute.needs_idempotency());
        assert!(Lane::Mutate.needs_idempotency());
        assert!(Lane::System.needs_idempotency());
    }
```

- [ ] **Step 3: Run lane tests**

Run: `cargo test -p alephcore --lib lane -- --nocapture`
Expected: All tests PASS

- [ ] **Step 4: Add idempotency check to handler dispatch**

In `src/gateway/server/handler.rs`, find the section where lane dispatch happens (around line 311-328). The current code:

```rust
                                    } else {
                                        // --- Lane concurrency control ---
                                        debug!("RPC dispatch: method={}", req.method);
                                        let lane_result = ctx.lane_manager.acquire(&req.method).await;
                                        let response = match lane_result {
                                            Ok(_permit) => {
                                                let resp = process_request(&text, &ctx.handlers).await;
                                                // permit drops here, releasing the lane slot
                                                resp
                                            }
```

Replace the entire `} else {` block (from line 311 `// --- Lane concurrency control ---` through line 328) with the complete idempotency-aware version below. This replaces the original lane dispatch; all closing braces are included:

```rust
                                    } else {
                                        // --- Idempotency + Lane concurrency control ---
                                        debug!("RPC dispatch: method={}", req.method);

                                        // Extract idempotency_key from params (optional)
                                        let idempotency_key = req.params
                                            .as_ref()
                                            .and_then(|p| p.get("idempotency_key"))
                                            .and_then(|v| v.as_str())
                                            .map(String::from);

                                        let lane = crate::gateway::lane::Lane::for_method(&req.method);

                                        // Helper closure: standard lane dispatch (no idempotency)
                                        let do_lane_dispatch = |text: String, handlers: Arc<HandlerRegistry>, lm: Arc<LaneManager>, method: String, req_id: Option<Value>| async move {
                                            let lane_result = lm.acquire(&method).await;
                                            match lane_result {
                                                Ok(_permit) => process_request(&text, &handlers).await,
                                                Err(_) => serde_json::to_string(&JsonRpcResponse::error(
                                                    req_id,
                                                    INTERNAL_ERROR,
                                                    "Service congested, try again later",
                                                )).unwrap_or_default()
                                            }
                                        };

                                        // Check idempotency guard (only for non-Query lanes with a key)
                                        if let Some(ref key) = idempotency_key {
                                            if lane.needs_idempotency() {
                                                use crate::gateway::idempotency::AcquireResult;
                                                match ctx.idempotency_guard.try_acquire(key) {
                                                    AcquireResult::Cached(cached) => {
                                                        debug!("Idempotency hit: key={}", key);
                                                        let resp = JsonRpcResponse::success(req.id.clone(), cached);
                                                        serde_json::to_string(&resp).unwrap_or_default()
                                                    }
                                                    AcquireResult::Waiting(mut rx) => {
                                                        debug!("Idempotency: awaiting in-flight key={}", key);
                                                        let result = tokio::time::timeout(
                                                            std::time::Duration::from_secs(30),
                                                            async {
                                                                let _ = rx.changed().await;
                                                                rx.borrow().clone()
                                                            }
                                                        ).await;
                                                        match result {
                                                            Ok(Some(val)) => {
                                                                let resp = JsonRpcResponse::success(req.id.clone(), val);
                                                                serde_json::to_string(&resp).unwrap_or_default()
                                                            }
                                                            _ => {
                                                                serde_json::to_string(&JsonRpcResponse::error(
                                                                    req.id.clone(),
                                                                    INTERNAL_ERROR,
                                                                    "Request timed out waiting for in-flight duplicate",
                                                                )).unwrap_or_default()
                                                            }
                                                        }
                                                    }
                                                    AcquireResult::Proceed(slot) => {
                                                        // First request — slot auto-discards on panic (RAII)
                                                        let lane_result = ctx.lane_manager.acquire(&req.method).await;
                                                        match lane_result {
                                                            Ok(_permit) => {
                                                                let resp = process_request(&text, &ctx.handlers).await;
                                                                if let Ok(parsed) = serde_json::from_str::<JsonRpcResponse>(&resp) {
                                                                    if parsed.is_success() {
                                                                        if let Some(result) = parsed.result {
                                                                            slot.complete(result);
                                                                        } else {
                                                                            slot.discard();
                                                                        }
                                                                    } else {
                                                                        slot.discard(); // Error — let next request retry
                                                                    }
                                                                } else {
                                                                    slot.discard();
                                                                }
                                                                resp
                                                            }
                                                            Err(_) => {
                                                                slot.discard();
                                                                serde_json::to_string(&JsonRpcResponse::error(
                                                                    req.id.clone(),
                                                                    INTERNAL_ERROR,
                                                                    "Service congested, try again later",
                                                                )).unwrap_or_default()
                                                            }
                                                        }
                                                    }
                                                }
                                            } else {
                                                // Query lane — skip idempotency
                                                do_lane_dispatch(text, ctx.handlers.clone(), ctx.lane_manager.clone(), req.method.clone(), req.id.clone()).await
                                            }
                                        } else {
                                            // No idempotency key — standard lane dispatch
                                            do_lane_dispatch(text, ctx.handlers.clone(), ctx.lane_manager.clone(), req.method.clone(), req.id.clone()).await
                                        }
                                        // --- End idempotency + lane block ---
```

Note: The code after this block (the `connect` method guest_session_id extraction at the original line 330+) remains unchanged.

- [ ] **Step 5: Compile check**

Run: `cargo check -p alephcore`
Expected: Compiles without errors

- [ ] **Step 6: Run all existing tests to ensure no regression**

Run: `cargo test -p alephcore --lib -- --nocapture`
Expected: All existing tests still pass

- [ ] **Step 7: Commit**

```bash
git add src/gateway/server/handler.rs src/gateway/lane.rs
git commit -m "gateway: integrate IdempotencyGuard into RPC handler dispatch pipeline"
```

---

### Task 4: Final validation

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -W clippy::all`
Expected: No new warnings

- [ ] **Step 3: Run compile check for the whole workspace**

Run: `cargo check`
Expected: Clean compile

- [ ] **Step 4: Final commit if any clippy fixes needed**

```bash
git add -A && git commit -m "gateway: fix clippy warnings in idempotency module"
```
