# Note maintenance owned drain run / 笔记维护任务收尾实施记录

## Authorization and baseline / 授权与基线

2026-11-01: The current user explicitly selected “execute existing plan” in the session's structured authorization question, approving the existing design and plan, a local implementation branch, this authorization-first commit, production changes, TDD, verification and fresh read-only native Pi review. The planning template alone was not treated as authorization. / 当前用户已明确选择执行既有计划，批准设计、建立本地分支、先提交授权记录、实施、验证与独立审查；没有将规划模板本身当成授权。

- Repository baseline: clean local `main` = `origin/main` = `c732c2d575d508834cc36090c3f97a89af436ed2`, containing the merged watcher shutdown slice and maintenance planning commit `a22dc1dd6`.
- Branch: `note-maintenance-drain`, created from local `main` in the existing checkout. One checkout/worktree; no remote mutation authorized.
- Design: [approved design](../specs/2026-11-01-capability-core-note-maintenance-drain-design.md).
- Plan: [approved implementation plan](../plans/2026-11-01-capability-core-note-maintenance-drain-plan.md).

## Contract and workflow / 契约与流程

Retain the two actual startup tasks (orientation bootstrap and full rebuild) in a concrete source owner. Await stored handles in place, independently; retain first JoinError as sticky evidence; preserve cancellation ownership, startup timing, existing logs/stats/nonfatal errors and original server result. Drain watcher first, then maintenance in the existing common teardown funnel. Audit and narrowly repair the category JoinSet's early-error observation if needed. / 只收拢两个既有启动任务；原位等待、独立观察、错误保持、取消不丢所有权；保持启动时序与现有日志/非致命语义。

Selected workflow: `executing-plans` with the main session as sole writer, TDD before implementation, read-only source research, then fresh native Pi review of the complete change. The durable run record and plan-local execution ledger record deviations and evidence. / 主会话唯一写入，先测试后实现，独立只读审查。

Rulings / 执行裁定:

1. Plan Task 2's lifecycle RED precedes Task 1's production retention edits, because the explicit TDD contract governs their ordering. A minimal no-op drain skeleton may be used only to establish runtime RED; missing-symbol compiler failures are not lifecycle evidence.
2. Use the explicitly requested local short branch in the existing checkout, rather than adding a linked worktree. No concurrent writer will share it.
3. User-specified focused checks and disclosure of unrun checks govern over generic full-suite skill defaults. User-required re-review after concrete fixes governs over the execution skill's generic one-review rule.

## Planned verification / 计划验证

Before every Cargo invocation: `python qa/browser_live/check_memory.py`; continue only on ALLOW with at least 4 GiB available. Run Cargo serially with `CARGO_BUILD_JOBS=1`.

- Runtime RED/GREEN: pending/cancellation, exact retained IDs, independent JoinErrors, sticky failure, idempotence; real temporary Markdown/SQLite bootstrap and rebuild effects; existing nonfatal logs and failed corpora; local JoinSet regression if changed.
- Focused binary `note_maintenance` and `note_watcher_drain` tests; library `memory::notes::indexer` and `memory::notes::watcher` tests.
- `cargo check -p alephcore --tests`; `cargo check -p alephcore --bin aleph-server`.
- Affected-file rustfmt with `--config skip_children=true`, `git diff --check`, relative-link checks and static common-funnel audit.
- Fresh read-only native Pi review; fix concrete issues and re-review.

## Boundaries / 边界

No Memory switch, graph stop, backend/raw-store close, Compression/Dream/producer/reconciler/embedding lifecycle, Capability/Zahir/MCP/ACP integration, generic supervisor, watcher redesign, new dependency, Sandbox change, native/RSS claim, automatic push or merge. Early startup error cleanup remains excluded; no hard shutdown latency guarantee. / 不扩大全局 Memory 生命周期，不自动推送或合并。

## Evidence / 实测证据

Implementation and validation pending at this authorization commit. / 本授权提交时尚未实施或验证。
