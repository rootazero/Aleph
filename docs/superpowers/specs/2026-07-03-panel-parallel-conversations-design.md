# Panel 多会话并行 + 进行中红点 — 设计规格

> Date: 2026-07-03
> Scope: `interfaces/webchat`（Leptos/WASM Panel）纯前端状态架构重构。不碰后端、不碰 `src/harness/`（R10 零增长）、UI 逻辑留在 Panel（R2）。
> Status: Approved design → 待 writing-plans。

## 1. 目标 (Goal)

让 Panel 支持**多个 chat 会话同时进行**，手感等价于 codex CLI 开多个窗口跑平行任务：

- 多个会话可同时有 in-flight run。
- 侧栏会话列表 + 顶部标签条上，用**动态隐现的红点 🔴** 标记"正在进行"的会话。
- **点击对话列表**、**切换 tab**、**新建对话** 三种操作都**不终止**任何正在进行的会话。
- 只有 `run_complete` / `run_error` 才视为该会话"回复完成"（红点熄灭）。
- 显式终止（Esc / 停止按钮 → `ChatApi::abort(run_id)`）行为不变。

### 成功判据 (Success Criteria)

1. 会话 A 有 run 在跑时切到会话 B，A 的 `response_chunk` **不丢失**：切回 A 时 transcript 完整、且仍在流式推进。
2. 点击侧栏 session / 切 tab / 点"新建对话"后，`ChatApi::abort` **不被调用**，已有 run 继续跑到完成。
3. 会话在跑时，其侧栏行**和**顶部标签**同时**显示红点；`run_complete`/`run_error` 后两处红点同时消失。
4. 同一 agent 下可开多个并行会话，各自独立跑、独立红点。
5. 现有单会话行为、团队/群聊、切换/新建/删除会话流程无回归。

## 2. 现状与缺口（已核实）

| 层 | 现状 | 缺口 |
|---|---|---|
| 后端 | 按 `run_id` 并发跑多 run、广播 `run.*` 事件 | ✅ 无需改 |
| 活跃态 | 单 singleton `ChatState`（`app.rs:83`）；非活跃标签 = **冻结** `SessionSnapshot`（`state/sessions.rs`） | ❌ 切走即冻结；后台 run 的 `response_chunk` 在当前 singleton 的 `messages` 里找不到目标消息 → 被丢弃 |
| 事件路由 | `subscribe_run_events`（`view.rs:36`、`platform/phone/chat/mod.rs:34`）在 ChatView 挂载点绑死 singleton `chat`，所有 `run.*` 全灌进它 | ❌ 无 `run_id → 会话` 路由 |
| 侧栏红点数据 | `chat_sidebar.rs:296-297` 已有 `running`（per-session_key 引用计数）+ `run_to_session`（run_id→session_key）；订阅 `run_accepted` +1、`run_complete`/`run_error` -1（L433-477） | ⚠️ **数据层建好但从未在 session row 渲染红点**（`running` 仅在 L418 当重水合守卫用）→ 纯"功能连线"缺口；且为组件局部态，随侧栏卸载丢失 |
| 标签条红点 | `SessionTabs`（`components/session_tabs.rs`）按 `agent_id`，无进行中指示 | ❌ 完全没有 |
| 会话粒度 | tab == `agent_id`（一个 agent 一个标签）；`on_new_chat`（`chat_sidebar.rs:626`）清空当前标签复用同一 agent tab | 需升级为 session 粒度、按 agent 归类；"新建对话"须开新标签而非顶掉当前 |

关键洞察：这是**纯 Panel 端状态架构问题**。后端已并发跑 run 并广播事件；只需让每个已打开会话的活跃态**持续接收事件**（不再冻结），并按会话路由。

## 3. 架构决策 (已拍板)

- **并行单位 = session**，`agent_id` 保留作分组/归类键（利于记忆管理）。同一 agent 可开多个并行会话。
- **方案 1 · 活跃会话注册表**：`SessionMap` 从"存冻结快照"升级为"存活跃 `ChatState`"；一个全局 dispatcher 按 `run_id → ConvId` 路由。
- **红点打两处**：侧栏会话列表 + 顶部标签条，读**同一**数据源。
- **一步到位做完整 session 粒度**。
- **团队/群聊 v1 排除**在并行注册表之外，维持现状。

## 4. 数据模型

`state/sessions.rs` 主重构：

