# L2 Loom Concurrency Tests Design

> Date: 2026-03-01
> Status: Approved
> Parent: [Logic Review System Design](2026-02-28-logic-review-system-design.md)

## Overview

Complete the L2 layer of the Logic Review System: full `std::sync` import migration to `crate::sync_primitives`, 21 loom concurrency tests across 5 modules, and 4 HIGH-severity concurrency bug fixes.

## Scope

### What loom CAN test

- `std::sync::{Arc, Mutex, MutexGuard, RwLock}` — lock contention, deadlock detection
- `std::sync::atomic::{AtomicU64, AtomicU32, AtomicBool, Ordering}` — ordering correctness, lost updates

### What loom CANNOT test

- `tokio::sync::{Mutex, RwLock, mpsc, broadcast, watch, Semaphore}` — async runtime types
- `std::sync::{Once, OnceLock}` — API mismatch with loom equivalents
- Third-party library internals

## Part 1: Import Migration

### Migration Rule

| Original | Replacement | Notes |
|----------|-------------|-------|
| `use std::sync::Arc` | `use crate::sync_primitives::Arc` | |
| `use std::sync::{Mutex, MutexGuard, RwLock}` | `use crate::sync_primitives::*` | |
| `use std::sync::atomic::{AtomicU64, ...}` | `use crate::sync_primitives::*` | |
| `use tokio::sync::*` | **No change** | loom does not support tokio::sync |

### sync_primitives.rs Expansion

Add missing types actually used in the codebase:

```rust
// RwLock guards
#[cfg(not(loom))]
pub(crate) use std::sync::{RwLockReadGuard, RwLockWriteGuard};
```

Add lock hierarchy documentation at the top:

```rust
//! Lock Hierarchy (acquire in this order to prevent deadlock):
//! Level 0: StateDatabase (resilience/database)
//! Level 1: MemoryStore (memory/)
//! Level 2: ToolRegistry, ChannelRegistry (dispatcher/, gateway/)
//! Level 3: UI state, progress monitors
```

### Exclusions

- `tokio::sync::Mutex` / `tokio::sync::RwLock` (async locks)
- `std::sync::Once` / `OnceLock` (API mismatch)
- Third-party library internals

## Part 2: Loom Test Matrix (21 tests)

### dispatcher — `loom_concurrency.rs` (4 tests)

| Test | Target | Pattern |
|------|--------|---------|
| `loom_registry_concurrent_read_write` | Tool registry read/write race | 1 writer registers + 1 reader queries, verify no deadlock and consistent reads |
| `loom_engine_pause_resume_cancel` | Atomic flag coordination | 3 AtomicBool interleaved store/load, verify no illegal state combinations |
| `loom_atomic_counter_monotonic` | Event sequence monotonicity | Multi-thread fetch_add, verify all return values unique and monotonic |
| `loom_progress_snapshot` | Progress snapshot consistency | RwLock-protected state, 1 writer + 2 readers, verify no torn reads |

### gateway — `loom_concurrency.rs` (5 tests)

| Test | Target | Pattern |
|------|--------|---------|
| `loom_seq_counter_uniqueness` | Sequence number allocation | Multi-thread AtomicU64 fetch_add, verify N threads get N unique values |
| `loom_connection_state_transition` | Connection atomic state | AtomicBool connect/disconnect interleaving, verify final state consistency |
| `loom_request_id_allocation` | Request ID uniqueness | AtomicU32 fetch_add, verify no duplicate IDs |
| `loom_chunk_counter_reset` | Chunk counter reset safety | Thread A store(0) vs thread B fetch_add(1), verify no lost counts |
| `loom_execution_run_limit` | TOCTOU race detection | Simulate load → check → store, verify concurrent submit respects max limit |

### agent_loop — `loom_concurrency.rs` (3 tests)

| Test | Target | Pattern |
|------|--------|---------|
| `loom_anchor_store_read_write` | Anchor store concurrency | RwLock<Vec>, 1 writer push + 2 readers iterate, verify no deadlock |
| `loom_state_flag_coordination` | State flag combinations | Multiple AtomicBool (running/paused/aborted), verify legal combinations |
| `loom_shared_component_access` | Arc reference counting | Arc<T> cloned to multiple threads, verify correct drop count |

