# Restart prompt: note-vault watcher owned shutdown / 重启实施提示

**Status / 状态: Unimplemented / 未实施。** Use the companion design and plan to implement **only** the bounded source-owned shutdown of the existing note-vault watcher.

只实施既有 NoteVaultWatcher 的可等待收尾及生产 teardown 接线。This prompt becomes production authorization only when the user explicitly asks to execute it. Reading, reviewing or receiving this file alone is not approval. / 用户明确执行本 prompt 才构成实施授权；规划阶段没有授权生产改动。

## Locate and confirm / 定位核对

1. Run `git rev-parse --show-toplevel`, `git status --short --branch`, `git worktree list --porcelain` and inspect local branches before editing. Do not assume a machine-specific checkout path.
2. Completed Terminal changes were committed as `88fe4fc4c` (authorization), `cf52bb05a` (implementation), `b378b87bd` (reference/evidence), then fast-forward merged into local `main`. Verify ancestry. The old `terminal-live-config` branch/worktree may remain as historical delivery; do not mistake it for the new task or delete it without need.
3. Start a short branch such as `note-watcher-stop` from local `main` containing these commits **and this planning package**. Do not start from a stale remote missing local work, reset main, fabricate a merge or overwrite user edits. No automatic push/remote merge.
4. Before production edits record explicit current-user execution approval, baseline, scope and selected native/subagent workflow in `docs/superpowers/runs/2026-10-31-note-watcher-shutdown-run.md`. Commit that authorization record first. Planning documents alone are not approval.

## Read completely / 完整阅读

- `AGENTS.md`; `docs/reference/{REDLINES,CODE_ORGANIZATION,DEVELOPMENT,WORKSPACE}.md` applicable rules; memory and Gateway routing in `SUBSYSTEM_ROUTING.md`.
- **Design:** `docs/superpowers/specs/2026-10-31-capability-core-note-watcher-shutdown-design.md`.
- **Plan:** `docs/superpowers/plans/2026-10-31-capability-core-note-watcher-shutdown-plan.md`.
- `docs/reference/MEMORY_SYSTEM.md`, `docs/reference/memory/NOTES.md` watcher/indexer portions; `FEATURE_LOCATOR.md` current Capability/Memory/Terminal progress; previous Terminal run for limits, not as fresh verification.
- `docs/research/aleph_modularization_plan.md` §0.4 and roadmap, `Everything-externally-composable-is-a-Capability.md` Scope/lifecycle/effects sections. These explain the goal, not permission to execute all roadmap items. The dated §0.4 transition/mount baseline must be reconciled with current production source.
- `src/memory/notes/watcher.rs`, its `NoteIndexer`/`NoteStore` consumers and existing real-filesystem tests; actual resolved `notify-debouncer-full` 0.7.0 `Debouncer::{stop,stop_nonblocking,Drop}` source.
- `src/bin/aleph-server/commands/start/mod.rs` shared indexer/startup rebuild/watcher construction, common post-run-result teardown and final run-result return; `builder/subsystems.rs` helper placement and test conventions.
- TDD/current Rust test guidance and verification/review skills before tests/claims. One writer per checkout.

If reference inspection adds value, locate local `deepseek-harness` on this machine and inspect ownership/failed-cleanup evidence. The prior Terminal run already established setup-before-publication and failure retention; do not repeat broad ecosystem research, copy another registry/service, or create a generic supervisor.

## Mission / 目标

The existing watcher must retain the real consumer JoinHandle. `NoteVaultWatcher::shutdown(&mut self) -> Result<(), AlephError>` must stop the producer through one retained blocking stop helper, then observe the consumer drain to completion. Cancellation retains pending handles in the same source owner. A later call resumes the same work; failures stay failures.

本次为 Memory lifecycle 的先决资源边界，不是完整 Memory provider，不增加 graph/memory 开关；不会通过关闭图页面或 watcher 声称共享数据库、raw memory、session 或全部 Memory 已 cold。

## Fixed contract / 固定契约

