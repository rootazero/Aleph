# Teams 看板全生命周期连线（外科版）— Design Spec

- **日期**: 2026-06-27
- **范围**: `interfaces/webchat/` Panel only — **零 core 改动**
- **参考项目**: hermes-desktop（Electron Kanban）+ hermes-agent `kanban_db`
- **状态**: 设计已批准，待 writing-plans
- **红线核对**: R8（配置/管理经工具与对话，本 spec 只暴露既有 RPC，不造 GUI 配置表单）✅ · R7/R10（不以确定性代码替代 LLM/leader 的恢复判断，block 分类等故意不做）✅ · R2/R4（Panel 纯 I/O，不含业务逻辑）✅

---

## 1. 问题（证据锁定）

Teams 看板存在一条**贯穿三层的"最后一公里"连线断点**。能力在 core 与 gateway 已完整存在，断在 Panel：

| 层 | 现状 | 锚点 |
|----|------|------|
| **Core 状态机** | 10 态完整实现：`Pending`/`Blocked`/`InProgress`/`WaitingReview`/`Completed`/`Failed`/`Cancelled`/`Skipped`/`Paused` + 派生 `Unsatisfiable` | `src/agents/swarm/tasks/mod.rs:83-108` |
| **Gateway RPC** | 6 个生命周期动词**全部存在**：`teams.workflow.approve_step` / `reject_step` / `retry_step` / `teams.task.pause` / `resume` / `skip` | `src/gateway/handlers/teams/workflow.rs:246-619` |
| **Panel（断点）** | ① `board.rs` 只渲染 6 列 → **WaitingReview/Paused/Skipped 任务凭空消失**（无列、不折叠）<br>② `api/teams.rs` 的 `task_pause/resume/retry/skip` 是**死代码**（0 调用者，grep 已证），`approve`/`reject` 封装**根本不存在**<br>③ `task_drawer.rs` 仅 4 个按钮（start/complete/fail/cancel），全走通用 `update_task` PATCH，6 个专用动词够不着 | `board.rs:15-30` · `api/teams.rs:388-412` · `task_drawer.rs:111,254-269` |

**两个独立后果：**
1. **可见性 bug**：任务进入 review / paused / skipped 后从看板消失，用户无法看见也无法操作。
2. **死代码 + 不可达能力**：4 个客户端封装写好却从未接线；2 个真实 RPC（approve/reject）在 Panel 无入口。

> hermes 自己的 `KANBAN_GAP_REPORT.md` 横切结论恰好印证修法方向：**"转换走后端动词，别拓宽客户端 transition map，以免与状态机漂移。"** Aleph 的 6 个专用 RPC 正是为带副作用的转换而生（pause 要 gating dispatcher、skip 要 satisfy 依赖、approve 要盖 review 戳）。

---

## 2. 目标与非目标

### 成功标准
1. **每个 stored 状态都映射到一个可见列** —— 任务永不从看板消失。
2. **6 个专用生命周期 RPC 全部可从 task drawer 触达**，按当前状态门控，**经专用后端动词路由**（非通用 PATCH）。
3. `api/teams.rs` 的 4 个死封装获得真实消费者；补齐缺失的 `task_approve` / `task_reject` 两个薄封装。
4. **熵减**：修 `board.rs` "five-column" 过期注释；不留新死代码；grep 复核所有封装均有调用者。

### 非目标（各自另立 spec，本 spec 不碰）
- `scheduled` 延迟派发状态（净新 core 字段 + RPC；且与既有 cron/loop 工具语义重叠，R8 倾向用 cron 工具）。
- `archived` / 隐藏已完成切换（看板卫生，独立小 spec）。
- 任务 `decompose` 扇出 UI。
- Bridge 协议握手生产连线（CRITICAL 但属 `desktop/shared` 子系统，前次审计 §7.1-b 有意推迟；本机难编译验证；**单独成 spec**）。
- Office 3D agent presence 可视化（需净新后端 presence topic；novelty，低优先）。
- block 原因分类 / 确定性恢复规则（**故意不做**，违 R7/R10 —— 恢复判断属 leader/LLM）。

---

## 3. 架构与数据流

```
用户/leader 点击 drawer 动作
  → TeamsApi::task_*  (Panel 客户端薄封装)
    → teams.workflow.* / teams.task.*  (既有 gateway RPC)
      → core dispatcher 施加带副作用的状态转换
        → WS 事件 team.*.task.*  (既有事件总线)
          → 看板信号重渲染  (既有刷新管线)
```

**复用既有事件刷新管线**，本 spec 只补"可达动词 + 忠实列"，不引入任何新管道、新 WS topic、新 core 逻辑。

**核心设计原则：**
- 带副作用的转换（pause/resume/skip/retry/approve/reject）**必须走专用后端动词**；**绝不拓宽通用 `update_task` PATCH**（避免与状态机漂移）。
- 无副作用的简单状态写（start→in_progress / complete / fail / cancel）保持现状走 `update_task`。
- UI 只做**最小客户端门控**：终态隐藏明显无效动作；其余交后端裁决，拒绝则 toast 呈现错误并刷新（不乐观改本地态）。

---

## 4. 组件改动（全在 `interfaces/webchat/`，零 core 改动）

### 4.1 `…/teams/components/board.rs`
- 6 → 9 列。新增 `Waiting Review`、`Paused`、`Skipped` 三列。
- 列序（按生命周期流）：`Pending · Blocked · In Progress · Waiting Review · Paused · Completed · Skipped · Failed · Cancelled`。
- `Unsatisfiable` 仍折入 `Blocked` 列（维持现状）。
- 修顶部 doc 注释 `"five-column"` → 实际列数（熵减）。
- 网格 `repeat(auto-fit, minmax(220px, 1fr))` 已响应式，9 列窄屏自动换行，无需额外布局逻辑。
- **Skipped 单独成列**（已批准）：忠实优先；rare 故常空，auto-fit 空列代价低。