### memory — `loom_concurrency.rs` (5 tests)

| Test | Target | Pattern |
|------|--------|---------|
| `loom_daemon_singleton_init` | DreamDaemon singleton | Multi-thread compare_exchange(false, true), verify exactly 1 succeeds |
| `loom_compression_trigger_race` | Compression trigger race | Mutex<Instant> + AtomicU32 pending_turns, verify no lost events between trigger and reset |
| `loom_activity_timestamp_update` | Activity timestamp safety | AtomicI64 store/load, verify reads always return a legally-written value |
| `loom_metrics_counter_accuracy` | Metrics counter precision | Multi-thread fetch_add(1), verify final total = thread count |
| `loom_embedding_provider_swap` | Provider hot-swap | RwLock<Provider>, 1 writer swaps + readers read, verify no torn state |

### resilience — `loom_concurrency.rs` (4 tests)

| Test | Target | Pattern |
|------|--------|---------|
| `loom_lane_counter_accuracy` | Lane active count | Multi-thread fetch_add(1)/fetch_sub(1), verify final count = 0 |
| `loom_token_budget_concurrent` | Token budget TOCTOU | AtomicU64 fetch_add, verify total consumption never exceeds budget |
| `loom_seq_counter_per_task` | Per-task sequence uniqueness | RwLock<HashMap> + AtomicU64, verify same task gets unique incrementing IDs |
| `loom_database_mutex_contention` | Database lock contention | Mutex-protected writes from multiple threads, verify no deadlock and all complete |

### Module Registration

Each test file conditionally compiled:

```rust
// In each module's mod.rs
#[cfg(all(test, loom))]
mod loom_concurrency;
```

## Part 3: HIGH-Severity Bug Fixes

### Fix 1: Token Budget HashMap Unprotected

- **Location**: `resilience/governance/governor.rs`
- **Problem**: `HashMap<String, AtomicU64>` accessed without lock protection; concurrent insert during read causes panic
- **Fix**: Wrap in `RwLock<HashMap<String, AtomicU64>>`

### Fix 2: Channel Registry inbound_rx Deadlock Risk

- **Location**: `gateway/channel_registry.rs:49`
- **Problem**: `Arc<RwLock<Option<mpsc::Receiver>>>` — taking ownership requires write lock, blocks all other operations
- **Fix**: Replace with `std::sync::Mutex<Option<...>>` (take is instantaneous, no writer starvation)

### Fix 3: Execution Engine TOCTOU

- **Location**: `gateway/execution_engine/engine.rs`
- **Problem**: load active_runs.len() then insert — race window allows exceeding max_concurrent_runs
- **Fix**: Check + insert within the same write lock scope

### Fix 4: Cross-Module Lock Ordering Documentation

- **Location**: `sync_primitives.rs` header
- **Problem**: No documented lock hierarchy across modules
- **Fix**: Add lock hierarchy comment (Level 0-3), code convention only

## File Organization

```
core/src/
├── sync_primitives.rs              # Expanded + lock hierarchy docs
├── dispatcher/
│   └── loom_concurrency.rs         # 4 tests
├── gateway/
│   └── loom_concurrency.rs         # 5 tests
├── agent_loop/
│   └── loom_concurrency.rs         # 3 tests
├── memory/
│   └── loom_concurrency.rs         # 5 tests
└── resilience/
    └── loom_concurrency.rs         # 4 tests
```

## CI Integration

Existing `rust-core.yml` loom-tests job and `justfile` commands are already configured. No changes needed:

```yaml
# .github/workflows/rust-core.yml — loom-tests job
cargo test --features loom --lib
  env: RUSTFLAGS="--cfg loom", LOOM_MAX_PREEMPTIONS=3
  timeout-minutes: 30, continue-on-error: true
```

```bash
# justfile
just test-loom
```

## Execution Order

1. Expand `sync_primitives.rs` (new types + lock hierarchy docs)
2. Migrate all 5 modules' `std::sync` imports → `crate::sync_primitives`
3. Fix 4 HIGH-severity bugs
4. Write 21 loom tests (module by module)
5. Verify: `cargo test --workspace` passes (non-loom)
6. Verify: `just test-loom` passes
