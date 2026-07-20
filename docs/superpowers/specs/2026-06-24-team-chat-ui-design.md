# 团队聊天界面优化：顶部成员状态条 + 底部任务条

- **日期**: 2026-06-24
- **范围**: Panel (Leptos/WASM, `interfaces/webchat/`) 前端 UI 重排
- **后端改动**: 无（复用已有数据、事件、DTO）
- **触发**: 团队模式（`chat.team_id.is_some()`）；单 agent 聊天不受影响

> 本 spec 的代码引用已对照真实代码库核验（2026-06-24，多 agent 对抗式核验）。「零后端改动 / 纯 I/O Panel」论点成立。

---

## 1. 背景与目标

团队（multi-agent）聊天目前已具备：

- `components/team_participants.rs` — 左上角**折叠**的重叠头像簇 + 点开弹出 roster，每个成员带状态点（工作中=琥珀 / 完成=青 / 错误=红 / 空闲=灰）与 leader 标记（当前是英文字面 `leader` 文本，team_participants.rs:143）。团队模式 gating 在调用方 `view.rs:190`，非组件内部。
- `views/chat/messages.rs` — Telegram 式消息归属（仅在 agent 切换时显示头像+名字）。
- `views/chat/agent_identity.rs` — 基于 FNV-1a 哈希的稳定 agent 配色 + emoji/monogram 头像解析。

目标是把团队执行任务的状态信息**直接呈现在聊天窗口**：

1. **顶部**：把折叠头像簇升级为「常驻横向成员状态条」——一眼看到全员状态（队长 / 工作中 / 空闲…）。
2. **底部**：在输入框上方新增「任务条」——展示当前最需关注的团队任务及其状态（如 `任务 · 鉴权边界情况 · 待审阅`）。

两个硬约束（来自代码）：

- macOS 顶部有 30px 拖拽带 `.aleph-main-drag-band`（`position:absolute; top:0; height:var(--aleph-band-h)`，web=0 / macOS=30px；仅 `html[data-platform=macos]` 下 `pointer-events:auto` + `-webkit-app-region:drag`）。顶部状态条必须像现有 `team_participants` 一样标记 `aleph-no-drag` 并叠于其上。另有 `.aleph-sidebar-toggle`（基线 `z-index:60`、`left:10px`，macOS 覆盖为 `left:72px` 清开红绿灯）与 `session_tabs` 浮带需避让。
- 底部 composer 是 `absolute inset-x-0 bottom-0` 浮层，并通过 `ResizeObserver` 把 stack 高度写入 `--composer-clearance`（CSS 变量，作用于 `<html>`）驱动消息列表底部 padding。任务条须与输入框共存。

---

## 2. 决策记录（已与用户确认）

| 议题 | 决策 |
|------|------|
| 「审阅中」状态语义 | **复用现有四态**（Idle/Working/Done/Error），不动后端 `MemberStatus` 枚举。「审阅中」当示意，不单独建态。（后端确无 Reviewing 态。） |
| 顶部条响应式行为 | **方案 2 + 折叠后仍可逐人看状态**：宽窗常驻文字胶囊；窄窗/成员多自动坍缩为头像簇，点击展开 popover 逐人列状态。 |
| 底部任务条内容/交互 | **单个「最需关注」任务 + 计数**，点击打开任务面板（实现为聊天内滑出抽屉，见 §3.2）。 |
| 顶部布局与拖拽带共存 | **方案 A**：顶部浮层，no-drag 胶囊岛，复用现有 `team_participants` 的 `z-[60]` + `aleph-no-drag` 浮层模式；胶囊间隙保留可拖窗。 |

---

## 3. 组件设计

### 3.1 `TeamRosterBar`（顶部成员状态条）

演进自 `components/team_participants.rs`（而非新建，避免双源）。

**渲染数据**：`chat.team_members: Vec<TeamMemberView>`（已存在，`state.rs`），字段含 `agent_id / name / emoji / role / is_leader / status`。`MemberStatus = { Idle, Working, Done, Error }`（`interfaces/webchat/src/views/chat/state.rs:240-247`）。**成员状态已由 `team_events.rs:59-70` 的 `.activity` 分支实时驱动，本次无需改动该路径。**

