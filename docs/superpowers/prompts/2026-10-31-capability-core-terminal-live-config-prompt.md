# Next Session: Terminal Async Live-Config Closure
# 下一会话：Terminal 异步配置启停闭环

Implement only the bounded Terminal explicit live-config slice in the companion design and plan. Read them completely before production edits. This prompt becomes implementation authorization only when the user explicitly asks to execute it; merely reading this file is not approval.

只实施配套 design/plan 中受限的 Terminal 显式配置启停闭环。用户在新会话明确要求执行本 prompt 时，记录其对这两份文档的实施授权；本文件自身不声称本次规划已批准或实施生产修改。保留授权边界，不再实施上一阶段已完成的生命周期重写。

## Locate and confirm / 定位与核对

Use `git rev-parse --show-toplevel` for this machine's checkout. Do not assume an absolute repository path. Check `git status --short --branch`, `git worktree list --porcelain` and local branches before editing.

Previous work is already on local `main`: `1505d06e5` approval, `2372b2c22` lifecycle implementation, `075b508a8` references/validation. The last session had only the main checkout and local `main`; there was no separate worktree/branch to merge or delete. These changes and this planning package were kept local. Verify those facts; do not fabricate a merge, delete main, or reset to an older remote.

上一阶段已经在本地 main，无额外会话 worktree/分支。若需新 worktree，从包含本交接文档的本地 `main` 建立短分支，例如 `terminal-live-config`；不得从缺少本地实现的旧 remote tip 开始。保留用户改动，不自动 push/merge remote。

## Read first / 首先阅读

1. `AGENTS.md`; `docs/reference/REDLINES.md`, `ARCHITECTURE.md`, `GATEWAY.md`, `TERMINAL_RUNTIME.md`, `FEATURE_LOCATOR.md` current Capability/Terminal sections; `DEVELOPMENT.md` validation/memory rules.
2. `docs/superpowers/specs/2026-10-30-capability-core-source-owned-contract-design.md`.
3. `docs/superpowers/specs/2026-10-31-capability-core-terminal-lifecycle-design.md` and `docs/superpowers/runs/2026-10-31-terminal-lifecycle-run.md` (completed baseline, including limits).
4. **New design:** `docs/superpowers/specs/2026-10-31-capability-core-terminal-live-config-design.md`.
5. **New plan:** `docs/superpowers/plans/2026-10-31-capability-core-terminal-live-config-plan.md`.
6. `docs/research/aleph_modularization_plan.md` §0.4 and implementation roadmap; `docs/research/Everything-externally-composable-is-a-Capability.md` lifecycle/effects sections. These inform the direction, not blanket authorization for the whole roadmap.
7. `src/config/live_apply.rs`, `src/config/patcher.rs`, `src/config/reload_impact.rs`; `src/gateway/pty/{mod,manager,session}.rs`; `src/gateway/handlers/config.rs`; current execution/search live-apply callers.
8. `src/bin/aleph-server/commands/start/mod.rs` patcher construction and shared bus; `builder/handlers/config.rs` reload registration; `src/gateway/server/mod.rs` boot tests; existing self_config and patcher test fixtures.

If reference inspection adds value, locate the local `deepseek-harness` checkout using this machine's reference workspace, but do not copy its separate registry/service or repeat unrelated research. The previous run already used it to check post-setup publication and failure evidence.

## Mission / 目标

A Gateway started with `policies.terminal.enabled = false` must stay cold, then become usable after an explicit supported configuration update without a process restart. A normal explicit disable must await the existing flush driver's join before reporting the Terminal target live. This finishes the previous plan's permitted sync/cold-bus fallback; it does not replace the completed source lifecycle.

目标链路：disabled boot → explicit config enable → one Ready driver → real `pty.screen` on the existing Gateway bus → explicit config disable and source-owned join → re-enable with a new checked generation。

Supported write faces are patch, shared-patcher self_config update/rollback, rollback and explicit RPC reload. The automatic file-watcher loop remains outside this slice.

## Fixed implementation contract / 固定契约

