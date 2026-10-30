# Next task prompt: Capability Core source-owned contract gate

你是 Aleph Capability Core 的架构规划与审查 agent。下一阶段的目标不是直接接入 Feishu，而是为所有 externally composable provider 建立一个 source-owned、可验证、不会重复 authority 的 Capability contract。

## Current state

仓库：`/Volumes/TBU4/Workspace/Aleph`

当前 `main`：`5b2115d36`，包含：

- Stage H：ExtensionManager-managed tool-only WASM 的 declaration-only discovery 与显式 activation 边界。
- Stage G：生产 provider-neutral Tool `ProjectionHost` / `ZahirFacade` mount。
- `ToolHandlerRegistry`：Tool callable publication 的 canonical authority。
- `ProviderHandle` / `ProviderLifecycle`：provider state、instance generation、admission。
- `OwnershipTree`：owner、lifetime、owner generation、revoke/dispose；`OwnershipTree::bump` 使用 checked increment，耗尽时返回 `OwnershipError::GenerationExhausted`。
- `ChannelRegistry`：当前 Gateway channel instance、intent、association 和 lifecycle authority。
- Feishu：只有 Gateway-owned provider lifecycle slice；尚未成为 Zahir Capability。

当前 Feishu → Zahir 的 B8.2 A/B/C/D gates 仍全部 **NO-GO / deferred**。当前 Tool host evidence 不能推导 Feishu Capability evidence。

先阅读：

- `AGENTS.md`
- `docs/reference/REDLINES.md`
- `docs/reference/ARCHITECTURE.md`
- `docs/reference/FEATURE_LOCATOR.md`
- `docs/reference/GATEWAY.md`
- `docs/reference/TOOL_SYSTEM.md`
- `docs/research/Everything-externally-composable-is-a-Capability.md`
- `docs/research/aleph_modularization_plan.md`
- `src/capability/{descriptor,facade,backend,ownership,effect_claim,projection_host,zahir_facade}.rs`
- `src/gateway/channel_registry.rs`
- `src/gateway/interfaces/feishu/**`
- `src/gateway/pty/manager.rs`
- `src/memory/extensions/registry.rs`

## Mission

把这条 North Star 变成可实施的 source-backed contract：

> Everything externally composable is a Capability.
>
> 一切需要被 Agent、Runtime 或其他系统动态发现、调用、替换、订阅或管理的能力，都是 Capability。

目标架构是轻量 Capability Core 加上一组按需启动、可停止、可替换、可订阅的 provider。disabled provider 不得初始化线程、连接、数据库、缓存、token client、listener 或 websocket。

本阶段只完成设计与 gate 结论。没有明确的 design approval，不得修改 production Rust、测试、CapabilityKind、Gateway wiring 或 provider mount。

## 推荐方向

采用“source-owned canonical binding + Capability projection”方案：

- 每个已有 source 继续拥有自己的 association、intent、lifecycle 和资源事实源。
- `ChannelRegistry` 继续是 Gateway authority；不得创建第二个 Gateway registry、descriptor catalog、enabled-state map 或 runtime table。
- `ProviderHandle` 继续拥有 provider state、provider generation 和 admission。
- `OwnershipTree` 继续拥有 owner/lifetime/generation/revoke/dispose。
- `ToolHandlerRegistry` 继续是 Tool callable publication authority。
- `ProjectionHost` / `ZahirFacade` 只负责 derived describe/resolve/project/subscribe/read surface，不拥有 provider lifecycle。
- Capability descriptor、facets、source revision、visibility、lease 和 replacement proof 必须从已有 authority 派生，并明确各自的 authority，不合并成一个万能计数器。

这个方向保留了现有 source 的独立生命周期，代价是每种 source 都必须显式提供自己的 binding、snapshot、replacement 和 admission proof；这是为了避免一个中心化 registry 重新成为新的生命周期耦合点。

## Hard constraints

- 不新增 `CapabilityKind`，除非设计 gate 明确证明现有 kind 无法表达目标，并获得单独 architecture approval。
- 不创建第二个 registry、owner map、descriptor catalog、handler map、enabled-state map、runtime table 或 durable store。
- 不把 Feishu 特例倒推成通用 Core 形状；先定义所有 Gateway channel 共用的 source/lifecycle contract。
- 不把 `ChannelRegistry` 替换成 Zahir authority；不绕过 `ChannelRegistry` 做 Feishu facade glue 或 resolve bypass。
- 不把 internal admission fence 自动标记为 Capability `ProviderLifecycle` 或 `ExternalEffect` facet。
- 不把 live broadcast 当作 durable recovery source；durability 仍必须指向已有 durable authority。
- 不合并 provider generation、owner generation、descriptor revision、source revision 或 `SessionEvent.seq`。
- 不声称 native unload、JIT/engine reclamation、OS mapping reclamation、thread shutdown、RSS reduction 或 process isolation。
- 不实现 Feishu production mount、Feishu → Zahir route、EffectClaim producer、Safe Replay、external exactly-once 或 universal scheduler。
- 不修改 `src/harness/**`。
- 本阶段只读审计、设计和 plan；无 design approval 前不写 production code。

