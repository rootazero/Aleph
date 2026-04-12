# Teams 基础设施增强：广播、成员移除、Peek、任务锁

**Date:** 2026-04-08
**Status:** Approved
**Scope:** `src/teams/`, `src/builtin_tools/team/`, `src/agents/swarm/tasks/`
**Prerequisite:** `2026-04-08-teams-refactor-design.md`（已完成）

## 背景

对比 ClawTeam 项目后，识别出 Aleph teams 模块的四项基础设施缺失。这些都是通用团队协作的核心能力，不涉及角色专用逻辑，完全符合"聚焦基础设施"的方向。

### 设计原则

- 不照搬 ClawTeam，融合 Aleph 的 Rust/SQLite 架构优势
- 复用现有基础设施（MessageRouter、TeamStore、CoordTaskStore）
- R8 合规：锁是基础设施内部机制，不需要 LLM 手动管理
- P6 合规：不增加不必要的工具或抽象

---

## 1. 广播消息

### 问题

Leader 需要通知全体成员时，必须手动列出所有成员 ID 填入 `to` 字段。

### 方案

在 `message_send` 工具层实现，不改动 `MessageRouter`。

**改动文件：** `src/builtin_tools/team/message_send.rs`

### 设计

`MessageSendArgs` 新增字段：

```rust
/// If true, send to all team members (excluding sender).
/// Overrides `to` field — `to` is ignored when broadcast is true.
#[serde(default)]
pub broadcast: bool,
```

工具 `call()` 方法逻辑：
1. 当 `broadcast == true` 时，从 `TeamStore.get_members(team_id)` 获取成员列表
2. 过滤掉发送者自己
3. 将所有成员 ID 填入 `to` 列表
4. 正常调用 `router.send()`

### 架构决策

广播在 tool 层实现而非 MessageRouter 层，原因：
- **P1 (低耦合)**: MessageRouter 是纯消息路由原语，不应持有 TeamStore 引用
- **已有先例**: team_delegate 已在 tool 层查询 TeamStore 后操作
- **P6 (简洁)**: 三行逻辑不需要新的抽象层

---

## 2. 成员移除

### 问题

团队只能添加成员，无法移除。Shutdown 协议完成后无法清理成员记录。

### 方案

TeamStore trait 新增方法 + 新工具。

**改动文件：**
- `src/teams/store.rs` — TeamStore trait + SqliteTeamStore impl
- 新增 `src/builtin_tools/team/member_remove.rs`
- `src/builtin_tools/team/mod.rs` — 注册新工具

### Store 层

TeamStore trait 新增：

```rust
async fn remove_member(&self, team_id: &str, agent_id: &str) -> Result<()>;
```

SqliteTeamStore 实现约束：
- 校验团队存在且未 disbanded
- 禁止移除 leader（`team.leader_id != agent_id`）
- 执行 `DELETE FROM team_members WHERE team_id = ? AND agent_id = ?`

### Tool 层

新工具 `team_member_remove`：

```rust
pub struct TeamMemberRemoveArgs {
    pub team_id: String,
    pub agent_id: String,
}
```

- 验证调用者是 leader（只有 leader 可以移除成员）
- 调用 `store.remove_member()`
- 返回确认消息

### 与 Lifecycle 联动

`LifecycleManager.approve_shutdown()` 后，leader 可调用 `team_member_remove` 正式移除 agent。两个操作独立（消息协议 vs 数据操作），由 leader 的 LLM 决定是否连续执行（R8 合规）。

---

## 3. 消息 Peek

### 问题

`inbox.read()` 会标记消息为已读。Agent 无法预览收件箱状态（如：有多少未读？有紧急消息吗？）而不消费消息。

### 方案

Inbox 新增非消费读取方法 + 工具参数扩展。

**改动文件：**
- `src/teams/messages/inbox.rs` — 新增 peek 方法
- `src/builtin_tools/team/inbox_read.rs` — 新增参数

### Inbox 层

新增：

```rust
/// Non-destructive read — returns unread messages without marking as read.
pub async fn peek(
    &self,
    agent_id: &str,
    team_id: &str,
    msg_type: Option<&MessageType>,
) -> Result<Vec<TeamMessage>>;

/// Returns unread message count without reading content.
pub async fn peek_count(
    &self,
    agent_id: &str,
    team_id: &str,
) -> Result<PeekCount>;

pub struct PeekCount {
    pub to: u64,
    pub cc: u64,
}
```

实现与 `read()` 几乎相同，区别：
- `peek()` 不调用 `mark_read()`，不记录 `MessageRead` 事件
- `peek_count()` 只执行 COUNT 查询，不返回消息体

### Tool 层

`InboxReadArgs` 新增参数：

