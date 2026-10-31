# Terminal explicit live-config run / Terminal 显式配置闭环实施记录

Date / 日期：2026-10-31

Status / 状态：Completed bounded implementation and planned validation / 受限实施与计划验收已完成。

## Authorization and baseline / 授权与基线

The user explicitly requested execution of the companion [prompt](../prompts/2026-10-31-capability-core-terminal-live-config-prompt.md), [design](../specs/2026-10-31-capability-core-terminal-live-config-design.md) and [plan](../plans/2026-10-31-capability-core-terminal-live-config-plan.md) in this session. Authorization was recorded and committed before production edits. / 用户在本会话明确要求执行配套 prompt、design 与 plan；在生产修改前记录并提交此受限实施授权。

Baseline: local `main` at `1e5036200d96a128e162d7d21eb1839c1e1f8dbd`, containing approval `1505d06e5`, lifecycle implementation `2372b2c22`, references `075b508a8` and planning `b9a01b7e9`. Initial working tree was clean, with one main checkout and one local branch. Created `terminal-live-config` from that local main in `.worktrees/terminal-live-config`; no reset or remote operation. / 初始工作区干净；从包含既有实现的本地 main 建立隔离检出，不回退远端。

## Implementation and decisions / 实施与决策

One async `apply_live_sections(cfg, top_sections, event_bus)` executes the existing target table. Cold enable requires explicit context and uses the existing server bus; missing context returns non-live without initializing PTY. An absent manager plus disabled intent is satisfied without initialization. Stopping→enable first completes retained `deactivate`, then activates a checked new generation. Disable earns Terminal `live_applied` only after checked source cleanup and stored-driver join. / 唯一异步执行器沿用既有 target table；cold enable 显式使用原 server bus，无 context 不初始化 PTY、不报 live；无 manager 的 disable 不初始化。Stopping→enable 先完成 retained stop，再启 checked 新代；disable 仅在 cleanup/join 完成后获 live。

The local five-second async wait budget returns non-live on timeout or failure. Cancellation leaves the source-owned handle and evidence in PtyManager; no cleanup task is detached by the executor. This budget cannot preempt synchronous native work or bound Config lock acquisition, so it is not a hard RPC deadline. / 五秒局部异步预算对 timeout/失败返回 non-live；取消保留 source-owned handle/evidence，不 detach cleanup。预算不能抢占同步 native 工作，也不限制 Config 锁等待，不是 RPC 硬时限。

`ConfigPatcher::with_event_bus` stores an optional clone without starting PTY; production wires the original bus before sharing the patcher with RPC/tools. Patch, shared-patcher self_config update/rollback, rollback and explicit RPC reload use the same executor. Reload receives the bus from its registration context. Dedicated execution/search callers await the executor with `None` and their original exact targets. / Patcher 仅保存 optional bus clone，不启动 PTY；生产在共享 RPC/tools 前接入原 bus。所有受支持显式写入口共用执行器，reload 从注册 context 取得 bus，execution/search 保留精确 target 并 await `None` context。

Patch/rollback record their persisted mtime before the cancellable lifecycle wait, then apply the latest shared Config under a read guard held through completion. Reload installs under a write guard, releases it, and acquires the latest read guard; a queued writer can win that handoff. No Config write guard crosses join. The intentional trade-off is that writers wait while the read guard protects application. Source driver/cleanup inspection found no reverse application-Config lock acquisition. / Patch/rollback 在可取消的 lifecycle 等待前记录自身持久化 mtime，再持最新 Config 的 read guard 完成应用。Reload 释放安装配置的 write guard 后重新读取最新值，允许排队 writer 在交接时获锁；write guard 不跨 join。代价是应用期间 writer 等待；source driver/cleanup 未发现反向获取应用 Config 锁。