- Existing spawn API and sole owner stay. No registry/global slot/second enabled or owner map. Store consumer before returning success.
- Run consuming `Debouncer::stop` in one source-owned `spawn_blocking` task, storing its handle before awaiting. No blocking join on a Tokio worker, no take-then-await on a pending handle, no abort/discard helper to make a timeout pass.
- Await stored tasks by mutable reference. Producer stop return and consumer completion are separate observations. `stop` hides native thread join errors, so do not claim native-thread success/no-panic, all OS resources reclaimed or RSS reduction.
- Drain batches already emitted into the queue without cancelling their indexing/embedding midway. No promise for not-yet-emitted debounce events; existing startup reconcile is the recovery path. Keep per-file errors/logging behavior unchanged.
- Preserve terminal JoinError evidence so repeated calls cannot convert failure into Ok; attempt independent cleanup despite an error. Repeated successful shutdown is Ok without creating tasks. No internal timeout or hard shutdown latency claim.
- Drop only uses synchronous nonblocking fallback and warns about unobserved drain; it starts no new cleanup worker, blocks on no join and reports no success. Explicit cancellation safety requires keeping the owner alive for retry. Full async early-startup-error cleanup remains deferred.
- Add `pub(in crate::commands::start) async fn drain_note_vault_watcher(watcher: &mut Option<NoteVaultWatcher>) -> Result<(), AlephError>` in existing builder/subsystems. None is cold/satisfied; Some borrows the same owner in place. At the common post-`run_until_shutdown` funnel call it once and await; warn on failure while preserving sibling cleanup and original `run_result`.
- The separate detached startup `full_rebuild_all` task remains untouched and disclosed. Do not claim this drain closes all Memory writers or proves a whole-Memory replacement safe.

## TDD and verification / 测试与验证

Follow the plan's named cases and record meaningful runtime RED/GREEN, not an entire matrix inferred from one missing-method compiler error. Use real temp roots/SQLite and real watcher delivery for effects, owning-module private barriers around real work for deterministic cancellation/failure. No public test-only APIs to bridge lib/bin privacy; binary helper tests use normal APIs and source tests prove handle semantics. No manual cleanup join may supply the executor's missing completion.

Required cases: real emitted batch drains before success; exact consumer/stop-helper IDs survive cancellation in each phase; both failure directions retain Err and attempt other cleanup; idempotent success; replacement after old success receives real edits; consumer indexer Arc release; absent root no side effects; truthful nonblocking Drop fallback. Preserve existing create/delete, categories/scoped corpora/bulk and non-NotFound error behavior.

Use the plan's eight Cargo invocations: four focused lib groups, their combined run, binary drain tests, `cargo check -p alephcore --tests`, `cargo check -p alephcore --bin aleph-server`. Before **every** Cargo run `python qa/browser_live/check_memory.py`; proceed only if ALLOW and ≥4 GiB. Use `CARGO_BUILD_JOBS=1`; no concurrent Cargo including reviewers. Capture actual selected test counts; zero selected needs investigation. Run bounded affected-file rustfmt and `git diff --check`.

Independent fresh read-only native Pi review is required. Fix concrete findings and re-review. Distinguish helper behavior, source/static common-funnel wiring and actual OS-signal live E2E; do not claim unrun QA. Old Terminal 271/271 and its two Approved reviews are historical evidence, not validation of this slice.

## References, commits and delivery / 文档、提交与交付

After tests update current FEATURE_LOCATOR, MEMORY_SYSTEM and memory/NOTES watcher paragraphs; preserve historical records. Write exact source boundary, native stop error limitation, cancellation/Drop/backlog behavior and deferred scope. Finish the new bilingual run record with actual counts, failed attempts, warnings, unrun checks, independent review verdict and local commits. Check relative links and factual source names.

Commit intended source and reference changes with conventional messages; suggested sequence is authorization, `fix(memory): retain note watcher shutdown work`, `fix(memory): await note watcher during shutdown`, `docs(memory): record watcher shutdown evidence`. Keep branch/worktree for user review. Report final Git state, no automatic push or remote merge.

## Stop/deferred / 停止条件与延期

Stop for a source-backed architecture report if the debouncer cannot safely move to the retained blocking task on a supported target, cancellation forces loss of ownership, a second authority is necessary, or broad startup restructuring is required. Do not silently redesign.

No new Memory config, full-Memory/graph disable, startup-rebuild ownership change, backend close, ProviderHandle/kind/descriptor/BackendLease/Zahir/MCP/ACP, production generic channel mount, Feishu specialization, Sandbox security changes, dependencies, durable recovery/EffectClaim/replay/scheduler/harness, native unloading/isolation/RSS promises. Terminal's actual WebSocket stop/re-enable interleaving remains untested unless separately authorized and touched.

The next follow-up after this slice is a distinct Memory resource census/maintenance-owner design, followed by explicit cold/enable/stop/re-enable and generation fencing only after shared consumers are accounted for. / 完成后再独立规划 Memory 共享资源与整体启停，不提前宣称已完成终极架构。
