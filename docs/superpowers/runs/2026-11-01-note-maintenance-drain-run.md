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

### Source audit and RED setup / 源码审计与 RED 准备

- Authorization committed before production edits: `0bf60e1aa`.
- `reconcile_corpus` returned early from its category JoinSet on either `JoinError` or child `Err`; dropping the set can abort remaining category work without observing completion. The bounded correction is required. The existing collector loop is first extracted unchanged into a private `drain_category_tasks` boundary to test its real error/observation behavior, without adding a public seam or changing the indexing algorithm.
- Initial runtime RED uses a documented successful no-op `drain_note_maintenance` skeleton with real pending handles. This tests the missing lifecycle behavior rather than a missing API.
- First Cargo attempt was terminated by the tool's 60-second execution timeout while compiling; no tests ran. The subsequent run is detached and serial, with its own fresh memory ALLOW. Gate available bytes: `9015656448`, then `11227250688` (required `4294967296`). No success inferred from compilation.

- Owner runtime RED: `cargo test -p alephcore --bin aleph-server note_maintenance` selected 3 tests: **0 passed / 3 failed**, all because the no-op drain returned ready/success while work was pending. This is runtime lifecycle evidence, not a missing-type compiler error.
- JoinSet runtime RED: `cargo test -p alephcore --lib reconcile_corpus_drains_category_tasks_after_first_error` selected 1 test: **0 passed / 1 failed**, with `a category error must not return before the other category finishes`. The unchanged collector returned on the first child error while a gated sibling was pending.
- GREEN implementation now borrows both real task handles in place, clears only observed handles, and retains the first join failure even if the drain is cancelled while awaiting the sibling. The category collector records its first error and drains the remaining set before returning it. / 两处实现均先证明运行时缺陷，再补齐等待。
- Two task-specific spawn functions retain the original production wrapper bodies and logging fields, at the same startup positions. This local extraction lets real filesystem/SQLite tests exercise the production nonfatal warning/statistics wrappers; it introduces no generic lifecycle authority.
- Read-only research produced a usable source report, although the delegate process ended with SIGTERM at its limit. Its source observations were checked against the implementation; this research is not the required final independent review. / 研究报告可读，但进程以 SIGTERM 结束；不将其视为最终审查通过。

### Static production wiring / 静态生产接线

Compared both extracted task bodies with baseline `c732c2d575d508834cc36090c3f97a89af436ed2`; both are identical after whitespace normalization, including warning messages and all rebuild summary fields. Startup calls remain at the original positions; both actual handles enter the one concrete owner. / 两段任务体与基线逐项对比，仅空白变化。

The common source funnel stores `run_result = server.run_until_shutdown(shutdown_rx).await`, drains the existing watcher, then this maintenance owner, then closes the projection host and continues sibling teardown. There is no branch or result propagation between the server return and either drain; the original `run_result?` remains after teardown. This establishes source reachability for normal return and returned-server-error, not live-server/OS-signal execution evidence. / 正常返回与返回错误共用同一段代码；这是静态接线证据，不是信号或真机 E2E。

- The first GREEN attempt stopped at compilation with four `E0425`/`E0433` missing names in the startup module's explicit `use builder::{...}` list; no tests ran. Added the four imports and restarted the serial verification sequence after a fresh memory gate. / 首次 GREEN 因漏写四个导入而未进入测试，已修正；不将该次算成运行时测试。
- First fresh native Pi source review returned `Approved` (no P0–P2 findings), while its process exited SIGTERM after delivering the report. It did not run Cargo and missed the import defect caught by compilation; its approval is not a substitute for verification. Final re-review follows the complete verified change. / 首轮审查结论不代替编译与测试，最终变更将复审。

### GREEN and regression results / GREEN 与回归结果

All commands below ran serially with `CARGO_BUILD_JOBS=1`, each preceded by a fresh successful memory gate. Test filters selected nonzero cases. / 每次 Cargo 均先通过内存门，串行执行，过滤器均选中真实用例。

| Command | Passed | Failed | Filtered out |
| --- | ---: | ---: | ---: |
| `cargo test -p alephcore --bin aleph-server note_maintenance` | 5 | 0 | 123 |
| `cargo test -p alephcore --bin aleph-server note_watcher_drain` | 2 | 0 | 126 |
| `cargo test -p alephcore --lib memory::notes::indexer` | 39 | 0 | 21700 |
| `cargo test -p alephcore --lib memory::notes::watcher` | 15 | 0 | 21724 |

Total: **61 passed, 0 failed** in the final focused runs. The one JoinSet regression tests both child `Err` and panic, with a gated sibling that must complete before the collector returns its first error. The five maintenance tests cover both cancellation phases, exact retained IDs, cancellation after the first sticky failure, either JoinError direction, real Markdown/SQLite effects, idempotence and the real production wrappers' nonfatal warnings and all summary fields. / 最终定向测试合计 61 通过；未将 RED 失败、编译失败或未执行的测试计入通过数。

