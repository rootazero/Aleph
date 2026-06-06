# Panel-Chat 左侧会话栏 — 连线 + 修复设计

**日期**: 2026-06-07
**范围档**: 连线 + 修复（零新增后端，纯 Panel 层）
**分支**: main（直接手术式修改，遵循项目 CLAUDE.md 单分支规范；不使用 worktree）
**参考项目**: hermes-desktop / hermes-agent / hermes-web-ui / OpenSquilla / DeepSeek-Reasonix

---

## 1. 背景与问题

panel-chat 窗口左侧栏（chat 模式下的会话历史栏，`interfaces/webchat/src/components/chat_sidebar.rs`）存在：

1. **死连线（bug）**: 搜索框 `chat_sidebar.rs:466-473` 只是一个静态 `<span>` 占位，无 `<input>`、无过滤逻辑。用户看到搜索框却完全不工作。
2. **能力缺失**: 相比参考项目，缺少"会话运行中"指示与底部状态条。

同项目内已有可复用范式：`SettingsSidebar` / `MemorySidebar`（`mode_sidebar.rs`）已实现可用的客户端过滤输入。

## 2. 架构映射（参考 → Aleph）

三项能力全部映射到 Aleph **现有基础设施**，零新增后端 RPC、纯 Panel（WASM/Leptos）层，严守 R4（Interface 纯 I/O）、R6（YAGNI）与"连线优先"。

| 能力 | 参考项目实现 | Aleph 复用的现有设施 | 连线方式 |
|---|---|---|---|
| 会话搜索/过滤 | hermes-web-ui `useSessionSearch` | `SettingsSidebar` 客户端过滤范式 + 已有 i18n `chat.search_placeholder`（"Search chats..."） | 纯客户端 |
| 每会话"运行中"指示 | hermes `workingSessionIdSet`/`isWorking` | 事件总线 `stream.run_accepted/run_complete/run_error`；`RunAccepted` 载荷带 `session_key` | 事件驱动 live-only |
| 底部状态条 | hermes `SidebarStatusStrip` + `useSidebarStatus`（10s 轮询） | `activity.stats` RPC（已注册，`home.rs:243` 已调用，返回 `active_total`）+ `dashboard.is_connected`/`connection_error` | 复用 RPC + 轻量轮询 |

**明确排除（R6 YAGNI，范围问答已确认）**: 置顶/收藏、归档（均需新增后端）、列表虚拟化、拖拽排序。

## 3. 详细设计

所有变更集中在 Panel 层。主体在 `chat_sidebar.rs`，状态条抽为新小组件。

### 3.1 修复死搜索框（① 熵减点）

- **删除**: `chat_sidebar.rs:466-473` 的死 `<span>` 占位块（这是要清理的旧代码，满足熵减要求）。
- **新增**: 真实 `<input type="text">`，绑定新 `RwSignal<String>` `search_query`，结构照搬 `SettingsSidebar`（`mode_sidebar.rs:253-260`）的"搜索图标 + input"，复用 `chat.search_placeholder` i18n key。
- **过滤接线**: 在会话过滤处（当前 `chat_sidebar.rs:505-509` 的 `filtered` 构建）追加一道过滤：
  - `needle = search_query.get().trim().to_lowercase()`
  - `needle` 为空 → 全量显示（向后兼容，行为不变）。
  - 否则保留 `topic`（回退到 `key`）`to_lowercase().contains(&needle)` 的会话。
- 空结果时复用现有 `chat.no_conversations` 空态文案。

### 3.2 每会话"运行中"指示（② 事件驱动 live-only）

- **局部状态**（ChatSidebar 内）:
  - `running: RwSignal<HashMap<String, usize>>` — `session_key → 在飞 run 计数`（引用计数，正确处理同会话并发 run）。
  - `run_to_session: RwSignal<HashMap<String, String>>` — `run_id → session_key`，因为 `RunComplete`/`RunError` 载荷只带 `run_id`，需反查归属会话。
