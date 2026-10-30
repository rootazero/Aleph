# Capability Core source-owned contract gate — implementation plan

日期：2026-10-30
范围：架构审计、契约设计、conformance gate；不包含 production Rust 实现。

## Intent / 意图

把 “Everything externally composable is a Capability” 转化为一个可以约束 Gateway、Tool、Terminal、Memory 和 Sandbox 的 source-backed contract。下一阶段先解决 authority、identity、lifecycle、replacement、read-side lazy boundary 和 recovery boundary，避免为 Feishu 单独制造一套 Capability glue。

当前 Tool-only Stage F/G/H 已证明一条 bounded slice：Tool callable publication 由 `ToolHandlerRegistry` 拥有，provider 状态由 `ProviderHandle` 拥有，owner/lifetime/generation 由 `OwnershipTree` 拥有，`ProjectionHost` / `ZahirFacade` 只提供 derived surface。Feishu 仍只有 Gateway-owned provider lifecycle slice，B8.2 A/B/C/D 全部 NO-GO / deferred。

## Goals / 目标

- 形成 source-owned Capability contract，明确各 authority 的所有权和读取边界。
- 证明 declaration/read paths 不会启动 provider、认证、联网或初始化资源。
- 为 replacement、revoke、dispose、drain、late completion 和 stale lease 建立统一 failure matrix。
- 让 `ChannelRegistry` 继续作为 Gateway authority，使未来十几个 channel 可以遵循同一 contract。
- 让 Feishu、Terminal、Memory、Sandbox 作为 conformance audit 对象，而不是决定 Core 形状的特例。
- 输出 GO 或 NO-GO/deferred，不在未批准前进入 Rust wiring。

## Non-goals / 非目标

- 不创建 Feishu descriptor adapter、Feishu → Zahir route 或 production Capability mount。
- 不新增 `CapabilityKind`、第二 registry、descriptor catalog、owner map、runtime table、enabled-state map 或 durable store。
- 不实现 EffectClaim producer、Safe Replay、external exactly-once、universal scheduler 或 durable provider recovery。
- 不声称 native unload、JIT/engine reclamation、OS mapping/thread/RSS reclamation 或 process isolation。
- 不修改 `src/harness/**`，不迁移 Terminal/Memory/Sandbox，不重写 `ChannelRegistry`。

## Recommended approach / 推荐方案

优先选择 **source-owned canonical binding + derived Capability projection**：

- source 继续拥有 association、intent、lifecycle、resource 和 replacement facts；
- provider generation 由 `ProviderHandle` 拥有；
- owner/lifetime/generation/revoke/dispose 由 `OwnershipTree` 拥有；
- Tool callable publication 由 `ToolHandlerRegistry` 拥有；
- descriptor/facets/visibility/source revision 从 source 和既有 authority 派生；
- `ProjectionHost` / `ZahirFacade` 不成为第二 lifecycle authority。

中心化 Capability registry 作为对照方案保留在设计中，但应拒绝，因为它会重复 ChannelRegistry、ToolHandlerRegistry、enabled state、owner/generation 和 durable recovery 的权威，并把 Core 与具体 provider 生命周期重新耦合。

## Work breakdown / 工作拆解

### 1. R0 baseline audit

阅读并记录以下 source-backed facts：

- `src/capability/descriptor.rs`
- `src/capability/provider.rs`
- `src/capability/backend.rs`
- `src/capability/ownership.rs`
- `src/capability/facade.rs`
- `src/capability/projection_host.rs`
- `src/capability/zahir_facade.rs`
- `src/tools/registry.rs`
- `src/gateway/channel_registry.rs`
- `src/gateway/interfaces/feishu/**`
- `src/gateway/pty/manager.rs`
- `src/memory/extensions/registry.rs`

特别记录 `OwnershipTree::bump` 的 checked increment 与 `OwnershipError::GenerationExhausted`，避免把旧的 saturating-generation 表述带入新设计。

### 2. R1 contract model

产出：

- source / descriptor / provider / owner / lease / backend / projection 关系图；
- authority non-duplication 表；
- capability identity、source revision、provider generation、owner generation 和 session sequence 的边界；
- lifecycle state machine；
- replacement/revoke/dispose/late-completion failure matrix；
- `list` / `describe` / `project` / `resolve` / `subscribe` 的 no-start/no-auth/no-network 证明；
- visibility、scope、lease、effect claim 和 durability 的关系；
- provider kind 与 facet 的使用规则。

### 3. R2 alternative evaluation

比较：

- A：中心化 Capability registry / runtime table；
- B：source-owned canonical binding + derived projection。

以 R1、R3、R4、R6、R8 和既有 ChannelRegistry/ToolHandlerRegistry authority 为评价标准。设计必须解释未来多个 Gateway channel 如何复用 contract，而不是只解释 Feishu。

### 4. R3 conformance matrix

对 Tool、Generic Gateway binding、Feishu、Terminal、Memory、Sandbox 建立同一张矩阵，至少包含：

- discovery/read boundary；
- descriptor/kind/facets；
- provider instance/generation；
- owner/lifetime/revoke/dispose；
- source revision；
- replacement and stale completion；
- callable/event/subscription/external-effect status；
- live notification versus durable recovery；
- 缺失项的阶段归属和 deferred 理由。

Feishu 只能用于验证 contract 是否适用于一个真实 Gateway provider，不得反向授权 Feishu production mount。

### 5. R4 gate and handoff

如果 contract 中仍存在两个 authority 都能写同一事实、无法证明 read-side lazy boundary、无法证明 replacement fencing，或必须新增 kind/registry 才能成立，则输出 NO-GO/deferred，并停止 implementation planning。

如果通过 GO：

- 只写下一阶段独立 implementation plan；
- 明确一个最小 source-owned vertical slice；
- 先设计 approval，再进入 failing test / implementation；
- Feishu、EffectClaim、recovery 和其他 kinds 仍需各自独立 gate。

## Validation / 验收

本阶段的验证是文档和源码审计：

- `git diff --check`；
- source anchor existence 与行号记录；
- 双语段落语义一致；
- reference links 可解析；
- authority duplication grep；
- 明确未运行的 Cargo checks 和未完成的 runtime evidence。

不因“已有 Tool evidence”推断 Gateway/Feishu/Terminal/Memory/Sandbox 已完成。任何无法由当前源码支持的结论必须标为 proposal、NO-GO 或 deferred。

## Completion criteria / 完成标准

- design spec、plan、next-session prompt 三者术语一致；
- 明确推荐方案及拒绝替代方案的理由；
- 提供 source-backed authority map、state machine、failure matrix、conformance matrix；
- 给出唯一 GO 或 NO-GO/deferred 结论；
- 没有 production Rust、测试、CapabilityKind、registry 或 provider mount 改动；
- 最终报告列出实际读取/验证内容、未运行项、仍 deferred 范围和未执行的 destructive/external actions。
