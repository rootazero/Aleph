# Team Chat 交互入口设计 (Team Chat Entry)

**日期**: 2026-06-15
**状态**: 待实现
**类型**: 新功能（前端 + 后端连线）

## 背景与问题

承接 `panel-sidebar-paradigm-revert`（已完成回退）的遗留目标：在 chat 窗口加"团队/项目快速入口"。本轮**只做团队**，达标后再深耕、再加 project 入口。

**核心要求（用户明确）**：

1. 这是一个 **chat 入口**，不是导航。点击直接启动团队 chat —— 拼队 → 提需求 → 团队协作完成任务，用户在**一个窗口里看到不同 agent 群聊**。
2. **千万不能做成快捷导航**（点一下跳到 Teams tab 那种偷懒做法）。
3. 必须有**真实任务解决价值**，符合当前 agent teams 架构解决用户真实问题的能力，而非"多 agent 凑一起群聊只有观赏价值"。
4. 与 **Teams tab 彻底区分**：Teams tab 是团队功能的**数据控制台**；Team Chat 入口是用户↔Aleph teams 的**交互入口（chat 入口）**。两者面向同一批持久化 Team —— chat 是交互面，tab 是控制台面。

## 架构定位（红线对齐）

一句话：**把已有的 leader-DAG 团队后台，包成一个三栏群聊窗口**。复用既有基建，补"已造未连"的缺口，不重造。

- **R4（Interface 纯 I/O）**：Panel 三栏 UI 只渲染 RPC 响应与 topic 事件，不做任何编排 / 持久化 / 任务规划逻辑。
- **R7 / R9 / R10（LLM 主权 / 智慧在 prompt / 笨循环）**：团队编排由 **leader agent 一次 LLM 推理 + 现有团队工具**自然完成。`src/teams/dispatcher/`（笨循环）与 `src/harness/` 不新增任何推理。leader 的"智慧"在注入的编排 prompt 里。
- **R2（UI 唯一源）**：全部 UI 在 Leptos Panel；原生 bridge 不参与。
- **R8（一切是工具）**：拼队、建任务、委派、成员间通信全部走已有团队工具（`src/builtin_tools/team/`）。

## 决策日志（brainstorming 已敲定）

| # | 决策 | 选定 |
|---|------|------|
| 1 | 协作模型 | **Leader 编排**（贴合 Aleph 现有 leader-DAG），非 @mention 自由群聊 |
| 2 | 组队方式 | 现场拼队 → **落成持久化 Team**（复用 `TeamStore`，同时出现在 Teams tab 作为其控制台） |
| 3 | Leader 归属 | **当前活跃 agent（通常 main agent）默认为 leader/东道主**，创建时写入 `team.leader_id` 并固定；可改派（非 MVP 重点） |
| 4 | 窗口布局 | **三栏 A**：左名册 · 中群聊流（逐条归属）· 右可折叠工作区 |
| 5 | 右工作区 | **两个 tab**：交付物（artifact）+ 任务看板（CoordTask DAG/进度） |
| 6 | MVP 动态能力 | **标准版** = 基线 + 中途插话/转向 |

**Leader 规则细化（决策 3）**：拼队从"选 agent 并指定 leader"简化为"当前 agent 当东道主，往里加专家成员"。理由：① 连续性 —— 用户平时就在和 main agent 对话，开团队 chat 时它顺势成为 leader，且 leader 负责汇总回复用户，对话连续性完美；② 去掉一个选择步骤；③ 贴合 R5/R9（main agent 是前台代表，团队是后台；任何 agent 当 leader 都靠注入编排 prompt）。leader_id **固定于创建时**（持久化 Team 的 leader 必须稳定，dispatcher 才有确定编排者；重开该团队 chat 仍用存下的 leader）。

## 现有基建盘点（复用，不重造）