## Required work

### R0 — Reconfirm the current baseline

用当前源码重新确认：

- Tool capability 的 descriptor、callable publication、provider lease 和 projection path。
- `ChannelRegistry` 的 source-owned registration、replacement、stop、drain、revision 和 `register_bound_channel` contract。
- Feishu provider lifecycle 与 `ChannelRegistry` 的真实 authority 分工。
- `OwnershipTree::bump` 的 checked exhaustion semantics。
- Terminal、Memory、Sandbox 当前是否有独立 lifecycle/source authority，以及哪些资源会在 disabled/read-side 被初始化。

如果文档与源码不一致，记录为事实差异；不要先修代码。

### R1 — Define the universal source-owned Capability contract

设计并画出：

1. `Source`、`CapabilityDescriptor`、`ProviderHandle`、`OwnershipTree`、`BackendLease`、`CapabilityBackend`、`ProjectionHost` / `ZahirFacade` 的关系图。
2. authority non-duplication table：每个字段和状态由谁拥有、谁只能读取、谁能修改。
3. canonical identity：capability id、source identity、instance identity、provider generation、owner generation、descriptor/source revision 的组合与边界。
4. lifecycle state machine：discovered/cold/disabled/starting/ready/stopping/stopped/failed，以及每个状态允许的 read、call、replace、revoke、dispose 行为。
5. replacement protocol：旧 instance 撤销、in-flight drain、new instance publication、late completion 和 stale lease 的顺序与失败结果。
6. `describe` / `list` / `project` / `resolve` / `subscribe` 的 no-start、no-auth、no-network 证明。
7. visibility、scope、lease 和 effect claim 的关系；明确哪些 contract 可以在本阶段落地，哪些必须留给后续 EventSource/ExternalEffect/R5 gate。
8. source revision、provider generation、owner generation、session sequence 的独立性证明。

### R2 — Compare real alternatives

至少比较两种方案：

- A：中心化 Capability registry / runtime table。
- B：source-owned canonical binding + derived Capability projection。

对每种方案说明：authority duplication 风险、Gateway 兼容性、Tool 兼容性、replacement fencing、read-side lazy boundary、未来 Terminal/Memory 迁移成本、durable recovery 影响和 R1/R3/R4/R8 风险。

必须给出一个推荐方案和明确的拒绝理由。推荐不得只是“更简单”，要指出它如何保持现有 authority 不重复。

### R3 — Run conformance audits

不实现 wiring，只把同一 contract 对照以下 source：

- Tool：当前已完成的 canonical registry/ProjectionHost slice。
- Gateway：`ChannelRegistry` 与 Generic Webhook/loopback bound evidence。
- Feishu：只作为第一个 Gateway conformance audit，不作为 Core 形状的设计来源。
- Terminal：`src/gateway/pty/manager.rs`。
- Memory：`src/memory/extensions/registry.rs`。
- Sandbox：现有 sandbox authority 与资源边界。

每个 source 需要列出：

- 现有 authority。
- descriptor/facet 是否存在。
- provider instance/generation 是否存在。
- owner/lifetime/revoke/dispose 是否存在。
- read-side 是否无启动/认证/网络/资源初始化。
- replacement/late completion 是否可证明。
- subscription/observer 是否 live-only 或 durable。
- 缺失 contract 和是否属于本阶段、后续阶段或明确 deferred。

### R4 — Gate decision

输出一个明确的：

- **GO**：contract 足够稳定，可以进入独立 implementation plan；或
- **NO-GO / deferred**：指出第一个未闭合 contract，并停止 implementation planning beyond that gate。

如果是 GO，下一阶段仍必须先写独立 design spec 并获得 approval，之后才能选择一个最小 source-owned vertical slice 实施。默认候选是 Generic Gateway binding contract；Feishu production mount 不能自动成为下一步。

## Deliverables

1. 双语 design spec：`docs/superpowers/specs/YYYY-MM-DD-capability-core-source-owned-contract-design.md`。
2. 双语 implementation plan：`docs/superpowers/plans/YYYY-MM-DD-capability-core-source-owned-contract-plan.md`，只覆盖通过 gate 后的下一步，不提前承诺 Feishu wiring。
3. 下一会话可直接使用的 prompt：`docs/superpowers/prompts/YYYY-MM-DD-capability-core-source-owned-contract-prompt.md`。
4. stale/current claim census：记录源码与 reference 文档之间的事实差异、保留的 dated history 和 deferred boundary。
5. 明确列出不做的 production changes、未运行的 Cargo checks、未关闭的 capability kinds/providers，以及没有执行的 destructive/external actions。

完成后停止等待 architecture/design approval。不要直接修改 Rust，不要创建 CapabilityKind，不要 mount Feishu，不要 push。