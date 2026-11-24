# Memory Derived Writer Boundary Closure Plan / Memory 派生写入边界闭合计划

**Date / 日期:** 2026-11-02
**Status / 状态:** Plan prepared for the next execution session; production changes not authorized by this document alone. / 已为下一执行会话准备计划；本文件本身不构成生产修改授权。

## Current position / 当前进度

The bounded Memory lifecycle implementation is merged into local `main` at `3635236f4`, with implementation, evidence, and direct verification fixes in local commits `6745cdc21`, `4e53aafbe`, `d940a6ab2`, and `3635236f4`. The source-owned `MemoryRuntime` now owns derived resources, admission, retained workers and shutdown cleanup. Raw chat and raw session archival remain separately available while Memory is Off. Providerless live QA passed 100/100 and configured-provider live QA passed 108/108. The focused lifecycle suites, session-store suite, Core/server checks, clippy, Linux desktop cross check and Panel WASM check passed.

The remaining gap is a proof boundary. The runtime can only drain derived work that enters through its source-owned operation. The current implementation has already closed the main capture, delegation, worker, session-end and RPC paths, but the source census still contains candidate callback and child-work sites that must be classified against the actual current code. Until that census is complete, do not claim that every derived Memory writer is covered by the lifecycle owner or design a broader global switch.

The full workspace command still exits 101 because of failures in unchanged swarm, browser, browser-tool QA, runtime-manage, extension panel/projection, cluster approval and Discord fixture paths. Those failures are outside this plan unless a new source comparison proves a direct regression.

## Goal / 目标

Complete a source-backed census of Memory-derived callbacks, child tasks and effectful service paths. For every candidate, establish exactly one of these outcomes:

1. it is source-admitted and its actual work is retained by `MemoryRuntime`;
2. it is raw-only and intentionally remains available when Memory is Off;
3. it is pure synchronous computation with no Memory effect or owned lifetime; or
4. it is standalone/test-only and has no production lifecycle obligation.

Implement only the confirmed missing source-operation boundaries. Preserve existing behavior, task cadence, effect ordering, raw archival and standalone compatibility.

## Why this is next / 下一步原因

The completed runtime proves lifecycle behavior at its registered boundaries. A census closure proves that a new derived callback cannot silently bypass admission and continue after stop. This is the smallest useful next step toward the north-star requirement that externally managed runtime work has an explicit owner and observable lifetime.

## Scope / 范围

Start with the existing leads in the current source, then confirm them before editing:

- `src/agents/subagent_tool/spawn.rs` delegation completion and its A2A/spawner propagation;
- `src/memory/session_compactor/` and `src/memory/transcript_indexer/` capture filters and writer/indexer recreation;
- `src/memory/assembler/gather.rs`;
- `src/memory/dreaming/stages/{graph_recompute,mention_weave}.rs`;
- `src/memory/notes/graph/{minhash,relevance}.rs`;
- `src/memory/notes/profile/prompts.rs`;
- `src/memory/tool_signal_sink.rs`;
- `src/memory/flush/{mod,registry}.rs`;
- `src/memory/session_resume/writer.rs`;
- `src/memory/session_search_summary/synthesizer.rs`;
- any remaining production `spawn`, `spawn_blocking`, `JoinSet`, callback dispatch or concrete derived-service capture found by the census.

These are investigation leads, not assumed defects. Some may be pure CPU, raw-only, inline-owned, test-only or already source-admitted.

## Non-goals / 不在范围

- No new Memory enable/disable architecture, generation authority, registry, generic supervisor or Capability kind.
- No rewrite of raw `MarkdownSessionStore`, raw compaction or ordinary chat behavior.
- No provider hot replacement, native unloading, process isolation, RSS claim, distributed fencing or exactly-once external-effect guarantee.
- No changes to unrelated browser, swarm, cluster approval, Discord or extension tests merely to make workspace green.
- No Linux/Windows native execution; it was explicitly waived for this task.
- No broad refactor when a local source-owned operation boundary is sufficient.

## Working contract / 工作契约

- Use the existing `Weak<MemoryRuntime>` injection and `MemoryOperation` APIs. Do not create a second runtime accessor or global lifecycle state.
- A derived callback must capture or inherit the current operation before its effect begins. The actual callback and associated write must remain under source ownership if the caller future is cancelled.
- A raw-only path may bypass Memory admission only when its behavior is demonstrably raw archival and it does not mutate derived Memory state. Off must preserve that path.
- Pure computation may remain outside Memory ownership only when it performs no Memory I/O, provider call, cache mutation or retained asynchronous work. If it feeds a later derived effect, the effectful boundary still needs ownership.
- Preserve operation identity through nested tasks. Do not rely on Tokio task-local propagation across a spawned task without explicitly carrying the operation.
- Do not hold source, registry or tree locks across await or I/O. Do not abort, detach, replace or manually join an admitted production task.
- Keep comments in English and durable documents bilingual. Use existing standalone constructors and behavior for unbound tests.

