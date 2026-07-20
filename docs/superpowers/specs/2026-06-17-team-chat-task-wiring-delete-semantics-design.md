# 团队聊天任务连线 + 解散/删除语义修复 — 设计文档

- 日期: 2026-06-17
- 范围: 三个独立问题，共享"团队聊天/团队 tab"领域
- 红线相关: R7 (LLM 主权)、R9 (智慧在 Prompt)、R10 (薄 Harness/笨循环)、P2 (高内聚)、P6 (简洁)

---

## 0. 背景与核心发现

系统存在**两条互不相通的团队执行路径**：

| | 群聊（广播） | 任务编排（DAG） |
|---|---|---|
| 入口 | `teams.chat.send` → `GroupChatBroadcaster` (`src/teams/broadcast/mod.rs`) | agent 调 `team_delegate` / `task_create` |
| 行为 | 平等广播，成员互相回复 | 创建 `coord_task` 行 |
| 产生任务？ | **否** | **是** |

**关键诊断**：看板/计划/任务的**读取侧完全正常**：
- 全局 `KanbanView` (`interfaces/webchat/src/views/teams/kanban.rs`) 按 `team_id` 拉 `teams.list_tasks`，并订阅 `team.*.task.*` 事件做实时刷新。
- `coord_store.create_task`/`update_task` (`src/agents/swarm/tasks/store/mod.rs:71-94`) 确实发布 `team.<id>.task.created/.updated` 事件。

它们空白，**纯粹是因为群聊广播路径从不写入 `coord_task`**。这不是"没连线"，而是"上游无数据"。

**第二个关键发现（关于成员执行可视化）**：被 `team_delegate` 派活的成员执行**已经会**通过 `TeamFanoutEmitter` (`execute_member_task`, `src/.../runner.rs:240-254`) 把最终回复发到 `team.<id>.message`、把工作/完成状态发到 `team.<id>.activity`，前端 `subscribe_team_events` (`interfaces/webchat/src/views/chat/team_events.rs:23-71`) 已渲染为气泡 + 名册状态徽标。因此"成员执行实时进群聊"几乎随 Problem 1 的核心修复自动成立。

---

## 1. Problem 1 — 让 leader 在群聊里自动编排产生任务

### 根因
群聊里 leader 已能拿到 `team_delegate` / `task_create` 工具，`team_id` 也已注入 prompt。唯一障碍是 leader 的 prompt 把编排说成"可选、非义务"（`src/teams/broadcast/member_prompt.rs`：「但这是你的判断，不是义务」），LLM 默认只闲聊不派活。

### 方案（R7/R9/R10：纯 Prompt 驱动）
**严禁**新建确定性的意图分类器 / 任务规划管线 / dispatcher（违反 R7 LLM 主权 + R10 笨循环）。智慧迁移到 prompt（R9）。

1. **写侧（核心修复）** — 强化 `src/teams/broadcast/member_prompt.rs` 中 leader 段落：
   - 当用户消息构成需要团队完成的**实质任务**（而非寒暄/简单问答）时，leader **应当**先用 `team_delegate` / `task_create` 把工作拆成可追踪任务派给成员，再汇总产出回复用户。
   - 措辞从"可选、非义务"调整为"实质工作时的预期默认行为"，但**保留 LLM 判断**（不是硬性规则，不替模型做意图分类）。
   - 不改 member（非 leader）段落语义，避免普通成员争抢编排。

2. **读侧（已连通，仅需验证 + 一处小补强）**：
   - 全局 `看板`/`计划` 已通过 `team.*.task.*` 实时刷新——**无需改动**，仅在验证阶段确认。
   - per-chat 工作区 `任务` tab (`interfaces/webchat/src/components/workspace_panel.rs` `TeamTasksView`) 当前只在 `team_members` 信号变化时 refetch。补一个 `team.*.task.*` 订阅触发 refetch，使其与全局看板一致地实时刷新。

3. **成员执行流式进群聊（子决策 A = 要做）**：
   - 最终回复 + working/done 状态**已自动 fanout**（见背景第二发现），随核心修复成立，无需额外改动。
   - **可选增强（标注为 P1 内的 optional polish）**：当前 `TeamFanoutEmitter` (`src/teams/broadcast/team_fanout.rs`) 只在 `RunComplete`(最终) 和 `ToolStart`(activity) 时发布。若要更细的实时进度气泡，可让其额外发布步骤/增量事件到 `team.<id>.message`。范围可控；若实现成本或噪声偏高，实现阶段可降级为仅保留"最终回复 + 状态"。

### 验证标准
- 开群聊 → 给团队一个实质需求（如"调研 X 并产出报告"）→ `看板`/`计划`/`任务` 出现对应 `coord_task` 且随状态推进。
- 被派活成员的回复以归属气泡形式出现在群聊，名册状态在 working/done 间切换。

---

## 2. Problem 2 — 侧栏「删除」改名「解散」

### 根因
侧栏 `⋯ → 删除` (`interfaces/webchat/src/components/chat_sidebar.rs:1109` 区域，`do_delete_group`) 调 `TeamsApi::disband()` → `teams.disband` → 软删除（`status='disbanded'`，`src/teams/store.rs:393-428`）。语义是"解散"，标签错叫"删除"。

### 方案
**纯前端 label 改动**：把该按钮文案从"删除"改为"解散"，走 i18n key（与 overview 中 `common.confirm_dissolve` 语义一致）。零后端改动。确认/新增对应 i18n 条目（中英）。

### 验证标准
- 侧栏群聊行 `⋯` 菜单显示"解散"，行为不变（软删除，团队从侧栏 active 列表消失，仍存在于团队 tab 概览的 disbanded 状态）。

