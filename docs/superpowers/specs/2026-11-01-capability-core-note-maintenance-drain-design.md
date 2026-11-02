# Startup note maintenance owned drain / 启动笔记维护任务所有权收尾设计

**Status / 状态:** Implemented on local `note-maintenance-drain`; see [execution evidence and limits](../runs/2026-11-01-note-maintenance-drain-run.md). / 已在本地分支实施，验证与边界见实施记录。

This design followed the completed bounded `NoteVaultWatcher` shutdown slice. It was originally planning material only; the user explicitly authorized its execution on 2026-11-01. The original design below records that decision's scope; the linked run records implementation and actual evidence. / 本设计原为不构成授权的规划材料；用户于 2026-11-01 明确授权执行。下文保留原设计范围，实际实施与证据以 run 为准。

## Intent and place in the Capability Core roadmap / 意图与路线位置

Aleph's north star is **Everything externally composable is a Capability**: anything that an Agent, Runtime or another system can discover, invoke, replace, subscribe to or manage must have an explicit capability contract. The operational target is a lightweight Core with providers that can be started on demand, stopped, replaced and observed.

The watcher slice removed one detached note-vault consumer and proved a source-owned, cancellable drain. Two other startup note-maintenance tasks still escape that boundary:

- `FsNoteOrientation::bootstrap` is launched with `tokio::spawn` and its handle is discarded near `src/bin/aleph-server/commands/start/mod.rs:1458`.
- `NoteIndexer::full_rebuild_all` is launched with `tokio::spawn` and its handle is discarded near `src/bin/aleph-server/commands/start/mod.rs:2639`.

Both tasks can write the note vault or its SQLite index after the watcher has drained. The next smallest useful boundary is therefore a source-owned owner for these two existing startup tasks, with one common teardown call. This is a prerequisite for a future Memory lifecycle design. It is not the Memory provider, a graph switch, or a universal maintenance supervisor.

## Source-backed baseline / 源码基线

The current local `main` is `3a3def27f`, the fast-forwarded result of the completed `note-watcher-stop` branch. It is clean and ahead of `origin/main` by five commits; no remote operation is part of this proposal.

At startup, `FsNoteOrientation::bootstrap` in `src/memory/notes/orientation/fs_orientation.rs:112` creates the agent directory, writes the default or generated `SCHEMA.md`, rebuilds `index.md`, writes Obsidian configuration and appends a log entry. The startup call currently logs an error inside a detached task and continues.

`NoteIndexer::full_rebuild_all` in `src/memory/notes/indexer.rs:609` scans all note corpora, uses `full_rebuild` for the selected agent, reconciles other corpora, sweeps transaction residue and records per-corpus failures without failing the whole startup task. Its current startup wrapper logs aggregate statistics and failed corpora, then discards the returned `JoinHandle<()>`.

`NoteIndexer::reconcile_corpus` currently fans out category work with a `JoinSet`. Its error path returns from the join loop early. The implementation task must determine whether that path can leave child work unobserved; if so, it may make a localized drain-after-first-error correction. A broad indexer rewrite is outside this proposal and is a stop condition.

The existing common teardown after `server.run_until_shutdown(shutdown_rx).await` already drains the watcher and then other owned resources. That is the correct integration point for the new owner. The detached startup `full_rebuild_all` and orientation task are the missing links.

The Memory boundary remains shared. `SqliteMemoryBackend` and `MarkdownSessionStore` are created unconditionally, and the same Memory area is used by graph handlers, retrieval, CompressionService, DreamDaemon, producer scheduling, event reconciliation and embedding consumers. A maintenance drain for two startup tasks cannot prove that raw Markdown, note/vector storage, graph, all writers, providers, threads or code are cold.

## Goals / 目标

- Keep the real orientation bootstrap and full-rebuild `JoinHandle`s in one source-owned local owner.
- Observe both tasks at the existing common post-run teardown point before the owner leaves scope.
- Preserve cancellation ownership: dropping a caller's wait future must leave the same pending handles available for a later retry.
- Observe both tasks independently. A `JoinError` from one task must not skip the other; the first observed failure remains sticky on later drain calls.
- Preserve existing startup semantics: orientation errors remain logged and nonfatal; rebuild aggregate statistics and per-corpus warnings remain unchanged; no readiness barrier is introduced.
- Make the completion boundary testable with real temporary Markdown/SQLite effects and private owning-module seams for deterministic cancellation and failure.
- Establish evidence that later Memory lifecycle work can build on without claiming more than these two task owners cover.

## Non-goals and anti-goals / 非目标与反目标

- No Memory or graph enable/disable switch, cold mode, stop/re-enable operation or generation fencing.
- No closing of `SqliteMemoryBackend`, `MarkdownSessionStore`, embedding providers or shared locks.
- No changes to CompressionService, DreamDaemon, MemoryProducerScheduler or event reconciler stop behavior.
- No provider hot replacement, descriptor/lease/Zahir/MCP/ACP work, Capability registry or generic maintenance supervisor.
- No moving startup work into the watcher, NoteIndexer or a second owner map.
- No readiness barrier, startup ordering redesign or early-startup-error cleanup expansion.
- No native unload, process isolation, executable-page reclamation or RSS claim.
- No dependency or lockfile changes.

The anti-goal is a successful-looking teardown that only joins the outer task while inner category tasks can still be running, or that labels this narrow maintenance boundary as “Memory stopped.”

## Approaches / 方案取舍

