# Stage G Provider-Neutral Production Mount Readiness Run
# Stage G Provider-Neutral Production Mount 就绪运行记录

- Date / 日期: 2026-10-29
- Branch / 分支: `stage-g-provider-mount`
- HEAD at Task 3 start / Task 3 开始时 HEAD: `b59bd3f1ab9383339c001e61f858bd3ba80947e2`
- Base / 基线: `2dd7c07999ce9603cff74c0a3b3b0abfb638b5e7`
- Status / 状态: verified with manual review / 已验证（手工 review）
- Final verification HEAD / 最终验证 HEAD: `e717db22a` (formatting-only source commit) / `e717db22a`（格式化-only source commit）

## Purpose / 目的

This record captures bounded evidence that the existing production `ProjectionHost` composes the startup canonical `ToolHandlerRegistry` and shared `OwnershipTree` through the existing Extension lifecycle. It records evidence and limits; it does not add a production mount or a new authority.

本记录收集现有 production `ProjectionHost` 使用 startup canonical `ToolHandlerRegistry` 与 shared `OwnershipTree`，并通过既有 Extension lifecycle 运行的 bounded evidence。它记录证据与边界，不新增 production mount 或新的 authority。

## Authority table / Authority 表

| Fact / 事实 | Existing authority / 既有 authority | Stage G evidence / Stage G 证据 |
|---|---|---|
| Discovery and enabled/disabled state / discovery 与 enabled/disabled 状态 | `ExtensionManager` and `plugin_state.rs` | Existing manager fixture and `set_plugin_enabled` path |
| Provider state, generation and admission / provider state、generation、admission | `ProviderHandle` and `ProviderLifecycle` | Existing activation and stale-generation assertions |
| Plugin/runtime incarnation and cleanup / plugin/runtime incarnation 与 cleanup | `RuntimeOwner`, `EffectScope`, loader disposer | Existing receipt, identity and drop probes |
| Canonical callable publication / canonical callable publication | `ToolHandlerRegistry` | Shared canonical Arc in new host-backed tests |
| Owner/revoke/dispose facts / owner、revoke、dispose 事实 | Shared `Arc<OwnershipTree>` | Same tree passed to manager fixture and `ProjectionHost` |
| Capability projection/notification / capability projection/notification | `ProjectionHost` and `ZahirFacade` | Host facade identity and host-backed dispatch |
| Channel lifecycle / channel lifecycle | `ChannelRegistry` | Remains separate and unmodified |

## Production composition / Production composition

The source-backed production composition remains `/Volumes/TBU4/Workspace/Aleph/src/bin/aleph-server/commands/start/mod.rs:233-281`: startup creates one ownership tree and one canonical tool registry, mounts `ProjectionHost`, installs it in the process slot, and waits for readiness. The same tree reaches `ExtensionManager`; the same registry Arc is attached before the first plugin load. Normal shutdown at `/Volumes/TBU4/Workspace/Aleph/src/bin/aleph-server/commands/start/mod.rs:4093-4115` awaits `ProjectionHost::close_and_await` before registry-owned scopes are disposed.

源码盘点确认 production composition 位于 `/Volumes/TBU4/Workspace/Aleph/src/bin/aleph-server/commands/start/mod.rs:233-281`：startup 创建一个 ownership tree 与一个 canonical tool registry，挂载 `ProjectionHost`，把它安装到 process slot，并等待 ready。同一个 tree 被传给 `ExtensionManager`；同一个 registry Arc 在首次 plugin load 前被 attachment。正常 shutdown 位于 `/Volumes/TBU4/Workspace/Aleph/src/bin/aleph-server/commands/start/mod.rs:4093-4115`，先 await `ProjectionHost::close_and_await`，再 dispose registry-owned scope。

## Implemented behavior tests / 已实现行为测试

| Test / 测试 | Evidence / 证据 | Result / 结果 |
|---|---|---|
| `stage_g_production_host_reuses_canonical_authorities_for_cold_and_activation` in `/Volumes/TBU4/Workspace/Aleph/src/extension/lifecycle.rs` | Shared tree and canonical registry; host readiness/facade identity; Cold describe/project empty; zero guest-byte reads/instance creation before enable; production enable; Ready/generation; payload A; host worker joins | 4/4 focused invocations passed; final verification recorded below |
| `stage_g_production_host_replacement_fences_stale_facade_and_drops_exact_owner` in `/Volumes/TBU4/Workspace/Aleph/src/extension/lifecycle.rs` | Payload A through host facade; old generation/lease/binding/entry retained; reload to Cold; stale provider/binding/facade route fails closed; old exact receipt drops after reference release; successor executes payload B and drops exactly | 4/4 focused invocations passed; final verification recorded below |

