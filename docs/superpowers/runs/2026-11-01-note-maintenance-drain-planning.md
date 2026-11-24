# Startup note maintenance owned drain planning run / 启动笔记维护任务所有权收尾规划记录

**Status / 状态:** Planning complete; implementation not authorized in this session / 规划完成；本会话未授权实施生产改动

## User decision / 用户决策

The user requested that the completed `note-watcher-stop` branch be merged into local `main`, then asked for the next task's plan and restart prompt under the Capability Core north star:

> Everything externally composable is a Capability.

After source reconnaissance, three approaches were presented: whole Memory lifecycle now, a generic maintenance supervisor, or a bounded startup note-maintenance owner. The user selected the bounded startup note-maintenance owned drain.

This record documents planning evidence only. It is not production authorization. A later explicit request to execute the restart prompt is required before source edits.

## Baseline / 基线

- Repository root: `/Volumes/TBU4/Workspace/Aleph`
- Local branch after integration: `main`
- Fast-forward merge: `52e13cb3d..3a3def27f`
- `main` is clean and ahead of `origin/main` by five commits; no push, reset, history rewrite or remote merge was performed.
- `note-watcher-stop` remains as a local branch pointing to the merged commit.
- Completed watcher delivery remains bounded to source-owned watcher drain; it does not claim whole-Memory coldness.

## Source-backed findings / 源码发现

1. `src/bin/aleph-server/commands/start/mod.rs:1458` starts `FsNoteOrientation::bootstrap` in a detached Tokio task. The task can create the agent directory, write `SCHEMA.md`, rebuild `index.md`, write Obsidian configuration and append a log entry.
2. `src/bin/aleph-server/commands/start/mod.rs:2639` starts `NoteIndexer::full_rebuild_all` in a detached Tokio task. The wrapper logs aggregate index statistics and failed corpora, then drops its `JoinHandle`.
3. `src/memory/notes/indexer.rs:609` preserves per-corpus nonfatal behavior. `src/memory/notes/indexer.rs:410` uses a `JoinSet` for category scans; its early error path requires explicit review because dropping a JoinSet may abort children without observing their terminal results.
4. The common teardown after `server.run_until_shutdown(shutdown_rx).await` already awaits the merged watcher drain before later shared-resource disposal. It is the correct place to await a separate startup-maintenance owner.
5. Memory remains shared: note SQLite/vector, raw Markdown/session storage, graph, retrieval, CompressionService, DreamDaemon, producer scheduling, event reconciliation and embedding references have separate owners or consumers. None may be described as stopped by this narrow task.

## Selected package / 选定 package

Created without production edits:

- Design: `docs/superpowers/specs/2026-11-01-capability-core-note-maintenance-drain-design.md`
- Plan: `docs/superpowers/plans/2026-11-01-capability-core-note-maintenance-drain-plan.md`
- Restart prompt: `docs/superpowers/prompts/2026-11-01-capability-core-note-maintenance-drain-prompt.md`

The proposed implementation retains the two real startup `JoinHandle<()>` values in a concrete `NoteMaintenanceOwner`, observes them independently with sticky JoinError evidence, preserves cancellation for retry, and calls the same owner from the existing common funnel. A local `JoinSet` drain correction is allowed only if source audit proves it is needed; broad indexer restructuring is a stop condition.

## Deferred and unrun / 延期与未执行

No production source was changed. No Cargo command, build, test, clippy, rustfmt, live server or OS-signal QA was run in this planning session. The implementation plan requires the memory gate before every future Cargo command, `CARGO_BUILD_JOBS=1`, serial execution, runtime RED/GREEN evidence, a fresh read-only native Pi review and final reference/run-record updates.

Deferred by design: whole-Memory cold/enable/stop/re-enable, generation fencing, backend closure, provider replacement, Compression/Dream/producer/reconciler shutdown, embedding admission, Capability/Zahir/MCP/ACP, generic supervisor, Sandbox, native unloading and RSS claims.
