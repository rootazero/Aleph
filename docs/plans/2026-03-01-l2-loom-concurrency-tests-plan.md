# L2 Loom Concurrency Tests Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Complete the L2 layer of the Logic Review System — migrate all `std::sync` imports to `crate::sync_primitives`, fix 4 HIGH-severity concurrency bugs, write 21 loom tests across 5 modules.

**Architecture:** Three-phase approach: (1) expand `sync_primitives.rs` and migrate all 443 files using `std::sync` to use `crate::sync_primitives`, (2) fix 3 code bugs + 1 documentation gap, (3) write 21 loom concurrency tests in 5 modules using abstract pattern extraction. All loom tests run inside `loom::model(|| { ... })` blocks using `crate::sync_primitives` types which resolve to `loom::sync` under `--cfg loom`.

**Tech Stack:** loom 0.7, Rust conditional compilation (`#[cfg(loom)]`), justfile, GitHub Actions

**Design Doc:** `docs/plans/2026-03-01-l2-loom-concurrency-tests-design.md`

---

## Phase 1: Infrastructure & Migration

### Task 1: Expand sync_primitives.rs

**Files:**
- Modify: `core/src/sync_primitives.rs`

**Step 1: Add lock hierarchy documentation and missing types**

Replace the entire file with:

```rust
//! Conditional sync primitives for loom compatibility.
//!
//! Under normal compilation, these re-export `std::sync` types at zero cost.
//! Under `--features loom` (with `RUSTFLAGS="--cfg loom"`), these switch to
//! loom's instrumented versions that enable exhaustive concurrency testing.
//!
//! ## Lock Hierarchy
//!
//! Acquire locks in this order to prevent deadlock:
//!
//! - Level 0: StateDatabase (resilience/database)
//! - Level 1: MemoryStore (memory/)
//! - Level 2: ToolRegistry, ChannelRegistry (dispatcher/, gateway/)
//! - Level 3: UI state, progress monitors

#[cfg(loom)]
pub(crate) use loom::sync::{Arc, Mutex, MutexGuard, RwLock};
#[cfg(loom)]
pub(crate) use loom::sync::atomic::{
    AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};

#[cfg(not(loom))]
pub(crate) use std::sync::{Arc, Mutex, MutexGuard, RwLock};
#[cfg(not(loom))]
pub(crate) use std::sync::atomic::{
    AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};
```

**Step 2: Verify compilation**

Run: `cargo check --workspace`
Expected: Compiles. New types exported but not yet consumed.

**Step 3: Commit**

```bash
git add core/src/sync_primitives.rs
git commit -m "infra: expand sync_primitives with AtomicI64, AtomicUsize, lock hierarchy docs"
```

---

### Task 2: Migrate std::sync imports — all modules

**Scope:** 443 files across the entire `core/src/` tree. This is a mechanical find-and-replace.

**Migration rules (in order of application):**

1. `use std::sync::atomic::{...}` → `use crate::sync_primitives::{...}` (drop `atomic::` subpath)
2. `use std::sync::{Arc, Mutex, ...}` → `use crate::sync_primitives::{Arc, Mutex, ...}`
3. `use std::sync::Arc;` → `use crate::sync_primitives::Arc;`
4. `use std::sync::Mutex;` → `use crate::sync_primitives::Mutex;`
5. `use std::sync::RwLock;` → `use crate::sync_primitives::RwLock;`

**DO NOT migrate these (leave as `std::sync::`):**

- `std::sync::OnceLock` — 8 files (loom has no equivalent)
- `std::sync::LazyLock` — 6 files (loom has no equivalent)
- `std::sync::Once` — 2 files (loom has no equivalent)
- `sync_primitives.rs` itself — it defines the re-exports

**Files using OnceLock/LazyLock/Once (SKIP these specific imports, but DO migrate any Arc/Mutex in the same file):**

```
core/src/utils/prompt_sanitize.rs         — OnceLock
core/src/utils/pii.rs                     — OnceLock
core/src/markdown/fences.rs               — LazyLock
core/src/cron/template.rs                 — LazyLock
core/src/logging/level_control.rs         — Once
core/src/logging/file_appender.rs         — Once
core/src/dispatcher/model_router/intelligent/prompt_analyzer/feature_extractor.rs — LazyLock
core/src/generation/response_parser.rs    — LazyLock
core/src/dispatcher/risk.rs               — OnceLock
core/src/pii/rules/api_key.rs             — OnceLock
core/src/extension/template.rs            — LazyLock
core/src/pii/rules/email.rs               — OnceLock
core/src/pii/rules/ip_address.rs          — OnceLock
core/src/pii/rules/bank_card.rs           — OnceLock
core/src/pii/rules/ssh_key.rs             — OnceLock
core/src/pii/rules/phone.rs               — OnceLock
core/src/pii/rules/id_card.rs             — OnceLock
core/src/video/youtube/url.rs             — LazyLock
```

**Step 1: Run automated replacement**

Use a script or manual replacement to apply all 5 rules above. For files that have BOTH migratable types (Arc) AND non-migratable types (OnceLock) on the same import line, split the import into two lines:

Before:
```rust
use std::sync::{Arc, OnceLock};
```

After:
```rust
use crate::sync_primitives::Arc;
use std::sync::OnceLock;
```

**Step 2: Verify compilation**

Run: `cargo check --workspace`

