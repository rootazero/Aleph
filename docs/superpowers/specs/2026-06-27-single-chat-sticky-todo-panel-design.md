# 单聊 Chat 置顶实时 Todo 面板 — 设计文档

> **Single-Chat Sticky Todo Panel** — surface the agent's task decomposition as a
> live, in-place-updating checklist at the top of the single-chat window, checking
> off each step as it completes.
>
> 日期：2026-06-27 ｜ 状态：Design approved，待 plan
> 范围：仅单聊（single-chat）Panel 窗口。团队聊天窗口另作后期 spec。

---

## 1. 背景与动机 (Background)

用户口头需求："chat 对话窗口显示 agent LLM 分解任务之后的 todo list，并完成一个、打勾一个的显示效果"，并要求做**深度架构连线 + 功能增强 + 细节打磨 + 错误修复**。

参考项目 **codex** 有教科书级 `update_plan` 工具（`plan: [{step, status: pending|in_progress|completed}]`，"at most one in_progress"，TUI 渲染成打勾清单）。

### 现状勘探结论（关键）

Aleph 的"大脑 + 底座"**已经存在**，唯独缺最后一段到 UI 的结构化连线：

| 组件 | 文件 | 现状 |
|------|------|------|
| **scratchpad 工具** | `src/builtin_tools/scratchpad.rs` | ✅ 已有 `SetPlan / StartItem / CompleteItem` 三态生命周期 |
| **三态数据模型** | `src/memory/scratchpad/manager.rs` | ✅ `PlanItemStatus`（Pending→InProgress→Done，含 `glyph()`）、`PlanItem{text,status}`、`ScratchpadSnapshot`（`objective`/`items`/`is_objective_complete()`/`current()`/`render_progress()`） |
| **收尾门控** | `src/verification/scratchpad_goal_verifier.rs` | ✅ 模型想停但清单有 `- [ ]` 未打勾时 veto 顶回继续（结构化看门狗，非 JudgeVerifier，守 R7/R10） |
| **文本侧信道** | `src/gateway/execution_engine/scratchpad_progress_sink.rs` | ⚠️ 已识别 `ToolCallCompleted{scratchpad}` 并把 `render_progress` 文本镜像到用户 channel——但 ① 默认 **OFF**（`[execution] progress_push`）② 纯**文本气泡**非结构化 widget ③ 走 `ChannelRegistry`（适合 Telegram，不是 Panel sticky 面板） |
| **Panel 渲染** | `interfaces/webchat/src/platform/wide/views/chat/{events,messages,view}.rs` | ❌ scratchpad 调用只作为**通用 tool-call 气泡**散在时间线，无合并的实时 widget、无打勾动画 |

**缺口一句话**：结构化的 `ScratchpadSnapshot` 在 core 里有，但**从没以结构化形式送到 Panel 渲染成实时 widget**。本特性补这段连线。

---

## 2. 架构红线契合 (Redline Conformance)

| 红线 | 契合方式 |
|------|---------|
| R7 LLM 主权 | 计划由 LLM 调 scratchpad 工具产生；系统不做任务分解推理 |
| R8 工具即一切 | 计划的增删改打勾全经 scratchpad 工具 |
| R4 Interface 纯 I/O | Panel 只投影渲染结构化事件，不处理业务逻辑 |
| R10 笨循环 | 零 harness emit 点、零新 `LoopTraceEvent` 变体 |
| R2 UI 唯一源 | 复杂 widget 在 Leptos Panel，非原生 |
| R3/P6 核心轻量 + YAGNI | core 仅动 `ScratchpadOutput` 一处；不引入新原语、不重构 task_manage/goal |
| R5 AI 主动到达 | 进度主动浮现给用户，无需切换上下文 |

---

## 3. 端到端数据流 (Data Flow)

```
LLM 调 scratchpad(set_plan / start_item / complete_item / clear)
  → 工具算出 ScratchpadSnapshot（已有 manager.snapshot()）
  → ScratchpadOutput 携带结构化 snapshot          ★ core 唯一改动
  → ToolCallCompleted 事件（result JSON 内含 snapshot，本就流到 Panel stream.*）
  → events.rs::apply_trace_event "tool_call_completed" 分支
      识别 tool_name=="scratchpad" → 解析 snapshot → chat.set_plan(...)   ★ panel 新增
  → TodoPanel 组件读 chat.plan 信号 → 置顶进度环卡片，原地打勾            ★ panel 新增
```

