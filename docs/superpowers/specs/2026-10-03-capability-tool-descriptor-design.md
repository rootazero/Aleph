# Capability Census + Tool Descriptor 主链设计规格

- 日期：2026-10-03
- 分支：`feature/capability-tool-descriptor`
- 状态：待用户审阅
- 范围：第一期 Capability 收敛子项目

## 1. 背景与目标

Aleph 已有 Tool、Plugin、Skill、MCP、ACP、Agent、Runtime 等可动态接入的能力，但它们分别拥有注册表、元数据、快照和协议投影。结果是同一能力的“能否调用、如何调用、能否重放、何时释放”可能由不同结构分别表达，形成语义漂移和生命周期泄漏。

本期不做全仓库一次性重写，而是以 Tool 作为第一个真实 Capability kind，建立最小、可验证的统一主链：

```text
existing Tool registration
        ↓
Tool capability descriptor
        ↓
registry snapshot + change feed + scoped disposal
        ↓
existing model-visible / protocol projection
        ↓
recovery replay decision
```

目标不是增加第二个 Tool Registry，而是把现有 `ToolHandlerRegistry` 收敛为（或明确承载）Capability-backed 的事实源；`LoopToolRegistry` 等运行时结构继续作为消费者/解析器，不得再维护一份可调用能力元数据。

## 2. 非目标与硬边界

- 不修改 `src/harness/`。
- 不把 `src/capability/mod.rs` 的进程级 `CapabilitySlot<T>` 改造成动态 registry；它继续负责启动安装/拒绝/诊断。
- 不在本期统一重写 Skill、Agent、Plugin、MCP、ACP 的全部注册流程。
- 不新增没有真实消费者的 kind、scope 层级或 replay 变体。
- 不引入重型依赖，不合并 Aleph 的独立持久化存储。
- 不按工具名称在 recovery 中新增特判；恢复策略只能来自 descriptor/调用时快照。
- 不让 descriptor 层依赖具体 MCP/RPC 实现；协议只能投影 descriptor。

## 3. 现状映射与职责边界

| 现有结构 | 本期处理 | 新职责 |
|---|---|---|
| `src/tools/registry.rs::ToolHandlerRegistry` | 扩展或收敛，不复制 | Tool descriptor 的注册、替换、移除、snapshot、change feed、scope disposer |
| `src/tools/service.rs::ToolDefinition` / metadata | 保留兼容入口，减少重复字段来源 | 从 descriptor 生成现有工具定义/展示数据 |
| `src/tools/runtime.rs::LoopToolRegistry` | 保持为消费者 | resolve handler、并发/确认/幂等查询，不拥有全局发现事实 |
| `src/extension/effects/scope.rs::EffectScope` | 复用 | 绑定注册效果和 disposer，逆序清理 |
| session/recovery 相关模块 | 增加 descriptor 语义读取接缝 | 根据调用时 descriptor 的 replay policy 处理未知结果 |
| MCP 或 RPC 的一个已有出口 | 接入单一投影 | 验证同一 descriptor 的跨面一致性 |
| `docs/reference/FEATURE_LOCATOR.md` | 补充状态和入口 | 记录 descriptor 真源、投影和生命周期契约 |

最终取哪一个具体协议出口，以实现前的源码清点为准：必须选择已有稳定消费者，不能为了证明架构而造新 endpoint。

## 4. Descriptor 契约

本期 descriptor 只包含现有真实消费者需要的稳定语义。字段名称和精确类型须在 writing plan 阶段结合当前 Tool 定义确认，但契约必须覆盖：

- 稳定的 capability/tool name；
- kind（本期为 Tool）；
- schema version；
- 输入 schema，以及已有出口实际需要的输出/结果约束；
- visibility/owner 标识（不得把可见性 Scope 与生命周期 Scope 混为一谈）；
- replay policy，默认 `Unsafe`；
- 现有工具的幂等、确认和并发语义；
- descriptor revision 或等价 change token；
- 可投影的来源/关系信息（例如 plugin/MCP source），但不把部署机制误当 Capability kind。

### Replay 规则

- `Unsafe`：调用已落盘但没有结果时，恢复不得自动重放；必须生成现有恢复路径可理解的未知结果/验证提示。
- `Safe`：只有调用时记录的 descriptor 与当前解析到的 descriptor 都明确允许 Safe，才可自动重放；descriptor 被替换或缺失时降级为未知结果。
- 外部 MCP/ACP 工具默认 `Unsafe`，除非已有协议元数据明确提供只读/安全语义；本期不凭“外部”身份推断 Safe。

## 5. 注册与生命周期

`ToolHandlerRegistry` 是唯一运行时事实源。每次注册/替换必须：

1. 校验名称、schema、revision 和 descriptor/handler 配对；错误尽早返回，禁止成功 no-op。
2. 将 registration 绑定到调用者提供的 `EffectScope`（或已有等价 scope seam）。
3. 返回可幂等的 disposer/registration handle。
4. 在 registry 内更新单一 snapshot 与 change feed。
5. 确保已开始的调用持有旧 handler 的稳定引用；替换只影响后续 resolve。

