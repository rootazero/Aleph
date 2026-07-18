# 群聊体验深耕 — 设计文档 (Group Chat UX Deepening)

> 状态: 已通过头脑风暴确认 (2026-06-16)。下一步交由 writing-plans 拆解为实施计划。
> 实施节奏: **一份 spec，两阶段实施**(阶段一纯前端先出活，阶段二带后端)。

## 目标 (Goal)

让 Aleph 群聊从"能用"走向"好用、可回溯":

1. **人物更明显** — 群聊气泡用头像 + display name 呈现发言者，身份信息移到气泡外。
2. **历史可接续** — 群聊像单聊一样进入侧栏历史列表，用户随时回来续聊。
3. **@ 可发现** — 输入 `@` 从花名册选人，杜绝手敲 id 打错被静默丢弃。

## 背景与现状 (Ground Truth)

探查已确认以下事实(file:line 为探查时位置，实施时以实际为准):

- **气泡归属已存在但简陋**: `views/chat/messages.rs:477-492` 当 `ChatMessage.agent_id`(`state.rs:214`)有值时，在气泡上方渲染一个彩色 display name 标签；颜色来自 `team_events.rs:13-16` 的 `agent_color(index)`——**按花名册顺序取色，成员变动即串色**；**无头像**。
- **后端已持久化一切**: 群聊消息存于 `team_messages` 表(`teams/messages/store.rs`)，`list_team_messages`(store.rs:119-130)**刻意不按 expires_at 过滤**，3650 天 TTL，可长期回放;团队注册表 `teams`/`team_members` 持久(`teams/store.rs`);成员查询 `get_agent_teams(agent_id)`(store.rs:104-105)可回答"agent X 在哪些群"。
- **缺口窄**: ① 没有面向面板、回放群聊**消息**的 RPC(`teams.chat.thread`@handlers/teams.rs:833-891 只回任务/交付物);② 侧栏(`chat_sidebar.rs`)只列单聊、按选中 agent 过滤(:684-720);③ team 模式未进 `SessionSnapshot`(state.rs:789-846)，切 tab 即丢。
- **@ 只认 id**: `teams/messages/mentions.rs` 解析 `@<id>`(`[A-Za-z0-9_-]+`，大小写敏感)，`@all`/`@everyone`→`MENTION_ALL`;`builtin_tools/team/message_send.rs:106-161` 与 `broadcast/targets.rs:19-57` 拿 token 直接比对 roster 的 **agent_id**，**不在花名册就静默 continue 丢弃**。`AgentDefinition.name`(`config/types/agents_def.rs:198-200`)**无唯一约束**，可重名;`id`(:192)是唯一主键。前端 `composer/mod.rs:100-143` 是纯 textarea，**无任何 @ 自动补全**。
- **身份元数据齐备**: `agents.list` 返回 `AgentSummary{id, name, emoji, ...}`(`api/agents.rs:9-17`)。`emoji` 已在 admin 侧栏渲染(`agents_sidebar.rs:177-193`);`avatar` 图片字段定义了但**零消费**(死字段，本轮保留不用)。

## 确认的 UX 决定 (Confirmed via Visual Brainstorm)

| 决定 | 选择 |
|------|------|
| 群聊气泡布局 | **A. 经典 IM**: 头像在气泡外侧、display name 在气泡上方、连续同 agent 合并、气泡只含文本 |
| 头像来源 | agent `emoji` → 回退名字首字 monogram；圆形底色按 **agent_id 哈希**取稳定色 |
| @ 提及 | 加 @ 自动补全(头像+名字+灰 id)，插入唯一 `@<id>`；气泡内 `@<id>` 保持原文、仅轻高亮；**后端零改** |
| 侧栏布局 | **C. 群聊可折叠 + 单一滚动**: 顶部"群聊"可折叠小节(无独立滚动条) + 下接"单聊"，共用一根滚动条 |
| 新建按钮 | 文字按钮 → **"+"方形图标按钮**(文字转 title/aria-label) |

---

## 共享地基 (Shared Foundation)

**新前端 helper**: `agent_identity(id) -> AgentIdentityView { name, emoji, color }`