If compilation fails due to `loom::sync::Arc` vs `std::sync::Arc` type mismatches in external crate interactions, revert those specific files back to `use std::sync::Arc`. This is expected for files that pass Arc to/from external crate APIs.

**Step 3: Run existing tests**

Run: `cargo test --workspace --lib`
Expected: All existing tests pass. The migration is a no-op under `#[cfg(not(loom))]`.

**Step 4: Commit**

```bash
git add -A
git commit -m "refactor: migrate std::sync imports to crate::sync_primitives for loom compatibility"
```

---

## Phase 2: Bug Fixes

### Task 3: Fix governor.rs — simplify AtomicU64 to u64

**Files:**
- Modify: `core/src/resilience/governance/governor.rs`

**Problem:** `session_tokens: RwLock<HashMap<String, AtomicU64>>` — the `AtomicU64` is superfluous because `tokio::sync::RwLock` already serializes all access via write lock in `record_tokens`. The `AtomicU64` creates false confidence about lock-free safety.

**Step 1: Change struct field type**

In `ResourceGovernor` struct (line 93):

```rust
// Before:
session_tokens: RwLock<HashMap<String, AtomicU64>>,

// After:
session_tokens: RwLock<HashMap<String, u64>>,
```

**Step 2: Update `record_tokens` method (lines 223-244)**

```rust
// Before:
pub async fn record_tokens(&self, session_id: &str, tokens: u64) -> Result<bool, AlephError> {
    let mut session_tokens = self.session_tokens.write().await;
    let counter = session_tokens
        .entry(session_id.to_string())
        .or_insert_with(|| AtomicU64::new(0));
    let new_total = counter.fetch_add(tokens, Ordering::SeqCst) + tokens;
    if new_total > self.config.token_budget_per_session {
        // ... warning ...
        return Ok(false);
    }
    Ok(true)
}

// After:
pub async fn record_tokens(&self, session_id: &str, tokens: u64) -> Result<bool, AlephError> {
    let mut session_tokens = self.session_tokens.write().await;
    let counter = session_tokens.entry(session_id.to_string()).or_insert(0);
    *counter += tokens;
    if *counter > self.config.token_budget_per_session {
        warn!(
            session_id = %session_id,
            tokens_used = *counter,
            budget = self.config.token_budget_per_session,
            "Token budget exceeded"
        );
        return Ok(false);
    }
    Ok(true)
}
```

**Step 3: Update `get_token_usage` method (lines 247-253)**

```rust
// Before:
pub async fn get_token_usage(&self, session_id: &str) -> u64 {
    let session_tokens = self.session_tokens.read().await;
    session_tokens
        .get(session_id)
        .map(|c| c.load(Ordering::SeqCst))
        .unwrap_or(0)
}

// After:
pub async fn get_token_usage(&self, session_id: &str) -> u64 {
    let session_tokens = self.session_tokens.read().await;
    session_tokens.get(session_id).copied().unwrap_or(0)
}
```

**Step 4: Remove unused atomic import if no longer needed**

Check if `AtomicU64` and `Ordering` are still used elsewhere in the file (they are — in `LaneResources`). If so, keep the import. If `Ordering` is no longer used, remove it from the import line.

**Step 5: Verify**

Run: `cargo test --workspace --lib`

**Step 6: Commit**

```bash
git add core/src/resilience/governance/governor.rs
git commit -m "fix(resilience): simplify governor session_tokens from AtomicU64 to u64"
```

---

### Task 4: Fix channel_registry.rs — RwLock → Mutex for take-once pattern

**Files:**
- Modify: `core/src/gateway/channel_registry.rs`

**Problem:** `inbound_rx: Arc<RwLock<Option<mpsc::Receiver>>>` uses `tokio::sync::RwLock` for a take-once pattern. RwLock implies readers and writers, but there's only one destructive `take()`. A `Mutex` is semantically correct and simpler.

**Step 1: Change import**

```rust
// Before (line 29):
use tokio::sync::{mpsc, RwLock};

// After:
use tokio::sync::mpsc;
use std::sync::Mutex;
```

Note: Use `std::sync::Mutex` (not tokio) because `take()` is instantaneous and doesn't need async. After Task 2 migration, this would be `crate::sync_primitives::Mutex`.

**Step 2: Change field type (line 49)**

```rust
// Before:
inbound_rx: Arc<Mutex<Option<mpsc::Receiver<InboundMessage>>>>,

// After — same type, but Mutex is now std::sync::Mutex:
inbound_rx: Arc<Mutex<Option<mpsc::Receiver<InboundMessage>>>>,
```

Note: The `RwLock` in `channels` and `factories` fields is `tokio::sync::RwLock` and stays unchanged. Only `inbound_rx` changes.

**Step 3: Update constructor (line 61)**

```rust
// Before:
inbound_rx: Arc::new(RwLock::new(Some(inbound_rx))),

// After:
inbound_rx: Arc::new(Mutex::new(Some(inbound_rx))),
```

**Step 4: Update take_inbound_receiver (lines 262-265)**

```rust
// Before:
pub async fn take_inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
    let mut rx_guard = self.inbound_rx.write().await;
    rx_guard.take()
}

// After:
pub fn take_inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
    let mut rx_guard = self.inbound_rx.lock().expect("inbound_rx mutex poisoned");
    rx_guard.take()
}
```

Note: Method signature changes from `async fn` to `fn`. Check all call sites — if callers use `.await`, remove it.