Identical patch/self_config updates retain their early no-op path: no write, backup, mtime change, lifecycle retry or fabricated live result. Explicit `config.reload` finishes the retained stop after an incomplete persisted disable. Persistence success and live application remain separate. / 相同 patch/self_config 保持早退 no-op，不写文件、不备份、不改变 mtime、不重试 lifecycle、不伪造 live；持久化 disable 未完成时由真实 explicit reload handler 完成 retained stop，持久化成功与 live application 分开。

## Test-first evidence / 测试先行证据

Runtime RED evidence / 运行时 RED 证据：

- Original synchronous executor: `terminal_disable_reports_live_only_after_join` returned `[]` before join; cold explicit enable also returned `[]`. Each selected test failed 0/1. / 原同步执行器在 join 前返回，cold enable 未接通，两个用例各失败 1 项。
- Async signature with old Terminal arm: the stop-timeout test failed its elapsed-five-seconds assertion; missing explicit bus incorrectly earned Live; cancellation/Stopping returned Ready instead of Pending. / 仅迁移签名仍未等待预算、仍恢复全局 bus、不能等待 retained stop，均有独立运行时失败。
- Optional patcher bus API without forwarding: patcher enable and real `config.patch` returned non-live/restart. / 仅存引用但未接线时，真实 patcher/RPC enable 未获 Live。
- Source application and bus wiring then produced **24 passed, 2 failed**: `reload retained the write guard across live apply` and `a subsequent update must not conflict with its own cancelled write`. These drove the write→read handoff and mtime-ordering fixes. / 接线后两项真实失败驱动 reload guard 与取消后自身 mtime 冲突修复。

Fixture compilation errors, an invalid watcher fixture (`At least one agent must be configured`), and the later borrowed-vs-owned Gateway bus error were corrected; none counts as behavior RED. The existing global-PTY serial guard also caught free test helpers accessing the singleton outside test bodies. Helpers now receive an explicit `&PtyManager` from tagged tests; the guard/scanner were unchanged. A missed five-call signature migration was completed and checked by a repository-wide census before restarting the full matrix. Earlier stopped attempts are excluded from the final results below. / Fixture 编译及配置错误不冒充行为 RED；既有串行守卫发现 helper 隐式访问全局实例后，改为由带标记的测试显式传入实例，没有放宽守卫。遗漏的五处签名调用已补齐并全仓核对；前面停止的轮次不计入最终通过结果。

Coverage uses real PTYs, temp config persistence and actual `pty.screen` delivery on the disabled GatewayServer's original bus. Cold-global wrappers re-exec a fresh test process. Test-only driver barriers retain stop/generation and await the real driver before the barrier; no manual cleanup supplies the executor's required join. Covered cases include repeat enable, awaited stop, Stopping→enable, stale generation, timeout/cancel retention, genuine cleanup/join failure, exact verdicts, all supported write faces, no-op→reload recovery, dry-run/save/rollback failure and competing Config writers. / 使用真实 PTY、临时文件与原 bus 实际帧；cold 证明在新子进程中运行。Barrier 先等待真实 driver，保留 stop/generation，测试不以手动 cleanup 补 executor 的 join；覆盖全部计划行为与失败边界。

## Final verification / 最终验证

All **16 Cargo invocations** passed serially. Before every invocation, `python qa/browser_live/check_memory.py` returned ALLOW for the 4 GiB gate; the minimum observed available memory was 9.69 GiB. Tests used `CARGO_BUILD_JOBS=1`; reviewer sessions ran no Cargo. Historical 194-test results belong to the previous lifecycle run. / 本轮 16 次 Cargo 调用全部串行通过，每次前均执行内存门且 ALLOW，最低可用内存 9.69 GiB；测试 jobs=1，审查不并发 Cargo；不复用历史 194 项计数。

Each focused row used `CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib <filter>` after the memory gate. / 下表每行按指定 filter 单独执行：