Task 1 fixture change is committed as `e79db18e9 test(core): share ownership tree with stage G host fixture`. Task 2 behavior evidence is committed as `b59bd3f1a test(core): prove stage G host lifecycle reuse`.

Task 1 fixture change 已提交为 `e79db18e9 test(core): share ownership tree with stage G host fixture`。Task 2 behavior evidence 已提交为 `b59bd3f1a test(core): prove stage G host lifecycle reuse`。

## Memory-gated command ledger / Memory gate 命令记录

Every Cargo command below had its own immediately preceding `python qa/browser_live/check_memory.py` invocation with exit 0 and `ALLOW`; the gate values and actual outcomes are recorded in the table.

下列每条 Cargo 命令都已有独立且紧邻其前的 `python qa/browser_live/check_memory.py`，并且 exit 0、输出 `ALLOW`；gate 数值与实际结果已记录在表格中。

| Command group / 命令组 | Memory gate / memory gate | Result / 结果 |
|---|---|---|
| `stage_g_production_host_reuses_canonical_authorities_for_cold_and_activation` | 3 serial pre-format runs + 1 post-format run; each 1 passed | 4/4 passed; final gate `ALLOW`, `11202723840 / 4294967296` |
| `stage_g_production_host_replacement_fences_stale_facade_and_drops_exact_owner` | 3 serial pre-format runs + 1 post-format run; each 1 passed | 4/4 passed; final gate `ALLOW`, `7948648448 / 4294967296` |
| Stage F cold/activation/replacement + B3 residual | 4 commands; each 1 passed | 4/4 passed; gates `ALLOW` (`9268281344`, `9620537344`, `8714420224`, `11180556288`) / `4294967296` |
| ProjectionHost tests | 42 passed, 0 failed | gate `ALLOW`, `11215339520 / 4294967296` |
| Startup mount/readiness and shutdown ordering | 2 commands; each 1 passed | corrected command `cargo test --bin aleph-server ...`; gates `ALLOW` (`10957979648`, `9759244288`) / `4294967296` |
| Stage E exact WASM load/reload/failure | 3 commands; each 1 passed | gates `ALLOW` (`9380102144`, `8484356096`, `6949879808`) / `4294967296` |
| WASM runtime module | 14 passed, 0 failed | gate `ALLOW`, `7323271168 / 4294967296` |
| `cargo check -p alephcore --tests` | passed | gate `ALLOW`, `7265140736 / 4294967296` |
| `cargo clippy -p alephcore --tests -- -D warnings` | passed | gate `ALLOW`, `10486022144 / 4294967296` |
| `cargo fmt --all --check` | initial check found formatting-only diff; `cargo fmt --all` fixed it and a second check passed | gates `ALLOW` (`10513301504`, `10402840576`, `9007022080`) / `4294967296` |
| `git diff --check` | passed after formatting | no Cargo gate required |

The first startup attempt used the plan’s package spelling `cargo test -p aleph-server ...` and failed because this repository’s root package is `alephcore`; the corrected `cargo test --bin aleph-server ...` commands ran both tests successfully. Every successful Cargo command had its own immediately preceding `ALLOW` gate. The known nonfatal linker warning `ld: __eh_frame section too large (max 16MB) to encode dwarf unwind offsets in compact unwind table` appeared in test builds.

第一次 startup 尝试使用计划中的 package 写法 `cargo test -p aleph-server ...`，因本仓库 root package 是 `alephcore` 而失败；修正为 `cargo test --bin aleph-server ...` 后两条测试均成功执行。每条成功的 Cargo 命令都有自己紧邻的 `ALLOW` gate。测试构建中重复出现已知非致命 linker warning：`ld: __eh_frame section too large (max 16MB) to encode dwarf unwind offsets in compact unwind table`。

## Residual failures and boundaries / Residual failure 与边界

The early post-mount startup `?` path can still leave the process-slot host and its workers alive until process exit when control returns before `run_until_shutdown`. The normal shutdown funnel drains the host, but Stage G does not add detached cleanup or change startup ownership.

mount 后、进入 `run_until_shutdown` 前的 startup `?` path 仍可能让 process-slot host 与 worker 保持到进程退出。正常 shutdown funnel 会 drain host，但 Stage G 不新增 detached cleanup，也不改变 startup ownership。

Existing failed read, failed build, publication collision, and disposer-failure residuals remain owned by the Stage F lifecycle tests. Stage G does not convert residual state into a clean Ready/create/drop result.

既有 failed read、failed build、publication collision 与 disposer-failure residual 仍由 Stage F lifecycle tests 负责。Stage G 不把 residual state 转换成 clean Ready/create/drop 结果。

## Proof limits / 证明边界

