# Logic Review Report
**Module**: init_unified
**Scope**: Full static review of src/init_unified/ (coordinator.rs, mod.rs, error.rs)
**Date**: 2026-05-31
**Mode**: strict

## Findings

### [Critical] Orphaned / Unwired Public API
- **Location**: `src/init_unified/mod.rs`, `src/init_unified/coordinator.rs`
- **Trigger condition**: Module exports public APIs but has zero callers in the entire codebase
- **Expected behavior**: All `pub` APIs should have verified callers or be documented as external-facing
- **Actual behavior**: `needs_initialization()`, `InitializationCoordinator`, `InitProgressHandler`, `InitializationResult` are exported via `lib.rs` but never called from Rust core, desktop, interfaces, or tests
- **Suggested fix**: Wire into the boot sequence, or document as desktop-only API with clear integration notes. Current state violates R11 (Wiring Completeness).

### [Warning] TOCTOU in needs_initialization
- **Location**: `mod.rs:22-31`
- **Risk**: Filesystem state can change between the three `exists()` checks. Not atomic.
- **Current impact**: low
- **Suggestion**: Document as best-effort check; consider returning missing components for diagnostics.

### [Warning] generate_config temp file could collide
- **Location**: `coordinator.rs:329`
- **Risk**: `config.toml.tmp` might already exist from a previous crashed initialization attempt.
- **Current impact**: low
- **Suggestion**: Use a unique temp filename to prevent collision.

### [Warning] Directories rollback could leave empty config_dir
- **Location**: `coordinator.rs:227-253`
- **Risk**: After removing empty subdirectories, the parent `config_dir` might also be empty (if it was newly created) but is never removed during rollback.
- **Current impact**: low
- **Suggestion**: Consider removing config_dir if it was created by this initialization and is now empty.

### [Warning] No unit tests for init_unified module
- **Location**: `src/init_unified/`
- **Risk**: Zero test coverage for initialization logic, error types, and phase enumeration.
- **Current impact**: medium
- **Suggestion**: Add tests for `needs_initialization()`, `InitPhase`, `InitError`, and coordinator error paths.

## Previously Fixed (from 2026-05-22 review)

The following critical issues from the previous review have been resolved:

1. ✅ **Panic guard**: RAII `Guard` struct now ensures `INITIALIZING` flag is reset even on panic (lines 91-97)
2. ✅ **SQLite WAL cleanup**: Database rollback now removes `-wal` and `-shm` files (lines 213-221)
3. ✅ **Rollback error surfacing**: Rollback failures are now included in `error_message` (lines 138-144)
4. ✅ **Panic payload extraction**: `spawn_blocking` panic payload is now extracted and formatted (lines 393-403)
5. ✅ **Temp file cleanup**: Failed rename now cleans up the temp file (line 336)

## Summary
| Level | Count |
|-------|-------|
| Critical | 1 |
| Warning | 4 |
| Suggested Test | 1 |

## Automated Verification
- `cargo check -p alephcore --lib` — **PASS** (0 errors, 3 pre-existing warnings in unrelated module)
- `cargo test -p alephcore --lib init_unified` — **0 tests found** (coverage gap confirmed)
