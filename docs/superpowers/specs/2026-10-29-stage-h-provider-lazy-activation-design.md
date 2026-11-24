# Stage H Provider-Neutral Lazy Activation Design
# Stage H Provider-Neutral Lazy Activation 设计

**Date / 日期:** 2026-10-29
**Status / 状态:** Approved; implementation pending validation / 已批准，待实现验证
**Scope / 范围:** ExtensionManager-managed tool-only WASM providers only / 仅限 ExtensionManager 管理的 tool-only WASM provider

## Intent / 意图

Aleph must be able to discover an installed tool-only WASM extension without constructing its guest runtime, then activate that exact discovered owner only through an explicit lifecycle call. Activation must publish the callable surface through the existing canonical `ToolHandlerRegistry`, while stop, replacement, stale handles, and failed disposal keep the existing generation, receipt, admission, and residual semantics.

Aleph 需要能够发现已安装的 tool-only WASM extension，而不构造 guest runtime；随后只能通过明确的生命周期调用激活该次发现所对应的 owner。激活必须继续通过既有 canonical `ToolHandlerRegistry` 发布 callable surface；停止、替换、陈旧 handle 和 dispose 失败必须保持现有 generation、receipt、admission 与 residual 语义。

The work is deliberately bounded. It proves the existing production owner can expose a cold read path and an explicit activation path. It does not turn every Aleph provider into one universal runtime.

本工作有意保持边界。它证明现有 production owner 可以暴露冷读路径和显式激活路径；不会把所有 Aleph provider 统一成一个 universal runtime。

## Current facts / 当前事实

The ownership census found the following existing authorities:

ownership census 已确认以下既有 authority：

- `ExtensionManager` owns extension discovery, durable enabled/disabled configuration, reload, and control operations.
- `ExtensionManager` 负责 extension discovery、持久化 enabled/disabled 配置、reload 与 control operation。
- `RuntimeOwner` owns one plugin runtime incarnation, its `ProviderHandle`, owner generation, activation task, exact WASM identity, publication receipt, admission, disposer, and failure residual.
- `RuntimeOwner` 负责一次 plugin runtime incarnation、其 `ProviderHandle`、owner generation、activation task、exact WASM identity、publication receipt、admission、disposer 与失败 residual。
- `ProviderHandle` and `ProviderLifecycle` provide provider state, generation, and fail-closed call admission; they do not create external resources.
- `ProviderHandle` 与 `ProviderLifecycle` 提供 provider state、generation 和 fail-closed call admission；它们不创建外部资源。
- `ToolHandlerRegistry` is the canonical callable publication source. Each plugin publication carries its own producer `ProviderHandle`; the registry-wide built-in provider is a compatibility gate and cannot be replaced by one plugin provider.
- `ToolHandlerRegistry` 是 canonical callable publication source。每条插件 publication 携带自己的 producer `ProviderHandle`；registry-wide 的 builtin provider 是兼容性闸门，不能被某个插件 provider 替换。
- `ProjectionHost` and `ZahirFacade` derive snapshots, leases, and notifications from the canonical registry and `OwnershipTree`. Their read-side methods do not start providers.
- `ProjectionHost` 与 `ZahirFacade` 从 canonical registry 和 `OwnershipTree` 派生 snapshot、lease 与 notification；它们的读侧方法不启动 provider。
- `ChannelRegistry`, `PtyManager`, and `MemoryExtensionRegistry` remain provider-local authorities. No Stage H implementation may make them universal backends.
- `ChannelRegistry`、`PtyManager` 与 `MemoryExtensionRegistry` 继续是 provider-local authority。本阶段不得把它们变成 universal backend。

The current defect is at the production entry boundary: `plugins.list`, plugin management read actions, and Hub installed reconciliation call `ensure_loaded()`, whose purpose is to discover and activate every eligible Cold owner. Thus a read can create a guest runtime before the caller explicitly requests activation.

当前缺口在 production entry boundary：`plugins.list`、plugin management 的读操作和 Hub installed reconciliation 会调用 `ensure_loaded()`；而该方法的职责是发现并激活所有 eligible Cold owner。因此，读操作可能在调用者明确请求激活前创建 guest runtime。

## Decision / 决策

Separate declaration discovery from runtime activation inside the existing `ExtensionManager`.

在既有 `ExtensionManager` 内部分离 declaration discovery 与 runtime activation。

