# Subagent Dead Code Cleanup Design

> 清理子代理系统中的死代码和空壳工具，巩固 SubagentTool 作为唯一的临时子代理调度路径。

## 背景

SubagentTool 渐进重构的前 4 步已完成（agent_type/model/background/context_summary），旧的 TaskTool 已在 commit `a30e02e3` 中被删除。经全面评估，剩余两个"旧路径"候选为死代码/空壳，应予清理。

## 评估结论

### 删除：SubAgentHandler（死代码）

- **位置**: `src/components/subagent_handler.rs` (~472 行)
- **问题**: `SubAgentHandler::new()` 仅在自身 `#[cfg(test)]` 中调用，整个 `src/` 无消费者。它被 `pub use` 导出但零使用。
- **原因**: 基于 EventBus 的生命周期追踪设计属于旧架构遗留。当前 SubagentTool 使用 `BackgroundAgentTracker` 直接管理子代理生命周期，两者功能重叠但互不关联。
- **清理范围**:
  - 删除 `src/components/subagent_handler.rs`
  - 从 `src/components/mod.rs` 移除 `pub mod subagent_handler` 和 `pub use subagent_handler::SubAgentHandler`

### 删除：EscalateTaskTool（空壳占位）

- **位置**: `src/builtin_tools/escalate_task.rs` (~103 行)
- **问题**: `call()` 仅做字符串校验后返回 `{accepted: true}`，不触发任何实际路由或执行策略切换。
- **违反原则**:
  - R8（LLM 主权）— 给 LLM 提供一个虚假能力，accept 后无后续动作
  - P6（简洁性 / YAGNI）— 无必要的实体
- **清理范围**:
  - 删除 `src/builtin_tools/escalate_task.rs`
  - 从 `src/builtin_tools/mod.rs` 移除 `pub mod escalate_task` 和 re-export
  - 从 `src/executor/builtin_registry/definitions.rs` 移除 import、BuiltinToolDefinition 条目、create_tool match arm
  - 从 `src/executor/builtin_registry/groups.rs` 移除 "escalate_task" 条目

### 保留：TeamDelegateTool（不同职责）

- **位置**: `src/builtin_tools/team/delegate.rs` (~401 行)
- **理由**: 与 SubagentTool 正交 — 不同层级（Gateway vs AgentLoop）、不同生命周期（持久 vs 临时）、不同关系模型（leader→member vs parent→child）。它是团队协作系统（team_create/delegate/status/disband）的核心组件，删除会破坏 Team 子系统。

## 变更汇总

| 文件 | 操作 |
|------|------|
| `src/components/subagent_handler.rs` | 删除 |
| `src/components/mod.rs` | 移除引用 |
| `src/builtin_tools/escalate_task.rs` | 删除 |
| `src/builtin_tools/mod.rs` | 移除引用 |
| `src/executor/builtin_registry/definitions.rs` | 移除 import + 定义 + match arm |
| `src/executor/builtin_registry/groups.rs` | 移除 "escalate_task" |

**~575 行删除，2 个文件删除，4 个文件微调。零新依赖，零功能损失。**

## 测试策略

1. `cargo check -p alephcore` — 编译通过
2. `cargo test -p alephcore --lib` — 全量单元测试通过
3. `cargo clippy -p alephcore -- -D warnings` — 无新警告
4. 确认无 dead_code 或 unused import 警告
