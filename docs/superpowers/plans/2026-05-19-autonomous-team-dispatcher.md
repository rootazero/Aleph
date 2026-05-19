# 自主团队调度器 实施计划 (Autonomous Team Dispatcher Implementation Plan)

> **For agentic workers:** 本计划由作者在本会话内联执行（inline execution）。每个 Task 末尾有验证门，须通过后方可继续。Steps 用 checkbox (`- [ ]`) 追踪。

**Goal:** 为 Aleph team 子系统补上缺失的自主调度器，使 leader 创建任务 DAG 后由 `TeamDispatcher` 事件驱动地推进到完成。

**Architecture:** 新增 `src/teams/dispatcher/`（事件驱动、进程内、tokio task 执行），复用现有 `CoordTaskStore` DAG / `ExecutionAdapter` / 锁机制 / 事件总线。删除被取代的 swarm 感知层与 kanban 死代码。

**Tech Stack:** Rust, tokio (Notify/Semaphore/JoinHandle), rusqlite, async_trait。

**Spec:** `docs/superpowers/specs/2026-05-19-autonomous-team-dispatcher-design.md`

---

## 文件结构 (File Structure)

**新建：**
- `src/teams/dispatcher/mod.rs` — `TeamDispatcher` 结构、`DispatcherConfig`、后台循环、公共 API
- `src/teams/dispatcher/schedule.rs` — `dispatch_once`
- `src/teams/dispatcher/runner.rs` — `run_task` + 共享 `execute_member_task`
- `src/teams/dispatcher/handoff.rs` — `build_handoff_context`
- `src/teams/notifier.rs` — `TeamNotifier` 事件处理器
- `src/builtin_tools/team/plan_submit.rs` — `plan_submit` 工具
- `src/builtin_tools/team/plan_resolve.rs` — `plan_resolve` 工具

**修改：**
- `src/teams/mod.rs` — 注册 `dispatcher`、`notifier` 模块；移除 `kanban`、`lifecycle` 导出
- `src/agents/swarm/mod.rs` — 移除已删文件导出
- `src/event/types.rs` — 新增 `TeamTaskFailed`
- `src/builtin_tools/team/delegate.rs` — 改用共享 `execute_member_task`
- `src/builtin_tools/team/create.rs` — 记录 `origin_session`
- `src/builtin_tools/team/mod.rs` — 注册新工具
- `src/builtin_tools/task_manage/create.rs` — 创建后 `signal()`
- `src/teams/store.rs` — 删除 `team_tasks` 表与 JOIN
- `src/executor/builtin_registry/{definitions,constructor,registry,config,groups}.rs` — 注册 `plan_submit`/`plan_resolve`
- `src/bin/aleph-server/commands/start/builder/agent_init.rs` — 移除 SwarmCoordinator；构造 `TeamDispatcher`
- `src/bin/aleph-server/commands/start/mod.rs` — 移除 `KanbanAutoUnblocker`；注册 `TeamNotifier`、启动 dispatcher 循环

**删除：**
- `src/agents/swarm/{coordinator,aggregator,context_injector,context_provider,collective_memory,rules,tools}.rs`
- `src/teams/kanban/`（`mod.rs` + `unblocker.rs`）
- `src/teams/lifecycle.rs`
- `src/arena/events.rs::to_swarm_event`

---

## Phase 1 — 死代码清理 + bug 修复（纯减法，零风险先行）

### Task 1.1: 删除 swarm 感知层

**Files:** Delete `src/agents/swarm/{coordinator,aggregator,context_injector,context_provider,collective_memory,rules,tools}.rs`; Modify `src/agents/swarm/mod.rs`

- [ ] 删除上述 7 个文件
- [ ] `swarm/mod.rs`：移除 `mod coordinator;` 等声明与对应 `pub use`，仅保留 `tasks`、`bus`、`events`
- [ ] 全仓 grep 引用：`rg "swarm::(coordinator|aggregator|context_injector|context_provider|collective_memory|rules)"` — 修复每个引用点（应仅 boot 代码）
- [ ] 验证：`cargo check -p alephcore` 编译错误只剩 boot 处（Task 1.2 修复）

