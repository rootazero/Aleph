# NoteVaultWatcher shutdown execution record

**Date / 日期:** 2026-11-01  
**Status / 状态:** Execution authorized; implementation in progress / 已授权执行；实施中

## Authorization / 授权

The user explicitly authorized execution with `执行` after the implementation prompt stated that planning documents alone were not production authorization. This record is committed before any production source edit.

用户在确认规划阶段不构成生产授权后明确回复“执行”。本记录先于所有生产源码修改提交。

## Checkout baseline / 检出基线

- Repository root: `/Volumes/TBU4/Workspace/Aleph`
- Branch: `note-watcher-stop`
- Branch base: local `main` at `52e13cb3db8cf27a2a9bede773cbcc8742cb28e5`
- Worktree count: one worktree, the current checkout
- Worktree before this record: clean; no staged, unstaged or untracked production changes
- Ancestry verified: `88fe4fc4c`, `cf52bb05a`, `b378b87bd`, and planning commit `3a73e8c13` are ancestors of the base
- Remote actions: none; no push, merge, reset or history rewrite

The current source baseline is the unimplemented design state: `src/memory/notes/watcher.rs` stores only the debouncer and discards the consumer JoinHandle; the production start path stores `_note_vault_watcher` without an explicit common-teardown drain; `builder/subsystems.rs` has no watcher drain helper.

## Selected execution workflow / 执行方式

- Main agent remains the sole writer in this checkout.
- Implement source behavior with TDD: write owning-module tests, observe meaningful RED, then add the smallest production boundary and observe GREEN.
- Use real temporary filesystem roots, SQLite-backed note storage and real watcher delivery for effect coverage; keep private lifecycle seams inside the owning watcher module for deterministic cancellation and JoinError cases.
- Run Cargo serially with `CARGO_BUILD_JOBS=1`; before every Cargo invocation run `python qa/browser_live/check_memory.py` and proceed only on `ALLOW` with at least 4 GiB.
- Run the plan's focused tests/checks, affected-file rustfmt and `git diff --check`.
- Perform an independent fresh read-only native Pi review after implementation and verification; fix concrete findings and re-review.

## Fixed scope / 固定范围

Implement only source-owned, awaitable shutdown for the existing `NoteVaultWatcher` and its production teardown call:

- retain the real consumer JoinHandle;
- retain one `spawn_blocking` helper that owns `Debouncer::stop`;
- preserve pending handles across cancellation and retain observed JoinError as sticky failure;
- drain batches already emitted to the queue;
- provide a truthful synchronous nonblocking `Drop` fallback;
- await the same watcher owner in the common `run_until_shutdown` teardown funnel;
- preserve existing reconciliation behavior and error logging.

## Explicit exclusions / 明确不做

No Memory provider, config switch, graph/full-Memory disable, backend close, startup `full_rebuild_all` ownership change, Capability descriptor/lease/Zahir/MCP/ACP, generic registry/supervisor, channel integration, Sandbox change, dependency or lockfile update, durable recovery/EffectClaim/replay/scheduler/harness work, native unloading, RSS or whole-Memory coldness claim. The separate detached startup `full_rebuild_all` task remains untouched.

## Baseline verification state / 基线验证状态

No Cargo command has been run yet for this implementation. Therefore no test, build, warning or failure is claimed as passed or reproduced in this run record. The first production edit is permitted only after this authorization record is committed.

## Acceptance evidence to record later / 后续必须记录

Record actual RED/GREEN results and selected/pass/fail counts for every required focused command, the binary drain tests, both checks, rustfmt and diff checks; record memory-gate outcomes, warnings, failed attempts, unrun live/native checks, independent review verdict, commits and final branch/worktree state. Do not convert planning evidence or static wiring evidence into runtime or OS-signal E2E claims.