The implementation will introduce a read-safe discovery phase and keep explicit activation on the existing `RuntimeOwner` path:

实现将增加一个读安全的 discovery phase，并继续沿用现有 `RuntimeOwner` 路径执行显式激活：

1. A declaration-only pass scans and parses extension metadata, applies the existing admission policy, and publishes declaration rows. For eligible tool-only WASM it creates the existing `RuntimeOwner` in `Cold`; it does not call `mount_parsed`, start a provider, read guest bytes, create a guest instance, open provider resources, or publish callable tools.
2. declaration-only pass 扫描并解析 extension metadata，应用既有 admission policy，并发布 declaration row。对 eligible tool-only WASM，它创建现有 `Cold` 状态的 `RuntimeOwner`；它不会调用 `mount_parsed`、启动 provider、读取 guest bytes、创建 guest instance、打开 provider resource 或发布 callable tool。
3. A derived discovery cache flag, protected by the existing lifecycle guards, prevents redundant scans. It is a cache of work already performed, not durable enabled state, provider state, a generation map, or a second owner table. Explicit reload invalidates it through the existing `ExtensionManager` lifecycle.
4. 由既有 lifecycle guard 保护的 derived discovery cache flag 防止重复扫描。它只是已完成工作的缓存，不是持久化 enabled state、provider state、generation map 或第二张 owner table。显式 reload 通过既有 `ExtensionManager` lifecycle 使其失效。
5. `ensure_loaded()` first ensures declarations, then activates the captured Cold owners outside `load_guard`, and finally performs the existing current-owner/Ready/Loaded/identity/receipt/scope proof before publishing the loaded cache flag.
6. `ensure_loaded()` 先确保 declarations 存在，再在 `load_guard` 外激活捕获的 Cold owner，最后继续执行现有的 current-owner/Ready/Loaded/identity/receipt/scope proof，然后发布 loaded cache flag。
7. `set_plugin_enabled(id, true)` ensures the declaration for the requested id and continues to activate only through the existing `set_plugin_enabled → activate_owner` path. It does not delegate activation to a projection, registry snapshot, or new coordinator.
8. `set_plugin_enabled(id, true)` 确保目标 id 的 declaration 存在，然后继续只通过现有的 `set_plugin_enabled → activate_owner` 路径激活。它不会把激活委托给 projection、registry snapshot 或新 coordinator。
9. Read-only production callers use declaration discovery: `plugins.list`, plugin read actions, and Hub installed reconciliation. Startup catalog construction and agent execution retain explicit boot/run activation semantics and remain documented as activation boundaries.
10. 只读 production caller 使用 declaration discovery：`plugins.list`、plugin read action 和 Hub installed reconciliation。启动 catalog 构造与 agent execution 保持显式 boot/run activation 语义，并继续明确标记为 activation boundary。

The declaration pass must be factored so it can be run without first unmounting existing runtimes. A read after activation must observe the existing rows and owner incarnation; it must not trigger `unmount_all`, clear `runtime_owners`, or rebuild the canonical registry. Existing reload and explicit mount behavior will continue to use the same `ExtensionManager` owner and guard boundaries.

Declaration pass 必须被拆分为可以在不先卸载现有 runtime 的情况下运行。激活后的读操作必须观察既有 row 和 owner incarnation；它不能触发 `unmount_all`、清空 `runtime_owners` 或重建 canonical registry。现有 reload 与显式 mount 行为继续使用同一 `ExtensionManager` owner 和 guard boundary。

## Approaches considered / 已考虑方案

### A. Existing owner with an internal cold-read phase — selected / 方案 A：既有 owner 加内部冷读 phase——已选择

Reuse `ExtensionManager`, `RuntimeOwner`, `ProviderHandle`, `ToolHandlerRegistry`, and `OwnershipTree`. Factor declaration discovery from the current activation pass and route read callers to it.

复用 `ExtensionManager`、`RuntimeOwner`、`ProviderHandle`、`ToolHandlerRegistry` 和 `OwnershipTree`。把 declaration discovery 从当前 activation pass 中拆出，并让读 caller 使用 declaration discovery。

This preserves the existing single-flight task, exact publication receipt, owner incarnation check, generation fencing, admission drain, disposer, and residual model. The cost is a lifecycle refactor around discovery caching and the need to test read-before-activate, concurrent activation, and reload invalidation.