**后端**（探索确认已存在）：
- `src/teams/types.rs` — `Team`（含 `leader_id`）、`TeamMember`、`TeamStatus`。
- `src/teams/store.rs` — `TeamStore` trait（create/get/list/add_member/remove_member/disband/delete），SQLite 持久化。
- `src/teams/dispatcher/` — `TeamDispatcher`（笨循环），`CoordTask` DAG，`runner.rs::execute_member_task()`（拉起 owner agent 跑 harness）。
- `src/teams/messages/` — `MessageRouter` + `TeamMessage`（异步收件箱、to/cc、线程）。**成员间通信已有，群聊流直接渲染即可，不新建消息层。**
- `src/teams/events.rs` — `TeamEventType` 枚举（审计日志框架）。
- `src/event/types.rs` — `AlephEvent::Team*` 变体。
- `src/gateway/handlers/teams.rs` + `src/gateway/router.rs` — 团队 RPC（teams.list/get/disband/delete/list_tasks/create_task/update_task）。
- `src/gateway/event_bus.rs` — `TopicEvent` + `topic_matches()`（glob 订阅）。
- `src/builtin_tools/team/` — 团队工具集（task_submit/message_send/inbox_read/session_*/member_add…）。
- **事件 fan-out 先例**：goal-loop 可观测性引入的 `OriginFanoutEmitter` / `GatewayEventEmitter`（包裹 run 事件发射器，把事件再广播到额外目标）——团队事件归属层照此先例做。

**前端**（探索确认已存在）：
- `interfaces/webchat/src/views/chat/{view,messages,state,events,composer/mod,workspace_panel,session_tabs}.rs` — 单 agent chat 全链路。
- `interfaces/webchat/src/components/{chat_sidebar,nav_menu,mode_sidebar}.rs` — 回退后的弹窗范式侧栏。
- `interfaces/webchat/src/views/teams/` — Teams tab 数据控制台（Overview/Kanban/Plan/Replay/Workers）。
- `interfaces/webchat/src/api/{chat,teams}.rs` — RPC 封装。

## 缺口（要补的"已造未连"）

1. **团队会话启动**：无"用户用自然语言把需求交给团队 → leader 编排"的 gateway 入口。
2. **逐条归属 + 流式事件**：主事件流没有 per-agent 消息归属，团队执行无流式进度事件（任务跑完才原子记录）。
3. **统一线程检索**：无"按时间合并 消息+artifact+任务状态"的检索 RPC，重开 chat 无法 hydrate。
4. **交付物投递**：artifact 是拉取模型（`task_read_artifact`），未推给 panel 工作区。

## 设计

### 一、后端（工程量主体 —— 连线四块）

#### B1. 团队会话启动 RPC

新增 `teams.chat.send(team_id, text, [attachments]) -> { run_id }`：
- 把用户需求 + **leader 编排 prompt** + team 上下文（成员名册、各成员能力简述）投给 `team.leader_id` 指向的 **leader agent**，复用 `src/harness/` 跑一轮 run。
- leader 在这一轮里用现有团队工具（建 `CoordTask`、`message_send`、委派）驱动成员；dispatcher 笨循环照常拉起被委派的成员 run。
- **中途插话** = 用同一 `team_id` 再次调用 `teams.chat.send`；leader 接力（把新消息作为续来的用户输入注入团队上下文）。
- 路由注册在 `src/gateway/router.rs`，handler 在 `src/gateway/handlers/teams.rs`。
- **R10 守卫**：此 handler 只负责"拉起 leader run + 注入上下文"，不做任何意图分析 / 任务拆解 / 完成度判断 —— 那些全在 leader 的 LLM 推理里。

#### B2. 团队事件归属层（逐条归属 + 流式）

当一个 run 属于某 team（run 上下文带 `team_id`）时，把它的关键事件（message / tool_call / artifact / 状态变化）**再广播**到团队 topic，带 `agent_id`：
- topic 命名：`team.<team_id>.message`、`team.<team_id>.activity`、`team.<team_id>.task`。
- 实现照 `OriginFanoutEmitter` 先例：包裹 run 的事件发射器，run 属于 team 时把事件 fan-out 到团队 topic（事件 payload 加 `agent_id` 归属字段）。**不改 `src/harness/`**——归属/fan-out 发生在 gateway/event-bus 边界。
- leader run 与被委派的成员 run 都经此层 → panel 看到每个 agent 逐条贡献 + 实时状态。