### Task 1.2: 移除 boot 中的 SwarmCoordinator

**Files:** Modify `src/bin/aleph-server/commands/start/builder/agent_init.rs`

- [ ] 删除 `SwarmCoordinator` 创建/`.start()`/3 个 loop 的代码块（约 `agent_init.rs:419-495`）
- [ ] 保留 `agent_message_bus`（live，`task_wait` 依赖）
- [ ] 删除 `TeamInboxContextProvider` 注入 swarm injector 的代码（约 `:454-460`）
- [ ] 验证：`cargo check -p alephcore` + `cargo check --bin aleph-server` 通过

### Task 1.3: 删除 teams/kanban/

**Files:** Delete `src/teams/kanban/`; Modify `src/teams/mod.rs`, `src/bin/aleph-server/commands/start/mod.rs`

- [ ] 删除 `src/teams/kanban/` 整个目录
- [ ] `teams/mod.rs`：移除 `pub mod kanban;` 与 `pub use kanban::{...}`
- [ ] `start/mod.rs`：删除 `KanbanAutoUnblocker` 注册块（约 `mod.rs:1005-1050`），保留 `TeamEventLogger`
- [ ] 验证：`cargo check --bin aleph-server` 通过

### Task 1.4: 删除 lifecycle.rs 与 to_swarm_event

**Files:** Delete `src/teams/lifecycle.rs`; Modify `src/teams/mod.rs`, `src/arena/events.rs`

- [ ] 删除 `src/teams/lifecycle.rs`，`teams/mod.rs` 移除 `pub mod lifecycle;`
- [ ] 删除 `src/arena/events.rs` 的 `to_swarm_event()` 函数（确认零调用者后）
- [ ] 验证：`cargo check -p alephcore` 通过

### Task 1.5: 修复 TeamSummary.task_count bug

**Files:** Modify `src/teams/store.rs`

- [ ] 删除 `team_tasks` 建表语句（`store.rs:136` 附近）
- [ ] `TeamSummary` 查询移除 `LEFT JOIN team_tasks`（`store.rs:253`、`:454`）；`task_count` 暂置 0 或由调用方经 `CoordTaskStore` 填充
- [ ] 决策：`task_count` 改为 `team_status`/handler 调用 `CoordTaskStore::list_tasks(team_id)` 计数（store 层不跨库 JOIN，符合 P5）
- [ ] 验证：`cargo test -p alephcore --lib teams::` 全绿
- [ ] **Commit:** `git commit -m "teams: remove dead swarm sensing + kanban layers, fix task_count"`

**Phase 1 验证门:** `cargo build -p alephcore` 成功；teams 模块测试基线全绿。

---

## Phase 2 — TeamDispatcher 核心

### Task 2.1: 新增 TeamTaskFailed 事件

**Files:** Modify `src/event/types.rs`

- [ ] 在 `EventType` 枚举加 `TeamTaskFailed`
- [ ] 在 `AlephEvent` 加 `TeamTaskFailed { team_id: Option<String>, task_id: String, error: String }`
- [ ] 补 `event_type()`、`name()` 的 match 分支
- [ ] 验证：`cargo check -p alephcore` 通过

### Task 2.2: 提取 execute_member_task

**Files:** Create `src/teams/dispatcher/runner.rs`(部分); Modify `src/builtin_tools/team/delegate.rs`

- [ ] 在 `runner.rs` 写 `pub async fn execute_member_task(context, agent_id, task_text, session_key, timeout) -> MemberRunResult`，封装 delegate.rs 现有的"查 registry → 构建 RunRequest → spawn → timeout → 取 last reply"逻辑
- [ ] `MemberRunResult { reply: Option<String>, error: Option<String>, status: enum Completed|Failed|Timeout }`
- [ ] `delegate.rs` 改为调用 `execute_member_task`，保持现有输出语义不变
- [ ] 验证：`cargo test -p alephcore --lib` 中 `team_delegate` 相关测试不变全绿

