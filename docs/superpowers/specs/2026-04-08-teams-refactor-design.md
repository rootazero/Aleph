# Teams 模块重构：剔除过度设计，聚焦基础设施

**Date:** 2026-04-08
**Status:** Approved
**Scope:** `src/teams/`, `src/builtin_tools/team/`, `src/agents/prompts/`

## 背景

Teams 模块参考了学术界关于团队协作的研究，内置了 Explorer（探索者）和 Critic（批评者）角色，包含完整的 ReviewScore 评审系统。这属于过度设计 — Aleph 作为通用 AI agent，不应内置特定的对抗性角色和评审流程。角色定义应由用户在创建 team 时根据需要自行设置。

参考项目：[ClawTeam](/Volumes/TBU4/Github/ClawTeam) — 聚焦团队基础设施（消息、任务、生命周期），角色只是字符串标签。

## 设计原则

- **R8 (LLM 主权)**: ReviewScore 的维度评分、最低挑战数等验证逻辑是用确定性代码替代 LLM 判断，属于"越俎代庖"
- **R10 (智慧在 Prompt 中)**: 如需 Critic 角色，用户在 agent prompt 中定义行为即可
- **P6 (简洁性/YAGNI)**: 删除未被充分使用的角色专用代码
- **学习但不照搬 ClawTeam**: 吸收其基础设施理念，发挥 Aleph SQLite + Rust 架构优势

---

## 1. 模块结构变化

### 重构前

```
src/teams/
├── mod.rs
├── types.rs
├── store.rs
├── artifacts.rs
├── events.rs
├── context.rs
├── messages/
│   ├── types.rs
│   ├── router.rs
│   ├── inbox.rs
│   └── store.rs
├── sessions/
│   ├── types.rs
│   ├── store.rs
│   └── coordinator.rs
└── roles/                      ← 整个删除
    ├── types.rs
    ├── prompts.rs
    ├── review.rs
    └── mod.rs
```

### 重构后

```
src/teams/
├── mod.rs
├── types.rs                    # role 保持 String
├── store.rs
├── artifacts.rs                # ArtifactType 精简
├── events.rs
├── context.rs
├── messages/
│   ├── types.rs                # MessageType 精简 + 新增变体
│   ├── router.rs
│   ├── inbox.rs
│   └── store.rs
├── sessions/
│   ├── types.rs
│   ├── store.rs
│   └── coordinator.rs
├── lifecycle.rs                ← 新增
└── plans.rs                    ← 新增
```

---

## 2. 类型系统变化

### 2.1 AgentRole 枚举 → 删除

`AgentRole` 枚举（Leader, Explorer, Critic, Worker, Custom）整个删除。`TeamMember.role` 字段在数据库中本已是 `String`，enum 只是多余的转换层。

Leader 身份通过 `Team.leader_id` 结构化标识，不依赖 role 字段。

### 2.2 MessageType 精简

```rust
// 改动后
pub enum MessageType {
    Message,
    SystemNotification,
    Idle,
    PlanApprovalRequest,
    PlanApproved,
    PlanRejected,
    ShutdownRequest,          // 新增
    ShutdownApproved,         // 新增
    ShutdownRejected,         // 新增
    Custom(String),           // 新增
}
```

删除：`Discovery`、`Challenge`、`ReviewRequest`、`ReviewResult`

`from_stored()` 对未知字符串 fallback 到 `Custom(s)`，保证旧数据兼容。

### 2.3 ArtifactType 精简

```rust
// 改动后
pub enum ArtifactType {
    Report,
    Code,
    Plan,                     // 新增
    Custom(String),
}
```

删除：`Discovery`、`Challenge`、`Review`

`from_stored()` 对旧值 fallback 到 `Custom(s)`。

### 2.4 TeamRoleConfig → 删除

整个结构体删除。所有字段（`role`、`prompt_template`、`review_dimensions`、`min_score_threshold`、`min_challenges`）都是 Critic 评审专用。

### 2.5 ReviewScore 全套类型 → 删除

删除：`ReviewScore`、`DimensionScore`、`Challenge`、`Severity`

---

## 3. Lifecycle 管理

基于现有消息层的协议，thin wrapper over `MessageRouter`。

```rust
// lifecycle.rs
pub struct LifecycleManager {
    msg_router: Arc<MessageRouter>,
    event_store: Arc<dyn EventLogStore>,
}
```

### API

| 方法 | 说明 |
|------|------|
| `request_shutdown(team_id, from_agent, leader_id, reason)` | Agent 请求关闭 → ShutdownRequest 消息 |
| `approve_shutdown(team_id, leader_id, agent_id, request_msg_id)` | Leader 批准 → ShutdownApproved 消息 |
| `reject_shutdown(team_id, leader_id, agent_id, request_msg_id, reason)` | Leader 拒绝 → ShutdownRejected 消息 |
| `send_idle(team_id, agent_id, leader_id, last_task)` | Agent 报告空闲 → Idle 消息 |

### 设计要点

- 不新增数据库表 — 交互通过现有消息系统
- 不自动杀进程 — Aleph 单进程，agent 是逻辑概念，"shutdown" 意味着从 team 移除
- 事件日志 — shutdown/idle 事件记录到 EventLogStore