Compilation checks both passed with exit 0: `cargo check -p alephcore --tests` (4m55s) and `cargo check -p alephcore --bin aleph-server` (1m44s). / 两项必需编译检查均成功。Across all **10** Cargo invocations, including the initial compilation timeout, intended RED runs and the corrected GREEN sequence, all memory gates returned ALLOW; minimum available bytes were `8499691520`, above the `4294967296` requirement. Cargo ran serially with one build job throughout.

Affected-file `rustfmt --edition 2021 --config skip_children=true --check` passed for all four Rust files; `git diff --check` passed. Source SHA-256 fingerprints were recorded for these four files during the final serial verification sequence and are checked again at acceptance, so local commits and later documentation edits cannot silently substitute different source. The plan ledger's acceptance command validates those hashes and completed durable logs; it does not rerun or impersonate Cargo. / 最终验收核对源码哈希和真实日志，避免把另一个代码树的结果当成本轮结果。

Observed warnings: existing unread `PluginProjection::panel_snapshot` at `src/extension/projection.rs:140`, and macOS linker `__eh_frame section too large (max 16MB) to encode dwarf unwind offsets in compact unwind table, performance of exception handling might be affected`. Neither was changed in this slice. / 既有 dead-code 与 macOS unwind 警告保留。

### Independent native Pi review / 独立原生 Pi 审查

Two fresh read-only reviewer sessions used `88api/gpt-6.1-sol`. The second, `del_mvx67i57_qrfl`, explicitly rechecked the corrected startup imports and the complete production/test diff and returned **Approved**, with no high-confidence P0–P2 findings. It checked owner cancellation/sticky failure, independent sibling observation, original wrappers, watcher→maintenance→disposal wiring and the local category JoinSet fix. It did not run Cargo; this run's counts come from the main session's actual logs. / 第二轮明确核对导入修正与最终源码，结论 Approved；测试结果由主会话实际验证。

Both review processes exited SIGTERM after delivering complete review reports; this is the process outcome, not a clean-exit claim. The reports' explicit source verdicts were usable and have been read; they do not establish pending checks or E2E. The earlier research delegate likewise supplied a usable source report before SIGTERM. All three delegate runs are accounted for. / 三个子进程均如实记录退出状态，没有把进程退出方式或审查意见充当测试通过。

### Unrun checks and residual boundaries / 未运行验证与残余边界

No whole-workspace suite, clippy, cross-platform test, live-server/OS-signal E2E, native isolation/unloading/RSS measurement, hard shutdown deadline test or early-startup-error cleanup was performed. The cancellation tests' 10-second timeout is only a test deadlock guard; production drain has no internal timeout. Early startup abandonment can still drop/detach these task handles before the common funnel; no new Drop cleanup or success claim was added. / 未做全量、跨平台或真机信号验证；提前启动失败的清理仍不在本次范围。

This owner observes only the two existing startup tasks. It does not stop all Memory writers, close shared NoteStore/raw-session storage or make Memory cold. The next separately scoped architecture task is the shared resource/reference census covering Compression, Dream, producer scheduler, event reconciler and embedding before a global lifecycle switch. / 不将这两个任务完成等同于整个 Memory 静止；后续先盘点资源，再单独设计全局生命周期。

### Local Git delivery / 本地 Git 交付

- Authorization-first: `0bf60e1aa docs(memory): record note maintenance drain authorization`.
- Production: `791d49382 fix(memory): retain and drain startup note maintenance` (retention and common-funnel wait combined into one cohesive fix commit).
- Tests: `ff5989abd test(memory): strengthen note maintenance lifecycle evidence`.
- Final evidence/current-reference update: the documentation commit containing this completed record. / 最终文档提交包含本记录与当前引用更新。
- Branch `note-maintenance-drain` is retained in the existing checkout; local `main` stays at baseline `c732c2d575d508834cc36090c3f97a89af436ed2`. No remote push/merge, reset, rebase, shared-resource close or unrelated refactor. / 保留本地分支，main 不动，不推送、不自动合并。

### Final acceptance / 最终验收

All required focused tests and both compilation checks passed. The final acceptance command verified the six completed command logs and exact unchanged source SHA-256 values for all four Rust files. New/modified relative links: **9 checked, 0 broken**. Affected-file rustfmt and diff whitespace checks passed. The original watcher run record was preserved; current Memory references and this slice's design/plan/prompt status now point to observed implementation evidence. / 必需验证全部通过；源码哈希一致，9 个变更链接有效，历史 watcher 记录未改。

Tasks 1 and 3 were implemented together under Task 2's active TDD boundary after runtime RED; their source work is one cohesive fix commit. The plan-local ledger closes the grouped tasks using the durable-log/hash acceptance command, explicitly without a redundant Cargo rerun. The two fresh reviews read the actual source diff; a matching committed-source review package is archived in the ignored execution workspace. / 计划内任务按 TDD 依赖合并执行，验收复用实际日志，不把台账检查冒充新测试。

Delivery remains the local `note-maintenance-drain` branch in the single existing checkout, with `main` unchanged at the baseline above. This final evidence and current-reference update is committed separately; the checkout is checked again after that commit. No push or merge was performed. / 最终仅本地交付，文档另行提交后再次检查工作区，不推送、不合并。
