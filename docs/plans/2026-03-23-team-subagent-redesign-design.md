# Team 功能重设计：Sub-Agent 替代命名 Agent

**日期**: 2026-03-23
**状态**: 已批准

## 问题

当前 `team_create` 为每个团队成员调用 `agent_create`，在 `AgentRegistry` 和文件系统中创建持久化命名 Agent。团队解散后这些 Agent 不会被清理，导致 Agent 列表积压大量一次性团队成员，管理混乱。

## 决策

Team 成员改为使用 **keep_alive Sub-Agent**，通过 persona 参数注入独立人格提示词。不再创建命名 Agent，零注册表污染，会话结束自动清理。

## 方案选型

| 方案 | 描述 | 优缺点 |
|------|------|--------|
| **A (选定)** | 改造 Sub-Agent，增加 persona + keep_alive | 改动最小，复用现有基础设施，符合奥卡姆剃刀 |
| B | 引入 TeamAgent 轻量层 | 概念清晰但增加新抽象 |
| C | Team 退化为纯任务编排 | 最简但丢失角色连续性 |

## 设计详情

### 1. Team 创建流程改造

**现状**: `team_create` → `agent_create` × N → 注册命名 Agent → 污染注册表

**改造后**:
```
team_create(name, members[{role, persona_prompt}])
  → subagent_spawn(persona=persona_prompt, keep_alive=true) × N
  → 记录 run_id 到 Team.members
  → Sub-Agent 保持存活，等待 steer
```

关键变化:
- `team_create` 不再调用 `agent_create`，改为 sub-agent spawn
- `TeamMember.agent_id` 改为 `TeamMember.run_id`，指向 Sub-Agent Run
- Leader 仍然是当前会话的主 Agent，不额外 spawn

### 2. Sub-Agent 人格注入机制

`subagent_spawn` 新增参数:
- `persona: Option<String>` — 人格提示词
- `keep_alive: bool` — 默认 false，true 时完成任务后不销毁

System prompt 组装顺序:
```
1. persona       ← "你是资深代码审查专家，风格严谨..."
2. agent_type    ← explore/plan/execute 的默认提示词
3. task context  ← 父 Agent 传入的任务上下文
```

`SubAgentRun` 新增字段:
```rust
pub struct SubAgentRun {
    // ... 现有字段不变
    pub persona: Option<String>,
    pub keep_alive: bool,
}
```

### 3. 状态机调整

新增 `Idle` 状态，keep_alive Sub-Agent 完成任务后进入 Idle 而非 Completed:

```
Pending → Running → Idle → Running → ... → Completed
                     ↑                        ↓
                     └── steer 重入 ──────────┘
```

- `Idle`: keep_alive Sub-Agent 完成当前任务，等待下一次 steer
- 同一成员多次 steer 共享会话历史（上下文延续）
- 非 keep_alive Sub-Agent 行为不变: Running → Completed

### 4. Team 生命周期与清理

**三种解散触发方式**:
1. **显式解散** — Leader 调用 `team_disband`，kill 所有 keep_alive Sub-Agent
2. **任务全部完成** — Team 所有 CoordTask 到达终态，自动解散
3. **会话结束** — `SubAgentRegistry` 清理策略自动回收

**清理内容**:
- Sub-Agent Run 从 `SubAgentRegistry` 移除
- 会话历史随会话消亡
- Team 记录在 SQLite 中保留，状态标记为 Disbanded（历史审计）
- 零文件系统残留、零 AgentRegistry 污染

### 5. 改动范围

**需要改的**:

| 文件 | 改动 |
|------|------|
| `agents/sub_agents/run.rs` | 增加 `persona`、`keep_alive` 字段；状态机增加 `Idle` |
| `agents/sub_agents/registry.rs` | 支持 Idle 状态的 steer 重入 |
| `builtin_tools/team_manage/create.rs` | 改为 spawn Sub-Agent |
| `builtin_tools/team_manage/launch.rs` | 同上 |
| `agents/swarm/tasks/mod.rs` | `TeamMember.agent_id` → `TeamMember.run_id` |
| `builtin_tools/subagent_spawn` | 接受 persona 和 keep_alive 参数 |
| Agent Loop system prompt 组装 | 插入 persona 前缀逻辑 |

**不需要改的**:
- `AgentRegistry` — 不涉及
- `agent_create` / `agent_delete` — 不涉及
- `CoordTaskStore` — 持久化逻辑不变
- `subagent_steer` / `subagent_kill` — 现有接口够用
- `session_send` — Team 不走 P2P

### 6. 约束与风险

- **成员数上限**: 8（与 Sub-Agent Lane 并发数一致），防止内存膨胀
- **Idle 状态机正确性**: 需确保所有转换路径覆盖（重点测试 Idle → Running 重入）
- **向后兼容**: 已有的命名 Agent 功能完全不受影响，Team 改造是独立变更