## Execution steps / 执行步骤

### 1. Establish the baseline and census ledger

Create a short run record under `docs/superpowers/runs/2026-11-02-memory-derived-writer-boundary-run.md` before production edits. Reconfirm the clean `main` baseline and read the project references, current lifecycle design/plan/run record, runtime modules, raw-session boundary and relevant worker modules.

Build a ledger with one row per candidate containing: file and function, trigger, effect type, current operation source, actual task owner, raw/derived classification, cancellation behavior, existing test, and decision. Search by behavior (`dispatch`, `spawn`, `spawn_blocking`, `JoinSet`, backend/provider/cache writes), not only by `MemoryBackend` type names.

### 2. Produce meaningful RED evidence for confirmed gaps

For each confirmed missing derived boundary, add the smallest same-module or nearby real-store test before implementation. Use real temporary Markdown/SQLite resources and a `Notify`/semaphore handshake. The test must prove an observable failure such as:

- Off skips the derived callback while the raw effect remains available;
- a Running callback is admitted before its effect and stop remains pending while it is held;
- cancelling the caller does not cancel the callback or associated write;
- a rebuilt/recreated consumer retains the same WeakRuntime boundary;
- sibling work is observed when one child fails.

Do not treat a missing field or compiler error as lifecycle RED. Do not use sleeps, global mocks, public test-only APIs or manual joins outside the real owner.

### 3. Implement only confirmed local boundaries

Propagate the existing optional `Weak<MemoryRuntime>` through the smallest production constructor/recreation path. At the effect boundary, reuse `insert_with_runtime_capture_filter`, `MemoryOperation::run`, `spawn`, or `run_blocking` as appropriate. Keep raw fallback behavior explicit and preserve existing nonfatal error/logging semantics.

If the site is already owned, add evidence and avoid code churn. If it is pure or raw-only, record why it is outside the derived lifecycle. Stop if closure requires changing the raw owner, adding a generic scheduler, or redesigning service construction.

### 4. Run focused GREEN checks

Before every Cargo command, run:

```text
python qa/browser_live/check_memory.py
```

Proceed only when it reports `ALLOW` with at least 4 GiB. Set `CARGO_BUILD_JOBS=1` and `CARGO_INCREMENTAL=0`; run Cargo serially. Run the exact affected module tests with positive selected counts, then:

- `cargo check -p alephcore --tests`;
- `cargo check -p alephcore --bin aleph-server`;
- relevant lifecycle suites for extensions, session compaction/indexing, Dream, flush, events and subagents;
- `cargo clippy --workspace --all-targets` only if the changed production boundary warrants it.

Record actual selected/pass/fail/ignored counts. Do not reuse older snapshots as current evidence.

### 5. Review and classify the result

Run a fresh read-only review against the entire working tree and this plan. The reviewer must check operation ownership, raw-vs-derived separation, cancellation, reconstruction propagation, lock order, and whether the ledger supports the final claim. Fix concrete findings and re-review. Keep the unchanged workspace failures as disclosed blockers; do not expand scope to unrelated failures.

### 6. Document and commit locally

Update the bilingual run record with the ledger, source boundaries, RED/GREEN evidence, warnings, unrun checks and remaining claims. Update `docs/reference/MEMORY_SYSTEM.md` or `FEATURE_LOCATOR.md` only if the current contract changes; preserve historical records. Use conventional commits, for example:

```text
fix(memory): close derived callback admission gaps
test(memory): prove derived writer ownership boundaries
docs(memory): record derived writer census
```

Keep the branch/worktree for review. Do not push or merge automatically.

## Stop conditions / 停止条件

Stop and report a source-backed blocker if:

- a candidate requires closing or changing raw session storage;
- its ownership cannot be represented by the existing `MemoryRuntime` and `MemoryOperation` without a second authority;
- a local boundary would require a broad service or startup rewrite;
- a test cannot distinguish caller cancellation from owned completion without sleep races or public test hooks;
- a remaining failure is in an unchanged workspace path and has no direct relation to this census.

## Acceptance / 验收

The task is complete when the ledger covers every discovered production candidate, each row has a source-backed classification, every confirmed derived effect is admitted and retained or has a documented existing owner, focused tests prove the new boundaries, current checks pass for the changed scope, the fresh review is accepted, and the run record states exactly what remains outside the guarantee. This task does not authorize a universal “all Memory writers stop” claim.
