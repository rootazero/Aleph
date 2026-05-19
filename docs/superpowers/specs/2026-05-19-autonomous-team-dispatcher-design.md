# 设计规格：自主团队调度器 (Autonomous Team Dispatcher)

**日期**: 2026-05-19
**分支**: `team-autonomous-dispatcher`
**作者**: Claude (Aleph 协作)
**状态**: 设计待评审

---

## 1. 背景与问题 (Background)

参考项目 `hermes-agent`（Python 超级 AI 助手）的多智能体协作机制——**Kanban 看板**——对照 Aleph 现有 team 子系统，发现 Aleph 的 team **基建齐全但关键连线断裂**。

### 1.1 hermes-agent 的 team = Kanban 看板

| 机制 | 说明 |
|------|------|
| 持久化 DAG | SQLite `tasks` + `task_links`，依赖即协调协议 |
| 自主调度器 | `dispatch_once` 循环：回收僵死 claim → `recompute_ready`(父完成则子 `todo→ready`) → 原子 CAS `claim_task` → spawn worker |
| `build_worker_context` | 把任务体/历史尝试/父任务结果/角色历史/评论装配成**交接上下文信封**注入 worker |
| 完成/失败推送 | notifier 轮询事件 → 推送给发起用户 |

**值得学习**：DAG 即协调、交接上下文信封、原子 claim、未知 assignee 应显式失败。
**应当超越**：60s 轮询延迟、独立 OS 进程开销、无活体 agent 间消息、调度器单点。

### 1.2 Aleph team 的现状缺陷

Aleph 的 team 当前是**同步委派 (synchronous delegation)** 模型：

- `team_delegate` 是**阻塞式工具调用**——leader LLM 必须为每个任务手动调用、`tokio::spawn` 后 `timeout` 等待（默认 300s）、内联返回结果。本质是"穿了 team 外衣的 subagent"。
- **没有自主调度器**。创建任务 DAG 后，没有任何东西去执行 `ready` 的任务。

**断裂的连线**（基建已存在，仅缺连接）：

1. `CoordTaskStore`（`coord_tasks` / `coord_task_dependencies`）拥有完整 DAG、`get_newly_unblocked()`、`acquire_lock/release_lock/release_stale_locks`、循环检测——**但无消费者驱动它自主推进**。
2. `KanbanAutoUnblocker` 订阅 `AlephEvent::TeamTaskCompleted`，但 `team_delegate` 和 `task_update` **都不向 GlobalBus 发这个事件**（仅 `teams/events.rs` 内部构造）。它监听了一个没人发的事件。
3. `KanbanAutoUnblocker` 解除阻塞的是 `SqliteArtifactStore`（`task_artifacts` 表），而任务 DAG 在 `coord_tasks`——**连存储都对不上**。
4. `task_update` 把完成事件发到 `AgentMessageBus`（`ImportantEvent`），只有 `task_wait` 消费——与 GlobalBus 的 team 事件体系割裂。

**Bug**：`TeamSummary.task_count` 恒为 0——`teams/store.rs` JOIN 了废弃的 `team_tasks` 表，而真实任务写在 `coord_tasks`。

**死代码**（~1800-2200 行，零非测试调用者）：
- `src/agents/swarm/{coordinator,aggregator,context_injector,context_provider,collective_memory,rules,tools}.rs`——"swarm 感知层"，后台循环输出从不进入任何 prompt，且违反红线 **R10**（"Context Aggregation 多层合并"属越俎代庖）。
- `src/teams/kanban/`（`SqliteKanbanBoard`、`KanbanBoard` trait、`KanbanColumns`、`KanbanAutoUnblocker`）——被 `CoordTaskStore` 取代。
- `src/teams/lifecycle.rs`（269 行）——零调用者。
- `src/teams/plans.rs`（`PlanManager`，311 行，测试齐全）——零调用者（本设计将**接线为功能**，非删除）。