```rust
/// If true, peek at messages without marking them as read.
#[serde(default)]
pub peek: bool,

/// If true, only return unread count (no message content).
#[serde(default)]
pub count_only: bool,
```

路由逻辑：
- `count_only == true` → `inbox.peek_count()` → 返回 `{to: N, cc: N}`
- `peek == true` → `inbox.peek()` → 返回消息但不标记已读
- 默认 → `inbox.read()` → 标记已读（现有行为不变）

---

## 4. 任务锁

### 问题

多个 agent 可能并发操作同一个 CoordTask（如同时 update 状态），存在竞态风险。

### 方案

CoordTask 新增锁字段 + CoordTaskStore 新增锁方法。锁由工具自动管理，不暴露为独立工具。

**改动文件：**
- `src/agents/swarm/tasks/mod.rs` — CoordTask 新增字段
- `src/agents/swarm/tasks/store.rs` — 新增锁方法 + DB migration

### 类型变化

```rust
pub struct CoordTask {
    // ... existing fields ...
    /// Agent currently holding the lock (None = unlocked).
    pub locked_by: Option<String>,
    /// When the lock was acquired (for stale lock detection).
    pub locked_at: Option<DateTime<Utc>>,
}
```

### Store 层新增方法

```rust
/// Acquire lock. Ok if acquired or already held by same agent. Err if held by another.
async fn acquire_lock(&self, task_id: &str, agent_id: &str) -> Result<()>;

/// Release lock. Only the holder can release.
async fn release_lock(&self, task_id: &str, agent_id: &str) -> Result<()>;

/// Release all locks older than max_age. Returns count released.
async fn release_stale_locks(&self, max_age: chrono::Duration) -> Result<usize>;
```

### 实现要点

- **原子性**: `UPDATE coord_tasks SET locked_by = ?1, locked_at = ?2 WHERE id = ?3 AND (locked_by IS NULL OR locked_by = ?1)` — 单条 SQL，无需应用层锁
- **幂等性**: 同一 agent 重复 acquire 同一 task 不报错
- **超时保护**: `release_stale_locks(Duration::minutes(30))` 防止 agent 崩溃后永久死锁
- **自动管理**: 不暴露为独立工具

### 自动锁管理（通过现有工具）

| 时机 | 操作 | 位置 |
|------|------|------|
| `team_delegate` 分配任务 | 自动 acquire_lock | delegate.rs |
| `task_submit` 提交结果 | 自动 release_lock | task_submit.rs |
| Task Failed/Cancelled | 自动 release_lock | delegate.rs |

### DB Migration

```sql
ALTER TABLE coord_tasks ADD COLUMN locked_by TEXT;
ALTER TABLE coord_tasks ADD COLUMN locked_at TEXT;
```

在 `SqliteCoordTaskStore::migrate()` 中添加（与现有 migration 模式一致）。

---

## 5. 变更总结

### 新增文件

| 文件 | 预估行数 | 说明 |
|------|----------|------|
| `src/builtin_tools/team/member_remove.rs` | ~70 | 成员移除工具 |

### 修改文件

| 文件 | 变化 |
|------|------|
| `src/builtin_tools/team/message_send.rs` | +broadcast 参数 (~30 行) |
| `src/builtin_tools/team/inbox_read.rs` | +peek/count_only 参数 (~30 行) |
| `src/builtin_tools/team/mod.rs` | 注册 member_remove |
| `src/teams/store.rs` | +remove_member() (~20 行) |
| `src/teams/messages/inbox.rs` | +peek()/peek_count() (~40 行) |
| `src/agents/swarm/tasks/mod.rs` | +locked_by/locked_at 字段 |
| `src/agents/swarm/tasks/store.rs` | +acquire/release/stale_locks (~80 行) + migration |
| `src/builtin_tools/team/delegate.rs` | +auto acquire_lock |
| `src/builtin_tools/team/task_submit.rs` | +auto release_lock |
| `src/executor/builtin_registry/builder.rs` | 注册 member_remove 工具 |

### 净新增

~270 行代码，全部是通用基础设施。

---

## 6. 与 ClawTeam 对比：Aleph 的超越

| 能力 | ClawTeam | Aleph（增强后） |
|------|----------|-----------------|
| 广播 | `broadcast()` 独立函数 | tool 层参数，不污染路由器 |
| 成员移除 | 无 | ✓ 有，含 leader 保护 |
| Peek | `peek()` + `peek_count()` | ✓ 平齐 + 与 To/Cc 角色集成 |
| 任务锁 | 文件级 advisory lock | **SQL 原子锁 + 超时保护 + 自动管理** |
| 锁可靠性 | 进程崩溃后需手动清理 | `release_stale_locks()` 自动恢复 |
| 锁暴露 | 需手动调用 | 工具自动管理（R8 合规） |
