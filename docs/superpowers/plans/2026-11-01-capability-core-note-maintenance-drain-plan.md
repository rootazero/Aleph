# Startup note maintenance owned drain implementation plan / 启动笔记维护任务所有权收尾实施计划

> **For the later implementation session:** read the companion design completely. This document is an unimplemented plan. The restart prompt becomes production authorization only after the user explicitly asks to execute it.

## Goal / 目标

Retain the two existing startup note-maintenance tasks—orientation bootstrap and `NoteIndexer::full_rebuild_all`—in one concrete source owner, then await that owner in the existing common post-`run_until_shutdown` teardown funnel. Preserve startup behavior and make cancellation, failure and completion evidence truthful.

This plan deliberately stops before whole-Memory cold/enable/stop/re-enable semantics. The watcher shutdown slice is already merged into local `main`; this plan covers the next two detached startup writers only.

## Architecture / 架构

Use a small concrete `NoteMaintenanceOwner` in the existing `src/bin/aleph-server/commands/start/builder/subsystems.rs` boundary unless source inspection shows a narrowly scoped adjacent private builder module is materially clearer. It owns two real `tokio::task::JoinHandle<()>` values:

- the orientation bootstrap task currently created near `src/bin/aleph-server/commands/start/mod.rs:1458`;
- the full-rebuild task currently created near `src/bin/aleph-server/commands/start/mod.rs:2639`.

The current task bodies keep their existing logging and return `()`. The owner stores each handle in an `Option` so an observed completion can be removed while a pending handle remains owned across a cancelled drain. A source-local sticky failure string records the first observed `JoinError`; it is returned as `AlephError::other(...)` on the current and later calls.

The owner is a task-specific authority, not a generic supervisor. It does not own the watcher, the backend, the raw store, the embedder or any other Memory writer. The existing watcher drain remains a separate source-owned boundary and is called before this owner in the common funnel so external note edits are no longer admitted while startup maintenance is being observed.

## Constraints / 约束

- Locate the current git root, branch and worktrees before edits. Start a short branch such as `note-maintenance-drain` from local `main` at or after `3a3def27f`.
- Record the explicit user authorization, baseline, scope and selected workflow in a new run record before production edits; commit that record first.
- Read `AGENTS.md`, applicable `docs/reference` routing/redline/development/workspace documents, the companion design, this plan, the restart prompt, Memory docs, current startup source, orientation source, indexer source and the existing watcher run record.
- Keep startup ordering, readiness behavior, logging, per-corpus nonfatal semantics and the original `run_result` unchanged.
- No new config field, capability registry, generic lifecycle framework, provider handle, backend close, generation fence, dependency, lockfile or broad startup refactor.
- Before every Cargo invocation run `python qa/browser_live/check_memory.py`; continue only with `ALLOW` and at least 4 GiB available. Use `CARGO_BUILD_JOBS=1`; never run Cargo concurrently.
- Read the repository's current Rust/TDD test guidance before editing tests. Use real effects for the production behavior and private same-module seams only for deterministic cancellation/failure.

## Task 1: Audit and retain the real startup handles

**Files:**

- Modify: `src/bin/aleph-server/commands/start/mod.rs` only at the orientation spawn, full-rebuild spawn and common teardown call site.
- Modify or create narrowly: `src/bin/aleph-server/commands/start/builder/subsystems.rs` (preferred) for `NoteMaintenanceOwner` and its drain helper.
- Inspect: `src/memory/notes/orientation/fs_orientation.rs`, `src/memory/notes/indexer.rs`.

- [ ] Confirm the exact current task construction and that both async blocks return `()` after preserving their existing logs.
- [ ] Replace the discarded orientation handle with a named local handle. Do not add a readiness barrier or move the bootstrap.
- [ ] Replace the discarded full-rebuild handle with a named local handle. Do not change its aggregate statistics, per-corpus warnings or startup timing.
- [ ] Construct `NoteMaintenanceOwner` after both handles exist and before the common long-running server phase. Keep the owner alive through the existing return funnel.
- [ ] Add an in-place helper with a concrete signature such as:

```rust
pub(in crate::commands::start) async fn drain_note_maintenance(
    owner: &mut NoteMaintenanceOwner,
) -> Result<(), AlephError>
```

The helper delegates to the same owner and must not replace it with a new owner or take pending handles out before awaiting them.
- [ ] Add a truthful `Drop` warning if pending handles are abandoned by an early startup return. Drop must not spawn a cleanup task, synchronously join, or claim success. Early-startup-error cleanup remains outside this slice.

### JoinSet audit and bounded correction

Read `NoteIndexer::reconcile_corpus` in full before deciding whether the outer full-rebuild handle is sufficient evidence. The current category `JoinSet` returns early on a child error. If that can leave category tasks unobserved, make only this local correction:

1. record the first child error or join error;
2. continue `join_next()` until the `JoinSet` is empty so every spawned category task reaches an observed terminal result;
3. return the original first error after the set is drained;
4. preserve the current successful aggregation and `full_rebuild_all` per-corpus continuation behavior.

If this requires changing the indexing algorithm, error model or concurrency structure broadly, stop and report the source-backed blocker instead of widening the task.

## Task 2: Write lifecycle tests before implementation

Add a private owning-module test module next to the owner, preferably in `src/bin/aleph-server/commands/start/builder/subsystems.rs` or a narrowly scoped sibling test module. Do not expose production test-only APIs across crate boundaries.

Required cases:

- `note_maintenance_drain_waits_for_real_bootstrap_and_rebuild_effects`: use a real temporary note root and real `SqliteMemoryBackend`; spawn the production orientation bootstrap and full rebuild tasks; hold deterministic completion with a private barrier where needed; prove the expected schema/index and note index effects are observable before `Ok` returns.
- `cancelled_drain_retains_same_task_ids`: hold the orientation task, call the owner drain and drop the caller future while it is pending; assert the exact orientation and rebuild `JoinHandle::id()` values remain in the owner; release the task and retry. Repeat with rebuild pending after orientation has completed. Do not spawn a replacement task.
- `one_join_failure_still_observes_the_other`: use private synthetic task seams to panic one retained task while the other remains pending; assert the drain returns `Err`, observes the other task, and remains `Err` on repeat. Test both failure directions if the owner implementation has asymmetric paths.
- `successful_drain_is_idempotent`: after both tasks complete successfully, call the drain again and prove no new task is created and no effect is repeated.
- `production_tasks_preserve_nonfatal_behavior`: use a real corpus that produces the existing per-corpus warning/error shape and prove the outer task still reaches its normal terminal result; do not convert expected per-corpus failures into a process startup failure.
- `reconcile_corpus_drains_category_tasks_after_first_error`: add this only if the JoinSet audit requires the local correction. Use a private failing category seam and a second observable category task to prove the second task reaches its terminal observation before the first error is returned.

The real filesystem/SQLite tests must exercise actual orientation and index behavior. Barriers and panic seams are for task ownership and ordering only; they must not replace the production task bodies in the effect test. No sleep-based race orchestration and no manual join outside the owner are acceptable.

## Task 3: Implement the owner and common teardown

- [ ] Add the concrete owner fields for the two optional JoinHandles and sticky failure evidence.
- [ ] Implement drain by observing orientation and rebuild independently through mutable handle references. Remove a handle only after its completion result has been observed. Continue to the second handle after the first JoinError.
- [ ] Preserve the first terminal JoinError as a source-local string and return a fresh `AlephError::other(...)`. Repeated calls after failure remain `Err`; repeated success is `Ok` with no new work.
- [ ] Keep caller cancellation outside the owner: dropping the drain future must not abort or discard either task.
- [ ] Call the helper once immediately after `server.run_until_shutdown(shutdown_rx).await`, after the existing watcher drain and before later shared-resource disposal. Warn on failure, preserve `run_result`, and continue the existing sibling teardown.
- [ ] Ensure both normal shutdown and returned server error reach this same funnel. A static source audit is required; do not call it OS-signal E2E.
- [ ] Keep `full_rebuild_all` and orientation body semantics visible in the final diff. Do not rename their logs or silently change warning severity.

## Task 4: TDD and verification

Before each command, run the memory gate and set `CARGO_BUILD_JOBS=1`. Run serially:

```text
cargo test -p alephcore --bin aleph-server note_maintenance
cargo test -p alephcore --bin aleph-server note_watcher_drain
cargo test -p alephcore --lib memory::notes::indexer
cargo test -p alephcore --lib memory::notes::watcher
cargo check -p alephcore --tests
cargo check -p alephcore --bin aleph-server
```

If the JoinSet correction changes indexer behavior, keep the indexer focused test and add the named regression; do not substitute a narrower test if the focused test fails. Record actual selected, passed and failed counts. Zero selected requires investigation.

Run affected-file formatting with the repository-approved command, then `git diff --check`. Check all introduced relative links and verify that the common funnel call is before later owner/resource disposal. Full workspace tests, clippy, cross-platform checks, OS-signal live E2E and live Panel QA remain unrun unless a concrete change makes one necessary; record them as unproven.

## Task 5: Independent review and evidence

Dispatch a fresh read-only native Pi review after implementation and verification. The reviewer must inspect:

- exact ownership of both real startup handles;
- cancellation preserving the same handles;
- independent failure observation and sticky error behavior;
- whether the JoinSet error path can leave category tasks unobserved;
- preservation of orientation/rebuild logs, stats and nonfatal semantics;
- common funnel position and original `run_result` preservation;
- the boundary between this owner and watcher, graph, Compression, Dream, producer, reconciler, embedding and shared store consumers.

Fix concrete findings and re-review. Do not treat the review as proof of OS-signal E2E or whole-Memory shutdown.

After review, update the current Memory references and a bilingual run record with:

- exact local baseline, branch and commits;
- user authorization and scope;
- actual RED/GREEN and focused counts;
- memory gate outcomes and existing warnings;
- JoinSet decision and evidence;
- review verdict;
- unrun checks and remaining resource owners;
- precise claims: two startup tasks observed, not Memory coldness.

Suggested commits:

```text
docs(memory): record note maintenance drain authorization
fix(memory): retain startup note maintenance tasks
fix(memory): await startup note maintenance during shutdown
test(memory): strengthen note maintenance lifecycle evidence
docs(memory): record note maintenance drain evidence
```

Keep the implementation branch and worktree for review. Do not push or merge automatically.

## Stop conditions / 停止条件

Stop with a source-backed report if:

- either task cannot be retained without changing startup ordering or adding a second authority;
- cancellation causes a task handle to be lost or requires abort/detach;
- the JoinSet fix needs broad indexer restructuring;
- early startup error cleanup must be solved to make the common funnel safe;
- proving completion requires closing shared backend/session resources or changing other Memory writers;
- the desired result becomes a global Memory switch, provider replacement or generation fence.

The next independent task after this slice is a Memory resource census and maintenance-owner architecture covering CompressionService, DreamDaemon, MemoryProducerScheduler, event reconciler, embedding distribution, shared NoteStore and raw session storage. This plan must not preempt that design.
