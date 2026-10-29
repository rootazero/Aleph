# Stage G Provider-Neutral Production Mount Readiness Design
# Stage G Provider-Neutral Production Mount 就绪设计

- Date / 日期: 2026-10-29
- Branch / 分支: `stage-g-provider-mount`
- Scope / 范围: audit and bounded readiness evidence for the existing provider-neutral production mount
- Status / 状态: approved and verified with manual review / 已批准并完成手工验证

## Intent / 意图

Stage F proved that provider state, generation, call admission, canonical publication, and exact Extism guest-instance ownership are constrained by one existing `ExtensionManager` lifecycle. Stage G answers the narrower production question: does the existing startup composition expose that proven path through one provider-neutral capability host without creating a second authority?

Stage F 已证明 provider state、generation、call admission、canonical publication 与 exact Extism guest instance ownership 由同一条现有 `ExtensionManager` lifecycle 路径约束。Stage G 只回答一个更窄的 production 问题：现有 startup composition 是否已经通过一个 provider-neutral capability host 暴露这条路径，并且没有新增第二套 authority？

The census found that the mount already exists. `/Volumes/TBU4/Workspace/Aleph/src/bin/aleph-server/commands/start/mod.rs:233-281` creates the production `Arc<OwnershipTree>` and canonical `Arc<ToolHandlerRegistry>`, mounts `ProjectionHost` from them, installs the host in the process slot, and waits for readiness. The same tree is passed to `ExtensionManager`; the same registry Arc is attached to `ExtensionManager` before the first plugin load. Therefore this stage audits and proves the existing composition. It does not add another mount.

本阶段盘点确认 production mount 已经存在。`/Volumes/TBU4/Workspace/Aleph/src/bin/aleph-server/commands/start/mod.rs:233-281` 创建 production `Arc<OwnershipTree>` 与 canonical `Arc<ToolHandlerRegistry>`，使用二者挂载 `ProjectionHost`，把 host 安装到 process slot，并等待 ready。同一个 tree 被传给 `ExtensionManager`；同一个 registry Arc 在首次 plugin load 前被挂载到 `ExtensionManager`。因此本阶段审计并证明现有 composition，不再新增 mount。

## Goals / 目标

1. Record the production authority chain and prove that the startup host is provider-neutral and source-owned.
2. Add the smallest behavior evidence needed to connect cold observation, activation, replacement, stale rejection, failure residuals, and host shutdown to the existing authorities.
3. Keep `ToolHandlerRegistry`, `OwnershipTree`, `ProviderHandle`, `RuntimeOwner`, `ExtensionManager`, and the WASM disposer as the only authorities for their respective facts.
4. Keep exact `LoadedWasmPlugin` drop evidence separate from provider state, generation, projection, notification, OS mapping, JIT, thread, and RSS claims.
5. Leave all concrete channels and other provider kinds deferred.

1. 记录 production authority chain，并证明 startup host 是 provider-neutral 且由既有 source 拥有。
2. 只补充连接 cold observation、activation、replacement、stale rejection、failure residual 与 host shutdown 的最小行为证据。
3. 保持 `ToolHandlerRegistry`、`OwnershipTree`、`ProviderHandle`、`RuntimeOwner`、`ExtensionManager` 与 WASM disposer 分别作为各自事实的唯一 authority。
4. 将 exact `LoadedWasmPlugin` drop 证据与 provider state、generation、projection、notification、OS mapping、JIT、thread、RSS 结论严格分开。
5. 保持所有具体 channel 与其他 provider kind deferred。

## Non-goals and anti-goals / 非目标与反目标

- No new production mount, wrapper, route, channel adapter, or provider registry.
- No Feishu, R2, Terminal, Memory, Sandbox, or built-in channel integration.
- No second `OwnershipTree`, `ToolHandlerRegistry`, descriptor catalog, owner map, generation map, runtime table, lifecycle state machine, or cleanup authority.
- No public plugin lifecycle API change solely to expose this evidence.
- No fix for the known early post-mount startup `?` residual in this stage. The common shutdown funnel drains `ProjectionHost` before registry-owned scopes are disposed, but an early return before `run_until_shutdown` can leave the process-slot host alive until process exit. This is documented as a bounded residual and remains a separate change.
- No claim of native unloading, engine-wide JIT reclamation, OS mapping reclamation, thread shutdown, or RSS reduction.