该方案保留现有 single-flight task、exact publication receipt、owner incarnation check、generation fencing、admission drain、disposer 与 residual model。代价是需要围绕 discovery cache 做 lifecycle refactor，并测试先读后激活、并发激活和 reload invalidation。

### B. Keep current behavior and issue readiness NO-GO / 方案 B：保持现状并发布 readiness NO-GO

Leave all read callers on `ensure_loaded()` and only document that the owner path is ready.

让所有读 caller 继续使用 `ensure_loaded()`，只记录 owner path 已经就绪。

This avoids code risk but leaves the Stage H cold-discovery contract false in production. It is unsuitable after the selected approach because the user chose to close this boundary in the existing owner.

该方案避免代码风险，但 production 中的 Stage H cold-discovery contract 仍然不成立。在已选择方案后它不再适用，因为用户选择在既有 owner 内关闭这一边界。

### C. Add a provider catalog or activation coordinator / 方案 C：新增 provider catalog 或 activation coordinator

Introduce a new table or coordinator to remember providers, generations, enabled state, or cleanup.

新增一张 table 或一个 coordinator 来记住 provider、generation、enabled state 或 cleanup。

Rejected because it duplicates `ExtensionManager`, `RuntimeOwner`, `ProviderHandle`, exact registry receipts, and cleanup ownership, violating the Stage H hard boundaries and the Capability architecture.

拒绝该方案，因为它会复制 `ExtensionManager`、`RuntimeOwner`、`ProviderHandle`、exact registry receipt 与 cleanup ownership，违反 Stage H 硬边界和 Capability architecture。

## Goals / 目标

- Make declaration discovery and read-side plugin listing cold for the bounded tool-only WASM slice.
- 使 bounded tool-only WASM slice 的 declaration discovery 与 plugin read-side listing 保持 cold。
- Ensure an explicit activation reaches `Ready` through the existing owner and canonical registry path.
- 确保显式激活通过现有 owner 与 canonical registry 路径到达 `Ready`。
- Preserve per-provider generation, owner generation, exact mount identity, exact registration receipt, admission, and disposer semantics.
- 保留 per-provider generation、owner generation、exact mount identity、exact registration receipt、admission 与 disposer 语义。
- Keep non-Ready calls and stale captures fail closed.
- 保持 non-Ready call 与 stale capture fail closed。
- Prove stop, replacement, and restart do not revive stale handles or create a second callable authority.
- 证明 stop、replacement 与 restart 不会复活 stale handle，也不会创建第二个 callable authority。
- Make failure and dispose residuals observable in the existing owner and plugin row model.
- 让 failure 与 dispose residual 在既有 owner 和 plugin row model 中可观察。

## Non-goals / 非目标

- No Feishu or other Gateway channel mount into Zahir.
- 不把 Feishu 或其他 Gateway channel mount 到 Zahir。
- No universal lazy activation contract for Channel, PTY, Memory Graph, Sandbox, MCP, or remaining capability kinds.
- 不为 Channel、PTY、Memory Graph、Sandbox、MCP 或其余 capability kind 声明 universal lazy activation contract。
- No replacement of the production `ToolHandlerRegistry` or use of `new_with_provider` to bind the mixed registry to one plugin.
- 不替换 production `ToolHandlerRegistry`，也不使用 `new_with_provider` 把混合 registry 绑定到某个插件。
- No new activation coordinator, provider catalog, owner map, generation map, runtime table, wrapper registry, or cleanup authority.
- 不新增 activation coordinator、provider catalog、owner map、generation map、runtime table、wrapper registry 或 cleanup authority。
- No new dependency, dynamic native loading, `unsafe`, process isolation, native unloading, engine/JIT reclamation, RSS claim, or thread reclamation claim.
- 不新增 dependency、dynamic native loading、`unsafe`、process isolation、native unloading，也不声明 engine/JIT、RSS 或 thread reclamation。
- No change to the canonical projection authority. `ProjectionHost`, `ZahirFacade`, snapshots, `describe`, `resolve`, `project`, and broadcast notifications remain read-side consumers.
- 不改变 canonical projection authority。`ProjectionHost`、`ZahirFacade`、snapshot、`describe`、`resolve`、`project` 与 broadcast notification 继续是读侧 consumer。

## Anti-goals / 反目标

The implementation must not make a successful no-op look like activation. A declaration row, `Cold` provider state, projection disappearance, or notification is not evidence of guest instance creation or exact drop.

