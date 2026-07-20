# Logic Review Report
**Module**: init_unified
**Scope**: Full static review of src/init_unified/ (coordinator.rs, mod.rs, error.rs)
**Date**: 2026-05-22
**Mode**: strict

## Findings

### [Critical] Panic in run_internal leaves INITIALIZING flag permanently true
- **Location**: `coordinator.rs:78-92`
- **Trigger condition**: `run_internal` panics (e.g., OOM, unwrap in dependency, spawn_blocking panic)
- **Expected behavior**: INITIALIZING AtomicBool should always be reset to false, even on panic
- **Actual behavior**: If `run_internal().await` panics, line 90 (`INITIALIZING.store(false, ...)`) never executes. All subsequent initialization attempts fail with "Initialization already in progress" until process restart.
- **Suggested fix**: Use RAII guard pattern — wrap the flag reset in a struct that implements `Drop`, or use `catch_unwind` with proper cleanup.

### [Critical] SQLite WAL artifacts not cleaned up on rollback
- **Location**: `coordinator.rs:198-201`
- **Trigger condition**: Database phase succeeds (creating SQLite DB in WAL mode), but a later phase fails triggering rollback
- **Expected behavior**: All initialization artifacts should be cleaned up, including temporary WAL files
- **Actual behavior**: Database rollback is skipped entirely to "avoid deleting pre-existing user data", but WAL files (`memory.db-wal`, `memory.db-shm`) created during this initialization are new artifacts, not pre-existing data. They are left behind.
- **Suggested fix**: In Database rollback, don't delete `memory.db`, but do delete `memory.db-wal` and `memory.db-shm` if they exist.

### [Warning] Rollback errors not surfaced in InitializationResult
- **Location**: `coordinator.rs:130-142`
- **Risk**: When a phase fails and rollback also fails, the rollback errors are only logged via `warn!`, but the returned `InitializationResult` only contains the original phase error. The caller/user has no visibility into the partial cleanup failure.
- **Current impact**: medium
- **Suggestion**: Include rollback errors in the returned `error_message` or add a `rollback_errors` field to `InitializationResult`.

### [Warning] spawn_blocking panic payload lost
- **Location**: `coordinator.rs:363-371`
- **Risk**: If `migrate_from_legacy` panics, the panic payload (which may contain a useful error message) is lost. Only generic "Runtime init task panicked" is reported.
- **Current impact**: low
- **Suggestion**: Attempt to extract the panic payload using `e.try_into_panic()` and format it if possible.

### [Warning] Config temp file not cleaned up on rename failure
- **Location**: `coordinator.rs:308-315`
- **Risk**: If `tokio::fs::write(&temp_path, toml_str)` succeeds but `tokio::fs::rename(&temp_path, &config_path)` fails, the temp file is left behind. On retry, the temp file may interfere or accumulate garbage.
- **Current impact**: low
- **Suggestion**: Clean up temp_path on rename failure.

### [Warning] TOCTOU race in needs_initialization
- **Location**: `mod.rs:22-31`
- **Risk**: Filesystem state can change between the `exists()` checks and the return. In concurrent or externally-modified environments, this can return stale results. Not critical for a best-effort check function.
- **Current impact**: low
- **Suggestion**: Document as best-effort, or return the actual missing components for better diagnostics.

## Summary
| Level | Count |
|-------|-------|
| Critical | 2 |
| Warning | 4 |
| Suggested Test | 0 |