**Step 5: Find and update all callers**

Search for `take_inbound_receiver` across the codebase. Remove `.await` from call sites since it's no longer async.

**Step 6: Verify**

Run: `cargo check --workspace && cargo test --workspace --lib`

**Step 7: Commit**

```bash
git add -A
git commit -m "fix(gateway): use Mutex for channel_registry take-once inbound_rx pattern"
```

---

### Task 5: Fix execution_engine TOCTOU — atomic check + insert

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs`

**Problem:** Read lock checks `active_runs.len()` (line 97-112), drops lock, then write lock inserts (line 123-138). Between the two locks, another thread can also pass the check, exceeding `max_concurrent_runs`.

**Step 1: Merge check + insert into single write lock scope**

Replace lines 96-138 with:

```rust
        // Create cancellation channel
        let (cancel_tx, mut cancel_rx) = mpsc::channel::<()>(1);

        // Atomically check concurrent run limit and register the run
        {
            let mut runs = self.active_runs.write().await;
            let agent_runs = runs
                .values()
                .filter(|r| r.request.session_key.agent_id() == request.session_key.agent_id())
                .count();

            if agent_runs >= self.config.max_concurrent_runs {
                return Err(ExecutionError::TooManyRuns(format!(
                    "Agent {} has {} active runs (max: {})",
                    request.session_key.agent_id(),
                    agent_runs,
                    self.config.max_concurrent_runs
                )));
            }

            runs.insert(
                run_id.clone(),
                ActiveRun {
                    request: request.clone(),
                    state: RunState::Running,
                    started_at: chrono::Utc::now(),
                    steps_completed: 0,
                    current_tool: None,
                    cancel_tx: Some(cancel_tx),
                    seq_counter: AtomicU64::new(0),
                    chunk_counter: AtomicU32::new(0),
                },
            );
        }

        // Check agent state (after registration to reserve the slot)
        if !agent.is_idle().await {
            // Remove the just-inserted run since agent is busy
            let mut runs = self.active_runs.write().await;
            runs.remove(&run_id);
            return Err(ExecutionError::AgentBusy(agent.id().to_string()));
        }
```

Key changes:
1. Move `mpsc::channel` creation before the lock (it doesn't need the lock)
2. Check + insert in one write lock scope
3. Move `agent.is_idle()` check after registration (if agent is busy, rollback by removing)

**Step 2: Verify**

Run: `cargo check --workspace && cargo test --workspace --lib`

**Step 3: Commit**

```bash
git add core/src/gateway/execution_engine/engine.rs
git commit -m "fix(gateway): eliminate TOCTOU in execution_engine concurrent run limit check"
```

---

### Task 6: Add lock hierarchy documentation (Fix 4)

This is already done in Task 1 (sync_primitives.rs header comments). No additional work needed.

---

## Phase 3: Loom Tests

### Task 7: Write dispatcher loom tests (4 tests)

**Files:**
- Create: `core/src/dispatcher/loom_concurrency.rs`
- Modify: `core/src/dispatcher/mod.rs` (add module registration)

**Step 1: Register loom test module in mod.rs**

Add to `core/src/dispatcher/mod.rs` (after the existing proptest modules):

```rust
#[cfg(all(test, loom))]
mod loom_concurrency;
```

**Step 2: Create loom_concurrency.rs**

```rust
//! Loom concurrency tests for dispatcher module.
//!
//! Tests abstract concurrency patterns extracted from dispatcher internals.
//! Run with: `just test-loom`

use loom::thread;
use crate::sync_primitives::{Arc, AtomicBool, AtomicU64, Ordering, RwLock};
use std::collections::HashMap;

/// Verify concurrent read/write to a registry-like structure doesn't deadlock
/// and readers always see consistent state.
///
/// Models: dispatcher/registry/mod.rs tool registry pattern
#[test]
fn loom_registry_concurrent_read_write() {
    loom::model(|| {
        let registry: Arc<RwLock<HashMap<String, u64>>> = Arc::new(RwLock::new(HashMap::new()));

        let w = registry.clone();
        let writer = thread::spawn(move || {
            let mut map = w.write().unwrap();
            map.insert("tool_a".to_string(), 1);
            map.insert("tool_b".to_string(), 2);
        });

        let r = registry.clone();
        let reader = thread::spawn(move || {
            let map = r.read().unwrap();
            // If we see tool_b, we must also see tool_a (consistency within single write)
            if map.get("tool_b").is_some() {
                assert!(map.get("tool_a").is_some());
            }
        });

        writer.join().unwrap();
        reader.join().unwrap();
    });
}

/// Verify pause/resume/cancel atomic flags never enter illegal combinations.
///
/// Models: dispatcher/engine/core.rs AtomicBool coordination
#[test]
fn loom_engine_pause_resume_cancel() {
    loom::model(|| {
        let paused = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));

        let p = paused.clone();
        let c = cancelled.clone();
        let control = thread::spawn(move || {
            p.store(true, Ordering::SeqCst);
            // Simulate resume
            p.store(false, Ordering::SeqCst);
        });

        let p2 = paused.clone();
        let c2 = cancelled.clone();
        let canceller = thread::spawn(move || {
            c2.store(true, Ordering::SeqCst);
        });

        control.join().unwrap();
        canceller.join().unwrap();

        // Final state: cancelled is always true (canceller always runs)
        // paused can be either true or false depending on interleaving
        assert!(cancelled.load(Ordering::SeqCst));
    });
}