**两种视觉态（响应式）**：

- **展开态（宽窗，成员数 ≤ 阈值）**：横向常驻胶囊条，每个胶囊 =
  - 头像圆盘（`agent_identity` 配色 + emoji 或 monogram）
  - 名字
  - 状态点（点色按四态）+ 状态文字（见 3.3）
  - leader 额外一个**本地化「队长」chip**（替换现有英文 `leader` 文本；与活动状态并存：chip 标角色、点/文字标状态）
- **坍缩态（窄窗 或 成员数 > 阈值）**：现有重叠头像簇（最多 N 个 + 「+N」溢出盘）；点击展开 popover，**逐人列出名字 + 状态点 + 状态文字 + leader 标记**（沿用现有弹出层结构）。

**坍缩判定（纯函数，可单测）**：坍缩当 `member_count > 4` **OR** `estimated_total_capsule_width > container_width`。单胶囊宽估 ~140px（头像 ~24px + 名称 + 状态点/文字），`container_width` 取消息区容器宽。两条件为 OR，阈值为前端常量。**加 hysteresis**（展开/坍缩用不同阈值，留缓冲）防边界抖动。

**布局（方案 A）**：

- `position:absolute; top:0;` 浮层，`z-[60]`，标 `aleph-no-drag` + `data-tauri-drag-region="false"`（复用 `team_participants` 现有定位）。
- macOS 下起始 `left` 偏移须清开红绿灯（~0–70px）与 `.aleph-sidebar-toggle`（macOS `left:72px`, `z-60`）。沿用/微调现有 cluster 的左偏移逻辑。
- 胶囊本身 no-drag；胶囊之间与右侧透明间隙保留 `-webkit-app-region:drag`，仍可拖窗。
- 新增 CSS 变量 `--aleph-team-roster-h`：**roster 条净高（位于 macOS 30px 拖拽 band 之下，不含 band）**，由 `ResizeObserver` 观测 roster 元素实时写入（镜像 `--composer-clearance` 机制），自动跟随展开/坍缩高度变化。团队模式下消息列表 `padding-top = band 高 + --aleph-team-roster-h`，防遮挡。

### 3.2 `TeamTaskStrip`（底部任务条）+ `TeamTaskDrawer`（任务抽屉）

新建 `components/team_task_strip.rs`。

**位置**：挂入 composer 浮层 stack，与 `AttachmentPreviewBar` / `QueuedPromptBar`（实际顺序见 `composer/mod.rs:684,686`）同级（`stack_ref` 容器），落在输入框正上方。因 stack 高度经 `ResizeObserver` 写入 `--composer-clearance`，任务条自动被纳入清空高度，不与输入框/消息列表打架。

**内容**：单条「最需关注」任务：

```
● {状态点} 任务 · {task.subject} · {状态文字}        [+N]
```

- **「最需关注」选择**（纯函数，可单测）：优先级 `WaitingReview > InProgress > 其它非终态 > 终态`；同级按 `completed_at → started_at → created_at`（取最近，**无 `updated_at` 字段**）取最近，仍相等按 `task id` 排序保证确定性。
- `+N` = 团队下其余任务计数（总数 − 1）；**`N == 0` 时隐藏该徽标**。
- 无任务 → 整条隐藏（不占空间）。

**交互**：点击 → 打开 **`TeamTaskDrawer`**（聊天内滑出抽屉，列出本团队全部任务）。
- 不采用 URL deep-link：经核验**不存在 `team_id` 参数化路由**（Teams tab 用内部 `TeamsTabState` context 信号，默认 `TeamsSubTab::Overview`，`views/teams/mod.rs:43-57`），无法用普通链接携带 `team_id`。
- 抽屉**直接复用已 fetch 的 `team_tasks` 数据**（见下），几乎零新数据接线；自带列表布局与空/错误态。R4-clean，不引入 chat→teams 跨视图耦合。