- 不新增 production mount、wrapper、route、channel adapter 或 provider registry。
- 不接入 Feishu、R2、Terminal、Memory、Sandbox 或任何 built-in channel。
- 不新增第二个 `OwnershipTree`、`ToolHandlerRegistry`、descriptor catalog、owner map、generation map、runtime table、lifecycle state machine 或 cleanup authority。
- 不为了暴露本阶段证据而修改 public plugin lifecycle API。
- 本阶段不修复 mount 后、进入 `run_until_shutdown` 前的 startup `?` residual。正常 shutdown funnel 会先 drain `ProjectionHost` 再 dispose registry-owned scope，但早期 return 可能让 process-slot host 保持到进程退出；这作为明确边界记录，并留给独立变更。
- 不声称 native unloading、engine-wide JIT 回收、OS mapping 回收、thread shutdown 或 RSS 下降。

## Existing authority contract / 现有 authority contract

| Fact / 事实 | Sole authority / 唯一 authority | Stage G treatment / 本阶段处理 |
|---|---|---|
| Durable discovery and enabled/disabled state | `plugin_state.rs` + `ExtensionManager` | Observe through existing production lifecycle |
| Provider state, generation, admission, transitions | `ProviderHandle` + `ProviderLifecycle` | Reuse current admission and generation |
| Provider/runtime incarnation, identity, receipt, task and disposer | `RuntimeOwner` + existing `EffectScope` | Reuse exact owner and disposer |
| Canonical callable publication | `ToolHandlerRegistry` | Use the startup canonical Arc |
| Owner/revoke/dispose facts | Production `Arc<OwnershipTree>` | Use the tree shared by startup and ExtensionManager |
| Capability projection and live notification | `ProjectionHost` + `ZahirFacade` | Treat as projection/notification only |
| Exact WASM instance cleanup | Existing loader identity/disposer | Keep receipt/drop evidence here |
| Channel-local lifecycle | `ChannelRegistry` and its bound registration | Keep separate; do not promote it |

## Production composition / Production composition

The production composition is:

1. Startup creates one `OwnershipTree` and one canonical `ToolHandlerRegistry`.
2. Startup passes the same tree to `ExtensionManager::with_defaults_with_ownership_tree`.
3. Startup mounts `ProjectionHost::mount_with_diagnostics((*tool_registry_phase2).clone(), Arc::clone(&ownership_tree), diagnostics_enabled)`.
4. `ProjectionHost` constructs `ZahirFacade` from the passed registry and tree, subscribes to both existing publication streams, and owns only source/applier tasks and consumer queues.
5. Startup installs the host in `PROJECTION_HOST` and waits for its initial readiness.
6. `agent_init` attaches the same canonical registry Arc to `ExtensionManager` before every first `ensure_loaded` path; `set_tool_handler_registry` rejects a different Arc or late attachment.
7. Plugin activation, replacement, and disposal remain inside `ExtensionManager`/`RuntimeOwner`/`EffectScope`.
8. Normal shutdown awaits `ProjectionHost::close_and_await` before disposing builtin and markdown-skill registry owners.

The host therefore composes existing authorities. It must not become the owner of provider state, provider generation, plugin admission, canonical publication, WASM identity, or exact cleanup.

生产 composition 如下：

