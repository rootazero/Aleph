# 团队群聊 LLM 命名 + 重命名/删除对齐单聊 — 设计 Spec

- **日期**: 2026-06-16
- **状态**: 已批准设计，待实现计划
- **作者**: Claude (brainstorming)

## 1. 背景与问题 (Background)

团队群聊（`teams.chat`）的群聊名称是**固定模板**：用户在创建弹窗（`team_compose.rs`）留空名称时，前端用 `format!("{leader}{team_default_suffix}")`（`team_default_suffix = "的群聊"`）拼出 `main的群聊` 这类固定串，写入 `teams.name`。

单聊则不同：在**首条用户消息**时由 LLM 自动生成一个简短主题作为标题（`src/gateway/execution_engine/execute.rs:515`），并可在侧栏**重命名 / 删除**。

用户诉求：团队群聊应**和单聊一致**——
1. 名称由 LLM 生成简短主题（不再是 `main的群聊`）；
2. 允许在群聊列表中**重命名**群聊；
3. 允许在群聊列表中**删除**群聊；
4. 样式与操作逻辑均与单聊保持一致。

## 2. 目标 / 非目标 (Goals / Non-Goals)

### Goals
- 团队群聊创建时**保留**名称输入框：填了就用用户的名字；**留空**则首条消息后 LLM 自动生成主题。
- 彻底移除固定串 `{leader}的群聊`；留空时的临时占位名改为中性的 **"新群聊"**，并在首条消息后被 LLM 主题替换。
- 侧栏群聊行支持**重命名**与**删除**，三态交互（normal / inline-edit / delete-confirm）与单聊会话行**像素级一致**。
- 删除采用**软删除**（`teams.disband`）+ 侧栏只显示 `active` 团队。

### Non-Goals
- 不把团队群聊并入 session 模型（保持 teams 独立的表 / 编排 / RPC）。
- 不改单聊的命名 / 重命名 / 删除行为（只**抽取**其 LLM 主题逻辑供复用）。
- 不做"硬删除/物理清除团队"（任务/消息/快照历史保留，可恢复）。
- 不改团队管理页（`views/teams/*`）对 disbanded 团队的展示策略——本 spec 只改 **chat 侧栏**的群聊区。

## 3. 现状勘察 (Current Architecture — verified)

| 关注点 | 位置 | 现状 |
|---|---|---|
| 单聊自动主题 | `src/gateway/execution_engine/execute.rs:515-595` | 首条消息时 `tokio::spawn` 调 `provider_registry.get("haiku")`（回退 default），prompt `"Generate a concise topic title (5-10 characters, same language…)"`，system `"You are a title generator. Output ONLY the title…"`，temperature 0.3；失败/空回退为截断 20 字 + `…`；`sm.set_topic()` 持久化；emit `stream.session_updated {session_key, topic}` 让侧栏刷新 |
| 团队创建 RPC | `src/gateway/handlers/teams.rs` `handle_create` (~196) | `CreateTeamParams{name, leader_id, description, members…}`；**拒绝空 name**；**不做 dup-name 校验**；`store.create_team(NewTeam{name, description, leader_id})` + `add_member` |
| 团队发消息 | `src/gateway/handlers/teams.rs:3031` `handle_chat_send` | 校验 → 确认 team 存在 → `msg_store.send_message_with_ttl(NewMessage{from_agent: RESERVED_USER_HANDLE …}, 3650d)` 持久化用户消息 → `tokio::spawn(GroupChatBroadcaster::dispatch_user)` |
| TeamStore | `src/teams/store.rs` | trait + `SqliteTeamStore`。已有 `create_team` / `disband_team`（active→disbanded，emit `TeamDisbanded`）/ `delete_team`（**要求先 disbanded**才能物理删）/ `set_protocol`（`UPDATE teams SET protocol`）/ `get_agent_teams`（**返回所有 status**）。迁移用 `add_column_if_missing`（store.rs:178）。**无 rename 方法** |
| 前端创建弹窗 | `interfaces/webchat/src/views/chat/team_compose.rs` | `resolve_team_compose` 返回 `Ok(Some(name))`/`Ok(None)`/`Err`；L95 `Ok(None) => format!("{leader}{suffix}")`；调 `TeamsApi::create(&dash, &name, "", &leader, &members)` |
| 前端 TeamSummary | `interfaces/webchat/src/api/teams.rs` | 群聊条目带 `id` / `name:String` / **`status:String`** / `members_preview` / `last_message` |
| 前端侧栏 | `interfaces/webchat/src/components/chat_sidebar.rs` | `reload_data`(L243) 同时拉 `sessions.list` + `agents.teams`(→`groups` signal)。群聊行 (~830-880)：纯 `<button>` + 头像簇 + name + last_msg，**无操作菜单**。会话行 (~937+)：三态（editing/deleting/menu）；`do_rename`→`sessions.set_topic`；`do_delete`→`sessions.delete`。状态信号 `editing_key`/`deleting_key`/`menu_open_key`/`edit_text`/`is_saving`/`edit_input_ref`；`groups_expanded` 控制群聊区折叠。订阅 `run.session_updated`→`reload_data` |
| i18n | `interfaces/webchat/locales/zh.json:176` | `"team_default_suffix": "的群聊"` |