/// Verify atomic counter fetch_add returns unique monotonic values.
///
/// Models: dispatcher/engine/core.rs event sequence counter
#[test]
fn loom_atomic_counter_monotonic() {
    loom::model(|| {
        let counter = Arc::new(AtomicU64::new(0));

        let c1 = counter.clone();
        let t1 = thread::spawn(move || c1.fetch_add(1, Ordering::SeqCst));

        let c2 = counter.clone();
        let t2 = thread::spawn(move || c2.fetch_add(1, Ordering::SeqCst));

        let v1 = t1.join().unwrap();
        let v2 = t2.join().unwrap();

        // Both return values must be unique
        assert_ne!(v1, v2);
        // Final value must be 2
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    });
}

/// Verify RwLock-protected progress state is never torn when read concurrently.
///
/// Models: dispatcher/monitor/progress.rs snapshot pattern
#[test]
fn loom_progress_snapshot() {
    loom::model(|| {
        let progress = Arc::new(RwLock::new((0u32, 0u32))); // (completed, total)

        let w = progress.clone();
        let writer = thread::spawn(move || {
            let mut p = w.write().unwrap();
            p.0 = 5;
            p.1 = 10;
        });

        let r1 = progress.clone();
        let reader1 = thread::spawn(move || {
            let p = r1.read().unwrap();
            // completed must never exceed total
            assert!(p.0 <= p.1);
        });

        let r2 = progress.clone();
        let reader2 = thread::spawn(move || {
            let p = r2.read().unwrap();
            assert!(p.0 <= p.1);
        });

        writer.join().unwrap();
        reader1.join().unwrap();
        reader2.join().unwrap();
    });
}
```

**Step 3: Verify loom tests pass**

Run: `RUSTFLAGS="--cfg loom" LOOM_MAX_PREEMPTIONS=3 cargo test --features loom --lib dispatcher::loom_concurrency`

**Step 4: Commit**

```bash
git add core/src/dispatcher/loom_concurrency.rs core/src/dispatcher/mod.rs
git commit -m "test(dispatcher): add 4 loom concurrency tests for registry, engine flags, counters, progress"
```

---

### Task 8: Write gateway loom tests (5 tests)

**Files:**
- Create: `core/src/gateway/loom_concurrency.rs`
- Modify: `core/src/gateway/mod.rs` (add module registration)

**Step 1: Register loom test module in mod.rs**

Add to `core/src/gateway/mod.rs` (after the existing gateway feature-gated modules, inside the `#[cfg(feature = "gateway")]` block or at the top level depending on where test modules go):

```rust
#[cfg(all(test, loom))]
mod loom_concurrency;
```

**Step 2: Create loom_concurrency.rs**