1. Startup 创建一个 `OwnershipTree` 与一个 canonical `ToolHandlerRegistry`。
2. Startup 将同一个 tree 传给 `ExtensionManager::with_defaults_with_ownership_tree`。
3. Startup 调用 `ProjectionHost::mount_with_diagnostics((*tool_registry_phase2).clone(), Arc::clone(&ownership_tree), diagnostics_enabled)`。
4. `ProjectionHost` 使用传入的 registry/tree 构造 `ZahirFacade`，订阅既有 publication stream 与 ownership stream，并且只拥有 source/applier task 与 consumer queue。
5. Startup 把 host 安装到 `PROJECTION_HOST`，等待初始 ready。
6. `agent_init` 在每个首次 `ensure_loaded` 路径前将同一个 canonical registry Arc 传给 `ExtensionManager`；`set_tool_handler_registry` 拒绝不同 Arc 或过晚 attachment。
7. Plugin activation、replacement 与 disposal 仍由 `ExtensionManager`/`RuntimeOwner`/`EffectScope` 负责。
8. 正常 shutdown 先 await `ProjectionHost::close_and_await`，再 dispose builtin 与 markdown-skill registry owner。

因此 host 只是组合既有 authority，不能拥有 provider state、provider generation、plugin admission、canonical publication、WASM identity 或 exact cleanup。

## Approaches considered and decision / 已考虑方案与决定

### A. Audit the existing startup host — chosen / 方案 A：审计现有 startup host — 已选择

Use the production `ProjectionHost` already mounted from the canonical registry and shared ownership tree. Add only bounded RED/readiness evidence and bilingual documentation. This preserves one source of truth and avoids a new lifecycle boundary.

Trade-off: the evidence must follow several existing components and cannot be simplified into a new local test owner. That cost is required to prove the production composition rather than a parallel fixture.

使用现有 production `ProjectionHost`，它已经由 canonical registry 与 shared ownership tree 挂载。只增加 bounded RED/readiness 证据与双语文档，保持单一事实源并避免新的 lifecycle boundary。

代价是证据必须穿过多个现有组件，不能简化成一个本地 test owner；这是证明 production composition 而不是 parallel fixture 所必需的成本。

### B. Add a thin wrapper — rejected / 方案 B：增加薄 wrapper — 拒绝

A wrapper is unnecessary because `ProjectionHost` is already the named provider-neutral composition point. Adding one would create another place that could accidentally retain or close authoritative resources.

当前已经存在命名明确的 provider-neutral composition point `ProjectionHost`，因此 wrapper 没有必要；新增 wrapper 会制造另一个可能保留或关闭 authoritative resource 的位置。

### C. Promote a concrete channel or add a provider registry — rejected / 方案 C：提升具体 channel 或新增 provider registry — 拒绝

This would choose the shared Core shape from a channel and violate the stage boundary. `ChannelRegistry` remains channel-local and cannot become the universal capability owner in this stage.

这会用单个 channel 决定共享 Core 形状并违反阶段边界。`ChannelRegistry` 必须保持 channel-local，本阶段不能把它提升为 universal capability owner。

## RED and readiness evidence / RED 与 readiness evidence

The implementation plan must begin with behavior RED against the real existing composition. The tests may reuse Stage E/F fixture instrumentation and existing static startup-order checks; they must not create a second production authority merely to make assertions easier.

实施计划必须先针对真实 existing composition 建立 behavior RED。测试可以复用 Stage E/F fixture instrumentation 与既有 startup-order static checks，但不能为了方便断言而创建第二套 production authority。

### 1. Cold and discovered observation / Cold 与 discovered observation

Through the real discovery and provider declaration path, inspect plugin list/status/descriptor/projection surfaces while the provider is discovered, Cold, or Disabled. Assert zero guest-byte reads, zero runtime creation, zero instance creation, and no fabricated callable publication. Manifest metadata reads remain allowed and must be recorded separately.

通过真实 discovery 与 provider declaration 路径，在 provider 为 discovered、Cold 或 Disabled 时读取 plugin list/status/descriptor/projection surface。断言 guest-byte read、runtime create、instance create 均为零，且没有伪造 callable publication。manifest metadata read 可以发生，但必须单独记录。

### 2. Existing host composition / 既有 host composition

Assert that the production host is composed from the canonical registry and production ownership tree, and that `ZahirFacade::is_backed_by_registry` and existing publication behavior identify the same canonical source. No test may instantiate a second production tree or registry.

