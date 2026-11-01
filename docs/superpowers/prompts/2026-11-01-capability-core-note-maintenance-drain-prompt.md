# Restart prompt: startup note maintenance owned drain / 重启实施提示

**Status / 状态: Unimplemented / 未实施。** This prompt is a production implementation authorization template for a later session. The current planning session only writes and commits the design, plan and prompt. Planning documents alone are not authorization. Execute production changes only after the user explicitly asks to execute this prompt. / 本 prompt 是后续会话的生产实施授权模板；本轮只写并提交 design、plan、prompt。规划文档本身不构成授权，只有用户明确要求执行本 prompt 才能改生产代码。

## Mission / 目标

Implement only the next bounded Memory lifecycle prerequisite: source-owned, awaitable completion for the two existing startup note-maintenance tasks:

1. the `FsNoteOrientation::bootstrap` task currently detached near `src/bin/aleph-server/commands/start/mod.rs:1458`;
2. the `NoteIndexer::full_rebuild_all` task currently detached near `src/bin/aleph-server/commands/start/mod.rs:2639`.

Retain both real `tokio::task::JoinHandle<()>` values in one concrete `NoteMaintenanceOwner`, observe them independently in the existing common post-`server.run_until_shutdown(shutdown_rx).await` funnel, and preserve cancellation ownership so a later call observes the same tasks. Preserve existing startup timing, orientation error logging, full-rebuild statistics, failed-corpus warnings and the original `run_result`.

This slice supports the Capability Core north star—externally managed runtime work must have an explicit owner and observable lifetime—but it is not a Memory provider or a global Memory switch.

## Locate and confirm / 定位核对

Before editing, run and inspect:

```text
git rev-parse --show-toplevel
git status --short --branch
git worktree list --porcelain
git branch --all --verbose --no-abbrev
git log --oneline --decorate -8
```

Use the current repository root. The expected local baseline is `main` at or after `3a3def27f`, which contains the merged NoteVaultWatcher shutdown commits. Start a short branch such as `note-maintenance-drain` from local `main`; do not reset to a stale remote, overwrite edits, push or merge remotely.

Before production edits, write `docs/superpowers/runs/2026-11-01-note-maintenance-drain-run.md` with the explicit current-user authorization, baseline, scope, selected workflow and planned checks. Commit that authorization record first.

## Read completely / 完整阅读

- repository `AGENTS.md` and applicable `docs/reference/{REDLINES,CODE_ORGANIZATION,DEVELOPMENT,WORKSPACE,SUBSYSTEM_ROUTING}.md`;
- design: `docs/superpowers/specs/2026-11-01-capability-core-note-maintenance-drain-design.md`;
- plan: `docs/superpowers/plans/2026-11-01-capability-core-note-maintenance-drain-plan.md`;
- current Memory references: `docs/reference/MEMORY_SYSTEM.md`, `docs/reference/memory/NOTES.md`, relevant `FEATURE_LOCATOR.md` section and the completed watcher run record;
- `src/bin/aleph-server/commands/start/mod.rs` around orientation bootstrap, shared NoteIndexer/full rebuild, watcher drain and common teardown;
- `src/bin/aleph-server/commands/start/builder/subsystems.rs` owner/helper/test conventions;
- `src/memory/notes/orientation/fs_orientation.rs` bootstrap effects;
- `src/memory/notes/indexer.rs` `full_rebuild_all` and `reconcile_corpus` JoinSet/error path;
- current Rust/TDD and review guidance before writing tests.

The existing watcher is already implemented and merged. Do not re-implement or widen it.

## Fixed implementation contract / 固定契约

- Retain the real orientation and full-rebuild JoinHandles before their spawn scopes end. A concrete `NoteMaintenanceOwner` is the sole owner for these two handles; no generic supervisor, registry, global slot, second enabled decision or owner map.
- Keep the existing task bodies and startup order. Orientation bootstrap still logs its own `bootstrap` failure and returns `()`. The full-rebuild wrapper still logs aggregate `IndexStats` and failed corpora and returns `()`.
- Add an in-place asynchronous drain boundary in the existing builder boundary, for example:

```rust
pub(in crate::commands::start) async fn drain_note_maintenance(
    owner: &mut NoteMaintenanceOwner,
) -> Result<(), AlephError>
```

