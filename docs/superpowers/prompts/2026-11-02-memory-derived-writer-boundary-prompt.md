# Restart Prompt: Memory Derived Writer Boundary Closure / 重启提示：Memory 派生写入边界闭合

**Status / 状态: Unimplemented / 未实施。** This prompt is the execution handoff for the next bounded task. The previous Memory runtime lifecycle work is already merged into local `main`; this prompt authorizes only the scope below when explicitly invoked by the user in the next session. / 本 prompt 是下一有界任务的执行交接。上一阶段 Memory runtime 生命周期工作已合并到本地 `main`；只有下一会话明确执行本 prompt 时，才可修改生产代码。

## Mission / 目标

Close the remaining source-backed lifecycle boundary gap around Memory-derived callbacks and child work. Build a complete census of production Memory callback dispatch, `spawn`, `spawn_blocking`, `JoinSet`, provider/cache effects and derived service captures. Classify every site as source-admitted, raw-only, pure synchronous, or standalone/test-only. Implement only confirmed missing source-operation propagation using the existing `Weak<MemoryRuntime>` and `MemoryOperation` contracts.

This is a prerequisite for any future claim that all derived Memory writers are owned by one runtime. It is not a new global Memory switch and does not expand raw archival shutdown.

## Starting state / 起始状态

- Work from the current clean local `main`, expected HEAD `3635236f4` or later.
- The completed commits are `6745cdc21`, `4e53aafbe`, `d940a6ab2`, and `3635236f4`.
- The completed bounded runtime already owns derived resources, admission, retained workers and shutdown cleanup. Raw chat/session archival remain independently available Off.
- Providerless live QA passed 100/100; configured-provider live QA passed 108/108. Focused lifecycle and session-store checks passed. Workspace still has disclosed failures in unchanged swarm/browser/browser-tool QA/runtime-manage/extension and cluster/Discord paths; do not repair those under this prompt.
- User waived native Linux/Windows execution for this task.

## Read before editing / 编辑前完整阅读

Read repository `AGENTS.md`, the applicable `docs/reference/{REDLINES,CODE_ORGANIZATION,DEVELOPMENT,WORKSPACE,SUBSYSTEM_ROUTING}.md`, the approved design and completed run record:

- `docs/superpowers/specs/2026-11-01-memory-runtime-lifecycle-design.md`
- `docs/superpowers/plans/2026-11-01-memory-runtime-lifecycle-plan.md`
- `docs/superpowers/runs/2026-11-01-memory-runtime-lifecycle-run.md`
- `docs/superpowers/plans/2026-11-02-memory-derived-writer-boundary-plan.md`

Read the current runtime operation, capture helper, session compactor/indexer, worker, subagent delegation, Dream/flush/event/session summary paths before deciding that any lead is a defect.

## Investigation leads / 调查入口

Confirm these against current source; do not assume they are all defects:

- `src/agents/subagent_tool/spawn.rs` delegation completion and A2A/spawner propagation;
- `src/memory/session_compactor/` and `src/memory/transcript_indexer/` capture paths;
- `src/memory/assembler/gather.rs`;
- `src/memory/dreaming/stages/{graph_recompute,mention_weave}.rs`;
- `src/memory/notes/graph/{minhash,relevance}.rs`;
- `src/memory/notes/profile/prompts.rs`;
- `src/memory/tool_signal_sink.rs`;
- `src/memory/flush/{mod,registry}.rs`;
- `src/memory/session_resume/writer.rs`;
- `src/memory/session_search_summary/synthesizer.rs`;
- all remaining production Memory-related `spawn`, `spawn_blocking`, `JoinSet`, callback dispatch and concrete provider/backend/cache captures.

## Fixed constraints / 固定约束

- Main agent remains the sole writer. Use read-only investigation/review children when useful; never allow concurrent writers in one worktree.
- Use the existing `Weak<MemoryRuntime>` and `MemoryOperation`; do not add a second registry, global runtime, generation authority, generic lifecycle supervisor, Capability kind or provider proxy.
- A derived callback must admit before its effect begins, and the actual callback plus associated write must remain source-owned when its caller is cancelled.
- Raw-only archival continues Off and may bypass Memory admission only when it performs no derived mutation. Ordinary chat must remain available.
- Preserve operation context explicitly across spawned tasks. Do not rely on task-local propagation through `tokio::spawn`.
- Do not hold lifecycle/registry/tree locks across await or I/O. Do not abort, detach, replace or manually join an admitted production task.
- Preserve cadence, ordering, nonfatal logging and standalone constructors. Comments English; durable documents bilingual.
- No sleep races, global mocks, public test-only APIs, unrelated refactors or fixes for unchanged workspace failures.

## Required workflow / 必须流程

1. Confirm Git state and write `docs/superpowers/runs/2026-11-02-memory-derived-writer-boundary-run.md` before production edits.
2. Create a ledger row for every candidate: path/function, trigger, raw/derived effect, admission source, owner, cancellation behavior, existing test and classification.
3. For each confirmed missing derived boundary, write a real-store RED using temporary Markdown/SQLite and Notify/semaphore handshakes. A compiler error alone is insufficient.
4. Implement the smallest local source-operation propagation. If a candidate is already owned, record evidence and do not rewrite it. If it is pure/raw-only/standalone, document the reason.
5. Before every Cargo command run `python qa/browser_live/check_memory.py`; require `ALLOW` with at least 4 GiB. Set `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`, run serially, and require positive selected counts.
6. Run affected focused tests, `cargo check -p alephcore --tests`, `cargo check -p alephcore --bin aleph-server`, and any justified clippy run. Record exact results.
7. Obtain a fresh read-only review of the entire current diff. Fix concrete findings and re-review.
8. Update the bilingual run record, then commit locally with conventional messages. Do not push or merge.

## Required behavior evidence / 必须证明的行为

- Off skips derived callbacks while raw archival remains observable.
- Running admits the derived callback before its effect and stop remains pending while the callback is held.
- Cancelling the caller does not cancel an admitted callback or associated write.
- Recreated indexer/writer/service paths preserve the same WeakRuntime boundary.
- Sibling tasks are observed when one fails, and existing error/logging behavior remains intact.
- The final census supports the exact claim made in the run record; it must not imply that every raw writer or unrelated workspace test is Memory-derived.

## Stop conditions / 停止条件

Stop and report if closing a gap requires changing raw session ownership, adding another lifecycle authority, redesigning startup/service construction broadly, weakening a fail-closed boundary, or hiding an unchanged workspace failure. A candidate that cannot be classified from source and real effects is a blocker for this task, not permission to guess.

## Delivery / 交付

Keep the final branch/worktree local. Report completed files, census classifications, exact RED/GREEN counts, review verdict, unrun checks, remaining workspace blockers and explicit non-actions. Do not claim universal all-writer stop, native unload, RSS reduction, process isolation or exactly-once external effects.
