# 工作流回显 × 工作区面板整合设计

**日期**: 2026-06-05
**状态**: 已批准设计，待 writing-plans
**关联**: [2026-06-05-workspace-panel-redesign-design.md](./2026-06-05-workspace-panel-redesign-design.md)（右侧工作区面板重构本体）

## 1. 背景与问题

Agent 多步执行（Think→Act 循环）的叙述文字在 WebChat Panel 左侧聊天里**连成一整段**，没有按步骤分隔显示。例如：

> 好的，我已经获取了定时任务输出结果，现在让我搜索合适的配图，然后创建HTML展示页面。让我使用一些公开可用的图片URL…HTML文件已创建完成！让我验证一下文件…

每一句本属于一轮独立的迭代（各自触发了工具调用），但用户看到的是无分隔的一大段。

同期，聊天窗口**右侧工作区面板已经重构**为 Activity Timeline（commit `328dadcb7` / `b8df41134`）：它消费 `agent_trace` 事件，按 `(run_id, tool_id)` 平铺列出工具调用，可 inline 展开看 args/result。但它**只消费了 `tool_call_started/completed`**，没用上迭代叙述。

**目标**：把"多步执行回显"与重构后的工作区面板整合——左右两侧都按步骤分段，共享步骤主键，点击互相高亮；并原生兼容已有的打字机/即时两种输出模式。

## 2. 根因分析

"连成一整段"在两种 output_mode 下各因一个原因发生，最终表现相同：

- **打字机模式**：前端 `append_chunk` 把所有 `response_chunk` delta 累加进**同一个**气泡（`state.rs:427`），从不分段。前端虽有 `finalize_intermediate`（`state.rs:386`）分段机制，但它依赖 `is_intermediate=true` 信号，而该信号从未被产生（`event_drain.rs:119` 硬编码 `is_intermediate: false`）。
- **即时模式**：后端 `GatewayEventEmitter` 把非 final 的 `response_chunk` 缓冲进 `instant_buffer`，直到 final 一次性吐出全文（`impls.rs:62-128`）——天然就是一大段。

**关键事实**：`AgentTrace` 事件（含 `TextEmitted { iteration, stream, text }`、`TurnStarted { iteration }`、`ToolCallStarted/Completed { iteration, … }`）**在两种模式下都立即发出**，不受 `instant_buffer` 影响（测试 `test_instant_mode_passes_non_chunk_events` 证实）。它原生携带 `iteration`，是唯一"两模式即时 + 显式分步主键"的数据源。

## 3. 架构决策

### 3.1 选定路线 A：前端 + agent_trace 驱动

用一条 `agent_trace` 流统一驱动左右两侧的**结构与文本**，`response_chunk` 退化为打字机模式专属的实时预览叠加。

| 维度 | 路线 A（选定，agent_trace 驱动） | 路线 B（弃用，gateway is_intermediate） |
|------|--------------------------------|----------------------------------------|
| 即时模式分步 | ✅ `TextEmitted` 立即到，每步整段填充 | ⚠️ 依赖 buffer flush，工具/叙述到达时序错位，分组不一致 |
| 打字机逐字 | ✅ `response_chunk` 实时叠加当前气泡 | ✅ 同通道边界，零竞态 |
| 跨高亮 iteration 主键 | ✅ 每个 agent_trace 事件原生带 iteration，左右天然对齐 | ❌ 边界计数 ≠ iteration（无工具的轮次错位） |
| 改动面 | 纯前端，不碰 gateway/harness/协议 | 碰 gateway，仍需额外对齐 iteration |
| 红线 | 符合 R4（Interface 纯 I/O）/R10（薄 harness） | 为显示问题动核心 |

### 3.2 核心规则（模式感知）

- **权威结构 + 文本 = `agent_trace`**：
  - `TurnStarted{N}` → 切步边界（finalize 当前步、开 iteration=N 新步）。
  - `TextEmitted{N, stream, text}` → 填充第 N 步文本（含 `Final`）。
  - `ToolCallStarted/Completed{N}` → 把工具调用挂到第 N 步。
  - 共享主键 `(run_id, iteration)`。
- **`response_chunk` = 打字机模式实时预览**：逐字叠加到"当前迭代气泡"；到达的 `TextEmitted{N}` 把该气泡文本**校正为权威值**（抹平跨流边界竞态）。
- **即时模式忽略 final dump**：最终答复文本走 `agent_trace`（`TextEmitted{Final}` / `SessionCompleted.final_text`），不再使用拼接的 instant dump；`is_final` 的 `response_chunk` 内容被忽略。

### 3.3 两模式最终表现

- **打字机**：每步叙述在左侧气泡逐字打出；右侧时间线同步分组，工具卡片穿插。
- **即时**：每步叙述在该轮完成时整段出现（符合"即时"语义）；右侧同样分组。
- 两模式：左右皆按步分段，`(run_id, iteration)` 双向点击高亮。

## 4. 详细设计

> 全部改动落在 `interfaces/webchat/`。gateway / harness / 协议 / CLI 不动。

### 4.1 数据模型

**`views/chat/state.rs` — `ChatMessage`**
- 新增 `iteration: Option<usize>`：中间步骤气泡打上其迭代号；非分步消息（用户消息、单轮回复）为 `None`。

**`state/layout.rs` — `WorkspaceState`**
- 新增按迭代分组的结构。最小形态：
  - `step_narration: RwSignal<HashMap<(String, usize), String>>` —— `(run_id, iteration)` → 该步叙述文本。
  - 现有 `tool_payloads: HashMap<(run_id, tool_id), ToolPayload>` 保留；新增 `(run_id, tool_id) → iteration` 的归属映射（或在 timeline 构建时从 ChatState 推导）。