**正交保证**：非 Panel 通道（Telegram / headless daemon）继续走现有 `ScratchpadProgressSink` 文本镜像（默认 OFF）；Panel widget 走 stream 投影（单聊默认 ON）。两条路读同一批事件但输出形态不同，**互不重复、互不依赖**。

---

## 4. Core 改动（最小，单点）

**文件**：`src/builtin_tools/scratchpad.rs`

- `ScratchpadOutput` 新增字段 `snapshot: Option<PlanSnapshotDto>`。
- 新类型 `PlanSnapshotDto`（serde + JsonSchema）：

```rust
struct PlanSnapshotDto {
    objective: Option<String>,
    items: Vec<PlanItemDto>,   // { text: String, status: "pending"|"in_progress"|"completed" }
    complete: bool,            // = snapshot.is_objective_complete()
}
```

- 由现有 `manager.snapshot()`（已在 `progress_echo` 中调用）直接映射。
- ⚠️ `PlanItemStatus` **未派生 serde**（仅 `Debug/Clone/Copy/Eq`），故 `PlanItemDto` 自带 serde：`Pending/InProgress/Done` → `"pending"/"in_progress"/"completed"`（与该枚举 doc 注释 "pending → in_progress → completed" 的命名意图一致；不改原枚举派生，避免牵动 manager 其它用法）。
- **仅** mutating action（`set_objective / set_plan / start_item / complete_item / clear`）附带 snapshot；`read` 不带（pull 非 progress，与文本 sink 的 `PROGRESS_ACTIONS` 口径一致）。
- 零新协议变体、零 harness emit 点——snapshot 搭现有 `ToolEnd.result` JSON 顺风车。

---

## 5. Panel 改动（widget）

### 5.1 状态层
**文件**：`interfaces/webchat/src/platform/wide/views/chat/state.rs`
- `ChatState` 新增 `plan: RwSignal<Option<PlanView>>`，绑定当前活跃单聊会话的最新计划。
- `PlanView` = Panel 侧轻量镜像（objective / items[{text,status}] / complete / blocked）。

### 5.2 投影层
**文件**：`interfaces/webchat/src/platform/wide/views/chat/events.rs`
- `apply_trace_event` 的 `"tool_call_completed"` 分支（现 line 57）：当 `tool_name == "scratchpad"` 时从 `result` 解析 `snapshot` → `chat.set_plan(Some(view))`；`clear` 或 `complete==true` 收尾 → 适当收起/清空。
- 纯函数解析（`snapshot_json → PlanView`）抽出便于单测。

### 5.3 组件层（Direction B · 进度环卡片 · 默认折叠）
**新文件**：`interfaces/webchat/src/platform/wide/views/chat/todo_panel.rs`（或 components/ 下）
- 插入点：`view.rs` 聊天列顶部，`MessageList` 之上、`SessionTabs` 之下（line ~203–208 区域）。
- **折叠态（默认）**：圆形进度环（conic-gradient，success 绿）+ "任务计划 · m/n" + 当前步骤摘要 pill；点头部展开。
- **展开态**：完整清单。完成项=绿勾 + 删除线；进行中=紫色脉冲点 + `primary-subtle` 高亮 + "进行中" tag；待办=空框。
- **打勾动画**：✓ SVG 描边画入（stroke-dashoffset）+ 行背景 `success-subtle` 闪一下后归位 + 删除线渐入。
- **出现/隐藏**：`plan == None` 自动隐藏；objective 设定 → 滑入；`clear` / 全部完成 → 环 100% 后收起。
- 配色对齐设计 token：primary `oklch(0.55 0.120 310)`、success `oklch(0.55 0.120 130)`、`primary-subtle`/`success-subtle`（`interfaces/webchat/styles/tailwind.css`）。
- 视觉基准：`.superpowers/brainstorm/4644-1782562118/content/todo-style.html`（方向 B）。

---

## 6. 自动 per-chat 画板（功能增强）