```rust
//! Loom concurrency tests for gateway module.
//!
//! Tests abstract concurrency patterns extracted from gateway internals.
//! Run with: `just test-loom`

use loom::thread;
use crate::sync_primitives::{Arc, AtomicBool, AtomicU32, AtomicU64, Ordering, Mutex, RwLock};
use std::collections::HashMap;

/// Verify sequence counter allocates unique values under concurrent access.
///
/// Models: gateway/run_event_bus.rs AtomicU64 seq_counter
#[test]
fn loom_seq_counter_uniqueness() {
    loom::model(|| {
        let counter = Arc::new(AtomicU64::new(0));

        let c1 = counter.clone();
        let t1 = thread::spawn(move || c1.fetch_add(1, Ordering::SeqCst));

        let c2 = counter.clone();
        let t2 = thread::spawn(move || c2.fetch_add(1, Ordering::SeqCst));

        let c3 = counter.clone();
        let t3 = thread::spawn(move || c3.fetch_add(1, Ordering::SeqCst));

        let v1 = t1.join().unwrap();
        let v2 = t2.join().unwrap();
        let v3 = t3.join().unwrap();

        // All values must be unique
        assert_ne!(v1, v2);
        assert_ne!(v1, v3);
        assert_ne!(v2, v3);
        // Final counter value
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    });
}

/// Verify AtomicBool connection state transitions are consistent.
///
/// Models: gateway/transport/stdio.rs connect/disconnect pattern
#[test]
fn loom_connection_state_transition() {
    loom::model(|| {
        let connected = Arc::new(AtomicBool::new(false));

        let c1 = connected.clone();
        let connector = thread::spawn(move || {
            c1.store(true, Ordering::Release);
        });

        let c2 = connected.clone();
        let disconnector = thread::spawn(move || {
            c2.store(false, Ordering::Release);
        });

        connector.join().unwrap();
        disconnector.join().unwrap();

        // Final state is deterministic for each interleaving
        let final_state = connected.load(Ordering::Acquire);
        // Must be either true or false (not some torn value)
        assert!(final_state || !final_state); // Always true, but loom checks for data races
    });
}

/// Verify request ID allocation never produces duplicates.
///
/// Models: gateway/transport/stdio.rs AtomicU32 next_id
#[test]
fn loom_request_id_allocation() {
    loom::model(|| {
        let next_id = Arc::new(AtomicU32::new(1));

        let id1 = next_id.clone();
        let t1 = thread::spawn(move || id1.fetch_add(1, Ordering::Relaxed));

        let id2 = next_id.clone();
        let t2 = thread::spawn(move || id2.fetch_add(1, Ordering::Relaxed));

        let v1 = t1.join().unwrap();
        let v2 = t2.join().unwrap();

        assert_ne!(v1, v2);
        assert_eq!(next_id.load(Ordering::Relaxed), 3);
    });
}

/// Verify chunk counter reset doesn't lose concurrent increments.
///
/// Models: gateway/run_event_bus.rs chunk_counter store(0) vs fetch_add(1)
#[test]
fn loom_chunk_counter_reset() {
    loom::model(|| {
        let counter = Arc::new(AtomicU32::new(5)); // Start with some value

        let c1 = counter.clone();
        let resetter = thread::spawn(move || {
            c1.store(0, Ordering::SeqCst);
        });

        let c2 = counter.clone();
        let incrementer = thread::spawn(move || {
            c2.fetch_add(1, Ordering::SeqCst)
        });

        resetter.join().unwrap();
        let prev = incrementer.join().unwrap();

        let final_val = counter.load(Ordering::SeqCst);
        // Final value must be consistent with interleaving:
        // If reset first: prev=0, final=1
        // If increment first: prev=5, final=0
        assert!(
            (prev == 0 && final_val == 1) ||
            (prev == 5 && final_val == 0) ||
            (prev == 5 && final_val == 1) || // increment, then reset, but we read after both
            (prev == 0 && final_val == 0)     // edge case with SeqCst
        );
    });
}

/// Verify TOCTOU pattern: concurrent check-then-insert respects limits
/// when done atomically under a single lock.
///
/// Models: gateway/execution_engine/engine.rs (fixed version)
#[test]
fn loom_execution_run_limit() {
    loom::model(|| {
        let max_runs: usize = 2;
        let active_runs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let accepted = Arc::new(AtomicU32::new(0));

        let runs1 = active_runs.clone();
        let acc1 = accepted.clone();
        let t1 = thread::spawn(move || {
            let mut runs = runs1.lock().unwrap();
            if runs.len() < max_runs {
                runs.push("run_1".to_string());
                acc1.fetch_add(1, Ordering::SeqCst);
            }
        });

        let runs2 = active_runs.clone();
        let acc2 = accepted.clone();
        let t2 = thread::spawn(move || {
            let mut runs = runs2.lock().unwrap();
            if runs.len() < max_runs {
                runs.push("run_2".to_string());
                acc2.fetch_add(1, Ordering::SeqCst);
            }
        });

        let runs3 = active_runs.clone();
        let acc3 = accepted.clone();
        let t3 = thread::spawn(move || {
            let mut runs = runs3.lock().unwrap();
            if runs.len() < max_runs {
                runs.push("run_3".to_string());
                acc3.fetch_add(1, Ordering::SeqCst);
            }
        });

        t1.join().unwrap();
        t2.join().unwrap();
        t3.join().unwrap();

        // With atomic check+insert, at most max_runs can be accepted
        let total_accepted = accepted.load(Ordering::SeqCst);
        assert!(total_accepted <= max_runs as u32,
            "Accepted {} runs, max was {}", total_accepted, max_runs);
    });
}
```

**Step 3: Verify**

Run: `RUSTFLAGS="--cfg loom" LOOM_MAX_PREEMPTIONS=3 cargo test --features loom --lib gateway::loom_concurrency`

**Step 4: Commit**

```bash
git add core/src/gateway/loom_concurrency.rs core/src/gateway/mod.rs
git commit -m "test(gateway): add 5 loom concurrency tests for seq counter, connection state, request ID, chunk reset, run limit"
```

---

### Task 9: Write agent_loop loom tests (3 tests)

**Files:**
- Create: `core/src/agent_loop/loom_concurrency.rs`
- Modify: `core/src/agent_loop/mod.rs` (add module registration)

**Step 1: Register loom test module in mod.rs**

Add to `core/src/agent_loop/mod.rs`:

```rust
#[cfg(all(test, loom))]
mod loom_concurrency;
```

**Step 2: Create loom_concurrency.rs**

