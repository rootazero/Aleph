---
date: 2026-04-04
topic: two-phase-exec-approval
---

# Two-Phase Exec Approval + LLM 判断

## Problem Frame

当前工具执行审批系统使用数字化的 confidence 阈值（默认 0.7）来决定是否需要用户确认。这是一个确定性规则引擎，违反了架构红线 Arch-R8（LLM 主权原则）— 用硬编码阈值替代了 LLM 擅长的语义判断。

LLM 天然具备评估工具调用安全性和合理性的能力（理解上下文、判断副作用、评估参数风险），但当前系统没有利用这个能力。结果是：低风险操作频繁打扰用户确认，高风险操作可能因 confidence 数值偏高而滑过。

## Requirements

> **命名约定**: 本文档中 R1-R16 为本地需求编号。引用 CLAUDE.md 架构红线时使用 `Arch-R8`、`Arch-R10` 前缀以避免歧义。

**Core: LLM 自审机制**

- R1. LLM 在同一次生成中同时输出 tool_call 和审批决策（`approval_action`），零额外 LLM 调用，符合 Arch-R10
- R2. `approval_action` 三个值：`auto_execute`（LLM 认为安全，直接执行）、`ask_user`（需要用户确认）、`block`（LLM 认为不应执行）
- R3. LLM 同时输出 `approval_reason`（自然语言理由），用于用户确认界面展示和运行时日志（通过 `tracing` 宏输出，不涉及独立审计存储）

**Safety Floor（硬安全兜底）**

- R4. 系统维护一个不可被 LLM 覆盖的 `always_confirm` 工具列表（用户可配置），列表中的工具无论 LLM 输出什么 approval_action，一律强制 `ask_user`。这是对 LLM 自审循环信任风险的确定性兜底
- R5. 当 LLM 输出的 approval_action 缺失、无法解析、或值不在 {auto_execute, ask_user, block} 枚举内时，系统默认回退为 `ask_user`，永远不默认 `auto_execute`

**Block 处理**

- R6. LLM 输出 block 时同时指定 `block_action`：`notify`（危险操作，通知用户）或 `retry`（有更好方案，静默重试）
- R7. `retry` 最多 2 次（per tool_call invocation，计数器在每次新的 tool_call 时重置），超过后自动升级为 `notify`
- R8. retry 前从对话历史中移除被 block 的 assistant turn，并注入一条 system message 说明 block 原因，引导 LLM 生成替代方案（避免重复生成相同调用）
- R9. `notify` 时复用现有确认界面展示工具名、参数摘要、block 理由，用户选择 Approve（执行）或 Cancel（取消）。不引入新的 UI 组件

**与现有系统的关系**

- R10. 完全替代现有的 confidence-based confirmation 机制。涉及两个独立代码路径：dispatcher 确认子系统（`src/dispatcher/confirmation.rs`、`async_confirmation.rs`）和 agent loop 工具执行路径（`src/agent_loop/tool_pipeline.rs`）
- R11. 保留 TrustStage 机制（Draft → Trial → Verified）作为 LLM 判断的上下文输入 — LLM 可参考工具的信任阶段做出更好的判断
- R12. 保留 async approval forwarding（approval_bridge）— `ask_user` 决策仍通过现有通道转发给用户

**Prompt 集成**

- R13. 通过 system prompt 模板指导 LLM 输出审批决策，不增加新的中间件或 LLM 调用
- R14. Prompt 中提供所有已注册工具的 TrustStage 聚合列表（非 per-call，因为具体工具在生成前未知）、`always_confirm` 列表、会话上下文作为判断依据
- R15. Prompt 明确指导 LLM：approval_reason 中不得复述工具参数原文（防止敏感信息泄露到日志），只描述操作意图和安全评估

**Structured Output 载体**

- R16. approval_action 和 approval_reason 通过 ProviderResponse 的 text 字段以约定格式（JSON block）输出，系统在 tool_call 执行前解析。不修改 NativeToolCall 结构，不要求 provider 支持 structured output — 仅依赖 prompt 引导