### 4.2 `…/src/api/teams.rs`
- 新增 `task_approve(state, task_id) -> Result<(), String>` → `teams.workflow.approve_step`。
- 新增 `task_reject(state, task_id, reason: Option<&str>) -> Result<(), String>` → `teams.workflow.reject_step`。
- `task_pause` / `task_resume` / `task_retry` / `task_skip`（line 388-412）已存在 → 经 4.3 转为 live。

### 4.3 `…/teams/components/task_drawer.rs`
- 将固定 4 按钮行替换为**按状态门控的动作集**：

| 当前状态 | 可用动作 | 路由 |
|---------|---------|------|
| `Pending` | start · pause · skip · cancel | start/cancel→`update_task`；pause→`task_pause`；skip→`task_skip` |
| `Blocked` | pause · skip · cancel | 同上 |
| `InProgress` | complete · fail · pause · cancel | complete/fail/cancel→`update_task`；pause→`task_pause` |
| `WaitingReview` | approve · reject · skip · cancel | approve→`task_approve`；reject→`task_reject`（弹可选 reason）；skip→`task_skip`；cancel→`update_task` |
| `Paused` | resume · cancel | resume→`task_resume`；cancel→`update_task` |
| `Failed` | retry | retry→`task_retry`（re-queue 回 Pending，再从 Pending 行获得 cancel 等动作；不在 Failed 上直接 cancel —— 与既有 `terminal_locked` 语义一致） |
| `Completed` / `Skipped` / `Cancelled` | 终态：仅查看 | — |

- reject 弹一个可选 reason 输入（空 → 传 `None`）。
- 复用既有 `busy` 信号串行化所有动作（含新动作），防重复触发。

### 4.4 `…/teams/components/task_card.rs` — **无需改动**（planning 期核验修正）
- 核验发现：卡片**按 priority 着色**（`priority_class`），**不按 status 着色**；status 由所在**列标题**传达。故新列直接复用既有卡片渲染即可，task_card.rs **零改动**（YAGNI，避免引入卡片不存在的 status-hue 行为）。
- `task_drawer.rs::run_status_class` 是给 **Runs 审计子视图**的 run 记录着色（running/completed/failed/timeout），与 task 状态词表不同，亦无需改动。

### 4.5 `locales/en.json` + `locales/zh.json`（仅此 2 个 locale 文件）
- 新增 `teams.kanban.columns.{waiting_review, paused, skipped}`。
- 新增 `teams.kanban.actions.{pause, resume, skip, retry, approve, reject}`。
- 新增 reject-reason 输入提示标签。

---

## 5. 错误处理与边界

- **Busy 锁**：复用既有 `busy` 信号，扩展覆盖 6 个新动作，防双触发。
- **后端拒绝无效转换**（竞态：任务状态在我们手下已变）→ toast 呈现 RPC 错误串 + 刷新 drawer；**不乐观改本地态**。
- **reject reason 可选** → 空则传 `None`；实现时核验 `reject_step` 签名是否接受 optional reason（若必填则 UI 改为必填校验）。
- **WaitingReview 无 reviewer 配置** → approve/reject 仍可用，人工 reviewer = panel 用户。
- **终态任务** → 不渲染任何动作按钮（只读），避免对已关闭任务发无效 RPC。

---

## 6. 验证（极度节制 cargo）

- **编译门**：改完**仅跑一次** webchat crate 的 `cargo check`（WASM/Leptos 目标）。**不碰 core**（无核心改动 → 无 core 测试风险）。`t_string!` 缺键即编译失败 → i18n 完整性部分由编译器强制。
- **死代码核查**：grep 确认 `task_pause` / `task_resume` / `task_retry` / `task_skip` 改后均有调用者（从 0 → ≥1）。
- **手动 / E2E 测试计划**（文档化，待有 server 时执行）：
  1. 建一个 team + 若干带依赖的 task。
  2. 驱动任务分别进入 `waiting_review` / `paused` / `skipped`（经新 drawer 动作或工具）。
  3. 验证：(a) 落入正确的新列；(b) 该状态下门控动作子集正确；(c) `approve` 推进 review 任务并按 core 语义解阻下游、`reject` 后可 retry；(d) `pause` 后 dispatcher 不再 claim、`resume` 回 Pending；(e) `skip` 满足下游依赖。

---

## 7. 分支与归档

- **所有代码改动在新建 worktree 分支**（如 `feat/teams-kanban-lifecycle-wiring`），**严禁碰 main**（任务协议 + 项目 EnterWorktree 约定：会话内只合并不删除）。
- 本 spec 落 `docs/superpowers/specs/`，受 `.gitignore` 管控（默认本地不提交，符合项目约定）。

---

## 8. 影响面小结

- **改动文件**：**4 个**，全在 `interfaces/webchat/`（board.rs / api/teams.rs / task_drawer.rs / en.json + zh.json）。task_card.rs 经核验**无需改动**（见 §4.4）。
- **新增 LOC**：约 +120 ~ +160（drawer 动作分支最重），其中含替换既有 4 按钮逻辑。
- **删除**：0（死封装经接线转 live，而非删除 —— 其包裹的 RPC 是真实且必要的）。
- **core 改动**：0。
- **新依赖**：0。
- **红线风险**：0。