### 1. Direct local owner for the two startup tasks — selected / 直接持有两个启动任务——已选择

Retain the existing orientation and rebuild handles in a small `NoteMaintenanceOwner` placed beside the existing startup drain helper. The owner stores the two real `JoinHandle<()>` values, waits for them by mutable reference, records sticky join failures and drains the second task after the first fails. The common funnel awaits this same owner.

This keeps the authority where the tasks are created, changes only two spawn sites and one teardown point, and creates evidence for the next Memory design. Its limitation is deliberate: it covers only these two startup tasks.

### 2. Generic maintenance supervisor — rejected / 通用维护 supervisor——拒绝

A registry or reusable task supervisor could look uniform, but it would introduce a second lifecycle authority before the Memory resource inventory is complete. It would also encourage adding Compression, Dream and producer tasks without first defining their ownership and replacement contracts.

### 3. Whole Memory provider lifecycle — deferred / 一次完成 Memory provider 生命周期——延期

A single provider switch would target the final architecture directly, but current startup creates shared note/session resources unconditionally and distributes backend, indexer and embedding references to many consumers. It cannot safely promise stop or replacement until those consumers and their admitted work are accounted for.

## Fixed contract / 固定契约

1. Keep the existing task bodies and startup timing. Replace only the discarded `JoinHandle`s with retained values. Orientation bootstrap still logs its own `bootstrap` error and returns `()`. Full rebuild still logs aggregate stats and failed corpora and returns `()` from its wrapper.
2. Add one source-owned owner for the two handles. The owner is a concrete Memory-startup type, not a generic lifecycle framework, registry, global slot or second enabled decision. Internal field names are implementation choices; the owner must retain both real handles before the construction call returns.
3. Expose one in-place asynchronous drain boundary, for example `drain_note_maintenance(&mut self) -> Result<(), AlephError>` or the existing builder-scoped equivalent. It must await stored handles through mutable references, never take-and-await a pending handle, and never abort or detach work to make a test pass.
4. The drain has no internal timeout and no hard latency claim. If the caller cancels while either task is pending, the owner remains alive with the same handle identity. A later call continues observing those tasks.
5. Observe the orientation and rebuild tasks independently. If one returns `JoinError`, preserve source-local failure evidence and still observe the other. A later call remains `Err`; a successful repeated call returns `Ok` without spawning work.
6. The normal and `run_until_shutdown` error paths use the same common funnel. The drain runs before later teardown disposes the shared resources, warns on failure, preserves the original `run_result` and continues sibling cleanup.
7. A narrow `Drop` fallback may warn when an early startup error drops a pending owner, but it launches no new cleanup task, blocks on no join and reports no success. Early-startup-error cleanup is disclosed as a separate gap.
8. `full_rebuild_all` retains its existing per-corpus nonfatal behavior. If `reconcile_corpus` needs a localized JoinSet error-drain change to make the outer completion evidence truthful, preserve the first error, drain the remaining category tasks, then return the same error. Stop if that requires broad algorithmic restructuring.
9. The resulting evidence proves only that the two retained startup tasks reached their observable terminal result. It does not prove that watcher, graph, recall, Compression, Dream, producer, reconciler, embedding or shared database work has stopped.

## Evidence boundary / 证据边界

The implementation must prove:

- a real orientation bootstrap and real full rebuild complete their Markdown/SQLite effects before the owner returns `Ok`;
- cancellation while either task is pending retains the same `JoinHandle::id()` and retries the same work;
- successful drain is idempotent and creates no new task;
- failure of either task remains `Err` and does not prevent observing the other;
- the production common funnel drains the same owner on both normal and returned-error paths;
- the original startup statistics, warnings and nonfatal per-corpus behavior remain intact;
- if the category `JoinSet` error path is touched, its child completion behavior is tested directly.

Static funnel inspection is separate from OS-signal/live-server E2E. Neither is a proof of native thread reclamation, database closure or complete Memory quiescence.

## File boundary / 文件边界

Expected implementation scope:

- `src/bin/aleph-server/commands/start/mod.rs`: retain the two existing startup handles and create the concrete owner; add one common-funnel drain call. Keep this large file structurally unchanged.
- `src/bin/aleph-server/commands/start/builder/subsystems.rs` or a narrowly scoped adjacent private builder module: owner state, drain helper and owning-module tests. Do not create a generic supervisor.
- `src/memory/notes/indexer.rs`: touch only if source audit and a focused regression require a local JoinSet drain correction.
- `docs/reference/{FEATURE_LOCATOR,MEMORY_SYSTEM}.md`, `docs/reference/memory/NOTES.md`: update after implementation evidence, preserving the unimplemented proposal history and precise limits.
- New run record for actual authorization, validation, review and local commits.

## Progression / 后续顺序

1. Completed: source-owned `NoteVaultWatcher` shutdown and common teardown drain.
2. **This proposal:** source-owned completion for orientation bootstrap and startup full rebuild.
3. Separate resource census and architecture proposal for CompressionService, DreamDaemon, producer scheduler, event reconciler, embedding/provider references, shared NoteStore and raw session storage.
4. Only after that census: explicit Memory sub-capability cold/enable/stop/re-enable semantics and generation fencing.
5. Later: provider-neutral Gateway channel registry production integration, Sandbox lifecycle design, and capability projections where callable/effect evidence exists.

The selected approach advances the north star by making another externally managed runtime activity observable and stoppable at its source owner, while keeping the evidence honest about the rest of Memory.
