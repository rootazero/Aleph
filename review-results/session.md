# Module: session

**Date**: 2026-07-19
**Reviewers**: 4 parallel agents (security, logic, architecture, quality)

## Summary
- Path: `src/session/` (13 files, ~4k LOC)
- Raw issues found: 35
- After filtering (high-confidence only): 4

## High-Confidence Issues (will fix)

### 1. `retire_from` non-atomic UPDATE+DELETE — HIGH (security/logic)
- **File**: `src/session/store.rs:399-434`
- **Description**: The UPDATE that marks `retired_at` and the DELETE that removes FTS rows run as two separate statements. If the FTS DELETE fails (e.g. disk-full), the events are already retired and `recall_events` won't return them, but their content remains in the BM25 mirror and `search_events` can still hand it back to the model.
- **Fix**: Wrap both statements in `BEGIN IMMEDIATE` / `COMMIT` with explicit `ROLLBACK` on error.

### 2. `wake` proceeds after shutdown timeout — HIGH (logic)
- **File**: `src/session/in_process.rs:213-221`
- **Description**: On `timeout(SHUTDOWN_GRACE, rx)` timeout, code logs a warning and proceeds to `spawn_actor`. The SQLite `session_events.seq` counter is per-actor, so two live actors for the same session can both allocate overlapping sequences and clobber each other's inserts.
- **Fix**: Return `SessionError::ShutdownTimeout` instead of falling through; new `ShutdownTimeout` variant added to `service.rs`.

### 3. `detach` reports success after shutdown timeout — HIGH (logic)
- **File**: `src/session/in_process.rs:265-273`
- **Description**: Same timeout-fallthrough as `wake`. Returning `Ok` here lets the caller perform direct store writes while the old actor may still be appending events.
- **Fix**: Return `SessionError::ShutdownTimeout`.

### 4. `SessionActor` exposes pub fields — MEDIUM (quality)
- **File**: `src/session/actor.rs:40-49`
- **Description**: `pub id: SessionId` and `pub store: Arc<dyn SessionEventStore>` have no external consumer; all access is via `self.` from within the struct's methods.
- **Fix**: Tighten to `pub(crate)`.

## Skipped Issues (low signal / design choices / high risk)

- **Cross-module dependency cycles** (events→gateway/orchestrator, tool_trace↔tools, session↔context) — architectural refactor, requires product owner sign-off.
- **Global `OnceLock` service locators** — design choice; documented in source.
- **`store.rs` 1395 lines** — refactor risk; would touch every test.
- **`in_process.rs` 530 lines** — same.
- **State-machine violations** in `state.rs` (TurnStarted/TurnEnded/Tool* events ignoring turn_id) — needs reducer redesign; high regression risk.
- **`store.rs` FTS no backfill on existing rows** — only matters for upgrades from a pre-FTS build; new deployments always create FTS at construction time.
- **`store.rs` sequence monotonicity** — append allows regression; design choice consistent with idempotent retry semantics.
- **`i64::MAX` clamp on `from_seq`** — defensive saturation; matches existing behavior on other methods.
- **Duplicate shutdown / send-reply / emit-or-warn blocks** in `in_process.rs` and `tool_trace.rs` — mechanical refactor with high regression risk; deferred.
- **Public re-exports of internal details** in `mod.rs` — design choice.
- **Future-facing abstractions** (`driver.rs`, `projection.rs`) without production consumer — defer to deletion when broader audit confirms.

## Status
- 4 high-confidence issues fixed in this pass.
- Committed without per-module `cargo check` per user instruction.
- Full project `cargo check` deferred to end of sweep.