#### B3. 统一线程检索 RPC

新增 `teams.chat.thread(team_id, [limit]) -> { items: [ThreadItem] }`：
- `ThreadItem` = 按时间排序的合并流，每项是 `{ kind: message|artifact|task_status, agent_id, content/ref, timestamp }`。
- 数据源：`MessageRouter` 的团队消息 + `CoordTask` 状态变迁 + artifact 元数据。
- 供 panel 进入/重开团队 chat 时 hydrate 三栏。

#### B4. 交付物暴露

- 成员 `task_submit` 产出的 artifact，元数据（title/type/excerpt/artifact_id）随 B2 的 `team.<id>.task` 事件推给 panel；
- 全文按需经现有 artifact 读取路径（`task_read_artifact` 对应的 RPC）拉取，点开"交付物"项时调用。

### 二、前端（Leptos Panel，纯渲染）

#### F1. 入口与拼队（在 Chat 模式侧栏内）

- 在 `chat_sidebar.rs` 内（与 agent 切换器 + 会话列表同栏）加"团队群聊"入口。**不进 nav 弹窗**——它是 chat 入口，尊重刚回退的弹窗范式。
- 点击 → 拼队步骤：**Leader/东道主 = 当前活跃 agent（预填"你"，取自 `ChatState.agent_id`）** → 加成员（从现有 agent 列表多选）→「开始」。
- 「开始」→ 调团队创建 RPC（前端已有 `TeamsApi::create()`，**计划阶段需核实它接受 `leader_id` + 成员列表；若不接受则薄薄扩展，封装 `TeamStore::create_team`+`add_member`，不加业务逻辑**），写 `leader_id` = 当前 agent + 成员 → 返回 `team_id` → 打开三栏窗口、订阅 `team.<team_id>.*`。
- 重开已有团队 chat：出现在会话列表（或侧栏"团队"小分区）→ 调 `teams.chat.thread` hydrate。

#### F2. 三栏群聊视图（`ChatView` 团队变体）

- **左名册**：leader 徽标 + 成员，实时状态点（idle/working/done），由 `team.<id>.activity` 事件驱动。
- **中群聊流**：复用 `MessageList`/`MessageBubble`；`ChatMessage` 加 `agent_id` → 每条气泡按该 agent 的颜色/名字/头像归属；leader 计划、成员发言、工具调用、最终汇总按时间内联。
- **右可折叠工作区**：复用 `workspace_panel.rs`，加两个 tab —— **交付物**（artifact 列表，点开看全文）+ **任务**（实时 CoordTask DAG/进度，数据来自 `teams.get`/`teams.list_tasks` + `team.<id>.task` 事件增量）。
- **底部 composer**：复用 `composer/mod.rs`；团队模式下发送走 `teams.chat.send(team_id, …)`；运行中再发 = 中途插话/转向。

#### F3. 对现有 chat 代码的外科改动

- `ChatMessage += agent_id: Option<String>`（None = 单 agent 旧路径，零回归）。
- `ChatState += team_id: Option<String>`（标记团队模式，决定 composer 走哪个 send + 是否渲染三栏）。
- `events.rs` 事件投影新增处理 `team.<id>.{message,activity,task}` → push 归属气泡 / 更新名册状态 / 更新工作区两 tab。
- `MessageBubble` 渲染：有 `agent_id` 时加归属外观（颜色/名字）。
- per-agent 颜色分配：按成员在名册中的序号取稳定调色板（纯前端）。

### 三、数据流