- One async `apply_live_sections(cfg: &Config, top_sections: &[&str], event_bus: Option<&Arc<GatewayEventBus>>) -> Vec<&'static str>` executes the existing table. No sync twin or hand-inlined per-handler Terminal implementation.
- Add `ConfigPatcher::with_event_bus(self, event_bus: Arc<GatewayEventBus>) -> Self`, storing an optional clone of the **existing** server bus. Default offline construction has `None` and no PTY effects. Production patcher is wired before sharing it with RPC/tools.
- Explicit reload receives the existing `Arc<GatewayEventBus>` from its registration context. Execution/search-only callers await the shared executor with `None` and their original exact targets.
- Enable requires that explicit bus context. A Stopping manager first completes its retained stop; only then may activation create the next generation. No global bus discovery, replacement while Ready or extra runtime authority.
- Disable uses `try_manager` and awaits existing `deactivate`. Absent manager plus disabled is satisfied without initialization. Use the design's local **5-second async wait budget**; it is not a hard bound on blocking PTY work.
- Success includes Terminal in `live_applied` only after required source evidence. Timeout, cancel and genuine failure never count as success or discard ownership/evidence. Persisted success remains distinct from live application. Missing context continues to downgrade honestly.
- Recovery after an incomplete persisted disable is explicit `config.reload`, which must finish the retained join through the real handler. Identical patch/self_config updates remain no-ops: no lifecycle retry, backup, write or mtime change, and no fabricated live result. Preserve and test that behavior.
- No Config write guard across join. After installing/reloading, apply the latest shared Config under a read guard held through bounded completion; audit reverse lock acquisition. Do not replace this ordering with an unlocked enabled snapshot or a new epoch/enabled map.

## Test-first and execution / 测试先行与执行

Use the plan task-by-task with a subagent-driven workflow or equivalent native task/review ledger. One writer per checkout. Before production edits record the user's explicit execution authorization, baseline commits and scope in the new run record. Read TDD and current Rust test guidance before writing tests.

For each meaningful behavior, capture the intended RED, implement minimally and capture GREEN; do not claim the whole failure matrix RED from a single missing-API compilation error. Use real PTYs, real temp config persistence and actual Gateway bus delivery. Cold-global proofs use a fresh subprocess as the existing router test does, so they remain valid inside a combined suite. Preserve global PTY/spend serial keys.

Required cases include same-bus cold enable, repeated enable, awaited stop, Stopping→enable, new generation/stale rejection, timeout/cancel retaining the stored driver, genuine failure without Live, exact target verdicts, rollback/reload/self_config, identical-patch no-op followed by explicit-reload recovery, dry-run/save failure without effects and competing config writes. No manual test cleanup call may supply the driver join the executor was supposed to perform.

## Acceptance and checks / 验收与检查

Follow the exact check list in the plan: focused PTY/handler/live_apply/patcher/config/self_config/execution/search/policy/provider/router/visibility filters; a combined run; `cargo check -p alephcore --tests`; `cargo check -p alephcore --bin aleph-server`; affected Rust formatting; `git diff --check`. Run `python qa/browser_live/check_memory.py` before each Cargo command and use `CARGO_BUILD_JOBS=1` for tests. Do not run Cargo concurrently in writer/reviewer contexts.

The previous 194-pass result is historical. Report actual new counts, failing/unrun checks and warnings. Review independently before completion; fix concrete findings and re-review. Update current FEATURE_LOCATOR/reference paragraphs after tests, preserve historical phase records, and write `docs/superpowers/runs/2026-10-31-terminal-live-config-run.md` with observed evidence and local commits.

## Stop conditions and deferred work / 停止条件与延期

Stop for a source-backed architecture report if bus/config wiring requires a second registry, owner/enabled map, runtime table, durable store or bus singleton; if join needs reverse Config locking; if driver cleanup must be detached; or if Core/ProviderHandle would gain Gateway/platform effects. Do not turn a blocker into a broader redesign silently.

Keep all existing authorities and owner predicates. No Terminal descriptor/BackendLease/Zahir/MCP/ACP, Generic Gateway mount, Feishu, Memory/Sandbox, new kinds, subscribe, durable recovery, EffectClaim, replay, scheduler, harness, native unload/isolation/RSS promises. Driver join and kill/observed-exit evidence do not prove universal process/thread reclamation. The previous untested actual WebSocket async-visibility timing remains a disclosed limit unless this change touches that boundary.

Deliver implementation/doc commits, exact verification and review verdict, what remains deferred/unproven, and final Git/worktree state. Do not push or remotely merge automatically.