断言 production host 使用 canonical registry 与 production ownership tree 组合，并且 `ZahirFacade::is_backed_by_registry` 与既有 publication behavior 指向同一 canonical source。测试不得实例化第二个 production tree 或 registry。

### 3. Activation and admission / Activation 与 admission

Enable the real fixture through `ExtensionManager::set_plugin_enabled` and the existing activation path. Execute payload A through the existing canonical publication/call route. Record provider state, provider generation, guest-byte read, instance creation, and exact receipt separately. The call must be admitted by the current `ProviderHandle`; the facade must not bypass it.

通过 `ExtensionManager::set_plugin_enabled` 与既有 activation path 启用真实 fixture，并通过既有 canonical publication/call route 执行 payload A。分别记录 provider state、provider generation、guest-byte read、instance create 与 exact receipt。call 必须经过当前 `ProviderHandle` admission，facade 不得绕过它。

### 4. Replacement and stale identity / Replacement 与 stale identity

Use existing `reload_plugin` or equivalent existing replacement path. Verify that the old provider generation, canonical lease, binding, facade-facing identity, and call route fail closed after replacement. Execute payload B through the successor. Retain old owner, binding, lease, identity, and receipt references until the test has proven the exact old instance drop; then release them and verify the existing receipt sequence. ProviderState or projection disappearance alone is insufficient.

使用既有 `reload_plugin` 或等价的既有 replacement path。验证 replacement 后旧 provider generation、canonical lease、binding、facade-facing identity 与 call route 全部 fail closed。通过 successor 执行 payload B。保留 old owner、binding、lease、identity 与 receipt 引用，直到证明 exact old instance drop；然后释放这些引用并验证既有 receipt sequence。ProviderState 或 projection 消失本身不构成充分证据。

### 5. Failure residuals / Failure residuals

Exercise the existing failed read, failed build, publication collision, and disposer-failure paths where the Stage F fixture already provides them. Assert the actual Failed/residual state, retained receipt or scope where applicable, admission closure, and absence of a fabricated clean Ready/create/drop result. Do not convert a residual into a new cleanup path.

复用 Stage F fixture 已覆盖的 failed read、failed build、publication collision 与 disposer-failure 路径。断言真实 Failed/residual state、适用时保留的 receipt 或 scope、admission closure，以及没有伪造的 clean Ready/create/drop 结果。不得把 residual 转换成新的 cleanup path。

### 6. Host shutdown / Host shutdown

Verify the existing normal shutdown order: `ProjectionHost::close_and_await` drains source/applier workers before builtin and markdown-skill registry-owned scopes are disposed. Record the early post-mount startup error residual as an explicit limit; do not add detached cleanup in this stage.

验证既有正常 shutdown 顺序：`ProjectionHost::close_and_await` 在 builtin 与 markdown-skill registry-owned scope dispose 前 drain source/applier worker。将 mount 后 startup early error residual 作为明确限制记录；本阶段不新增 detached cleanup。

## Arc release and exact drop / Arc 释放与 exact drop

The evidence must distinguish host-local resources from plugin-instance ownership. Host shutdown releases ProjectionHost worker/consumer resources but does not revoke the shared registry/tree. Plugin stop remains ordered by the existing lifecycle: close admission, transition provider, revoke the exact publication receipt, drain in-flight calls, join the retained task, verify the current owner incarnation, dispose the exact scope, take identity and receipt, transition Stopped/Disabled, remove the owner, and release the drain permit.

证据必须区分 host-local resource 与 plugin-instance ownership。Host shutdown 释放 ProjectionHost worker/consumer resource，但不会 revoke shared registry/tree。Plugin stop 继续遵循既有 lifecycle 顺序：close admission、provider transition、revoke exact publication receipt、drain in-flight call、join retained task、验证 current owner incarnation、dispose exact scope、take identity 与 receipt、transition Stopped/Disabled、移除 owner、释放 drain permit。

The test must retain and then release any facade, provider, binding, lease, identity, owner, and receipt `Arc` that the existing fixture exposes. Only the existing exact `LoadedWasmPlugin` receipt/drop observation may establish the old instance drop. No projection or state transition may substitute for it.