**数据**：新增 `chat` 信号 `team_tasks: Vec<CoordTaskDto>`（**复用已有 `CoordTaskDto`**，`interfaces/webchat/src/api/teams.rs:225-229`，不新建结构）。
- 初始：`teams.list_tasks`（RPC 已存在，`src/gateway/handlers/teams/tasks.rs:30 handle_list_tasks`，返回含 `subject/status/owner/created_at/...`）。
- 增量：`team.<id>.task.<verb>` 事件（verb ∈ created/updated/completed/failed/cancelled），由 **`src/agents/swarm/tasks/store/mod.rs::emit_task_topic`**（store/mod.rs:82）发布。payload 仅含 `{task_id, team_id, status, owner, priority, timestamp}`，**不含 subject**。
  - `views/chat/team_events.rs` 新增分支匹配 **`topic.contains(".task.")`**（对齐 `views/teams/kanban.rs:63`，**不要用 `ends_with(".task")`**——真实 topic 是 4 段，永不命中）。
  - 收事件后按 `task_id` **upsert** 状态/owner 进缓存；若 `task_id` 不在缓存（如新建任务，缺 subject），触发一次 **debounced `teams.list_tasks` 重拉**补全。对终态/未知/已删除 id 优雅忽略，**不依赖事件顺序**。
- 纯渲染数据，前端不做任务规划/状态推导持久化（守 R4）。

### 3.3 状态 → 中文文案 / 点色映射（纯前端函数）

成员四态（`MemberStatus`，`interfaces/webchat/src/views/chat/state.rs:240-247`）：

| `MemberStatus` | 文案 | 点色（沿用现有） |
|----------------|------|------------------|
| `Working` | 工作中 | 琥珀 `#e0a458` |
| `Idle` | 空闲 | 灰 `#6b7280` |
| `Done` | 完成 | 青 `#4ec9b0` |
| `Error` | 错误 | 红 `#d16969` |

任务状态（`CoordTaskStatus`，`src/agents/swarm/tasks/mod.rs:82-108`，wire 为 snake_case 字符串；全 10 态）：

| `CoordTaskStatus` (wire) | 文案 |
|---|---|
| `waiting_review` | 待审阅 |
| `in_progress` | 进行中 |
| `pending` | 待处理 |
| `blocked` | 阻塞 |
| `completed` | 已完成 |
| `failed` | 失败 |
| `cancelled` | 已取消 |
| `skipped` | 已跳过 |
| `paused` | 已暂停 |
| `unsatisfiable` | 不可满足 |

`CoordTaskDto.status` 携带**原始 snake_case 枚举串**，由前端纯函数映射为文案/点色（守 R4：解析+渲染，不做语义判断）。未知串 → 直译枚举名 + 灰点，不 panic。

leader 角色标记：roster 胶囊上以本地化「队长」chip 呈现（角色，与活动状态并存）。

---

## 4. 与现有顶部浮层的协调

`session_tabs`（`components/session_tabs.rs`）仅在**≥2 个 open agent 会话**时渲染。

- **硬约束**：任何情况下**不得移除切回其它会话的入口**——因此**不直接抑制** `session_tabs`（早前「团队=单一会话故隐藏 tabs」的论断未经证实，会造成导航回归）。
- **默认行为**：仅团队会话打开时，`session_tabs` 本就不渲染（<2 会话），与 roster 无冲突；若用户另有其它会话标签，roster 条与 tabs 带**垂直堆叠**（roster 让位于 tabs 之下或并存），二者皆可见。
- **精确共存布局**在 plan 阶段对照 session 模型确认（团队会话在 `SessionMap` 中如何计数、能否与其它会话并存）。
- `BootCheckGate`（`fixed inset-0 z-[9000]` 连接遮罩）层级最高、连接期本就盖全屏，不受影响。

---

## 5. 数据流

```
初始:  teams.list_tasks (RPC) ──▶ chat.team_tasks (Vec<CoordTaskDto>) ──▶ TeamTaskStrip / Drawer 渲染
       teams.get / team_members ──▶ chat.team_members ──▶ TeamRosterBar 渲染
实时:  team.<id>.activity        (成员状态, 已接线无改动) ──▶ team_events.rs:59-70 ──▶ chat.team_members
       team.<id>.task.<verb>     (任务, 新增分支)        ──▶ team_events.rs(.contains ".task.") ──▶ upsert / 重拉 team_tasks
```