移除时先使新 resolve 看不到旧注册，再执行释放；释放失败必须进入既有 EffectScope report，不能静默吞掉。注册顺序与清理顺序遵循现有 `EffectScope` 合约，不另造生命周期协议。

## 6. 投影与数据流

注册成功后，descriptor 作为唯一元数据来源：

```text
Capability-backed ToolHandlerRegistry
  ├── snapshot → model-visible ToolDefinition
  ├── snapshot → one existing MCP/RPC/diagnostic projection
  ├── change feed → existing subscribers
  └── descriptor lookup → recovery replay decision
```

协议层只读取 descriptor 的投影，不反向修改 descriptor。`LoopToolRegistry` 解析 handler 时必须通过 registry 的既有 resolve/adapter seam；如果当前结构确实承担执行索引，则保留该索引，但删除其中重复的 descriptor/visibility/replay 元数据。

本期必须证明至少一个多面动词由同一 descriptor 驱动，例如：注册后的工具列表、一个既有协议 schema/列表出口、以及 recovery 的 replay 判定不能各自重新推导。

## 7. 错误、并发与替换语义

- 重复名称、无效 schema、descriptor 与 handler 不匹配、未知 descriptor revision：返回结构化错误。
- snapshot 和 change feed 使用确定性 revision；订阅者落后或 lagged 时必须走现有重建 snapshot 语义，不能假装没有变化。
- 注册和替换的状态更新必须在同一同步边界内完成，避免 subscriber 看到半注册状态。
- 已在飞调用不因热替换被强制中断；新调用解析到新版本。
- disposer 幂等；重复 unregister 不得破坏后续 registry 状态。
- registry 关闭时停止接受新调用/注册，并按 EffectScope 规则清理。
- 外部调用取消、超时、未决副作用沿用现有 ToolError/Recovery 语义；不在本期另建错误层。

## 8. 测试验收标准

必须新增或更新以下测试：

1. descriptor 注册、替换、移除、snapshot 和 revision/change feed。
2. 重复注册、坏 schema、缺失 handler、未知 revision 的失败路径。
3. EffectScope dispose 会移除注册，且 disposer 幂等、逆序清理和失败报告行为正确。
4. 替换期间已有调用保持旧实现，新调用使用新实现。
5. 同一 descriptor 驱动至少两个已有出口/消费者，防止投影漂移。
6. `ReplayPolicy::Safe`：满足调用时与当前 descriptor 双重 Safe 时可走现有安全重放路径。
7. `ReplayPolicy::Unsafe` 或 descriptor 被替换/缺失：生成未知结果/人工验证提示，不自动重放。
8. subscription lag 或 snapshot 重建（若现有 broadcast 语义适用）。
9. `src/harness/` 没有改动；被替代的旧元数据/注册结构确实被删除或明确证明仍是执行索引而非第二事实源。

遵守项目 Rust 门禁：至少运行 `cargo check --message-format short --all-targets`、目标 crate clippy、目标测试；最终 reviewer 必须自行重跑，不接受只引用他人报告。

## 9. 清理与文档

实现中每一处新增结构都必须对应一项旧结构处理记录：删除重复字段/快照/订阅转换，或明确说明它为何仍需保留。禁止为了兼容而永久保留两份事实源。

`docs/reference/FEATURE_LOCATOR.md` 补充：

- Tool Capability descriptor 的定义入口；
- Tool registry 是唯一事实源的边界；
- model/protocol/recovery 投影入口；
- EffectScope 所有权与 disposer 规则；
- replay 默认 Unsafe 和 Safe 的双重 descriptor 检查；
- 尚未纳入本期的 Skill/Agent/Plugin/MCP/ACP registry，避免文档宣称全仓统一已经完成。

必要时同步 `docs/reference/ARCHITECTURE.md`，但不把未实现的未来形态写成现状。

## 10. 成功定义

本期只有同时满足以下条件才算完成：

- 至少一个真实 Tool capability 以 descriptor 为单一事实源；
- 至少一个注册表/元数据重复结构被删除或彻底降级为纯执行索引；
- 至少一个 model/protocol/recovery 多面投影来自同一 descriptor；
- 至少一个真实 scope 的 dispose 消除能力注册泄漏；
- 至少 Safe/Unsafe 两种 replay 语义有测试；
- 旧工具调用和既有协议行为不被无意改变；
- `src/harness/` 零改动；
- 文档准确说明已完成范围和未完成范围；
- 所有代码、测试和文档变更在新 worktree 分支提交。

## 11. 设计取舍

选择最小 Tool 主链而不是全局 registry，是为了让编译器和行为测试能够在一个真实能力面上证明“单一事实源、可替换、可释放、可恢复”。代价是第一期仍会存在其他领域的局部 registry；这不是完成全局目标，而是刻意的分阶段边界。下一期只有在 Tool 主链的删除、投影和恢复验收通过后，才应评估 Skill/Agent/MCP/ACP 的映射，避免并行 Zahir registry 长期存在。