**关键洞察**：单聊在"首条消息后"命名（彼时才有内容）；团队在"创建时"已命名（彼时尚无内容）。因此"和单聊一致"= 把团队命名也推迟到**首条 `teams.chat.send`**。

## 4. 设计 (Design)

### 总体方针
团队保持独立实体；新增**与单聊并行**的 auto-name / rename / delete。唯一真实逻辑——LLM 主题生成——**抽取为共享 helper**，单聊与团队共用同一 prompt + 回退（避免 prompt 漂移；未来改 prompt 两端同时受益）。

> 备选已否决：(a) 在团队 handler 内**复制** LLM 调用 → prompt 双份漂移；(b) 把团队并入 session 模型 → 大重构、越界、零收益。

### A. 自动命名 (Auto-naming，服务端，镜像 `execute.rs:515`)

1. **抽取共享 helper**：把单聊内联的主题逻辑提为
   ```
   generate_conversation_topic(provider, message: &str) -> String
   ```
   （含 haiku 调用 + "5–10 字、同语言" prompt + 截断回退）。单聊路径改为调用它（DRY，行为不变）。建议落位 `src/gateway/execution_engine/topic.rs`（或等价小模块），由 planning 定稿。

2. **`teams` 新增列 `name_auto`**：`INTEGER NOT NULL DEFAULT 0`，经 `add_column_if_missing` 迁移。`NewTeam` 增 `name_auto: bool` 字段，`create_team` 写入。

3. **`teams.create` RPC**：`CreateTeamParams` 增可选 `auto_name: bool`（默认 false），透传到 `NewTeam.name_auto`。

4. **前端 compose 弹窗**（保留名称输入框）：
   - 填了名字 → `TeamsApi::create(name, auto_name=false)`。
   - 留空 → `TeamsApi::create(name="新群聊"(localized), auto_name=true)`。**删除** `format!("{leader}{suffix}")` 与对 `team_default_suffix` 的引用。

5. **`handle_chat_send` 触发**：进入后先 `store.take_auto_name_flag(team_id)`（原子 check-and-clear，见 §B 的 store 方法）：
   - 返回 `true` → 本次即"首条有意义消息"：`tokio::spawn` 调 `generate_conversation_topic(params.message)` → `store.rename_team(team_id, topic)` → emit `teams.changed {team_id, name}`（见 §D 刷新）。
   - 返回 `false` → 跳过（已命名或用户显式命名）。
   - **flag 本身即"首条消息"闸门**——无需统计 transcript 条数；check-and-clear 保证不重复触发（即便消息并发）。

### B. 重命名 (Rename，新 RPC，镜像 `do_rename`)

1. **TeamStore 新增**：
   - `rename_team(id, name) -> Result<()>`：`UPDATE teams SET name=?1 WHERE id=?2`；0 行→`NotFound`。供 auto-name 与手动 rename **共用**。
   - `take_auto_name_flag(id) -> Result<bool>`：`UPDATE teams SET name_auto=0 WHERE id=?1 AND name_auto=1`；返回 `affected > 0`（原子取并清）。
2. **新增 `teams.rename` RPC**（thin I/O）：params `{team_id, name}`；校验 name 非空 trim；调 `rename_team`。在 `register_teams_handlers`（`handlers/agents.rs`）注册。
3. **前端**：`do_rename_group(team_id, name)` → `teams.rename` → 成功后 `reload(dash)`。交互复用会话行 inline-edit（Enter 保存 / Esc 取消 / blur 保存）。

### C. 删除 (Delete，复用 `teams.disband` + 过滤)

1. **前端**：`do_delete_group(team_id)` → 已有 `teams.disband` → 成功后 `reload(dash)`；若被删的是当前打开的群聊，清空团队视图（镜像 `do_delete` 清 active session）。
2. **侧栏过滤**：群聊区列表 `.filter(|g| g.status == "active")`（`TeamSummary.status` 已存在，无需后端改动）。disbanded 团队即从列表消失。
3. **无需新 RPC / 无硬删除**：任务/消息/快照历史保留。

### D. 侧栏行 UX 对齐 (`chat_sidebar.rs`)

1. **三态状态机**：群聊行复刻会话行三态——
   - **normal**：头像簇 + name + last_msg + hover `⋯` 菜单（Rename / Delete）。
   - **edit**：inline `<input>` → `do_rename_group`。
   - **delete-confirm**：红色横幅 Confirm/Cancel → `do_delete_group`。
   - 复用相同 Tailwind class 与交互（5s 自动撤销删除确认、自动聚焦编辑框等）。
