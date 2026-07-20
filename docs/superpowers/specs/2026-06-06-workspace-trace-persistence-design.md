# 右栏持久化 + 左右分工重梳 设计 (2026-06-06)

## 背景 / 问题

WebChat Panel 右侧工作栏（`LayoutMode::Split`）的流式显示是**一次性**的：刷新页面 / 切会话再切回 / 重启 panel 后，右栏的步骤卡与工具 args/result 全部消失。

根因（读取侧三处叠加）：

1. 右栏完全派生自两个**瞬态内存信号**：`chat.messages` 中带 `iteration` 标签的 assistant 气泡（→ StepCard），以及 `WorkspaceState.tool_payloads`（→ 工具 args/result）。见 `interfaces/webchat/src/components/workspace_panel.rs:34` `timeline_groups`。
2. 这两个信号**只由 live streaming 事件写入**（`interfaces/webchat/src/views/chat/events.rs`），从不从持久层重建。
3. 重开 / 切会话时，历史水合处把线索**写死成空**：`interfaces/webchat/src/components/chat_sidebar.rs:202` `tool_calls: vec![]`、`:207` `iteration: None`；`timeline_groups` 用 `m.iteration?` 过滤，于是所有重载气泡被跳过，右栏渲染空白 `WorkspaceEmptyHero`。

**关键事实：数据其实已经持久化，且不在记忆库。** 每个 run 启动即 `persist_run_task_started`（`src/gateway/execution_engine/execute.rs:105`），全程把完整 `AgentTraceEvent` 序列落到 `task_traces` 表，`task_id == run_id`（`execute.rs:311` → `trace_task_persisted.then(|| run_id.clone())`）。`task_traces` 是**可观测 / trace 存储**（`src/resilience/database/traces.rs`），与 `src/memory/` 记忆系统完全独立——不进 Dream、不进 insights、不参与记忆检索。本设计只接这份已有的 trace 存储，**绝不写入记忆库**。

## 目标

1. **右栏持久化**：重开 / 刷新 / 切回会话后，右栏能从持久 trace 完整重建步骤卡 + 工具 args/result。
2. **左右分工**：左栏 = 对话（用户消息 + 最终回答）+ 轻量步骤摘要；右栏 = 步骤详情 + 工具 args/result + 文件抽屉。两边均可从同一份 trace 重建。
3. **左栏步骤滚动条**：同一 run 的中间步骤收进定高、内部滚动的容器，避免超长步骤把对话主列拉长；运行完成后**自动折叠成一行**。

## 非目标

- 不引入任何记忆库写入。
- 不改 live 流式协议（`AgentTraceEvent` / `chat.send` 事件流不变）。
- 不修复 `views/agent_trace.rs` 独立回放视图（与本任务无关，留作既有功能）。
- 不做 trace 跨设备同步、不做新的索引表。

## 架构：单一投影，双源喂入

```
                     ┌─ live:   events.rs WS 流（运行中）
  AgentTraceEvent ──┤                                        ──► apply_trace_event() ──► chat.messages (step strip)
                     └─ replay: 新 RPC 拉持久 trace（加载时）                              + workspace.tool_payloads (右栏详情)
```

唯一投影 `apply_trace_event`：把 `events.rs` 现有的 `match kind { ... }`（`tool_call_started` / `tool_call_completed` / `tool_summary` / `turn_started` / `text_emitted` / …）抽成纯函数，签名 `apply_trace_event(chat: &ChatState, workspace: &WorkspaceState, run_id: &str, trace_event: &serde_json::Value)`。已兼容 `type` / `kind` 双标签，replay 直接喂持久 `event_json` 即可。live 与 replay 共用此函数，杜绝两条路径漂移。

### 服务端：新增只读 RPC `trace.by_runs`

- 入参：`{ "run_ids": [String] }`。
- 实现：对每个 run_id 调既有 `db.get_traces_by_task(run_id)`（按 `step_index ASC` 返回 `Vec<TaskTrace>`，含 `event_json`）。
- 返回：`{ "runs": { "<run_id>": [ <event_json>, ... ] } }`，缺失 / 无 trace 的 run_id 返回空数组（不报错）。
- 注册位置同 `trace.list` / `trace.get`（`src/bin/aleph-server/.../agent_init/mod.rs:1277` 一带），handler 放 `src/gateway/handlers/trace_replay.rs`。
- 不动 `chat.history`（避免把大体积 args/result 塞进会话历史负载）。

### Panel：加载会话时重放

`chat.history` 已为每条消息带回 `run_id`（`api/chat.rs:13`）。在 `chat_sidebar.rs` 历史加载成功后：