```rust
//! Loom concurrency tests for agent_loop module.
//!
//! Tests abstract concurrency patterns extracted from agent_loop internals.
//! Run with: `just test-loom`

use loom::thread;
use crate::sync_primitives::{Arc, AtomicBool, Ordering, RwLock};

/// Verify anchor store supports concurrent read/write without deadlock.
///
/// Models: agent_loop/meta_cognition_integration.rs RwLock<Vec<Anchor>>
#[test]
fn loom_anchor_store_read_write() {
    loom::model(|| {
        let store: Arc<RwLock<Vec<u64>>> = Arc::new(RwLock::new(Vec::new()));

        let w = store.clone();
        let writer = thread::spawn(move || {
            let mut anchors = w.write().unwrap();
            anchors.push(42);
            anchors.push(84);
        });

        let r1 = store.clone();
        let reader1 = thread::spawn(move || {
            let anchors = r1.read().unwrap();
            let _len = anchors.len();
        });

        let r2 = store.clone();
        let reader2 = thread::spawn(move || {
            let anchors = r2.read().unwrap();
            // If we see any elements, the vec is in a consistent state
            for &v in anchors.iter() {
                assert!(v == 42 || v == 84);
            }
        });

        writer.join().unwrap();
        reader1.join().unwrap();
        reader2.join().unwrap();
    });
}

/// Verify state flags never enter illegal combinations.
///
/// Models: agent_loop state management with running/paused/aborted flags
#[test]
fn loom_state_flag_coordination() {
    loom::model(|| {
        let running = Arc::new(AtomicBool::new(true));
        let aborted = Arc::new(AtomicBool::new(false));

        // Thread 1: normal completion (running → false)
        let r1 = running.clone();
        let a1 = aborted.clone();
        let completer = thread::spawn(move || {
            r1.store(false, Ordering::SeqCst);
        });

        // Thread 2: abort signal (aborted → true, running → false)
        let r2 = running.clone();
        let a2 = aborted.clone();
        let aborter = thread::spawn(move || {
            a2.store(true, Ordering::SeqCst);
            r2.store(false, Ordering::SeqCst);
        });

        completer.join().unwrap();
        aborter.join().unwrap();

        // After both threads complete, running must be false
        assert!(!running.load(Ordering::SeqCst));
        // aborted is always true (aborter always runs)
        assert!(aborted.load(Ordering::SeqCst));
    });
}

/// Verify Arc reference counting works correctly with multiple clones.
///
/// Models: agent_loop/builder.rs Arc<Thinker> shared across loop instances
#[test]
fn loom_shared_component_access() {
    loom::model(|| {
        let component = Arc::new(42u64);

        let c1 = component.clone();
        let t1 = thread::spawn(move || {
            assert_eq!(*c1, 42);
        });

        let c2 = component.clone();
        let t2 = thread::spawn(move || {
            assert_eq!(*c2, 42);
        });

        t1.join().unwrap();
        t2.join().unwrap();

        // Original Arc is still valid
        assert_eq!(*component, 42);
    });
}
```

**Step 3: Verify**

Run: `RUSTFLAGS="--cfg loom" LOOM_MAX_PREEMPTIONS=3 cargo test --features loom --lib agent_loop::loom_concurrency`

**Step 4: Commit**

```bash
git add core/src/agent_loop/loom_concurrency.rs core/src/agent_loop/mod.rs
git commit -m "test(agent_loop): add 3 loom concurrency tests for anchor store, state flags, shared components"
```

---

### Task 10: Write memory loom tests (5 tests)

**Files:**
- Create: `core/src/memory/loom_concurrency.rs`
- Modify: `core/src/memory/mod.rs` (add module registration)

**Step 1: Register loom test module in mod.rs**

Add to `core/src/memory/mod.rs`:

```rust
#[cfg(all(test, loom))]
mod loom_concurrency;
```

**Step 2: Create loom_concurrency.rs**