- **订阅**: 复用现有 `dashboard.subscribe_events` + `subscribe_topic` 机制（chat_sidebar 已用它订阅 `stream.session_updated`）。新增订阅 `stream.run_accepted` / `stream.run_complete` / `stream.run_error`；Panel 内部分发 topic 为 `run.run_accepted` / `run.run_complete` / `run.run_error`（前端将 `stream.` 前缀转 `run.`，见 `frame.rs` 注释，与现有 `run.session_updated` 处理一致）。
  - `run.run_accepted`: 从载荷解析 `run_id` + `session_key` → `run_to_session[run_id]=session_key`；`running[session_key] += 1`。
  - `run.run_complete` / `run.run_error`: 从载荷解析 `run_id` → 反查 `session_key` → `running[session_key] -= 1`，归零则 `remove`；清理 `run_to_session[run_id]`。
- **渲染**: 运行中的会话行（`running` 含其 `key`）在标题行前显示一个脉动小圆点（Tailwind `animate-pulse`，主色）。非运行行不变。
- **清理**: `on_cleanup` 退订该订阅（与现有 `subscription_id` 退订模式一致）。
- **诚实边界**: live-only。刷新页面时已在后台运行的 run，要等下一个事件才点亮——与现有 `unseen_activity` live-only 范式一致。这是为坚持"零新增后端"刻意接受的取舍（避免新增"活跃会话集"RPC）。

### 3.3 底部状态条（③）

- **新组件**: `interfaces/webchat/src/components/sidebar/session_status_bar.rs`（约 60 行；在 `sidebar/mod.rs` 导出）。抽出而非内联的理由：`chat_sidebar.rs` 现 753 行，继续膨胀违反 P2（>500 行考虑拆分）；状态条自包含、可复用。
- **挂载**: 由 ChatSidebar 在其 flex 列底部渲染（会话列表 `overflow-y-auto` 区之下）。位于 `ModeSidebar` 底部 `NavMenu` 之上，不与之冲突。
- **内容**: 左 = 网关态文案（`Healthy` / `Degraded` / `Disconnected`，沿用 `home.rs:265-273` 的 `is_connected`/`connection_error` 推导逻辑 + 对应色调点）；右 = 活跃运行数（`activity.stats` 的 `active_total`，`tabular-nums`）。
- **数据获取**: 连接后立即取一次 + 每 10s 轻量轮询（`gloo_timers`，对齐 hermes `POLL_MS=10_000`；单次 `activity.stats` RPC，开销极小，能捕获后台/cron 在别处启动的 run）。未连接时显示 `Disconnected` 并清零。
- **i18n**: 新增少量 key（如 `chat.active_runs` / 复用现有网关态文案）。

## 4. 组件边界

| 单元 | 职责 | 依赖 | 可独立理解/测试 |
|---|---|---|---|
| `chat_sidebar.rs`（改） | 会话列表 + 搜索过滤 + 运行指示状态机 | `DashboardState`、`ChatState`、事件总线 | 是 |
| `session_status_bar.rs`（新） | 渲染网关态 + 活跃运行数 | `DashboardState`、`activity.stats` RPC | 是（纯展示 + 一个 RPC） |

接口向后兼容：搜索空查询 = 原行为；运行指示与状态条为纯增量 UI，不改任何现有 RPC 契约或会话选择/重命名/删除流程。

## 5. 测试与验证

- 纯 WASM/Leptos UI 变更，无后端业务逻辑。
- **按用户强制约束：完成后不跑 `cargo check`，直接提交。**
- 人工/后续验证项：
  1. 搜索框输入可实时过滤会话列表；清空恢复全量。
  2. 在某会话发起 run → 该行点亮脉动点；run 结束 → 熄灭；并发 run 引用计数正确。
  3. 状态条网关态随连接变化；活跃运行数与 `activity.stats` 一致。

## 6. 熵减（清理点）

- 删除 `chat_sidebar.rs:466-473` 死搜索 `<span>` 占位块（被可用 input 取代）。
- 不遗留未使用的 import/信号；新增订阅均配 `on_cleanup` 退订。

## 7. 提交

按用户工作流：扫描 → 计划 → 实施 → 提交。完成后直接提交（English commit message，`webchat:` scope，无 attribution）。