1. 收集本会话 assistant 消息的去重 `run_ids`。
2. 调 `TraceApi::by_runs(run_ids)` 拉持久 trace。
3. 对每个 run，按序对每个事件调 `apply_trace_event(&chat, &ws, run_id, &event)`，重建 `chat.messages` 的 iteration 气泡 + `tool_calls`，及 `workspace.tool_payloads`。
4. **移除** `chat_sidebar.rs:202/207` 写死 `tool_calls: vec![]` / `iteration: None` 的旧水合——改由重放产出（最终回答气泡仍来自 history 的 content）。

缺失 / RPC 失败 → 优雅降级到当前行为（左栏只显示最终回答，右栏 `WorkspaceEmptyHero`）。

**消息顺序**：live 路径中间步骤先于最终回答到达；replay 若先 `history.set(最终回答)` 再追加中间步骤，会把中间步骤错排到最终回答之后。解决：重放在 `history.set` **之前**进行——先用 trace 重放出每个 run 的「中间步骤气泡 + tool_calls」，再把 history 的最终回答按 `run_id` 归位拼接（中间步骤在前、最终回答在后），最后一次 `chat.messages.set` 写入完整有序序列。`StepStrip` 聚合在有序序列上做，天然正确。

## 左栏步骤滚动条

- `timeline::derive_timeline`（`views/chat/timeline.rs`）新增聚合：**同一 run 连续的 intermediate（iteration-tagged）消息**折叠为单个 `TimelineRow::StepStrip { run_id, steps }`；用户消息、最终回答仍是独立 `TimelineRow::Message`。
- 渲染（`messages.rs`）：`StepStrip` → `max-h-[~220px] overflow-y-auto` 容器，内部步骤逐行；运行中 stick-to-bottom 跟最新步骤。
- **完成自动折叠成一行**：当该 run 已完成（无 streaming 中间步骤 / 收到 `RunComplete`），整条 StepStrip 折叠为单行摘要（如 `#N 步 · 已完成`，可点击展开）。判定复用现有 run 完成信号（`chat.phase` / 末步状态）。
- `(run_id, iteration)` 左右联动高亮保留：StepStrip 内每步仍可点击 `focus_step`，右栏 StepCard 互相高亮。

## 错误处理 / 边界

- `trace.by_runs` 未知 run_id / 空 trace → 空数组，不报错。
- replay 与 live 不重复：切会话先 `ws.reset()`（`chat_sidebar.rs:180`）清空瞬态信号，再重放，避免叠加。
- 大 result 仍懒展开：右栏点击才渲染 JSON；replay 把事件拉回内存但 UI 不强制全渲染。
- 折叠态默认折叠已完成 run 的 StepStrip，但保留展开交互。

## 测试

1. **投影等价性**（核心不变量）：同一组有序 `AgentTraceEvent`，分别经 live 路径与 replay 路径，断言产出的 `chat.messages`（iteration / tool_calls）与 `workspace.tool_payloads` 相同。
2. **timeline 聚合**：连续 intermediate 折叠为单个 `StepStrip`；user / final 不被并入；多 run 各自成条。
3. **折叠态**：已完成 run 的 StepStrip 渲染为单行；运行中为滚动展开态。
4. **RPC**：`get_traces_by_task` 多 run 批量、空 run、`step_index ASC` 顺序；`trace.by_runs` 入参解析 + 缺失降级。
5. **降级**：RPC 失败时左栏仅最终回答、右栏空 hero，不 panic。

## 涉及文件（预估）

- 服务端：`src/gateway/handlers/trace_replay.rs`（+`handle_by_runs`）、`agent_init/mod.rs`（注册）。`traces.rs` 复用现有 `get_traces_by_task`，预计不改。
- Panel：`views/chat/events.rs`（抽 `apply_trace_event` 纯函数）、`api/trace.rs`（+`by_runs`）、`components/chat_sidebar.rs`（重放 + 移除写死水合）、`views/chat/timeline.rs`（`StepStrip` 聚合）、`views/chat/messages.rs`（滚动条 + 折叠渲染）。`components/workspace_panel.rs` 数据源不变（仍读 `chat.messages` + `tool_payloads`），只是这些信号现在能被重放填充。

## 红线核对

- **R4（Interface 纯 I/O）**：panel 只拉 trace JSON → 投影 → 渲染，无业务逻辑；服务端 RPC 只读包既有 DB 方法。
- **R7 / R9**：不新增确定性推理，纯重放已有事件。
- **不碰记忆库**：只读 `task_traces` 观测存储，与 `src/memory/` 隔离。
- **P6 简洁**：复用既有投影、既有 DB 方法、既有联动机制，新增面 = 一个只读 RPC + 一个 timeline 聚合分支。