订阅：`ChatView` 挂载时**无条件订阅全局 `team.*`**（`view.rs:66`），`team_events.rs` 按 `topic.starts_with("team.")` 过滤；新增 task 分支**无需新增订阅**。

---

## 6. 错误处理与降级（P7）

- `team_members` 为空 → 不渲染状态条。
- 成员无 `emoji` → 名字首字母 monogram（现有 `agent_identity` fallback）。
- `teams.list_tasks` 失败 / `team_tasks` 为空 → 任务条隐藏，**不阻断聊天**。
- 未知 `MemberStatus` / `CoordTaskStatus` 串 → 落「空闲/灰点」或直译 + 灰点，**不 panic**。
- **响应式竞态**：(a) 坍缩判定加 hysteresis 防阈值边界抖动；(b) `ResizeObserver` 观测→写 `--aleph-team-roster-h`→reflow 周期内 top-padding 短暂不准，用 debounce 或自校正（同 `--composer-clearance` 历史竞态类）；(c) 任务事件按 `task_id` upsert，对终态/未知/已删除 id 优雅忽略，不依赖事件到达顺序。
- 远程/纯壳 Panel：状态条/任务条与平台无关（仅「与拖拽带共存」是 macOS 专属逻辑，由 `data-platform=macos` 守卫）。

---

## 7. 测试

- **纯函数单测**：
  - 「最需关注」任务选择（优先级排序 + `completed_at→started_at→created_at` 取最近 + id 决胜）。
  - 状态 → 中文文案 / 点色映射（成员四态 + 任务 10 态 + 未知串 fallback）。
  - 坍缩阈值判定（`member_count>4` OR `估算宽>容器宽`，含 hysteresis）。
  - `+N` 计数（含 `N==0` 隐藏）。
- **组件渲染**：成员多/寡、有/无任务、leader 标记、坍缩态↔展开态切换、空团队、任务抽屉开关。
- **构建验证**：改动集中于 `interfaces/webchat/`；遵用户工作风格不跑全量 cargo，至多一次 `just wasm` / wasm 构建验证 CSS 与组件编译通过。
  - ⚠️ 前瞻提醒（非当前违规）：`@theme` 块内若写含 glob 的注释，`*/` 会提前终止 CSS 注释致 tailwind `CssSyntaxError`（曾发生过）——新增样式注释避免此模式。

---

## 8. 预估文件影响

**改**：
- `components/team_participants.rs` — 升级为响应式 `TeamRosterBar`（展开胶囊条 ↔ 坍缩头像簇）；leader 文本本地化为「队长」chip。
- `views/chat/state.rs` — 新增 `team_tasks: Vec<CoordTaskDto>` 信号（复用现有 DTO，**不新建结构**）。
- `views/chat/team_events.rs` — 新增 `topic.contains(".task.")` 分支，upsert `team_tasks` / 触发 debounced 重拉。（成员状态 `.activity` 分支 team_events.rs:59-70 **已存在，不改**。）
- `views/chat/view.rs` — 挂载/定位 `TeamRosterBar`；与 `session_tabs` 顶部共存（垂直堆叠，不抑制）。
- `views/chat/composer/mod.rs` — 在 stack 中挂载 `TeamTaskStrip`。
- `styles/tailwind.css` — 状态条/任务条/抽屉样式 + `--aleph-team-roster-h` 变量；消息列表团队模式 top padding。

**新**：
- `components/team_task_strip.rs` — 底部任务条 + `TeamTaskDrawer` 任务抽屉。
- 状态/文案映射与「最需关注」选择的纯函数模块（便于单测）。

---

## 9. 非目标（YAGNI）

- 不新增/不改后端枚举、RPC、事件、DTO。
- 不实现「审阅中」独立状态。
- 不重做消息归属 / 消息气泡布局（已满足截图）。
- 不在前端做任务规划、依赖求解或状态持久化（守 R4/R7）。
- 不实现 chat→teams 跨视图 deep-link（无 team_id 路由）；任务详情用聊天内抽屉。
- 不触碰 material / `.dark` 镜像主题以外的非相关 CSS。