---

## 3. Problem 3 — 修复团队概览「删除」无效（真正的级联硬删除）

### 现状（静态分析）
- `teams.delete` 已正确注册（`src/.../handlers/agents.rs:211` 的真实 handler 覆盖 `gateway/handlers/mod.rs:665` 的 stub；同款 `teams.disband` 工作正常证明覆盖生效）。
- `handle_delete` (`src/gateway/handlers/teams.rs:147-164`) → `store.delete_team()` (`src/teams/store.rs:430-459`)：要求 `status=='disbanded'`，对 disbanded 团队执行 `DELETE FROM teams`。
- 前端 `handle_delete` (`overview.rs:112-130`) 删除成功后**确实**会 `TeamsApi::list()` 刷新列表。
- `teams.list` (`handle_list`) 不按状态过滤，概览能看到 disbanded 团队及其 Delete 按钮——链路逻辑自洽。

### 根因（症状：点了没反应、团队还在列表）
链路静态自洽 → 症状几乎肯定是**错误被吞**：前端 `handle_delete`/`handle_disband` 失败时仅 `console.error`，用户不可见。真实报错字符串需运行时捕获（候选：device-tier/`method_authz` 拦截、状态前置条件、刷新竞态）。

### 方案
1. **先让错误可见**（systematic-debugging：复现优先）：
   - `overview.rs` 的 `handle_delete` / `handle_disband` 失败分支写入 `error_msg` 信号并渲染可见提示，而非仅 console。这一步把"点了没反应"变成可诊断的具体报错。

2. **live E2E 复现** 抓真实报错，按其修复对应断点。

3. **真正的级联硬删除（子决策 B = 做）**：
   - 当前 `delete_team()` 只删 `teams` + `team_members`（FK cascade），**遗留孤儿**：`team_messages` / `message_recipients`、`coord_tasks` 及其从属、`team_events`、`task_artifacts` 及其从属、`coord_team_snapshots`（分属不同 DB，无跨库 FK）。
   - **在 `handle_delete` 编排层做级联**（该层可访问各 store，保持 `TeamStore` 高内聚 P2）：依次调用各 store 新增的按 team 清理方法：
     - `MessageStore::delete_team_messages(team_id)`（含 `message_recipients`）
     - `CoordTaskStore::delete_team_tasks(team_id)`（含 runs/comments/journals/dependencies cascade）
     - `EventLogStore::delete_team_events(team_id)`
     - `ArtifactStore::delete_team_artifacts(team_id)`（含 artifact dependencies cascade）
     - `SnapshotStore::delete_team_snapshots(team_id)`
   - 顺序（明确）：先 best-effort 清各从属 store（单条失败仅 `log` 记录、不中断，继续清后续），**最后**执行 `teams.delete_team` 作为权威收尾——这样"整体删除成功"即保证团队从列表消失；个别从属清理失败只会残留可被忽略的孤儿（已 log，后续可补清）。各 `delete_team_*` 方法须幂等。
   - **注册改动**：`agents.rs` 的 `teams.delete` 注册需注入额外 store 依赖（当前只有 `store: TeamStore`）。

### 验证标准
- 概览中对 active 团队仅显示"Disband"；解散后显示"Delete"。
- 解散 → 删除某团队：团队从概览列表消失；若后端报错则界面可见具体信息（不再"点了没反应"）。
- 删除后查询各 DB：`team_messages` / `coord_tasks` / `team_events` / `task_artifacts` / `coord_team_snapshots` 中该 `team_id` 的行已清理（无孤儿）。

---

## 4. 不做（YAGNI / 红线）

- ❌ 不建群聊意图分类器 / 任务规划管线 / dispatcher（R7/R10）。
- ❌ 不重构群聊广播范式本身。
- ❌ 不改 `src/harness/`（笨循环边界，R10）。
- ⚠️ Problem 1 的"细粒度实时进度气泡"为 optional polish，可按成本降级为"最终回复 + 状态"。

---

## 5. 受影响文件清单（预估）

**Problem 1**
- `src/teams/broadcast/member_prompt.rs` — 强化 leader 编排 prompt（核心）
- `interfaces/webchat/src/components/workspace_panel.rs` — per-chat `任务` tab 加 `team.*.task.*` 订阅刷新
- (optional) `src/teams/broadcast/team_fanout.rs` — 额外发布步骤/进度事件

**Problem 2**
- `interfaces/webchat/src/components/chat_sidebar.rs` — 按钮文案 删除→解散
- i18n 资源（中英）

**Problem 3**
- `interfaces/webchat/src/views/teams/overview.rs` — 删除/解散错误可见化
- `src/gateway/handlers/teams.rs` — `handle_delete` 级联编排 + 注入 store 依赖
- `src/teams/messages/store.rs`、`src/agents/swarm/tasks/store/...`、`src/teams/events.rs`、`src/teams/artifacts.rs`、snapshot store — 各新增 `delete_team_*` 方法
- `src/bin/aleph-server/.../handlers/agents.rs` — `teams.delete` 注册注入额外 store

---

## 6. 验证总览（E2E）

部署刷新链遵循 CLAUDE.md：`just wasm` → `cargo build --release --bin aleph-server` → 替换运行中 binary → relaunch。

1. P1：群聊给实质任务 → 看板/计划/任务有内容 + 成员气泡/状态流入群聊。
2. P2：侧栏按钮显示"解散"，软删除行为不变。
3. P3：概览 解散→删除 → 列表移除 + 错误可见 + 各 DB 无孤儿。