2. **独立信号**：新增 `group_editing_id` / `group_deleting_id` / `group_menu_id` / `group_edit_text`，**不复用**会话行的 `editing_key` 等——保持会话行状态机零改动（外科手术，避免 key 命名冲突）。
3. **临时占位名**：留空创建的团队显示 "新群聊"，直到首条消息的 LLM 主题落地后被替换（与单聊 "新对话"→主题 一致）。
4. **异步刷新**：auto-name 在首条消息 RPC 返回**之后**才完成，需服务端推送让已打开的侧栏刷新 → 新增 `teams.changed` 事件，侧栏订阅后调 `reload_data`。
   - 手动 rename / delete 是用户动作，前端 RPC 返回后**直接** `reload`（同单聊），**不依赖** `teams.changed`。

### E. 测试 (Testing)

- **Host 可测单元**：
  - `generate_conversation_topic`：LLM 空/失败 → 截断回退；正常 → 原样 trim。
  - `rename_team`：成功 + `NotFound`。
  - `take_auto_name_flag`：首次 `true`、二次 `false`（幂等闸门）。
  - `resolve_team_compose`：已覆盖 blank/explicit（沿用）。
- **手动 / 集成 E2E**：
  - 留空建群 → 发首条消息 → 群聊名变为 LLM 主题（且 `teams.changed` 驱动侧栏刷新）。
  - 显式命名建群 → 发消息 → 名称不被覆盖。
  - 侧栏重命名 → 持久化 + 列表更新。
  - 侧栏删除 → 该群从列表消失（仍可在团队管理页见 disbanded）。

## 5. 改动文件清单 (File-by-file)

**后端 (Rust core)**
- `src/teams/store.rs`：`teams` 加列 `name_auto`（`add_column_if_missing`）；`NewTeam` 加 `name_auto`；`create_team` 写入；新增 `rename_team` / `take_auto_name_flag`（trait + Sqlite impl + 测试）。
- `src/gateway/handlers/teams.rs`：`CreateTeamParams` 加 `auto_name`；`handle_chat_send` 接 take-flag→生成→rename→emit `teams.changed`；新增 `handle_rename`。
- `src/gateway/execution_engine/topic.rs`（新）：`generate_conversation_topic` 共享 helper。
- `src/gateway/execution_engine/execute.rs`：单聊改调共享 helper（行为不变）。
- `src/bin/aleph-server/commands/start/builder/handlers/agents.rs`：注册 `teams.rename`。
- `teams.changed` 事件类型：复用现有 gateway 事件总线发布机制（planning 确认 `GatewayContext` 的 emitter 入口）。

**前端 (Leptos / WASM)**
- `interfaces/webchat/src/views/chat/team_compose.rs`：留空时 `name="新群聊"` + `auto_name=true`；删除 `{leader}{suffix}` 逻辑。
- `interfaces/webchat/src/api/teams.rs`：`create` 透传 `auto_name`；新增 `rename(team_id, name)`；（`disband` 若无则补）。
- `interfaces/webchat/src/components/chat_sidebar.rs`：群聊区过滤 `status=="active"`；群聊行三态化 + `⋯` 菜单 + 独立信号 + `do_rename_group`/`do_delete_group`；订阅 `teams.changed`→`reload_data`。
- `interfaces/webchat/locales/zh.json` / `en.json`：新增 `chat.new_group_chat`（"新群聊" / "New group chat"）；`team_default_suffix` 变为未引用（标注，删除与否由 planning 定）。

## 6. 红线 / 设计原则契合 (Redline fit)

- **R4（Interface 纯 I/O）**：`teams.rename` / `handle_chat_send` 仍是薄 handler；主题生成在 core helper，不是 interface 逻辑。
- **R7 / R9（LLM 主权 / 智慧在 Prompt）**：命名是 LLM 调用，非确定性规则。
- **R10（薄 Harness）**：不触碰 `src/harness/`。
- **外科手术原则**：群聊行用独立信号，会话行状态机零改动；删除复用既有 RPC；`status` 字段已存在。

## 7. 风险与开放细节 (Risks / open details for planning)

- **`teams.changed` 事件落位**：需在 planning 确认 `handle_chat_send` 的 `GatewayContext` 暴露的事件发布入口（broadcaster 已对 chat 气泡发流事件，emitter 可达）。若不可达，回退为复用 `run.session_updated` 帧触发侧栏 `reload`。
- **WASM 嵌入链**：改完 panel 需 `just wasm` → 重编 `aleph-server` binary → 替换运行中的 daemon（见 CLAUDE.md "Panel ↔ Daemon 资源嵌入链"），否则看不到效果。
- **dup-name**：`teams.create` 不做 dup 校验，多个 "新群聊" 临时同名可接受（首条消息后即被 LLM 主题区分）。
- **遗留团队**：建群后从不发消息者，名称保持 "新群聊"（可接受，等价单聊建后未发的占位）。