- 位置建议: `interfaces/webchat/src/views/chat/agent_identity.rs`(新文件)。
- 数据源: 已有的 `agents.list` 结果缓存为 `id -> AgentSummary` 映射(面板已在多处调用 `AgentsApi::list`)。
- `name`: `summary.name` → 缺失回退 `id`。
- `emoji`: `summary.emoji` → 缺失回退 `name`/`id` 首字 monogram。
- `color`: **`agent_id` 哈希** % 6 取调色板色(沿用 `team_events.rs:13-16` 的 6 色板)，**替换** `agent_color(index)` 的按序取色。提供 `agent_color_for_id(&str) -> &'static str`。
- 消费方: 气泡头像、侧栏头像簇、@ 调色板——三处统一从此 helper 取身份，收敛"name 不 id"的兜底。

> 红线: 纯前端呈现层，不含业务判断(合 R4)。

---

## 阶段一 — 气泡身份 + @ 补全 + 新建按钮 (纯前端，零后端)

### 1.1 群聊气泡 Layout A

- 改 `views/chat/messages.rs:477-492` 的群聊归属渲染:
  - **incoming**(`agent_id = Some`): 一行 = [头像圆 28px(emoji/monogram + `agent_color_for_id` 底色)] + [列: 上方彩色 display name(`agent_identity().name`) + 下方气泡(纯文本)]。
  - **连续合并**: 若本条 `agent_id` == 上一条渲染消息的 `agent_id`，隐藏头像与名字(留 30px 占位缩进)，仅渲染气泡。
  - **own**(`agent_id = None`): 维持现状右对齐、无头像无名。
- 单聊路径(`agent_id` 恒 `None`)完全不变，**零回归**。

### 1.2 @ 自动补全调色板

- 仿现有 Slash 调色板(`composer/mod.rs:17` import 的 `SlashPaletteView`)新增 **@ 调色板**组件:
  - 触发: textarea 输入 `@` 且处于 team 模式(`chat.team_id` 有值)。
  - 数据: 当前群 roster(`chat.team_members`) ∪ "@所有人"特殊项。每项渲染 `agent_identity` 的头像 + 名字 + 灰色 id。
  - 过滤: 按 `@` 后已输入文本实时过滤(匹配 name **或** id，不分大小写)。
  - 选中: 插入规范 `@<id> `(尾随空格);"@所有人" → 插入 `@all `。
- `mentions.rs` / `message_send.rs` / `targets.rs` **一行不动**(已认 `@<id>` 与 `@all`)。
- 气泡内 `@<id>` token 渲染为轻高亮(背景/前景微调)，**不替换成名字**(保持 id 无重名歧义)。

### 1.3 新建按钮图标化

- `chat_sidebar.rs:646-652` 文字按钮 → 方形图标按钮:
  - 内容 `{t_string!(i18n, chat.new)}` → "+" 图标(SVG plus 或 `＋` 字形)。
  - class `px-3 py-1.5` → 方形(如 `w-9 h-9 flex items-center justify-center`)，保留 `bg-primary` 系。
  - i18n 文本转为 `title` + `aria-label`(无障碍不丢)。

---

## 阶段二 — 侧栏群聊历史 (前端 Layout C + 两处轻量后端)

### 2.1 后端① 新 RPC `teams.chat.history`

- 新 handler(`gateway/handlers/teams.rs`): 入参 `{ team_id }`，读 `MessageStore::list_team_messages(team_id)`，映射为气泡 DTO `Vec<{ from_agent, content, msg_type, created_at }>`，按 `created_at` 正序。
- team 不存在 → 空列表(非错误)。
- 现有 `teams.chat.thread`(任务/交付物)**不动**;二者职责区分: thread = 工作区交付物，history = 聊天气泡。
- 前端 `api/team_chat.rs` 加 `TeamChatApi::history(team_id)` 包装。

### 2.2 后端② `agents.teams` 摘要增补

- `agents.teams(agent_id)` 返回的每个 team 摘要(`api/teams.rs:8-32` 对应 DTO)增补:
  - `members_preview: Vec<{ id, name, emoji }>` — 封顶 4 个，供头像簇渲染。
  - `last_message: Option<String>` — 最近一条消息一行预览，供群聊行副标题。
- 在 handler 侧一次性补齐(避免前端逐群 `teams.get` 的 N+1)。

### 2.3 前端侧栏 Layout C

- 改 `chat_sidebar.rs` 会话列表区(:682-953)为**单一滚动列表**:
  - 顶部"群聊"**可折叠**小节: 数据来自 `agents.teams(选中 agent)`;每行 = [成员头像簇(`members_preview` 经 `agent_identity` 上色叠放)] + [标题 + `last_message` 预览]。**agent 无群聊时整节隐藏**。
  - 下接"单聊"小节: 复用现有 `SessionEntry` 过滤渲染(:684-720)。
  - 折叠状态本地记忆(信号/localStorage 皆可，MVP 用内存信号)。