1. **拼队**：当前 agent 当 leader + 加成员 →「开始」→ `teams.create` 写 `leader_id`+成员 → 返回 `team_id` → panel 打开三栏、订阅 `team.<id>.*`。
2. **提需求**：`teams.chat.send(team_id, text)` → 后端用 leader 编排 prompt + team 上下文把 leader agent 拉起跑一轮 harness。
3. **编排**：leader 发计划（message）→ 建 `CoordTask` → 委派 → dispatcher 拉起成员 run；每条 成员 run/message/tool/artifact 经 B2 归属层广播为 `team.<id>.{message,activity,task}`，带 `agent_id`。
4. **实时渲染**：panel 投影事件 → 归属气泡 / 名册状态 / 工作区两 tab。
5. **收口**：leader 汇总 → message 事件 → 渲染成 leader 最终气泡；最终交付物进交付物 tab。
6. **中途插话**：用户运行中再发 → 同 `team_id` `teams.chat.send` → leader 接力转向。
7. **重开 hydrate**：`teams.chat.thread(team_id)` → 时间合并 消息+artifact+任务 → 重建三栏。

## MVP 边界 / Defer

**本轮含**：拼队（现有 agent，当前 agent 默认 leader）/ leader 编排群聊 / 三栏视图 / 实时状态 / 交付物+任务 tab / 中途插话转向。

**Defer 到深耕**：
- 拼队时现场新建 agent（需 agent 创建流）。
- 聊天中途增删成员（需中途上下文注入）。
- @点名某成员直接对话（需 mention 路由）。
- project 入口（独立一轮）。

## 测试

- **后端单测/集成**：`teams.chat.send` 拉起 leader run 并带 team 上下文；B2 归属层对 team run 的事件正确 fan-out 到 `team.<id>.*` 且带 `agent_id`；`teams.chat.thread` 按时间正确合并 消息+artifact+任务。
- **前端**：`ChatMessage` 有/无 `agent_id` 的归属渲染；事件投影更新名册/流/工作区两 tab；拼队流程正确调 `teams.create` 并以当前 agent 为 leader。
- **E2E（人工，按现有惯例）**：拼一支真实团队 → 给真实任务 → 看逐条归属的群聊 + 交付物 + 任务看板 → 中途插话使 leader 转向 → 重开该团队 chat 能 hydrate。

## 成功标准 / 验证

- [ ] Chat 侧栏出现"团队群聊"入口；点击进入拼队，leader 预填为当前活跃 agent，可加现有 agent 为成员。
- [ ] 「开始」创建持久化 Team（`teams.create`，leader_id=当前 agent），同一团队同时出现在 Teams tab。
- [ ] 提交需求后，leader agent 被拉起编排；三栏窗口实时显示：左名册状态、中群聊流逐条归属（leader+各成员）、右工作区交付物与任务两 tab 实时更新。
- [ ] 运行中可中途插话，leader 接力转向。
- [ ] 重开该团队 chat 经 `teams.chat.thread` 正确 hydrate 历史。
- [ ] `cargo check -p alephcore` 与 `just wasm` 双侧编译通过。
- [ ] `src/harness/` 与 `src/teams/dispatcher/` 未新增推理逻辑（R10 守卫）；Panel 无业务逻辑（R4）。
- [ ] 部署链：`just wasm` → 重编 `aleph-server` → 替换运行中 binary → Reload Panel，肉眼验三栏群聊端到端。

## 风险

- **中**。后端事件归属/fan-out 层是本设计最关键也最易出错处——需确保 team run 事件正确带 `agent_id` 且不污染单 agent 路径；照 `OriginFanoutEmitter` 先例可控。
- **R10 红线**：`teams.chat.send` handler 必须只做"拉起 leader run + 注入上下文"，任何任务拆解/路由/完成度判断都在 leader 的 LLM 推理里——实现时严防把编排逻辑写进 gateway/dispatcher。
- **leader run 与 dispatcher 的衔接**：leader 在一轮 run 内建任务+委派，dispatcher 异步拉起成员 run——需确认两者生命周期不打架（leader run 结束 ≠ 团队任务结束；团队仍在跑成员 run）。这是实现期要重点验证的衔接点。
- 单 agent chat 零回归：`agent_id`/`team_id` 均为 `Option`，旧路径不受影响。