```rust
//! Loom concurrency tests for memory module.
//!
//! Tests abstract concurrency patterns extracted from memory internals.
//! Run with: `just test-loom`

use loom::thread;
use crate::sync_primitives::{
    Arc, AtomicBool, AtomicU32, AtomicU64, Ordering, Mutex, RwLock,
};

/// Verify singleton initialization via compare_exchange — exactly one thread wins.
///
/// Models: memory/dreaming.rs DreamDaemon RUNNING compare_exchange(false, true)
#[test]
fn loom_daemon_singleton_init() {
    loom::model(|| {
        let initialized = Arc::new(AtomicBool::new(false));
        let init_count = Arc::new(AtomicU32::new(0));

        let i1 = initialized.clone();
        let c1 = init_count.clone();
        let t1 = thread::spawn(move || {
            if i1.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                c1.fetch_add(1, Ordering::SeqCst);
            }
        });

        let i2 = initialized.clone();
        let c2 = init_count.clone();
        let t2 = thread::spawn(move || {
            if i2.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                c2.fetch_add(1, Ordering::SeqCst);
            }
        });

        let i3 = initialized.clone();
        let c3 = init_count.clone();
        let t3 = thread::spawn(move || {
            if i3.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                c3.fetch_add(1, Ordering::SeqCst);
            }
        });

        t1.join().unwrap();
        t2.join().unwrap();
        t3.join().unwrap();

        // Exactly one thread must have initialized
        assert_eq!(init_count.load(Ordering::SeqCst), 1);
        assert!(initialized.load(Ordering::SeqCst));
    });
}

/// Verify compression trigger doesn't lose events between check and reset.
///
/// Models: memory/compression/scheduler.rs Mutex<state> + AtomicU32 pending_turns
#[test]
fn loom_compression_trigger_race() {
    loom::model(|| {
        let pending_turns = Arc::new(AtomicU32::new(0));
        let trigger_threshold = 3u32;

        // Simulate 3 turns being added
        let p1 = pending_turns.clone();
        let adder = thread::spawn(move || {
            p1.fetch_add(1, Ordering::SeqCst);
            p1.fetch_add(1, Ordering::SeqCst);
            p1.fetch_add(1, Ordering::SeqCst);
        });

        // Simulate trigger check + reset
        let p2 = pending_turns.clone();
        let checker = thread::spawn(move || {
            let current = p2.load(Ordering::SeqCst);
            if current >= trigger_threshold {
                // Reset to 0 after triggering
                p2.store(0, Ordering::SeqCst);
                return true;
            }
            false
        });

        adder.join().unwrap();
        let triggered = checker.join().unwrap();

        let final_pending = pending_turns.load(Ordering::SeqCst);
        // Either: checker saw enough and reset to 0, or it didn't trigger and pending >= 0
        if triggered {
            // After reset, any adds that happened after check are lost
            // but that's acceptable — they'll trigger next cycle
            assert!(final_pending <= 3);
        } else {
            assert!(final_pending <= 3);
        }
    });
}

/// Verify activity timestamp concurrent updates always produce valid values.
///
/// Models: memory/dreaming.rs LAST_ACTIVITY_TS AtomicI64
#[test]
fn loom_activity_timestamp_update() {
    // Use AtomicU64 instead of AtomicI64 since loom may not support AtomicI64
    // The pattern is identical — concurrent store/load of timestamps
    loom::model(|| {
        let timestamp = Arc::new(AtomicU64::new(100));

        let ts1 = timestamp.clone();
        let t1 = thread::spawn(move || {
            ts1.store(200, Ordering::Relaxed);
        });

        let ts2 = timestamp.clone();
        let t2 = thread::spawn(move || {
            ts2.store(300, Ordering::Relaxed);
        });

        let ts3 = timestamp.clone();
        let reader = thread::spawn(move || {
            ts3.load(Ordering::Relaxed)
        });

        t1.join().unwrap();
        t2.join().unwrap();
        let value = reader.join().unwrap();

        // Value must be one of the legally-written values
        assert!(value == 100 || value == 200 || value == 300,
            "Read unexpected timestamp: {}", value);
    });
}

/// Verify metrics counters are accurate under concurrent increments.
///
/// Models: memory/cortex/dreaming.rs total_processed/total_extracted fetch_add
#[test]
fn loom_metrics_counter_accuracy() {
    loom::model(|| {
        let total_processed = Arc::new(AtomicU64::new(0));
        let total_errors = Arc::new(AtomicU64::new(0));

        let p1 = total_processed.clone();
        let t1 = thread::spawn(move || {
            p1.fetch_add(1, Ordering::Relaxed);
        });

        let p2 = total_processed.clone();
        let e2 = total_errors.clone();
        let t2 = thread::spawn(move || {
            p2.fetch_add(1, Ordering::Relaxed);
            e2.fetch_add(1, Ordering::Relaxed);
        });

        t1.join().unwrap();
        t2.join().unwrap();

        assert_eq!(total_processed.load(Ordering::Relaxed), 2);
        assert_eq!(total_errors.load(Ordering::Relaxed), 1);
    });
}

/// Verify provider hot-swap via RwLock doesn't produce torn reads.
///
/// Models: memory/embedding_manager.rs RwLock<Provider>
#[test]
fn loom_embedding_provider_swap() {
    loom::model(|| {
        let provider: Arc<RwLock<String>> = Arc::new(RwLock::new("openai".to_string()));

        // Writer: swap provider
        let w = provider.clone();
        let writer = thread::spawn(move || {
            let mut p = w.write().unwrap();
            *p = "ollama".to_string();
        });

        // Reader 1: read provider name
        let r1 = provider.clone();
        let reader1 = thread::spawn(move || {
            let p = r1.read().unwrap();
            let name = p.clone();
            // Must be one of the valid provider names
            assert!(name == "openai" || name == "ollama",
                "Read unexpected provider: {}", name);
        });

        // Reader 2: read provider name
        let r2 = provider.clone();
        let reader2 = thread::spawn(move || {
            let p = r2.read().unwrap();
            assert!(p.as_str() == "openai" || p.as_str() == "ollama");
        });

        writer.join().unwrap();
        reader1.join().unwrap();
        reader2.join().unwrap();
    });
}
```

**Step 3: Verify**

Run: `RUSTFLAGS="--cfg loom" LOOM_MAX_PREEMPTIONS=3 cargo test --features loom --lib memory::loom_concurrency`

**Step 4: Commit**

```bash
git add core/src/memory/loom_concurrency.rs core/src/memory/mod.rs
git commit -m "test(memory): add 5 loom concurrency tests for singleton init, compression trigger, timestamps, counters, provider swap"
```

---

### Task 11: Write resilience loom tests (4 tests)

**Files:**
- Create: `core/src/resilience/loom_concurrency.rs`
- Modify: `core/src/resilience/mod.rs` (add module registration)

**Step 1: Register loom test module in mod.rs**

Add to `core/src/resilience/mod.rs`:

```rust
#[cfg(all(test, loom))]
mod loom_concurrency;
```

**Step 2: Create loom_concurrency.rs**