It must await each stored handle through a mutable reference. Do not take-and-await a pending handle, abort it, detach it, spawn replacement work or convert failure to `Ok`.
- A cancelled drain leaves the same pending handles in the owner. A later call observes the same `JoinHandle::id()` values. No internal timeout or hard latency claim.
- Observe orientation and rebuild independently. A `JoinError` from one does not skip the other. Retain the first failure as sticky source-local evidence and return `AlephError::other(...)` on current and later calls. Repeated success returns `Ok` without new work.
- Call the drain in the same common funnel as the existing watcher drain, immediately after `run_until_shutdown` returns and before later shared-resource disposal. Warn on failure, continue sibling teardown and preserve the original `run_result`.
- A `Drop` warning for early startup abandonment may be added, but Drop launches no cleanup task, performs no blocking join and reports no success. Early-startup-error cleanup remains outside this slice.
- Audit `reconcile_corpus`'s `JoinSet`: if an early error return can leave category tasks unobserved, make only a local correction that records the first error, drains the remaining set and returns that same error. Preserve successful aggregation and per-corpus nonfatal behavior. Stop if this requires broad indexer restructuring.

## TDD and required evidence / TDD 与必需证据

Use real temporary note roots and a real `SqliteMemoryBackend` for the production effect test. Use private same-module barriers or panic seams only for deterministic cancellation and JoinError behavior. Do not expose public test-only APIs, duplicate indexing logic, use sleep races or manually join tasks outside the owner.

Required cases:

- real orientation/bootstrap and full-rebuild Markdown/SQLite effects are observable before drain returns `Ok`;
- cancellation while either task is pending preserves both exact task IDs and retry observes the same tasks;
- either task's JoinError remains `Err` and the other task is still observed;
- successful drain is idempotent and spawns no new task;
- existing orientation error and per-corpus rebuild failure semantics remain nonfatal and logged;
- if the JoinSet path is changed, remaining category tasks are observed after the first error;
- the common production funnel drains this same owner on both normal and returned-server-error paths.

Run meaningful RED before production implementation. A missing type/API compiler error alone is not lifecycle evidence; use a runtime RED for pending, cancellation or failure behavior where feasible.

Before **every** Cargo command run:

```text
python qa/browser_live/check_memory.py
```

Proceed only on `ALLOW` with at least 4 GiB available. Set `CARGO_BUILD_JOBS=1` and run Cargo serially. At minimum run the focused binary lifecycle tests, the existing `note_watcher_drain` binary tests, affected indexer/watcher tests when relevant, `cargo check -p alephcore --tests` and `cargo check -p alephcore --bin aleph-server`. Record actual selected/pass/fail counts; zero selected needs investigation. Run affected-file rustfmt and `git diff --check`.

Fresh read-only native Pi review is required after implementation. Fix concrete findings and re-review. Distinguish owner behavior, static common-funnel reachability, actual OS-signal/live-server E2E and whole-Memory lifecycle claims.

## Explicit exclusions / 明确排除

Do not implement:

- Memory/graph enable, disable, cold, stop/re-enable or generation fencing;
- `SqliteMemoryBackend`, `MarkdownSessionStore`, raw session storage or shared lock closure;
- CompressionService, DreamDaemon, MemoryProducerScheduler or event reconciler stop;
- embedding provider hot replacement or provider admission fencing;
- Capability descriptor/lease/Zahir/MCP/ACP, ChannelRegistry, generic lifecycle supervisor or global registry;
- watcher redesign, startup readiness restructuring, Sandbox changes or new dependencies;
- native unloading, process isolation, executable-page reclamation or RSS claims;
- automatic push, remote merge or unrelated refactors.

Do not claim that this owner stops all Memory writers or makes the process cold. The next separate architecture task is a Memory resource census covering shared NoteStore/raw session storage, Compression, Dream, producer scheduler, event reconciler and embedding references before any global lifecycle switch is designed.

## Delivery / 交付

Use conventional commits, keeping the sequence understandable:

```text
docs(memory): record note maintenance drain authorization
fix(memory): retain startup note maintenance tasks
fix(memory): await startup note maintenance during shutdown
test(memory): strengthen note maintenance lifecycle evidence
docs(memory): record note maintenance drain evidence
```

Update current references only after observed validation, preserve historical watcher records, finish the bilingual run record with actual counts, warnings, unrun checks, review verdict, source boundaries and local Git state. Keep the branch/worktree for user review. Do not push or merge automatically.

## Stop conditions / 停止条件

Stop and report a source-backed blocker if retaining either task changes startup ordering materially, cancellation loses ownership, the JoinSet correction requires a broad rewrite, early startup errors must be solved for the common funnel to be safe, or the work requires closing shared resources or introducing a second lifecycle authority. Do not silently redesign toward the whole Memory provider.
