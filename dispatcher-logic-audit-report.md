# Logic Review Report
**Module**: dispatcher
**Scope**: Full static audit of src/dispatcher/ (34 files)
**Date**: 2026-05-22
**Mode**: strict

## Findings

### [Critical] ArcSwap read-modify-write race in ToolHealthCache
- **Location**: `src/dispatcher/registry/health.rs:131-144`
- **Trigger condition**: Concurrent calls to `register_probe()` or `unregister_probe()` from multiple threads/tasks
- **Expected behavior**: All probe registrations are preserved; no probe is silently lost
- **Actual behavior**: `load() -> clone -> modify -> store()` pattern is not atomic. If two threads interleave:
  1. Thread A loads HashMap H
  2. Thread B loads HashMap H (same snapshot)
  3. Thread A inserts probe X, stores H+X
  4. Thread B inserts probe Y, stores H+Y — probe X is lost
- **Suggested fix**: Use `ArcSwap::rcu()` which loops CAS until success. Applied in commit c8f1ce080.

### [Critical] ArcSwap read-modify-write race in ToolHealthCache::refresh
- **Location**: `src/dispatcher/registry/health.rs:218-220`
- **Trigger condition**: Concurrent `refresh()` calls for different tools, or `refresh()` racing with `invalidate_all()`
- **Expected behavior**: Every probe result is cached; no cache entry is silently overwritten
- **Actual behavior**: Same load-clone-store pattern as above. Two concurrent refreshes can cause one result to be lost.
- **Suggested fix**: Use `ArcSwap::rcu()` for the entries update. Applied in commit c8f1ce080.

### [Warning] Sync primitives import rule violation in registry
- **Location**: `src/dispatcher/registry/mod.rs:17`, `src/dispatcher/registry/types.rs:7`
- **Risk**: `tokio::sync::RwLock` imported directly instead of via `crate::sync_primitives::AsyncRwLock`. Under `--features loom` these paths diverge; loom cannot instrument the raw tokio import.
- **Current impact**: Low — only affects loom test coverage for async RwLock paths (loom does not instrument async RwLock by design, but the import rule exists for consistency)
- **Suggestion**: Replace with `crate::sync_primitives::AsyncRwLock`. Applied in commit c8f1ce080.

### [Warning] Risk evaluator regex patterns lack word boundaries
- **Location**: `src/dispatcher/risk.rs:31-46`
- **Risk**: Substring matches cause false positives. E.g., "capability" matches "api", "myexec" matches "exec", "happy" matches "pay".
- **Current impact**: Medium — low-risk operations may be incorrectly classified as high-risk, causing unnecessary confirmation prompts
- **Suggestion**: Add `\b` word boundaries to English-language patterns. Applied in commit c8f1ce080.

### [Warning] Similarity score computation allows values > 1.0
- **Location**: `src/dispatcher/tool_index/retrieval.rs:136`, `retrieval.rs:191`
- **Risk**: Vector search may return negative L2 distances (depending on backend implementation). Formula `1.0 / (1.0 + score)` with negative `score` yields `similarity > 1.0`, violating the [0, 1] invariant and causing incorrect hydration level classification.
- **Current impact**: Low — current backend returns non-negative distances, but the invariant is not enforced
- **Suggestion**: Clamp distance to non-negative: `1.0 / (1.0 + score.max(0.0))`. Applied in commit c8f1ce080.

### [Warning] get_by_name uses imprecise suffix matching
- **Location**: `src/dispatcher/registry/query.rs:319`
- **Risk**: `t.id.ends_with(&format!(":{}", name))` matches any tool whose ID ends with the name string. If `name` contains a colon (e.g., "fs:read_file"), it matches "mcp:fs:read_file" but also any ID ending with "fs:read_file".
- **Current impact**: Low — tool names do not typically contain colons
- **Suggestion**: Match only the last colon-delimited segment using `rsplit_once(':')`. Applied in commit c8f1ce080.

### [Warning] sync_primitives missing PoisonError export
- **Location**: `src/sync_primitives.rs:40`
- **Risk**: Existing code (`src/extension/registrar/wasm_registrar.rs`) imports `PoisonError` from `crate::sync_primitives`, causing compile error after the stash/unstash cycle.
- **Current impact**: Medium — breaks compilation
- **Suggestion**: Add `PoisonError` to the `std::sync` re-export list. Applied in commit c8f1ce080.

### [Warning] L2 optimization tasks are unbounded
- **Location**: `src/dispatcher/tool_index/coordinator.rs:297-359`
- **Risk**: Every tool that triggers L2 optimization spawns a detached `tokio::spawn` task. With hundreds of tools, this can exhaust tokio thread pool or memory.
- **Current impact**: Low — typical deployment has < 100 tools
- **Suggestion**: Add a semaphore or task limiter for concurrent L2 tasks. Document the fire-and-forget behavior.

### [Warning] register_custom_commands bypasses conflict resolution
- **Location**: `src/dispatcher/registry/registration.rs:339-392`
- **Risk**: Custom commands from config rules are inserted directly into the HashMap without checking for name conflicts with builtin/native/mcp/skill tools.
- **Current impact**: Low — name conflicts are resolved at query time via `find_best_match()` priority ordering
- **Suggestion**: Either add conflict checking or document the design decision that custom commands have lower priority and rely on query-time resolution.

## Summary
| Level | Count |
|-------|-------|
| Critical | 2 |
| Warning | 7 |
| Suggested Test | 0 |

## Automated Verification Results

### L1 (proptest)
- Dispatcher module has no dedicated proptest files. Existing proptest coverage in other modules all passed.

### L2 (loom)
- Command: `LOOM_MAX_PREEMPTIONS=3 cargo test -p alephcore --features loom --lib dispatcher::loom_concurrency`
- Results: **4 passed; 0 failed**
  - `loom_registry_concurrent_read_write` — ok
  - `loom_engine_pause_resume_cancel` — ok
  - `loom_atomic_counter_monotonic` — ok
  - `loom_progress_snapshot` — ok

### Unit Tests
- Command: `cargo test -p alephcore --lib dispatcher`
- Results: **206 passed; 0 failed**

## Commits
- `c8f1ce080` — dispatcher: fix race conditions, sync primitives imports, and edge cases from logic audit