### Task 2.3: handoff.rs — 交接上下文信封

**Files:** Create `src/teams/dispatcher/handoff.rs`

- [ ] `pub async fn build_handoff_context(coord_store, team_store, inbox, task: &CoordTask) -> String`
- [ ] 装配分段（每段字节截断，常量 `MAX_SECTION_BYTES`）：任务 / 依赖结果（遍历 `task.dependencies`，取 `Completed` 依赖的 `result`）/ 团队花名册（`team_store.get_members`）/ 未读消息（`inbox`）
- [ ] 单元测试：构造 mock store，验证各段装配与截断
- [ ] 验证：`cargo test -p alephcore --lib teams::dispatcher::handoff`

### Task 2.4: schedule.rs — dispatch_once

**Files:** Create `src/teams/dispatcher/schedule.rs`

- [ ] `dispatch_once(&self)`：`release_stale_locks` → `reclaim_orphaned`（`InProgress` 且无锁且不在 `running` map → `Pending`）→ `list_tasks(Pending)` → 过滤(owner 存在/未锁/未 plan-gate) → 排序 → claim + spawn
- [ ] 未知 owner → `update_task(Failed)` + 发 `TeamTaskFailed`
- [ ] 单元测试：可调度过滤、优先级排序、并发上限、未知 owner→Failed、孤儿回收
- [ ] 验证：`cargo test -p alephcore --lib teams::dispatcher::schedule`

### Task 2.5: mod.rs — TeamDispatcher

**Files:** Create `src/teams/dispatcher/mod.rs`; Modify `src/teams/mod.rs`

- [ ] `TeamDispatcher` 结构（见 spec §3.2）、`DispatcherConfig`（默认值）、`new()`、`signal()`、`spawn_loop()`（`tokio::select!{ notified, sleep(fallback) }` → `dispatch_once`）
- [ ] `teams/mod.rs` 加 `pub mod dispatcher;`
- [ ] 验证：`cargo check -p alephcore` 通过

### Task 2.6: runner 完成路径 + 事件发布

**Files:** Modify `src/teams/dispatcher/runner.rs`

- [ ] `run_task`：`build_handoff_context` → `execute_member_task` → 成功 `update_task(Completed)` + 建 artifact + `release_lock` + 发 `TeamTaskCompleted`；失败 `update_task(Failed)` + 发 `TeamTaskFailed`；末尾 `running.remove` + `signal()`
- [ ] 单元测试：成功/失败路径状态流转 + 事件发布
- [ ] 验证：`cargo test -p alephcore --lib teams::dispatcher`
- [ ] **Commit:** `git commit -m "teams: add TeamDispatcher core (schedule, runner, handoff)"`

**Phase 2 验证门:** `cargo build -p alephcore` 成功；dispatcher 单元测试全绿。

---

## Phase 3 — 触发接线 + 通知

### Task 3.1: task_create 触发 signal

**Files:** Modify `src/builtin_tools/task_manage/create.rs`, `src/executor/builtin_registry/config.rs`

- [ ] `BuiltinToolConfig` 加 `dispatch_signal: Option<Arc<Notify>>`
- [ ] `task_create` 创建任务后若有 `team_id` 则 `signal()`
- [ ] 验证：`cargo check -p alephcore`

### Task 3.2: boot 注册 TeamDispatcher

**Files:** Modify `src/bin/aleph-server/commands/start/builder/agent_init.rs`, `src/bin/aleph-server/commands/start/mod.rs`

- [ ] `agent_init.rs`：构造 `TeamDispatcher`（注入各 store + `GatewayContext` + `GlobalBus`），共享 `Arc<Notify>` 给 `BuiltinToolConfig.dispatch_signal`
- [ ] `start/mod.rs`：`tokio::spawn(dispatcher.spawn_loop())`
- [ ] 验证：`cargo check --bin aleph-server`

### Task 3.3: TeamNotifier

**Files:** Create `src/teams/notifier.rs`; Modify `src/teams/mod.rs`, `src/bin/aleph-server/commands/start/mod.rs`