| Filter / 过滤器 | Passed / 通过 | Failed / 失败 |
| --- | ---: | ---: |
| `gateway::pty` | 140 | 0 |
| `gateway::handlers::pty` | 23 | 0 |
| `config::live_apply` | 23 | 0 |
| `config::patcher` | 22 | 0 |
| `gateway::handlers::config` | 20 | 0 |
| `builtin_tools::self_config` | 16 | 0 |
| `gateway::handlers::execution_config` | 2 | 0 |
| `gateway::handlers::search_config` | 8 | 0 |
| `config::types::policies::terminal` | 4 | 0 |
| `capability::provider` | 10 | 0 |
| `gateway::server::tests::terminal_` | 2 | 0 |
| `gateway::event_visibility::tests::pty_frames_reach_only_the_owner_and_outlive_the_session` | 1 | 0 |

Combined run: **271 passed, 0 failed, 0 ignored**, using the following command after the same memory gate. This is one combined set, not a sum of repeated executions. / 组合运行 271 通过、0 失败、0 ignored，不累加重复执行次数：

```bash
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib -- \
  gateway::pty gateway::handlers::pty config::live_apply config::patcher \
  gateway::handlers::config builtin_tools::self_config \
  gateway::handlers::execution_config gateway::handlers::search_config \
  config::types::policies::terminal capability::provider \
  gateway::server::tests::terminal_ \
  gateway::event_visibility::tests::pty_frames_reach_only_the_owner_and_outlive_the_session
```

Additional checks / 其他检查：

- `CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib gateway::server::tests::terminal_disabled_router_does_not_initialize_manager -- --exact`: **1 passed, 0 failed**; fresh subprocess proof remains valid alongside the combined suite. / 精确 cold-boot 子进程复测通过。
- `cargo check -p alephcore --tests`: passed / 通过。
- `cargo check -p alephcore --bin aleph-server`: passed / 通过。
- `rustfmt --check --edition 2021 --config skip_children=true` on all **16 affected Rust files**, including the three new test files: passed / 受影响文件格式检查通过。
- `git diff --check`, unique/non-overlapping reference replacements and relative links introduced by those replacements: passed / 空白、替换与相对链接检查通过。

Observed warnings: test builds reported unused `Retracted` at `src/gateway/server/panel_asset_route.rs:82` and the macOS linker `__eh_frame section too large (max 16MB)` message. Checks reported 11 dead-code warnings for the same unchanged Panel asset module (including its identity/request/publication types and validation functions); the test target also reported `Retracted`. No warning cleanup was included. / 实际警告来自未修改的 Panel asset 模块及 macOS unwind 表大小；未扩大范围清理这些警告。

## Independent review / 独立审查

Native Pi reviewer `e9b07fbb-d91d-4ca6-bb0c-ab79ddcb9990` (`88api/gpt-6.1-sol`) reviewed the complete uncommitted implementation and three new test files. Verdict: **Approved**, no actionable code findings. It confirmed source completion, original-bus wiring, Config guard ordering, cancellation mtime handling and retained driver ownership; it requested current reference/comment synchronization. / 首轮独立审查完整 diff 与新增测试，无代码缺陷，文档与注释备注已落实。

Follow-up reviewer `935d30fa-bbd4-463f-9e71-4bf85ba6f2a8` independently reviewed the explicit-instance helpers and every caller, unchanged serial guard/scanner, original-bus fixture clone, `reload_impact` comment and all 15 exact replacements in four reference documents. Verdict: **Approved**, no fixes requested. Both reviews were read-only, ran no Cargo or delegates, and did not claim the then-running validation matrix had passed; the parent verified the final log counts above. / 补充复审确认 helper、全调用点、原守卫、fixture 与全部 15 处 reference 修订，无需修复；两轮均只读、不跑 Cargo、不再委派，最终计数由主会话核对实际日志。

Read-only research attempts `del_mvwbyegg_a9lw` and `del_mvwbzce4_23ln` returned no final result (the second reported unavailable `openai-codex/gpt-5.4`); the main session completed inspection. The first native review provider (`wokey`) failed with HTTP 402 `insufficient_balance`; an unsupported tool option launched no child. None of these attempts counts as delivered research or approval. / 空输出、不可用模型、额度不足及参数拒绝均未冒充完成证据。