---

## 4. Plan 审批工作流

基于消息层 + Artifact 存储。

```rust
// plans.rs
pub struct PlanManager {
    msg_router: Arc<MessageRouter>,
    artifact_store: Arc<dyn ArtifactStore>,
    event_store: Arc<dyn EventLogStore>,
}

pub struct PlanSubmission {
    pub artifact: TaskArtifact,
    pub message: TeamMessage,
}
```

### API

| 方法 | 说明 |
|------|------|
| `submit_plan(team_id, from_agent, leader_id, title, content, task_id)` | 存为 Plan Artifact + 发 PlanApprovalRequest |
| `approve_plan(team_id, leader_id, agent_id, plan_msg_id, feedback)` | 发 PlanApproved |
| `reject_plan(team_id, leader_id, agent_id, plan_msg_id, reason)` | 发 PlanRejected |

### 设计要点

- 复用 Artifact 存储 — 计划内容作为 `ArtifactType::Plan` 存入 `task_artifacts` 表
- 无状态机 — 消息本身就是状态记录，LLM 根据消息历史判断（R8 合规）
- 全链路可追溯 — task_id 串联 计划 → 审批 → 执行 → 产物

---

## 5. Tool 变化

### 删除（1 个）

| 工具 | 原因 |
|------|------|
| `review_score` | Critic 专用 |

### 新增（5 个）

| 工具 | 参数 | 说明 |
|------|------|------|
| `shutdown_request` | team_id, reason? | Agent 请求关闭 |
| `shutdown_respond` | team_id, agent_id, request_msg_id, approved, reason? | Leader 批准/拒绝 |
| `plan_submit` | team_id, task_id, title, content | 提交计划给 leader |
| `plan_approve` | team_id, agent_id, plan_msg_id, feedback? | Leader 批准 |
| `plan_reject` | team_id, agent_id, plan_msg_id, reason | Leader 拒绝 |

### 保留（13 个）

team_create, team_status, team_disband, team_delegate, message_send, inbox_read, team_digest, task_submit, task_read_artifact, session_collaborate, session_turn, session_read

总数：14 → 18（删 1 + 增 5）

---

## 6. Prompt 模板处理

### 删除

- `src/agents/prompts/team_explorer.md`
- `src/agents/prompts/team_critic.md`

### 保留并更新

- `src/agents/prompts/team_leader.md` — 删除 Explorer-Critic review cycle 指导，增加 Plan 审批和 Lifecycle 工具指导
- `src/agents/prompts/team_worker.md` — 删除 reviewer/critic 提及，增加 plan_submit 和 shutdown_request 指导

### Prompt 加载方式

删除 `roles/prompts.rs`。Prompt 加载移到 `team_delegate` 工具：
- `role == "leader"` → 注入 `team_leader.md`
- `role == "worker"` → 注入 `team_worker.md`
- 其他 → 使用用户提供的 `prompt` 参数

---

## 7. 删除与新增清单

### 删除文件

| 文件 | 行数 |
|------|------|
| `src/teams/roles/types.rs` | ~133 |
| `src/teams/roles/prompts.rs` | ~44 |
| `src/teams/roles/review.rs` | ~230 |
| `src/teams/roles/mod.rs` | ~10 |
| `src/builtin_tools/team/review_score.rs` | ~100 |
| `src/agents/prompts/team_explorer.md` | - |
| `src/agents/prompts/team_critic.md` | - |
| **合计** | **~517+** |

### 新增文件

| 文件 | 预估行数 |
|------|----------|
| `src/teams/lifecycle.rs` | ~150 |
| `src/teams/plans.rs` | ~250 |
| `src/builtin_tools/team/shutdown_request.rs` | ~60 |
| `src/builtin_tools/team/shutdown_respond.rs` | ~70 |
| `src/builtin_tools/team/plan_submit.rs` | ~80 |
| `src/builtin_tools/team/plan_approve.rs` | ~60 |
| `src/builtin_tools/team/plan_reject.rs` | ~60 |
| **合计** | **~730** |

### 净变化

+213 行，功能从"角色专用评审系统"转为"通用基础设施"。

---

## 8. 与 ClawTeam 对比：学习与超越

| 方面 | ClawTeam | Aleph（重构后） | 优势 |
|------|----------|-----------------|------|
| 存储 | 文件系统 JSON | SQLite | SQL 查询、事务、并发安全 |
| 传输 | File + P2P/ZMQ | 进程内 MessageRouter | 零网络开销、零序列化 |
| 角色 | 纯字符串 | 纯字符串 | 平齐 |
| Lifecycle | LifecycleManager | LifecycleManager | 平齐 + 事件日志 |
| Plan 审批 | PlanManager + 文件 | PlanManager + Artifact | 计划与任务全链路关联 |
| 会话协作 | 无 | CollaborativeSession | **Aleph 独有** |
| 事件溯源 | 简单事件日志 | EventLogStore | **Aleph 独有** |
| 上下文注入 | 无 | InboxContext | **Aleph 独有** |
| 消息升级 | 无 | EscalationRule | **Aleph 独有** |