测试必须保留并随后释放 fixture 暴露的 facade、provider、binding、lease、identity、owner 与 receipt `Arc`。只有既有 exact `LoadedWasmPlugin` receipt/drop observation 可以证明 old instance drop；projection 或 state transition 不得替代它。

## Verification and acceptance / 验证与验收

Every Cargo command must have a separate immediately preceding `python qa/browser_live/check_memory.py` run with exit 0 and `ALLOW`. The approved implementation plan must run the focused provider-neutral readiness tests serially three times, then the relevant WASM, Stage E/F, B3, capability, and gateway narrow regressions, followed by repository check, clippy, fmt, and diff checks. A zero-test filter is not evidence.

每条 Cargo 命令前必须单独立即执行 `python qa/browser_live/check_memory.py`，exit 0 且输出 `ALLOW`。获批后的 implementation plan 必须将 focused provider-neutral readiness tests 连续串行运行三次，再运行相关 WASM、Stage E/F、B3、capability 与 gateway narrow regressions，最后运行 repository check、clippy、fmt 与 diff checks。零测试 filter 不构成证据。

Stage G is GREEN only when:

- production owners and dependency direction are source-backed;
- the existing startup host passes the single-authority gate;
- no second registry/tree/lifecycle/generation/cleanup authority is added;
- cold, activation, replacement, stale-call, failure, and normal shutdown behavior are evidenced through existing composition;
- exact Extism receipt/drop evidence remains separate from native/OS claims;
- no concrete channel is connected;
- reviewer checks the complete diff and all findings are resolved.

Stage G 只有在以下条件同时满足时才是 GREEN：

- production owner 与 dependency direction 有源码证据；
- 现有 startup host 通过 single-authority gate；
- 没有新增第二套 registry/tree/lifecycle/generation/cleanup authority；
- cold、activation、replacement、stale-call、failure 与 normal shutdown 都通过既有 composition 得到证据；
- exact Extism receipt/drop 证据与 native/OS 结论分离；
- 没有接入具体 channel；
- reviewer 检查完整 diff 且所有 finding 已解决。

## Builder responsibilities / Builder 负责

The builder may choose the narrowest existing test location and reuse existing fixture instrumentation, but must not change the authority contract. The builder must decide exact test names and avoid duplicating Stage F assertions unless the new test connects them to the production host. Any proposed change to the early-error shutdown path, public plugin API, or production ownership must stop and return for a new architecture decision.

Builder 可以选择最窄的现有测试位置并复用 fixture instrumentation，但不能改变 authority contract。Builder 必须决定具体 test name，除非需要把 Stage F 证据连接到 production host，否则不得重复已有 assertion。任何对 early-error shutdown path、public plugin API 或 production ownership 的修改，都必须停下并重新请求 architecture decision。

## Deferred boundaries / Deferred 边界

Feishu/R2/Terminal/Memory/Sandbox, all concrete Gateway channel integration, remaining capability kinds, EffectClaim producer integration, automatic Safe Replay, universal durable scheduler, native unloading, process isolation, engine/JIT/mapping/thread/RSS reclamation, and the early-error drain fix remain deferred.

Feishu/R2/Terminal/Memory/Sandbox、所有具体 Gateway channel integration、其余 capability kind、EffectClaim producer integration、automatic Safe Replay、universal durable scheduler、native unloading、process isolation、engine/JIT/mapping/thread/RSS 回收，以及 early-error drain 修复均保持 deferred。

## Actions not performed / 未执行动作

No production mount was added, no concrete channel was connected, no dependency or runtime was introduced, no merge/push/external service action was performed, and no destructive cleanup or history rewrite was performed.

没有新增 production mount，没有接入具体 channel，没有引入 dependency 或 runtime，没有执行 merge/push/external service action，也没有执行 destructive cleanup 或 history rewrite。

## Implementation outcome / 实现结果

Task 1 and Task 2 of the approved implementation plan are complete. The shared-tree fixture and two host-backed behavior tests are committed as `e79db18e9` and `b59bd3f1a`. Final verification and documentation closure are complete; independent reviewer availability is recorded separately below.