- 新增 `focused_step: RwSignal<Option<(String, usize)>>` —— cross-highlight 焦点（与现有 tool 级 `focus_tool` 并存，新增 step 级粒度）。
- 新增 `current_iteration: RwSignal<Option<usize>>` —— 当前活动迭代，供 `response_chunk` 预览归位。
- `reset()` 扩展：清空 `step_narration` / `focused_step` / `current_iteration`（保留 layout mode，与现状一致）。

### 4.2 事件处理（`views/chat/events.rs` `subscribe_run_events`）

扩展 `agent_trace` 分支（现仅处理 `tool_call_started/completed`）：

- `TurnStarted{N}`：
  - 左：finalize 当前气泡（若有内容），开 `iteration=N` 的新 assistant 气泡。
  - 右：建立 step group N。
  - `current_iteration = N`。
- `TextEmitted{N, stream, text}`：
  - 左：把第 N 步气泡文本**设为**（非追加）`text`（权威校正，抹平打字机预览的边界漂移）。
  - 右：`step_narration[(run_id, N)] = text`。
  - `stream == Final` 的文本归入最终答复气泡。
- `ToolCallStarted{N}` / `ToolCallCompleted{N}`：
  - 维持现有 `record_tool_args/result` + chat `update_tool`，并记录该 tool 归属 `iteration=N`（供右侧分组与左侧 chip 归位）。

`response_chunk` 分支（打字机预览，模式无关地处理）：
- delta 追加到 `current_iteration` 对应气泡（实时逐字）。
- `is_final` 的 chunk：内容忽略（即时模式的 dump / 打字机末尾 token 都由 `agent_trace` 文本覆盖）。

> **跨流时序**：`agent_trace`（trace_sink，harness 内联）与 `response_chunk`（FlowStreamEvent drain，独立任务）不保证严格交错。打字机模式下边界处偶发"末位 token 落入相邻气泡"，由随后到达的 `TextEmitted{N}` 覆盖自愈。实现可选地按 `seq` 排序以进一步消除抖动（增强项，非必需）。

### 4.3 右侧时间线（`components/workspace_panel.rs`）

- `timeline_rows()` → `timeline_groups()`：从 agent_trace 派生的步骤构建，按 `iteration` 分组。
- 每组渲染：迭代号标题 + 叙述文字（`step_narration`）+ 该迭代的工具行（复用现有 `ActivityRow` / `PayloadBlock` / inline expand）。
- 空状态 `WorkspaceEmptyHero` 保留。

### 4.4 左侧渲染（`views/chat/messages.rs`）

- 每个中间步骤气泡顶部加一个可点击的"迭代 N"轻标签。
- 现有 tool chip 渲染保留（`messages.rs:281-348`）。

### 4.5 Cross-highlight 双向

- 单一 `focused_step((run_id, N))` signal 驱动：
  - 左→右：点左侧气泡的迭代标签 → 写 `focused_step` → 右侧 group N 高亮 + `scrollIntoView` + 进 Split（升级现有 `focus_tool_row → Split` 逻辑到 step 粒度）。
  - 右→左：点右侧 group N → 写 `focused_step` → 左侧气泡 N 高亮 + `scrollIntoView`。
- 高亮 = 两侧组件订阅同一 `focused_step` signal、据此加 ring/bg class。

## 5. 边缘情况

- **无工具的迭代**：右侧 group 只有叙述、左侧气泡只有文字。一致。
- **无叙述的迭代**：右侧 group 只有工具行；左侧气泡空文字 → 隐藏空文字、只渲染 chip。
- **打字机边界 token 漂移**：`TextEmitted{N}` 覆盖自愈。
- **即时模式 final dump**：忽略，最终答复走 `agent_trace`。
- **reset / 切项目 / 切 agent / 新会话**：扩展现有 `reset()` 清理新增 state（现有触发点 `chat_sidebar.rs` / `project_menu.rs` 不变）。

## 6. 范围与不变量

- **不动**：CLI（已分步）、harness、gateway、`shared/protocol`、FilesDrawer、现有 `focus_tool`（tool 级）行为。
- **红线**：纯 interface 层改动，符合 R4 / R10；不引入新的 LLM 调用或确定性推理替代。

## 7. 测试（host-safe，参考现有 `note_activity` host-safe 模式）

1. `TurnStarted{1..N}` 序列 → 正确切出 N 个左侧气泡 + N 个右侧 group。
2. `TextEmitted{N}` → 设置（覆盖）第 N 步文本，而非追加；打字机预览被覆盖。
3. `ToolCallStarted/Completed{N}` → 工具归入第 N 步（左 chip + 右 group）。
4. `focused_step` 双向映射：写入后两侧对应步骤高亮态正确。
5. 无工具 / 无叙述迭代的分组渲染。
6. 即时模式：仅 `agent_trace` 驱动即可构出完整分步（无 `response_chunk` 增量）；`is_final` dump 被忽略不重复。
7. `reset()` 清空新增 state，保留 layout mode。

## 8. 改动文件清单（全部在 `interfaces/webchat/`）

- `state/layout.rs` —— `WorkspaceState` 新字段 + `reset()` 扩展。
- `views/chat/state.rs` —— `ChatMessage.iteration` + 按迭代 finalize/新建气泡。
- `views/chat/events.rs` —— `agent_trace` 事件扩展 + `response_chunk` 预览规则。
- `views/chat/messages.rs` —— 左侧迭代标签 + 点击交互。
- `components/workspace_panel.rs` —— 时间线按迭代分组 + 高亮订阅。