实现不得把成功 no-op 伪装成激活。declaration row、`Cold` provider state、projection disappearance 或 notification 都不能作为 guest instance creation 或 exact drop 的证据。

The implementation must not hide activation behind a read API, start a provider from a snapshot or subscription, or use a registry-wide provider transition as a substitute for the actual plugin producer state.

实现不得把激活隐藏在 read API 后面，不得从 snapshot 或 subscription 启动 provider，也不得用 registry-wide provider transition 替代真实 plugin producer state。

## Lifecycle and authority model / 生命周期与 authority 模型

```text
read caller
   │
   ▼
ExtensionManager::ensure_discovered
   │  scan/parse/admit/declaration only
   │  no guest bytes, instance, provider resource, or callable publication
   ▼
PluginRegistry row + RuntimeOwner(Cold)

explicit boot / enable / run boundary
   │
   ▼
ExtensionManager::ensure_loaded or set_plugin_enabled
   │
   ▼
RuntimeOwner::activate_owner  ── single-flight per owner
   │
   ▼
ProviderLifecycle Cold → Starting → Ready
   │
   ▼
ToolHandlerRegistry::publish_plugin_batch_with_ready_commit
   │
   ▼
canonical callable publication → ProjectionHost/ZahirFacade derived view
```

The owner remains the only activation authority. The registry remains the only callable publication source. The projection remains a derived consumer. The cache flag only avoids repeated work and is never consulted as an enabled-state or provider-state source of truth.

owner 仍然是唯一 activation authority。registry 仍然是唯一 callable publication source。projection 仍然是 derived consumer。cache flag 只用于避免重复工作，绝不作为 enabled-state 或 provider-state 的事实源。

## Failure, stop, replacement, and restart / 失败、停止、替换与重启

- Read or declaration failure records the existing discovery/row error and never reports a mounted provider.
- read 或 declaration failure 记录既有 discovery/row error，绝不报告 mounted provider。
- Read/build/start failure keeps the owner failed, closes admission, preserves the failure history and any exact identity/scope residual, and exposes the failed row.
- read/build/start failure 让 owner 保持 failed，关闭 admission，保留 failure history 以及任何 exact identity/scope residual，并暴露 failed row。
- Publication failure leaves the incumbent canonical binding intact and retains the failed owner's publication/dispose residual according to the current `RuntimeOwner` path.
- publication failure 保持 incumbent canonical binding 不变，并按现有 `RuntimeOwner` 路径保留失败 owner 的 publication/dispose residual。
- Stop first closes admission, revokes the exact receipt, drains admitted work, joins the retained task, and disposes the exact scope. A disposer failure remains a residual and cannot become clean `Stopped`/`Ready` success.
- stop 先关闭 admission，再撤销 exact receipt、drain 已 admission 的 work、join retained task，并 dispose exact scope。disposer failure 保持 residual，不能变成 clean `Stopped`/`Ready` success。
- Replacement fences owner generation, provider generation, mount identity, binding, and captured leases. A successor activation publishes a new exact batch; old captures remain stale.
- replacement fence 住 owner generation、provider generation、mount identity、binding 与 captured lease。后继 activation 发布新的 exact batch；旧 capture 保持 stale。
- Explicit reload invalidates the derived discovery cache through the existing teardown/re-discovery path. No detached cleanup or second runtime table is introduced.
- 显式 reload 通过既有 teardown/re-discovery 路径使 derived discovery cache 失效。不引入 detached cleanup 或第二张 runtime table。

## Evidence and tests after approval / 批准后的证据与测试

Use the existing real WASM provider fixture and existing Stage E/F/G probes. Tests must be serial and run three times with non-zero counts, with every Cargo command immediately preceded by the memory gate.

使用既有真实 WASM provider fixture 与 Stage E/F/G probes。测试必须串行、带非零测试数运行三次；每个 Cargo 命令前必须立即运行 memory gate。

Required focused evidence:

所需 focused evidence：