### 2.4 进群 = 接续历史

- 群聊行 `on:click`:
  1. `teams.get(team_id)` 取花名册 → 设 `chat.team_id` + `chat.team_members`(`state.rs:342-348`)。
  2. `teams.chat.history(team_id)` 取气泡 → 填充 `chat.messages`(每条带 `agent_id`)。
  3. 进入 team 模式，订阅 `team.*`(复用 `team_events.rs:20-68` 的 `subscribe_team_events`)。
- **侧栏行即持久化入口**: team 模式无需进 `SessionSnapshot`;回来就点列表。

---

## 数据流 (Data Flow)

```
进群:  侧栏群聊行 click
        → teams.get(team_id)            ── roster → chat.team_members
        → teams.chat.history(team_id)   ── 气泡 → chat.messages (each agent_id)
        → subscribe team.<id>.*         ── 实时 .message 气泡 / .activity 花名册状态
发言:  composer (@ 调色板插 @<id>)
        → teams.chat.send {team_id, message}
        → GroupChatBroadcaster 扇出 → 成员 run → team.<id>.message 事件 → 气泡
身份:  agents.list ── 缓存 id→{name,emoji} ── agent_identity(id) ──┬─ 气泡头像
                                              (color=hash(id))      ├─ 侧栏头像簇
                                                                    └─ @ 调色板
```

## 错误处理 (Error Handling)

- `teams.chat.history`: team 不存在 → 空;store 错 → 面板提示"历史加载失败"，不崩。
- `agents.list` 不可达 → @ 调色板为空、回退手敲;头像缺 emoji → monogram，缺 name → id。
- 进群 hydration 任一步失败 → toast 提示，停留当前视图。
- 群聊行 `members_preview`/`last_message` 缺失 → 头像簇用 id 兜底、副标题留空。

## 测试策略 (Testing)

- **后端单测**: `teams.chat.history`(正序、TTL 无关、msg_type 映射);`agents.teams` 摘要含 `members_preview`(封顶 4)+ `last_message`。`mentions.rs` 已有测试，不动。
- **前端 host 侧单测**(沿用仓库已有 panel 逻辑单测模式): `agent_identity` 哈希色确定性 + 三级兜底;气泡连续合并(相邻同 agent 折叠);@ 调色板过滤(name/id、大小写无关)。
- **人工 E2E**: 侧栏点群 → 历史回放 → 发言带头像归属;`@` 自动补全插入 id → 目标成员响应;新建按钮图标点击新建会话。

## MVP 边界 (YAGNI — 本轮不做)

- team 模式写入 `SessionSnapshot`(切 tab 自动回群)——靠点侧栏回来，符合"随时回列表接续"。
- `@id → @名字` 气泡美化(已选保持 id)。
- 真实图片头像上传(`avatar` 死字段保留，emoji/monogram 已够)。

## 红线自检 (Redline Compliance)

- **R4(Interface 纯 I/O)**: "agent 参与哪些群"由 core 的 `agents.teams` 计算，面板只调用 + 渲染，不自算成员归属;历史回放、扇出皆 core 侧。
- **R10(薄 Harness)**: 全部改动在 `gateway/handlers` + `teams` + 面板，**不触碰 `src/harness/`**。
- 气泡/侧栏/颜色/折叠均为纯呈现层。

## 受影响文件清单 (Touch List)

**阶段一(前端)**
- 新增 `views/chat/agent_identity.rs`(共享 helper)
- 改 `views/chat/messages.rs`(气泡 Layout A)
- 改 `views/chat/team_events.rs`(`agent_color` → `agent_color_for_id`)
- 新增 @ 调色板组件 + 改 `views/chat/composer/mod.rs`(触发/插入)
- 改 `components/chat_sidebar.rs`(新建按钮图标化)

**阶段二(前端 + 后端)**
- 后端 改 `gateway/handlers/teams.rs`(新 `teams.chat.history` + `agents.teams` 摘要增补)
- 后端 视需要 `teams/messages/store.rs`(若需 last_message 取数辅助)
- 前端 改 `api/team_chat.rs`(history 包装)、`api/teams.rs`(DTO 增补)
- 前端 改 `components/chat_sidebar.rs`(Layout C: 折叠群聊节 + 进群 hydration)