已批准 implementation plan 的 Task 1 与 Task 2 已完成。shared-tree fixture 与两条 host-backed behavior test 分别提交为 `e79db18e9` 与 `b59bd3f1a`。最终验证与文档闭环已完成；独立 reviewer 的可用性在下文单独记录。

The early post-mount startup error residual remains explicitly deferred. No production mount, wrapper, channel integration, second authority, or cleanup authority was added.

mount 后 startup error residual 仍明确 deferred。没有新增 production mount、wrapper、channel integration、第二套 authority 或 cleanup authority。


The requested independent builder check could not start because the configured `openai/gpt-6.1-sol` provider had no API key. The failure output is retained at `/var/folders/qt/d007hr751cl59b3_8sptd8fr0000gp/T/acp-delegate/del_mvtcdb0x_xtz8.out`; no independent review is claimed.

按流程请求的独立 builder check 未能启动，因为配置的 `openai/gpt-6.1-sol` provider 缺少 API key。失败输出已保留在 `/var/folders/qt/d007hr751cl59b3_8sptd8fr0000gp/T/acp-delegate/del_mvtcdb0x_xtz8.out`；本阶段不宣称已通过独立审阅。

A manual builder check was performed against the written design and source-backed census. It found no unanswered question whose answer would change the selected approach or authority contract. Exact test location and names are intentionally left to the builder because the design requires reuse of the narrowest existing fixture and explicitly forbids changing production authority. The known early-error drain residual is explicitly classified as a separate decision, so it does not block this design.

已对设计与源码盘点执行手工 builder check。没有发现会改变已选方案或 authority contract 的未回答关键问题。具体 test location 与 test name 有意留给 builder 决定，因为设计要求复用最窄的既有 fixture，并明确禁止改变 production authority。已知 early-error drain residual 已明确归类为独立决策，因此不阻塞本设计。

## Final verification outcome / 最终验证结果

The Stage G focused pair passed three serial pre-format rounds and one post-format round, with one real test per invocation. Stage F/B3, ProjectionHost, startup ordering, Stage E exact WASM, WASM runtime, check, clippy, fmt, and diff verification passed. The only observed build warning was the known nonfatal `__eh_frame section too large` linker warning. The independent final reviewer completed with `88api/gpt-6.1-sol` and returned **APPROVE WITH NOTES**; no blocking finding was reported. The two optional Minor advisories are recorded in the run record and were not added because one is already guaranteed by source construction and the other duplicates Stage F evidence.

Stage G focused pair 在格式化前连续三轮、格式化后再一轮通过，每次 invocation 均实际运行一个测试。Stage F/B3、ProjectionHost、startup ordering、Stage E exact WASM、WASM runtime、check、clippy、fmt 与 diff verification 均通过。唯一观察到的 build warning 是已知非致命 `__eh_frame section too large` linker warning。独立 final reviewer 已使用 `88api/gpt-6.1-sol` 完成 review，结论为 **APPROVE WITH NOTES**；没有报告阻断项。两项可选 Minor advisory 已记录在 run record 中，本次未新增，因为一项已由 source construction 保证，另一项重复 Stage F evidence。

## Approval record / 批准记录

The user approved this design before implementation. The independent builder check could not start because the configured `openai/gpt-6.1-sol` provider lacked an API key; the exact failure remains recorded above and a manual builder check found no scope-changing unanswered question. The implementation, verification, and independent final review are complete under the selected approach. The final reviewer returned **APPROVE WITH NOTES** with only the two optional Minor advisories documented in the run record.

用户已在实现前批准本设计。独立 builder check 因配置的 `openai/gpt-6.1-sol` provider 缺少 API key 未能启动；精确失败信息已在上文记录，手工 builder check 未发现会改变范围的未决问题。当前已按选定方案完成实现、验证与独立 final review。final reviewer 返回 **APPROVE WITH NOTES**，仅提出 run record 中记录的两项可选 Minor advisory。