**文件**：`src/builtin_tools/scratchpad.rs` + `src/builtin_tools/scratchpad_registry.rs`
- `ScratchpadArgs.project_id` 改为可选；缺省时由当前 session/chat 派生默认 id（复用 `scratchpad_registry` 的 session_key→project_id 绑定）。
- 效果：模型无需取名即可在单聊开列清单（消除"凭空取名"摩擦，提高触发率）；显式 `project_id` 仍可做跨会话持久项目。
- Widget **不依赖 project_id**——只反映本 chat stream 的最新 snapshot，关联天然由 ChatView 订阅的 stream 决定。
- path-traversal 校验对自动派生 id 同样成立（派生 id 不含分隔符/`..`/前导点）。

---

## 7. 边界硬化 + 细节打磨 (Hardening)

- **空计划**：items 为空时不渲染 widget（仅 objective 无步骤 → 显示 objective 但无环进度）。
- **SetPlan 重置**：新计划完整替换旧 items，widget 原地刷新（不残留旧勾）。
- **至多一项 in_progress**：`StartItem` 自动把前一个 in_progress 降级为 pending/done（在 manager 层补/确认该不变量），widget 永远只高亮一项。
- **完成 banner**：最后一项打勾 → `COMPLETION_BANNER` → 环 100% + 收起。
- **被拦截态**：`verifier_veto` 事件现已文本回显（events.rs:134 "🔁 收尾被拦截（清单仍有未完成项）"）；widget 增加 blocked 视觉态（边框/图标提示），不抢焦点（R5 边界）。
- **折叠微提示**：折叠态下有步骤完成时 pill 微闪一下提示进度，不强制展开。

---

## 8. 范围边界 (Scope / YAGNI)

**做**：单聊 Panel widget + core snapshot 连线 + 自动 project_id + 边界硬化。

**不做（后期独立 spec）**：
- 团队聊天窗口的 todo 设计（另一种形态）。
- CLI / TUI 的富渲染（继续走文本镜像）。
- scratchpad / task_manage / goal 三套 plan 原语统一重构（聚焦本特性，守 R3/P6）。

---

## 9. 测试与验收 (Testing & Acceptance)

### 单元测试
- **core**：`PlanSnapshotDto` ← `ScratchpadSnapshot` 映射（含三态、complete、空计划）。
- **core**：`StartItem` 后"至多一项 in_progress"不变量。
- **panel**：`snapshot_json → PlanView` 投影纯函数（各 action、空、完成）。

### 权威运行时门
用户的 **macOS 完整 App + iOS-sim 双端流程**（重编本地 core 服新 dist → 实测：置顶面板随 set_plan 出现、complete_item 原地打勾、折叠/展开、全完成收起、多端同步）。

### 构建策略
- 实现期**不跑 cargo**（系统负担）。
- 收尾至多一次 `cargo check -p alephcore --lib`；Panel 经 `just wasm` 验。

### 验收标准
1. 单聊中模型调 `scratchpad(set_plan=…)` → 顶部进度环卡片（折叠态）出现。
2. `start_item` → 当前步骤高亮 + pill 摘要更新。
3. `complete_item` → 该项原地打勾（✓ 画入 + 绿闪 + 删除线）+ 环进度推进。
4. 全部完成 / `clear` → 环 100% 后面板收起。
5. 无活跃计划时面板不可见。
6. Telegram 等通道行为不变（文本镜像，受 `progress_push` 开关）。

---

## 10. 代码锚点速查 (Anchors)

| 关注点 | 锚点 |
|--------|------|
| scratchpad 工具 / 输出 | `src/builtin_tools/scratchpad.rs`（`ScratchpadOutput` / `progress_echo`） |
| 三态快照 | `src/memory/scratchpad/manager.rs`（`ScratchpadSnapshot` / `PlanItemStatus`） |
| 收尾门控 | `src/verification/scratchpad_goal_verifier.rs` |
| 文本侧信道（正交，不改语义） | `src/gateway/execution_engine/scratchpad_progress_sink.rs` |
| session→project 绑定 | `src/builtin_tools/scratchpad_registry.rs` |
| Panel 投影 | `interfaces/webchat/src/platform/wide/views/chat/events.rs`（`"tool_call_completed"`） |
| Panel 状态 | `…/chat/state.rs`（`ChatState`） |
| Panel 布局插入点 | `…/chat/view.rs`（`ChatView`，MessageList 上方） |
| 视觉基准 mockup | `.superpowers/brainstorm/4644-1782562118/content/todo-style.html`（方向 B） |