The tests establish exact `LoadedWasmPlugin` receipt/drop observations and the provider-neutral composition used by the existing in-process lifecycle. They do not establish native unloading, engine/JIT reclamation, OS mapping reclamation, thread shutdown, process isolation, or RSS reduction. Projection disappearance and ProviderState transitions are not used as substitutes for exact instance-drop evidence.

测试建立 exact `LoadedWasmPlugin` receipt/drop observation，以及既有 in-process lifecycle 使用的 provider-neutral composition。它们不证明 native unloading、engine/JIT 回收、OS mapping 回收、thread shutdown、process isolation 或 RSS 下降。Projection 消失与 ProviderState transition 不替代 exact instance-drop evidence。

## Reviewer status / Reviewer 状态

Independent final reviewer completed successfully with `88api/gpt-6.1-sol`. Verdict: **APPROVE WITH NOTES**. The reviewer found no Critical, Important, lifecycle, authority, security, or scope blocker. Full result: `/var/folders/qt/d007hr751cl59b3_8sptd8fr0000gp/T/acp-delegate/del_mvtjnf9f_sjvf.out`.

独立 final reviewer 已使用 `88api/gpt-6.1-sol` 成功完成。结论：**APPROVE WITH NOTES**。reviewer 未发现 Critical、Important、lifecycle、authority、安全或范围阻断项。完整结果：`/var/folders/qt/d007hr751cl59b3_8sptd8fr0000gp/T/acp-delegate/del_mvtjnf9f_sjvf.out`。

The reviewer recorded two optional Minor advisories: a future host accessor could support a direct shared `OwnershipTree` pointer-identity assertion, and Stage G could repeat receipt-ID ordering or successor-generation assertions already covered by Stage F. These are not correctness gaps and were not added: source construction already passes the same Arc, and duplicating Stage F evidence would not change the authority contract.

reviewer 记录了两项可选 Minor advisory：未来 host accessor 可以支持直接断言 shared `OwnershipTree` pointer identity；Stage G 可以重复 Stage F 已覆盖的 receipt-ID 顺序或 successor-generation assertion。这些不是 correctness gap，本次未新增：source construction 已传递同一个 Arc，重复 Stage F evidence 不会改变 authority contract。

An earlier reviewer attempt with unsupported `openai-codex/gpt-6-codex` remains historical tooling context only; the successful `88api/gpt-6.1-sol` review is the independent review result for this stage.

此前使用 unsupported `openai-codex/gpt-6-codex` 的 reviewer 尝试仅作为历史 tooling context 保留；本阶段的独立 review 结果以成功完成的 `88api/gpt-6.1-sol` review 为准。

Manual authority/diff checklist passed: the manager fixture, canonical registry, shared ownership tree, ProjectionHost and facade use one composition; no second registry/tree/lifecycle/generation/cleanup authority was added; host close occurs after plugin cleanup in the tests and before registry-owned scope disposal in the production source-order tests; stale calls fail closed before successor execution; exact old and successor receipts drop only after the retained references are released; residual and proof-limit language stays bounded; no concrete channel, new runtime, dependency, or native/OS claim was added.

手工 authority/diff checklist 通过：manager fixture、canonical registry、shared ownership tree、ProjectionHost 与 facade 使用同一 composition；没有新增第二套 registry/tree/lifecycle/generation/cleanup authority；测试中 host close 位于 plugin cleanup 之后，production source-order tests 验证 host close 先于 registry-owned scope dispose；stale call 在 successor 执行前 fail closed；exact old/successor receipt 只在 retained reference 释放后 drop；residual 与 proof-limit 文案保持有界；没有接入具体 channel、引入新 runtime/dependency，也没有新增 native/OS 结论。

## Deferred integrations / Deferred integration

Feishu, R2, Terminal, Memory, Sandbox, concrete Gateway channels, remaining capability kinds, EffectClaim producers, automatic Safe Replay, universal durable scheduling, external-effect exactly-once, native unloading, and early-error drain remain deferred.

Feishu、R2、Terminal、Memory、Sandbox、具体 Gateway channel、其余 capability kind、EffectClaim producer、automatic Safe Replay、universal durable scheduling、external-effect exactly-once、native unloading 与 early-error drain 继续 deferred。

## Actions not performed / 未执行动作

No production mount, wrapper, channel integration, dependency, runtime, merge, push, external service action, destructive cleanup, or history rewrite was performed. The two implementation commits and final documentation/formatting commit remain local until any separate publication decision.

没有新增 production mount、wrapper、channel integration、dependency 或 runtime，也没有执行 merge、push、external service action、destructive cleanup 或 history rewrite。两个 implementation commit 与最终文档/格式化 commit 在任何单独 publication decision 前均只保留在本地。