### 1.3 Aleph 的 Rust 架构优势

Aleph 的 team 成员是**进程内 tokio task**（经 `ExecutionAdapter`），拥有真正的 `abort_handle` 取消能力与结构化并发。这使 Aleph 可以做 hermes 做不到的事：

- **事件驱动零延迟调度**——任务完成立即触发下一轮，无 60s 轮询。
- **精确生命周期感知**——调度器持有 `JoinHandle`，无需 PID 探活。
- **结构化取消**——超时即 `abort()`，无 Python 线程"无法硬杀"问题。

---

## 2. 目标与非目标 (Goals / Non-Goals)

### 目标

1. **自主 DAG 执行**：leader 创建任务图（`task_create` 带 `blocked_by`）后，`TeamDispatcher` 事件驱动地把整张图推进到完成，零轮询延迟。
2. **交接上下文信封**：每个成员任务执行前注入确定性上下文（任务体 + 依赖结果 + 团队花名册 + 未读消息）。
3. **未知 owner 显式失败**：owner 不存在于 registry 时立即把任务标记 `Failed` 并附清晰错误（超越 hermes 的静默失败）。
4. **主动推送 (R5)**：团队完成 / 任务失败时经 Gateway 通道推送通知给发起用户。
5. **计划审批接线**：暴露 `plan_submit` / `plan_resolve` 工具；plan-gated 团队在计划获批前不调度。
6. **清理死代码**：删除 ~1800+ 行确认死代码，修复 `task_count` bug。

### 非目标

- 不改 `team_delegate` 的同步语义（保留为合法的同步委派原语，与自主路径并存）。
- 不引入跨进程 / 多机调度（Aleph 是"一核"架构 R6，OS `flock` 已强制单例——进程内调度即正确解，非妥协）。
- 不做任务自动重试 / 熔断器（v1 内 `Failed` 为终态；重试列为未来工作）。
- 不移动 `src/agents/swarm/tasks/`（保留原位避免大范围 import 改动，符合"非破坏性重构"）。
- 不做活体 agent 间实时消息推送（成员通过交接信封 + Inbox 拿到消息，被动消费）。

---

## 3. 架构设计 (Architecture)

### 3.1 新模块 `src/teams/dispatcher/`

调度器是 team 子系统的"调度模块"，物理上归属 `teams/`（**不进 `src/harness/`**——R10 要求 harness 保持 9 文件薄壳；调度器是子系统，不是认知层，是"12 模块各归其所"中的 scheduler 模块）。

```
src/teams/dispatcher/
├── mod.rs        — TeamDispatcher 结构、配置、公共 API、后台循环
├── schedule.rs   — dispatch_once：扫描可调度任务、并发上限、原子 claim
├── runner.rs     — 执行单个成员任务（经 ExecutionAdapter）
└── handoff.rs    — build_handoff_context：确定性交接上下文信封
```

### 3.2 核心：`TeamDispatcher`

```rust
pub struct TeamDispatcher {
    coord_store: Arc<dyn CoordTaskStore>,
    team_store: Arc<dyn TeamStore>,
    artifact_store: Arc<dyn ArtifactStore>,
    inbox: Arc<Inbox>,
    plan_manager: Arc<PlanManager>,
    context: GatewayContext,            // agent_registry + execution_adapter
    global_bus: Arc<GlobalBus>,         // 发 AlephEvent::TeamTask*
    config: DispatcherConfig,
    signal: Arc<Notify>,                // 触发一次调度
    running: Mutex<HashMap<CoordTaskId, JoinHandle<()>>>,
    semaphore: Arc<Semaphore>,          // 并发上限
}

pub struct DispatcherConfig {
    pub max_concurrent: usize,    // 默认 4
    pub lock_ttl_secs: u64,       // 默认 900
    pub task_timeout_secs: u64,   // 默认 600
    pub fallback_tick_secs: u64,  // 默认 60（兜底，捕捉丢失的 signal）
}
```