1. **Cold discovery:** construct the real fixture, invoke declaration discovery and the production read projection, and assert no guest-byte read, instance creation, provider `Starting`, provider resource open, callable publication, or registry revision change.
2. **冷 discovery：**构造真实 fixture，调用 declaration discovery 与 production read projection，并断言没有 guest-byte read、instance creation、provider `Starting`、provider resource open、callable publication 或 registry revision change。
3. **Explicit activation:** call the existing explicit activation owner path after cold discovery; assert one owner generation reaches `Ready`, the canonical batch is published, the actual guest output is callable, and the ProjectionHost observes the derived publication.
4. **显式 activation：**在 cold discovery 后调用既有 explicit activation owner path；断言一个 owner generation 到达 `Ready`、canonical batch 已发布、真实 guest output 可调用，且 ProjectionHost 观察到 derived publication。
5. **Single-flight:** issue concurrent explicit activation requests for the same owner and assert one guest instance/publication path, shared completion, and no duplicate receipt.
6. **Single-flight：**并发发起同一 owner 的 explicit activation，断言只有一条 guest instance/publication path、共享 completion，且没有重复 receipt。
7. **Failure residuals:** cover missing bytes, invalid bytes, publication collision, and disposer failure; assert Failed/closed admission and retained residuals, with no clean Ready/create/drop claim.
8. **Failure residual：**覆盖缺失 bytes、invalid bytes、publication collision 与 disposer failure；断言 Failed/closed admission 与 retained residual，不能声称 clean Ready/create/drop。
9. **Stop/replacement/restart:** stop an activated owner, assert stale captures fail closed, activate the successor through the same canonical registry, and assert the successor generation works while the old generation cannot.
10. **Stop/replacement/restart：**停止已激活 owner，断言 stale capture fail closed，再通过同一 canonical registry 激活 successor，并断言 successor generation 可用而旧 generation 不可用。
11. **Read after activation:** repeat list/describe/project/subscribe reads after activation and assert they remain projection-only and do not mount, remount, or create a second authority.
12. **激活后的 read：**在 activation 后重复 list/describe/project/subscribe 读操作，断言它们仍为 projection-only，不 mount、remount，也不创建第二 authority。

The tests do not claim OS mapping reclamation, engine/JIT reclamation, thread shutdown, RSS reduction, process isolation, or behavior of deferred provider kinds.

这些测试不声明 OS mapping reclamation、engine/JIT reclamation、thread shutdown、RSS reduction、process isolation，也不声明 deferred provider kind 的行为。

## Verification after design approval / 设计批准后的验证

After explicit approval, invoke the writing-plans skill and produce the implementation plan before changing production or test code. The implementation then follows the project test and memory-gate discipline:

获得明确批准后，调用 writing-plans skill，先产出 implementation plan，再修改 production 或 test code。之后实现必须遵循项目测试与 memory-gate 纪律：

- focused Stage H tests: serial, three runs, non-zero counts;
- focused Stage H tests：串行、三次、每次非零测试数；
- justified Stage E/F/G, B3, ProjectionHost, capability, gateway, and WASM regressions;
- 按变更路径运行有理由的 Stage E/F/G、B3、ProjectionHost、capability、gateway 与 WASM regression；
- `cargo check -p alephcore --tests`;
- `cargo check -p alephcore --tests`；
- `cargo clippy -p alephcore --tests -- -D warnings`;
- `cargo clippy -p alephcore --tests -- -D warnings`；
- `cargo fmt --all --check` and `git diff --check`;
- `cargo fmt --all --check` 与 `git diff --check`；
- independent read-only review over the complete diff.
- 对完整 diff 进行 independent read-only review。

Every Cargo command must be immediately preceded by `python qa/browser_live/check_memory.py`, and the result must record `ALLOW` with exit code 0.

每个 Cargo 命令前都必须立即运行 `python qa/browser_live/check_memory.py`，并记录 exit code 0 的 `ALLOW`。

## Builder decisions left / 留给实现者的决定

The builder may choose the smallest internal factoring of the current discovery code, the exact name and placement of the derived discovery cache field, and the narrowest production read-action dispatch table, provided the authority and behavior described above remain unchanged. The builder must not broaden the implementation to non-tool provider kinds or add a new lifecycle authority.

实现者可以在不改变上述 authority 与行为的前提下，选择当前 discovery code 最小的内部拆分方式、derived discovery cache field 的具体名称与位置，以及最窄的 production read-action dispatch table。实现者不得把实现扩展到 non-tool provider kind，也不得新增 lifecycle authority。

## Open status / 当前状态

This document is not implementation approval. No production code, test code, dependency, generated artifact, commit, merge, push, or publication has been changed or performed as part of writing it.

本设计文档不等于实现批准。编写本文件期间没有修改或执行 production code、test code、dependency、generated artifact、commit、merge、push 或 publication。