```text
ConvId            = 客户端稳定 id（新建会话即生成；session_key 于首个 chat.send 响应后回填）
ConvMeta          = { agent_id, session_key: Option<String>, label, agent_color }
LiveConversation  = 常驻 app-root Owner 下的一个 ChatState（后台会话也持续接收事件、复用全部 ChatState 变更方法）

SessionMap {
    live:    HashMap<ConvId, ChatState>,        // 每个已打开会话一个活跃态（后台会话用；见 §6 投影机制）
    meta:    RwSignal<HashMap<ConvId, ConvMeta>>,
    order:   RwSignal<Vec<ConvId>>,             // 标签条顺序
    active:  RwSignal<Option<ConvId>>,
    route:   HashMap<String /*run_id*/, ConvId>,        // + session_key → ConvId 反查
    running: RwSignal<HashMap<ConvId, usize>>,          // 引用计数；红点 = >0（统一数据源）
}
```

- `ConvId` 用客户端稳定 id（如递增本地序号或 uuid），因为**新建会话在首个 send 前尚无 `session_key`**。`route` 同时维护 `run_id → ConvId` 与 `session_key → ConvId`，`session_key` 于 `run_accepted` 回填 `meta`。
- `ConvId` 用 newtype 包裹，避免与 `agent_id` / `session_key` 裸 `String` 混用。

## 5. 事件路由（全局 dispatcher 上提到 app root）

将 `subscribe_run_events` 从 `ChatView` / phone 挂载点**上提到 `app.rs` 根**，改为按会话路由（替换现有"绑死 singleton"）：

- `run_accepted`（带 `run_id` + `session_key`）→ 取当前 `active` ConvId：`route[run_id] = ConvId`；回填 `meta[ConvId].session_key`；`running[ConvId] += 1`。
- 其余带 `run_id` 的事件（`response_chunk` / `agent_trace` / `tool_*` / `reasoning` …）→ `route[run_id]` 定位 ConvId → **活跃会话写 singleton `ChatState`，后台会话写 `live[ConvId]`**；两条路径复用**同一批** `ChatState` 方法（`append_chunk` / `begin_step` / `update_tool` …）。
- `run_complete` / `run_error` → `running[ConvId] -= 1`（归 0 则移除）；清 `route[run_id]`。

→ 后台会话 token **无损累积**，因其 `ChatState` 常驻且事件持续到达。

> 迁移注意：`view.rs` / `platform/phone/chat/mod.rs` 移除各自的 `subscribe_run_events` 挂载点绑定，改由根 dispatcher 统一分发。团队事件 `subscribe_team_events` 维持在原处（v1 不并行化）。

## 6. 活跃投影与切换（调用点零改动）

保留"固定 singleton context `ChatState`"作为**活跃会话的投影**，沿用现有 `capture_snapshot` / `restore_from` 切换机制——唯一变化是快照来源/去向变成**活跃的** `live[ConvId]`：

- `activate(new)`：`singleton → live[old]`（capture/restore 落盘出参），`live[new] → singleton`（恢复入参）。因 `live[new]` 一直在后台接收事件，恢复即**到位、无闪烁**。
- ~30 个 `expect_context::<ChatState>()` 调用点**全部不动**（context 仍是"活跃投影"）。

> 备选（不采用）：改成响应式 `SessionMap::active_chat()` 访问器，语义更纯但要动全部调用点、风险更高。实现细节在 writing-plans 阶段最终敲定，但默认走"固定 singleton + 切换时 copy"以最小化 churn 与回归面。

## 7. 红点连线（两处，单一数据源）

- **数据源统一**：删除 `chat_sidebar.rs` 局部的 `running` / `run_to_session`（组件局部、随卸载丢失），改读 app-root 常驻的 `SessionMap.running`。侧栏原有 `run_accepted/run_complete/run_error` 订阅逻辑上移进根 dispatcher（§5）。
- **侧栏列表**：session row（`chat_sidebar.rs` L1322 起的正常行渲染）按其 `SessionEntry.key`（`session_key`）解析出 ConvId（经 `meta` 的 `session_key → ConvId` 反查；未开成标签的 backend session 天然无 in-flight run，红点隐），再读 `SessionMap.running` → 渲染红点（`animate-pulse` 小圆点，进行中显、完成隐）。**补上从未连接的渲染**。
  - 为省去反查，`running` 可等价地直接以 `session_key` 为键（现有代码即如此，且每个进行中的 run 在 `run_accepted` 即带 `session_key`）；ConvId 与 session_key 二选一作 `running` 主键的最终取舍在 writing-plans 敲定，红点语义不变。
- **标签条**：`SessionTabs` 每个 tab pill 加同款红点。
- 红点 = `running[ConvId] > 0`；引用计数天然支持同一会话并发多 run。