### 3.3 触发模型（事件驱动，零轮询）

调度器后台循环：

```rust
loop {
    tokio::select! {
        _ = self.signal.notified() => {}
        _ = sleep(fallback_tick) => {}   // 兜底心跳
    }
    self.dispatch_once().await;
}
```

`signal()` 被以下三处调用：

1. **`task_create` 工具**——创建无依赖（或依赖已满足）的任务后。
2. **`runner` 完成回调**——任务完成后，触发下一轮以拾取新解除阻塞的子任务。
3. **`plan_resolve` 工具**——计划获批后，解除 plan-gate。

### 3.4 `dispatch_once` 流程

```
1. release_stale_locks(lock_ttl)              — 回收 TTL 过期的锁
2. reclaim_orphaned()                          — InProgress 且无锁且不在 running map
                                                 → 重置为 Pending（重启对账 / 崩溃恢复）
3. list_tasks(status = Pending)                — derive_status 已排除 Blocked
4. 过滤可调度任务：
     - owner.is_some()
     - locked_by.is_none()
     - team 未被 plan-gate（见 §3.7）
5. 按 priority desc, created_at asc 排序
6. for task in schedulable:
     - 若 owner 不在 agent_registry → update_task(Failed, "owner 'X' 不存在")
       并发 TeamTaskFailed 事件；continue（超越 hermes 静默失败）
     - semaphore.try_acquire() 失败 → break（达并发上限）
     - acquire_lock(task, owner) 失败 → 释放 permit, continue（被他人抢占）
     - update_task(InProgress)
     - running.insert(task_id, spawn(runner::run_task(...)))
```

崩溃恢复：服务重启后首轮 `dispatch_once`，`reclaim_orphaned` 把所有遗留 `InProgress`（持有上个进程的陈旧锁、不在新 `running` map）重置为 `Pending` 重新调度。

### 3.5 `runner::run_task`

```
1. ctx = build_handoff_context(task)           — §3.6
2. RunRequest { input: ctx + task.subject, session_key: task-scoped, timeout }
3. execute_member_task(...)                     — 共享执行函数（见 §3.8）
4. 成功：
     - update_task(Completed, result)
     - artifact_store.create_artifact(Report)   — 持久化结果（best-effort）
     - release_lock
     - global_bus.publish(TeamTaskCompleted)    — ← 修复缺失的连线
5. 失败 / 超时 / panic：
     - update_task(Failed, error)
     - release_lock
     - global_bus.publish(TeamTaskFailed)
6. running.remove(task_id); drop(permit)
7. dispatcher.signal()                          — 触发下一轮
```

### 3.6 `build_handoff_context`（交接上下文信封）

借鉴 hermes 的 `build_worker_context`，融合 Aleph。所有分段**字节上限截断**：

```markdown
## 任务
{subject}
{description}

## 依赖结果           ← 仅当 task 有已完成依赖（DAG 扇入通道）
### {dep.subject}
{dep.result}

## 团队
你是团队 `{team_id}` 的成员 `{owner}`。
成员：{roster + roles}        ← 来自 TeamStore.get_members

## 未读消息            ← 来自 Inbox（若有）
{messages}
```

注入为 `RunRequest.input` 前置块。无 prior-attempts 段（v1 不重试，列为未来）。

### 3.7 计划审批工具 (Plan Approval Tools)

- 新工具 `plan_submit { team_id, title, content, task_id? }`→`PlanManager::submit_plan`：创建 Plan artifact + 向 leader 发 `PlanApprovalRequest` 消息。
- 新工具 `plan_resolve { team_id, plan_message_id, submitter_agent_id, decision, feedback? }`→`approve_plan/reject_plan`：leader 批准/拒绝，回复提交者。