## Accepted Risks

- **LLM 自审循环信任**: 同一次生成中 LLM 既提议 tool_call 又输出 approval_action，不具备独立验证者属性。Prompt injection 可能导致 LLM 对危险操作输出 auto_execute。此风险通过 R4（always_confirm 硬兜底）和 R5（解析失败回退 ask_user）缓解，但不完全消除。这是"零额外 LLM 调用"约束下的有意取舍 — 独立 Gate LLM 可消除此风险但违反 Arch-R10

## Success Criteria

- 对 TrustStage=Verified 且不在 always_confirm 列表中的工具，用户确认率降至 20% 以下（当前为 ~100%）
- 对 always_confirm 列表中的工具和 TrustStage=Draft 的工具，确认率保持 100%
- 零额外 LLM API 调用（审批判断内嵌于主生成；block/retry 的重新生成不计入此标准，因为它替代了原本会发生的生成）
- 现有 approval forwarding 和 TrustStage 机制继续正常工作

## Scope Boundaries

- 不实现独立的 Gate LLM 调用（违反 Arch-R10）
- 不修改 TrustStage 的升级逻辑（保持现有 Draft → Trial → Verified 流程）
- 不改变 approval_bridge 的转发机制（只改变触发条件：从 confidence < threshold 变为 LLM 输出 ask_user）
- 不引入新 UI 组件 — block/notify 复用现有确认界面（Approve/Cancel），不增加 Force Execute 或 Regenerate 按钮
- 不涉及独立审计存储 — approval_reason 通过运行时 tracing 日志输出

## Key Decisions

- **Propose → Approve 模式**：LLM 在同一次生成中输出 tool_call + 审批决策，而非使用独立的 Gate LLM 调用。原因：符合 Arch-R8 和 Arch-R10，零额外成本。接受循环信任风险，通过 always_confirm 硬兜底缓解
- **替代而非叠加 confidence**：LLM 语义判断严格优于数字阈值，叠加只增加复杂度。原因：Arch-R8 禁止用确定性代码替代 LLM 推理
- **Block 双策略**：notify（危险）和 retry（可优化），由 LLM 自行选择。原因：Arch-R8 — 系统不做判断，只执行 LLM 决定
- **Retry 上限 2 次 per invocation**：防止无限循环，超限升级为用户通知。原因：安全兜底
- **always_confirm 硬兜底**：对高危工具类别保留确定性安全屏障，不交给 LLM 判断。原因：缓解自审循环信任风险
- **Text-based 载体**：approval_action 通过 text 字段 JSON block 传递，不修改 NativeToolCall。原因：避免改动所有 4 个 protocol adapter，降低实现复杂度
- **Retry 历史清理**：block/retry 时移除被 block 的 turn 并注入 system message。原因：防止 LLM 重复生成相同被 block 的调用

## Dependencies / Assumptions

- 假设 LLM 能可靠地在 text 字段中输出约定格式的 JSON block（approval_action + approval_reason），仅依赖 prompt 引导
- 依赖现有 approval_bridge 和 TrustStage 基础设施继续工作
- 假设 prompt token 增加（审批指令 + TrustStage 聚合列表）对性能影响可接受

## Outstanding Questions

### Deferred to Planning

- [Affects R10][Needs research] 移除 confidence-based confirmation 后，dispatcher 确认子系统和 agent loop 工具执行路径各自如何迁移？是否需要分阶段？
- [Affects R13][Technical] system prompt 模板中审批指令的具体措辞和格式，需要结合现有 prompt 架构设计
- [Affects R14][Technical] TrustStage 聚合列表的格式和注入位置（system prompt 的哪个层级）
- [Affects R16][Technical] text 字段中 JSON block 的具体解析策略和边界定界符设计

## Next Steps

→ `/ce:plan` for structured implementation planning