- [ ] `TeamNotifier` 实现 `EventHandler`，订阅 `TeamTaskCompleted`/`TeamTaskFailed`，从 team metadata 取 `origin_session` 经 `GatewayContext` 推送
- [ ] `start/mod.rs` 注册（与 `TeamEventLogger` 并列）
- [ ] 验证：`cargo check --bin aleph-server`

### Task 3.4: team_create 记录 origin_session

**Files:** Modify `src/builtin_tools/team/create.rs`

- [ ] 创建 team 时把当前 `SessionKey`/channel 写入 team `metadata.origin_session`
- [ ] 集成测试：创建 A→B→C 链 DAG，mock owner → 验证自动跑完；钻石 DAG 验证并发+扇入；失败任务下游不执行
- [ ] 验证：`cargo test -p alephcore --test '*'` 相关集成测试全绿
- [ ] **Commit:** `git commit -m "teams: wire dispatcher triggers + completion notifications"`

**Phase 3 验证门:** 集成测试——创建 DAG 后自动执行至完成。

---

## Phase 4 — 计划审批门控

### Task 4.1-4.2: plan_submit / plan_resolve 工具

**Files:** Create `src/builtin_tools/team/plan_submit.rs`, `plan_resolve.rs`; Modify `src/builtin_tools/team/mod.rs`

- [ ] `plan_submit { team_id, plan }` → `PlanManager::submit_plan`
- [ ] `plan_resolve { plan_id, decision, reason? }` → `approve_plan`/`reject_plan`；批准后 `signal()`
- [ ] 验证：`cargo check -p alephcore`

### Task 4.3: dispatcher plan-gate

**Files:** Modify `src/teams/dispatcher/schedule.rs`

- [ ] `dispatch_once` 过滤步骤加：team 存在未获批计划 → 跳过该 team 任务
- [ ] 单元测试：有未批计划时跳过、批准后调度
- [ ] 验证：`cargo test -p alephcore --lib teams::dispatcher`

### Task 4.4: 注册工具

**Files:** Modify `src/executor/builtin_registry/{definitions,constructor,registry,config,groups}.rs`

- [ ] 按现有 17 个 team 工具的模式注册 `plan_submit`/`plan_resolve`（definition + schema + dispatch arm + group）
- [ ] 验证：`cargo build -p alephcore` + `cargo test -p alephcore --lib`
- [ ] **Commit:** `git commit -m "teams: wire plan approval (plan_submit/plan_resolve + dispatcher gate)"`

**Phase 4 验证门:** 集成测试——plan-gated 团队批准前不跑、批准后跑。

---

## Phase 5 — e2e 验证 + 文档

### Task 5.1: e2e 验证

- [ ] 调用 e2e-verify skill 或手动实跑：起 server → 经工具创建多成员 team + 任务 DAG → 观察自动执行 + 通知推送

### Task 5.2: 文档更新

**Files:** Modify `docs/reference/MULTI_AGENT_SYSTEM.md`

- [ ] 更新为反映自主调度器实际行为；删除已不存在的 swarm 感知层描述

### Task 5.3: Changelog

**Files:** Modify `CHANGELOG.md`

- [ ] 加本次变更条目（English）
- [ ] **Commit:** `git commit -m "docs: update multi-agent docs for autonomous dispatcher"`

**Phase 5 验证门:** `cargo build` + `cargo test` 全绿；e2e 实跑通过。

---

## Self-Review 结论

- **Spec 覆盖**：spec §3-§9 全部映射到 Task（dispatcher 核心→P2，触发/通知→P3，plan→P4，清理→P1，文档→P5）。✅
- **类型一致**：`TeamDispatcher`/`DispatcherConfig`/`MemberRunResult`/`execute_member_task`/`build_handoff_context`/`dispatch_once`/`run_task` 跨 Task 命名一致。✅
- **无占位符**：每 Task 有明确文件、步骤、验证命令。代码细节在内联执行时按 spec 结构实现。✅
