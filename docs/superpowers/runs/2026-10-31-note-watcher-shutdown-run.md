# NoteVaultWatcher shutdown execution record

**Date / 日期:** 2026-11-01
**Status / 状态:** Implementation verified; independent review approved / 实施已验证；独立审查通过

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

The source baseline described above was verified before implementation. The authorization record commit is `dd730713a`; source implementation is `84655593a`; production teardown integration is `4bba875ac`; lifecycle evidence strengthening is `e89268fd4`. No remote action, history rewrite or automatic push was performed.

实施前已核对上述源码基线。授权记录提交为 `dd730713a`；源码实现提交为 `84655593a`；生产 teardown 接线提交为 `4bba875ac`；生命周期证据加固为 `e89268fd4`。未执行远端操作、历史改写或自动推送。

## Verification evidence / 验证证据

All Cargo commands were serial, used `CARGO_BUILD_JOBS=1`, and were preceded by `python qa/browser_live/check_memory.py`. Every gate returned `ALLOW` with at least 4 GiB available.

- Initial focused watcher compile attempt exceeded the 300-second tool timeout during compilation; it was rerun with a 900-second timeout and passed.
- `cargo test -p alephcore --lib memory::notes::watcher`: 15 selected, 15 passed.
- `cargo test -p alephcore --lib memory::notes::indexer`: 38 selected, 38 passed.
- `cargo test -p alephcore --lib memory::notes::graph`: 18 selected, 18 passed.
- `cargo test -p alephcore --lib gateway::handlers::graph`: 22 selected, 22 passed.
- `cargo test -p alephcore --lib -- memory::notes::watcher memory::notes::indexer memory::notes::graph gateway::handlers::graph`: 93 selected, 93 passed; this matched the union of the four focused filters.
- `cargo test -p alephcore --bin aleph-server note_watcher_drain`: 2 selected, 2 passed.
- `cargo check -p alephcore --tests`: passed.
- `cargo check -p alephcore --bin aleph-server`: passed.
- A later focused watcher rerun after strengthening the real-owner and Drop tests also passed: 15 selected, 15 passed.
- `rustfmt --edition 2024 --check` passed for `src/memory/notes/watcher.rs` and `src/memory/notes/watcher/lifecycle_tests.rs`; `git diff --check` passed after the test changes.

Observed warnings were existing `PluginProjection::panel_snapshot` dead code and the macOS linker warning `__eh_frame section too large (max 16MB) to encode dwarf unwind offsets in compact unwind table`.

- A later focused watcher rerun after strengthening the real-owner and Drop tests also passed: 15 selected, 15 passed.
- `rustfmt --edition 2024 --check` passed for `src/memory/notes/watcher.rs` and `src/memory/notes/watcher/lifecycle_tests.rs`; `git diff --check` passed after the test changes.

Observed warnings were existing `PluginProjection::panel_snapshot` dead code and the macOS linker warning `__eh_frame section too large (max 16MB) to encode dwarf unwind offsets in compact unwind table`.

所有 Cargo 命令均串行执行，设置了 `CARGO_BUILD_JOBS=1`，且每次执行前都运行内存门；每次均为 `ALLOW` 且可用内存不少于 4 GiB。

## Review and limits / 审查与边界

The first fresh read-only native Pi review identified three concrete evidence issues: failure tests did not sufficiently retain a real watcher owner, the Drop test did not hold a real reconciliation batch, and this run record was stale and had trailing whitespace. The implementation contract itself was judged consistent with the requested ownership semantics. The test gaps were fixed by using a real spawned watcher consumer in the stop-helper failure case, and by blocking a real emitted batch through `BlockingEmbedder` before Drop; the run record was corrected.

A fresh read-only native Pi re-review returned **Approved / no issues**. It confirmed the real-owner failure coverage, the 200 ms nonblocking Drop assertion with a held real batch, the in-place common funnel and preserved `run_result`, the corrected run record, and the documented `notify-debouncer-full` native join-error limitation.

本地未运行 OS signal / live server E2E、native debouncer join-error 注入、全 workspace/clippy、跨平台测试或 live Panel QA；这些结果不能从 focused tests 或静态 funnel 审计推导。`Debouncer::stop` 仍隐藏其 native thread `join()` 错误，因此本实现不声称 native thread 成功、所有 OS 资源已回收、RSS 降低或整个 Memory 已 cold/stoppable。startup detached `full_rebuild_all`、共享 NoteStore 及其他 Memory writer 未改变。