## 8. 会话粒度升级（tab == session，按 agent 归类）

- `SessionTabs` / `SessionMap` 键从 `agent_id` 换 `ConvId`；tab 文案用 session topic（`meta.label`），颜色/分组用 `agent_color`。
- 现有 `session_map.activate(chat, &agent_id)` 调用点（`chat_sidebar.rs` L333 自动选默认 agent、L530 选 session、L958 agent 下拉）迁移到 ConvId 语义。
- 侧栏点 session → `activate(ConvId)`（打开/聚焦标签）。
- **"新建对话"语义变更**（`on_new_chat` L626）：不再清空当前标签复用同一 agent tab，而是在选中 agent 下**新建一个 ConvId 并 activate**（开新标签），从而不顶掉当前正在跑的会话。
- **Cmd+1..9 / Cmd+W**（`session_tabs.rs::install_tab_hotkeys`）按 `order: Vec<ConvId>` 迁移。

## 9. 生命周期不变量

1. 点击对话列表 / 切 tab / 新建对话 **都不 abort** 任何 run —— 每会话独立常驻 `ChatState` + 后台事件路由；切换只换投影。
2. 只有 `run_complete` / `run_error` 令 `running[ConvId]` 归 0，才算完成（红点灭）。
3. 显式终止仍为 Esc / 停止按钮 → `ChatApi::abort(run_id)`，语义不变。

## 10. 边界与不做 (YAGNI)

- **团队/群聊**（`subscribe_team_events`、`chat.team_id`）v1 维持现状，不纳入并行注册表；`ConvMeta` 不为其扩展变体。
- **后台活跃态上限**：默认不设硬上限；若担心 WASM 内存，可对超过 N 个的后台会话 LRU 退化为"下次聚焦从后端 `hydrate_session_history` 重水合"。**v1 不做，列 backlog。**
- 不引入新依赖；不碰后端 RPC；不碰 `src/harness/`。

## 11. 改动清单（预估）

| 文件 | 改动 |
|---|---|
| `state/sessions.rs` | 主重构：`ConvId`/`ConvMeta` 模型、`live` 注册表、`route`、`running` 统一态、`activate`/`close`/`switch_by_index` 迁移 ConvId |
| `platform/wide/views/chat/events.rs` | `subscribe_run_events` 改为按 `run_id → ConvId` 路由；活跃/后台双路径复用 ChatState 方法 |
| `app.rs` | dispatcher 上提到根；注册表在根初始化 |
| `components/session_tabs.rs` | 键换 ConvId；tab 红点；Cmd 快捷键迁移 |
| `components/chat_sidebar.rs` | 删局部 running/run_to_session；session row 红点渲染连线；`activate` 迁 ConvId；`on_new_chat` 改开新标签 |
| `platform/wide/views/chat/view.rs`、`platform/phone/chat/mod.rs` | 移除挂载点 `subscribe_run_events` 绑定 |

## 12. 测试计划

- **注册表路由**：模拟 A 活跃、B 后台，注入 B 的 `run_accepted` + `response_chunk` → 断言 `live[B]` 累积、singleton(A) 不受污染；切到 B 后 transcript 完整。
- **不 abort 不变量**：activate 切换 / 新建 / 选 session 时断言未触发 abort 路径。
- **running 引用计数**：`run_accepted` +1、`run_complete`/`run_error` -1、归 0 移除；同会话并发两 run 的加减正确。
- **红点可见性**：`running[ConvId] > 0` ⇒ 侧栏行与标签 pill 均渲染红点；归 0 后均消失。
- **会话粒度**：同一 agent 下开两会话 → 两个独立 ConvId/标签/红点。
- 现有 `state/sessions.rs`、`state.rs` 的 snapshot/step 测试保持通过（回归）。

## 13. 风险

- **切换机制的 copy 顺序**：单线程 WASM 无数据竞争，但 `activate` 中 `singleton→live[old]` 与 `live[new]→singleton` 的先后须保证不自我覆盖。
- **route 表清理**：`run_complete`/`run_error` 必须清 `route[run_id]`，否则 run_id 复用时误路由（后端 run_id 唯一性可缓解，仍应显式清）。
- **session_key 回填时机**：`run_accepted` 前后到达的事件都要能落到正确 ConvId —— 依赖 `run_id` 主键 + `route` 在 `run_accepted` 即建立。
- **调用点churn 误判**：若最终选"固定 singleton + copy"路线，须确认无调用点缓存了旧 singleton 实例引用导致切换后读到陈旧信号（现有代码本就用 context，风险低）。