> **实施期修订**：原设计含"调度器 plan-gate"——团队有未获批计划时跳过其任务。实现时发现 `PlanManager` 是基于**消息**的审批流，没有可查询的"团队计划状态"字段；硬门控需新增 schema。按 P6 (KISS) / R10 (YAGNI)，**descope 调度器 plan-gate**：`plan_submit`/`plan_resolve` 作为独立工具接线（这已满足"接线 plans.rs 为功能"），调度器不耦合计划状态。若未来确需强制门控，再引入可查询的计划状态。

### 3.8 共享执行函数 `execute_member_task`

`team_delegate` 现有的"构建 RunRequest → spawn → timeout → 解析结果"逻辑提取为 `runner.rs` 的 `execute_member_task()`。`team_delegate` 与 dispatcher runner 共用——消除重复（P6 三次法则：第二处使用且逻辑量大，提取合理）。`team_delegate` 行为不变（非破坏性）。

### 3.9 通知 (`TeamNotifier`)

调度器**只发事件**（P1 解耦），不直接管投递。新增 `TeamNotifier`（`EventHandler`，与现有 `TeamEventLogger` 并列，boot 时经 `GlobalBus` 注册）：

- 订阅 `TeamTaskCompleted` / `TeamTaskFailed`。
- 任务**失败** → 立即向团队 leader 的收件箱发 `SystemNotification` 消息。
- 任务**完成** → 仅当该团队全部任务都进入终态时，向 leader 发"团队完成"汇总（避免逐任务噪音）。
- 投递走 `MessageRouter`（leader 从 `TeamStore.get_team().leader_id` 查得），不引入新 schema、不耦合 Gateway。leader 是面向用户的 agent；结合 handoff 的 inbox 段，团队进度自然回流。

需新增事件变体 `AlephEvent::TeamTaskFailed`（`event/types.rs`）。

> **实施期修订**：原设计拟从 team metadata 的 `origin_session` 推送到发起通道。实现时发现团队无可查询的"发起会话"字段，且 `task_create` 工具拿不到 ambient 会话上下文。改为通知 leader 收件箱——用现有 `MessageRouter`，零新 schema，仍满足 R5。

---

## 4. 数据模型变更 (Data Model)

| 变更 | 文件 | 说明 |
|------|------|------|
| 新增 `TeamTaskFailed` 事件 | `src/event/types.rs` | 失败终态事件，驱动 `TeamNotifier` |
| `task_create` 任务标记 `managed_by: dispatcher` | `task_create` 工具 | 写入现有 `metadata` JSON，区分自主任务与 `team_delegate` 任务，无 schema 变更 |
| 删除 `team_tasks` 表 + JOIN + `TeamSummary.task_count` 字段 | `src/teams/store.rs`、`types.rs` | 该字段零外部消费者且恒为 0；`team_status` 工具已能从 `CoordTaskStore` 给出真实计数 |

**无破坏性迁移**：`coord_tasks` schema 不变；`team_tasks` 是死表，删除其建表语句与 JOIN 即可（旧数据库残留空表无害）。

---

## 5. 死代码清理 (Cleanup)

**删除**（确认零非测试调用者）：

- `src/agents/swarm/coordinator.rs`、`aggregator.rs`、`context_injector.rs`、`context_provider.rs`、`collective_memory.rs`、`rules.rs`、`tools.rs`
- `src/teams/kanban/`（整个目录：`mod.rs` + `unblocker.rs`）
- `src/teams/lifecycle.rs`
- `src/arena/events.rs` 的 `to_swarm_event()` 函数
- boot 中 `SwarmCoordinator` 创建/启动（`agent_init.rs`）、`KanbanAutoUnblocker` 注册（`start/mod.rs`）

**保留**：`src/agents/swarm/tasks/**`、`bus.rs`、`events.rs`、`mod.rs`（live——`CoordTaskStore` + `task_wait`），仅裁剪 `mod.rs` 中指向已删文件的导出。

净 LOC：删除 ~1800-2200 行，新增 ~900-1100 行 → **整体负增长**，符合"避免屎山堆积"。