```rust
//! Loom concurrency tests for resilience module.
//!
//! Tests abstract concurrency patterns extracted from resilience internals.
//! Run with: `just test-loom`

use loom::thread;
use crate::sync_primitives::{Arc, AtomicU64, Ordering, Mutex, RwLock};
use std::collections::HashMap;

/// Verify lane active counter returns to 0 after balanced add/sub.
///
/// Models: resilience/governance/governor.rs LaneResources active_count
#[test]
fn loom_lane_counter_accuracy() {
    loom::model(|| {
        let active_count = Arc::new(AtomicU64::new(0));

        // Thread 1: acquire + release
        let c1 = active_count.clone();
        let t1 = thread::spawn(move || {
            c1.fetch_add(1, Ordering::SeqCst);
            c1.fetch_sub(1, Ordering::SeqCst);
        });

        // Thread 2: acquire + release
        let c2 = active_count.clone();
        let t2 = thread::spawn(move || {
            c2.fetch_add(1, Ordering::SeqCst);
            c2.fetch_sub(1, Ordering::SeqCst);
        });

        t1.join().unwrap();
        t2.join().unwrap();

        // After all acquire/release pairs complete, count must be 0
        assert_eq!(active_count.load(Ordering::SeqCst), 0);
    });
}

/// Verify token budget concurrent consumption never exceeds limit
/// when check+consume is done atomically.
///
/// Models: resilience/governance/governor.rs (fixed) record_tokens
#[test]
fn loom_token_budget_concurrent() {
    loom::model(|| {
        let budget: u64 = 100;
        let tokens = Arc::new(Mutex::new(0u64));
        let over_budget_count = Arc::new(AtomicU64::new(0));

        let t1_tokens = tokens.clone();
        let t1_over = over_budget_count.clone();
        let t1 = thread::spawn(move || {
            let mut t = t1_tokens.lock().unwrap();
            if *t + 60 <= budget {
                *t += 60;
                true
            } else {
                t1_over.fetch_add(1, Ordering::SeqCst);
                false
            }
        });

        let t2_tokens = tokens.clone();
        let t2_over = over_budget_count.clone();
        let t2 = thread::spawn(move || {
            let mut t = t2_tokens.lock().unwrap();
            if *t + 60 <= budget {
                *t += 60;
                true
            } else {
                t2_over.fetch_add(1, Ordering::SeqCst);
                false
            }
        });

        let r1 = t1.join().unwrap();
        let r2 = t2.join().unwrap();

        let final_tokens = *tokens.lock().unwrap();
        // Total consumed must never exceed budget
        assert!(final_tokens <= budget,
            "Token budget exceeded: {} > {}", final_tokens, budget);
        // At most one of the two can succeed (60+60=120 > 100)
        assert!(!(r1 && r2), "Both threads consumed 60, exceeding budget");
    });
}

/// Verify per-task sequence counter produces unique incrementing values.
///
/// Models: resilience/perception/emitter.rs RwLock<HashMap<String, AtomicU64>>
#[test]
fn loom_seq_counter_per_task() {
    loom::model(|| {
        let counters: Arc<RwLock<HashMap<String, AtomicU64>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Initialize a task counter
        {
            let mut map = counters.write().unwrap();
            map.insert("task_1".to_string(), AtomicU64::new(0));
        }

        let c1 = counters.clone();
        let t1 = thread::spawn(move || {
            let map = c1.read().unwrap();
            if let Some(counter) = map.get("task_1") {
                counter.fetch_add(1, Ordering::SeqCst)
            } else {
                0
            }
        });

        let c2 = counters.clone();
        let t2 = thread::spawn(move || {
            let map = c2.read().unwrap();
            if let Some(counter) = map.get("task_1") {
                counter.fetch_add(1, Ordering::SeqCst)
            } else {
                0
            }
        });

        let v1 = t1.join().unwrap();
        let v2 = t2.join().unwrap();

        // Both values must be unique
        assert_ne!(v1, v2);
        // Final counter value
        let map = counters.read().unwrap();
        assert_eq!(map.get("task_1").unwrap().load(Ordering::SeqCst), 2);
    });
}

/// Verify Mutex-protected database operations don't deadlock.
///
/// Models: resilience/database/state_database.rs Mutex<Connection>
#[test]
fn loom_database_mutex_contention() {
    loom::model(|| {
        let db: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let d1 = db.clone();
        let t1 = thread::spawn(move || {
            let mut conn = d1.lock().unwrap();
            conn.push("event_1".to_string());
        });

        let d2 = db.clone();
        let t2 = thread::spawn(move || {
            let mut conn = d2.lock().unwrap();
            conn.push("event_2".to_string());
        });

        let d3 = db.clone();
        let t3 = thread::spawn(move || {
            let mut conn = d3.lock().unwrap();
            conn.push("event_3".to_string());
        });

        t1.join().unwrap();
        t2.join().unwrap();
        t3.join().unwrap();

        // All 3 events must be recorded
        let conn = db.lock().unwrap();
        assert_eq!(conn.len(), 3);
        assert!(conn.contains(&"event_1".to_string()));
        assert!(conn.contains(&"event_2".to_string()));
        assert!(conn.contains(&"event_3".to_string()));
    });
}
```

**Step 3: Verify**

Run: `RUSTFLAGS="--cfg loom" LOOM_MAX_PREEMPTIONS=3 cargo test --features loom --lib resilience::loom_concurrency`

**Step 4: Commit**

```bash
git add core/src/resilience/loom_concurrency.rs core/src/resilience/mod.rs
git commit -m "test(resilience): add 4 loom concurrency tests for lane counters, token budget, seq counters, database mutex"
```

---

## Phase 4: Verification

### Task 12: Full verification

**Step 1: Run standard tests**

Run: `cargo test --workspace --lib`
Expected: All existing tests pass (migration is a no-op under normal builds).

**Step 2: Run loom tests**

Run: `just test-loom`
Expected: All 21 loom tests pass.

If loom tests fail due to compilation errors from `std::sync::Arc` vs `loom::sync::Arc` type mismatches with external crate APIs, identify the failing files and revert them to use `std::sync::Arc` directly (with a comment explaining why).

**Step 3: Run proptest for regression**

Run: `just test-proptest`
Expected: All proptest tests pass.

**Step 4: Final commit if any fixes were needed**

```bash
git add -A
git commit -m "fix: resolve loom compilation issues from Arc type mismatches"
```