## Reference findings / 参考结论

The local `deepseek-harness` Terminal service was inspected at `packages/terminal/terminal/src/index.ts`, `packages/terminal/terminal-bash/src/session.ts` and `packages/terminal/terminal/tests/service.spec.ts`. Useful parallels were setup-before-publication, retaining failed unpublished cleanup evidence, and deleting a session record only after successful close. Aleph keeps those effects in its existing PtyManager; no reference registry/service or unconditional map clearing was copied. / 深度参考聚焦先 setup 再发布、保留未发布对象的清理失败证据、成功关闭后再删 record；没有复制独立 registry/service 或无条件清空 map。

Updated current Terminal sections in `FEATURE_LOCATOR.md` (including §6.11 and the later Capability section), `ARCHITECTURE.md`, `GATEWAY.md` and `TERMINAL_RUNTIME.md`; the former next-planned slice now points to completed implementation evidence. Historical phase records and their limits remain unchanged. / 四份 reference 当前段落已校对，原下一阶段入口改为本次实施记录；历史阶段不改写。

## Local commits and delivery / 本地提交与交付

- `88fe4fc4c` — `docs(terminal): record live config authorization`.
- `cf52bb05a` — `feat(terminal): complete explicit live config toggles` (the integrated executor, wiring and test changes) / 集成实现与测试。
- Documentation, current references and this final run record are delivered together in `docs(terminal): record async live config validation` / 文档收尾与本记录在同一提交。

The local `terminal-live-config` branch and `.worktrees/terminal-live-config` checkout are retained. The original main checkout is not edited or merged by this work; no push or remote merge was performed. / 保留本地分支与隔离检出，不改写或合并主检出，不 push、不远端 merge。

## Post-delivery integration / 交付后的本地集成（2026-10-31）

After the implementation delivery described above, the user explicitly authorized merging the completed Terminal work into local `main` and preparing the next design/plan/prompt. The main checkout was clean; `git merge --ff-only terminal-live-config` moved local main from `1e5036200d96a128e162d7d21eb1839c1e1f8dbd` to `b378b87bd377295499c9a71347a318d94b681916`. Main now contains authorization `88fe4fc4c`, implementation `cf52bb05a` and documentation `b378b87bd`. `git merge-base --is-ancestor b378b87bd main` passed. The resulting production tree is identical to the tested branch; the merge introduced no new source change and did not rerun the test matrix. No push or remote operation was performed. The Terminal branch/worktree remains retained at `b378b87bd`.

上述实施交付后，用户另行授权合入本地 main 并规划下一阶段。已执行真实 fast-forward；先前的“main 未合并”描述保留为交付时历史，**不再描述当前 main**。后续 watcher 规划须从包含上述三个提交和规划包的本地 main 建立，不得将历史初始基线当作当前 tip。此次仅追加集成记录与未实施规划，生产代码与已测 Terminal 分支一致，未重跑测试、未 push，保留旧分支/worktree。

## Deferred and unproven / 延期与未证明

No automatic file-watcher reconciliation, lifecycle rewrite, second authority/store/bus singleton, Core/ProviderHandle platform effects, Terminal descriptor/BackendLease/Zahir/MCP/ACP, Generic Gateway mount, Feishu, Memory/Sandbox, other kinds, subscribe/durable recovery, EffectClaim/replay/scheduler/harness or native isolation work was implemented. / 未扩大至 watcher、既有生命周期重写、第二 authority、Core 平台副作用或延期 Capability 路线图。

Successful kill request or waiter-confirmed child exit, canonical cleanup and driver join do not prove descendant/reader-thread reclamation, native unload, RSS reduction or process isolation. Actual WebSocket asynchronous visibility stop/re-enable interleaving remains untested. No full-workspace test run, clippy, cross-platform execution or live Panel/native reclamation QA was performed. / source 证据不证明所有 OS/线程资源回收；真实 WS async visibility 时序仍未新增测试；未跑全 workspace、clippy、跨平台或真机 Panel/native 回收 QA。