---

## 6. 实施阶段 (Phasing)

| 阶段 | 内容 | 验证 |
|------|------|------|
| **P1** | 死代码清理 + `task_count` bug 修复 + 移除 boot 接线 | `cargo build -p alephcore` 通过；teams 测试通过 |
| **P2** | `TeamDispatcher` 核心（`mod`/`schedule`/`runner`/`handoff`）+ `execute_member_task` 提取 + `TeamTaskFailed` 事件 + 完成时发 `TeamTaskCompleted` | 单元测试：调度、claim、并发上限、孤儿回收、handoff 装配 |
| **P3** | 触发接线（`task_create` signal、boot 注册 dispatcher、fallback 循环）+ `TeamNotifier` 推送 | 集成测试：创建 DAG → 自动跑完；失败 → 推送 |
| **P4** | `plan_submit` / `plan_resolve` 工具 + 调度器 plan-gate | 集成测试：plan-gated 团队批准前不跑、批准后跑 |
| **P5** | e2e 验证 + 文档更新（`MULTI_AGENT_SYSTEM.md`）| 实跑一个多成员 team DAG |

每阶段独立可编译、可测试、可提交。P1 纯减法零风险先行。

---

## 7. 测试策略 (Testing)

- **单元测试**（`#[cfg(test)]`，内存 SQLite）：
  - `dispatch_once`：可调度过滤、优先级排序、并发上限、未知 owner→Failed、孤儿回收。
  - `build_handoff_context`：各分段装配 + 字节截断。
  - plan-gate：有未批计划时跳过。
- **集成测试**（`tests/` 或 `teams/integration_tests.rs` 扩展）：
  - 创建 A→B→C 链式 DAG，mock owner agent → 验证按序自动执行至全完成。
  - 钻石 DAG（A→[B,C]→D）→ 验证 B、C 并发、D 扇入。
  - 任务失败 → 验证 `TeamTaskFailed` 事件 + 下游不执行。
- **回归**：现有 teams 模块测试（基线 148 个）全绿；`team_delegate` 行为不变。
- 覆盖率目标 80%（新模块）。

---

## 8. 风险与缓解 (Risks)

| 风险 | 缓解 |
|------|------|
| 提取 `execute_member_task` 改动 `team_delegate` | 行为完全保持；用现有 `team_delegate` 测试做回归 |
| 删除 swarm 感知层影响 boot | 该层 `Arc` 在 `agent_init` 末尾本就被 drop，仅 `agent_message_bus` 被提取——移除是干净的反向操作 |
| 调度器与 `team_delegate` 双写 `coord_tasks` | 锁机制（`acquire_lock`）已保证互斥；`team_delegate` 创建的任务无 `blocked_by` 且自己执行，dispatcher 不会重复拾取（owner 锁 + InProgress 状态）|
| 并发执行多个成员的资源占用 | `max_concurrent` 信号量上限（默认 4）|
| 重启时 InProgress 任务丢失 | `reclaim_orphaned` 重启对账重新调度 |

---

## 9. 红线合规 (Redline Compliance)

- **R1**（大脑四肢分离）：调度器纯 Rust core 逻辑，不碰平台 API。✅
- **R3 / R10**（核心轻量 / 薄 harness）：调度器在 `teams/`，**不进 `src/harness/`**；删除违反 R10 的 swarm 感知层。✅
- **R5**（AI 主动到达）：`TeamNotifier` 多端推送。✅
- **R7 / R9**（LLM 主权 / 智慧在 prompt）：调度器是"笨循环"——只做 Think→Act 轮次调度，**不做意图分类 / 工具过滤 / 完成度判断**；任务编排由 leader LLM 经 `task_create` 完成。✅
- **R8**（工具即一切）：`plan_submit`/`plan_resolve` 暴露为工具。✅
- **P1/P2**（低耦合高内聚）：调度器发事件、`TeamNotifier` 投递，职责分离。✅
