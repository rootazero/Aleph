# 运行中追加发言 — 幽灵气泡 + 回合边界插入 + 强制插队（设计 / Design Spec）

- **日期**: 2026-06-28
- **范围**: wide 单聊（desktop / 宽屏 Panel）。phone 端另起 spec。
- **形态**: 纯前端（`interfaces/webchat/`）连线 + UI 修复，**零 Core 改动**。
- **遵守**: R4（Interface 纯 I/O）、R10（薄 Harness，不动笨循环）、P6（KISS）。

---

## 1. 背景与问题（Why）

任务运行时用户追加发言的「排队/插入」机制目前**几乎不可用**。根因经探索确认是**前后端两套机制没接通**，而非后端能力缺失：

- **后端（已具备，即用户所说"底层逻辑已出"）**
  - `src/gateway/execution_engine/` 的 `BusyInputMode` 三态：**Steer**（默认，把运行中到达的同会话发言注入活跃会话事件流，harness 在**下一个回合边界**经 `get_events()` 读到并织入）/ **Interrupt**（取消当前 run，发言作为新 run 重启）/ **Queue**（FIFO 等空闲）。
  - `gui:chat` 频道默认 **Steer**。后端还自带 steering **合并（coalesce）** 与 **post-run rescue**（收尾窗口漏接的补救）。
  - 回合边界信号**已存在**：`AgentTraceEvent::TurnStarted{iteration}` / `TurnCompleted{iteration,..}`（`shared/protocol/src/events.rs:285,310`），且 Panel 已订阅消费（`events.rs:121` 的 `turn_started → chat.begin_step`）。

- **前端（缺陷所在）**
  - 队列是**纯客户端** `ChatState::prompt_queue`；运行中回车只 `enqueue`（不发后端），**仅在 `busy→idle`（run 整体结束）才排空**（`shared/ui_logic/src/state/composer_queue.rs::should_auto_drain_on_settle`）。即"运行中插话"在 Panel 上**从未真正发生**——发言只是默默躺在**输入框上方的 chip**（`composer/queue_bar.rs`）里干等 run 全部结束。
  - 叠加 `active_run_id` 事件延迟，偶发"以为空闲点发送反而触发后端意外 steer"，体验进一步崩坏。

**对照参考项目**：Codex（`codex-rs/tui`）与 pi（`packages/agent`）都用**双队列**（pending steer / follow-up）、把排队预览**渲在对话区底部**、可 **Esc 强插**。Aleph 后端其实已等价具备这套能力，缺的只是**前端没接 + 视觉位置不对**。

---

## 2. 目标 / 非目标

### 目标（In Scope）
1. 追加发言以**幽灵气泡**形态呈现在**对话流底部**（可滚动消息区尾部，非输入框上方 chip）；插入后**融入对话流**。
2. 排队中可**继续追加**多条；每条可 **✕ 删除**、可**点开拉回输入框重编**（客户端暂存语义）。
3. 默认在 **agent 下一个回合边界**把整批一起插入（接通后端 Steer），而非等整个 run 结束。
4. **强制插队**：Esc 快捷键 + ⚡按钮双入口 → 中断当前 LLM 任务 → 排队发言自动插入 → 对话/任务自动重启。
5. 修复连线，使该机制**实际可用**。

### 非目标（Out of Scope）
- phone 端（另起 spec）。
- 任何 Core / Gateway / harness 改动（后端能力已足够）。
- 新增 JSON-RPC 方法（复用 `chat.send` + `chat.abort`）。
- 双队列语义区分（steer vs 纯 follow-up）——本期只做单一"回合边界插入"语义；强制插队覆盖"不想等"。
- "发即锁定"语义（已否决，采用可撤回可编辑）。

---

## 3. 行为契约（Behavior Contract）

| # | 场景 | 行为 |
|---|---|---|
| B1 | agent 空闲时回车 | 照常发送、起新 run（**不变**） |
| B2 | **agent 运行中回车** | 发言变成**对话流底部的幽灵气泡**（虚线描边 + 变淡，位于可滚动消息区尾部、Todo 面板之上），**暂存客户端、未发出** |
| B3 | 继续追加 | 可叠多个幽灵；保持提交顺序 |
| B4 | 删除 | 每个幽灵带 **✕**，点击从队列移除 |
| B5 | 编辑 | 点击幽灵正文 → 内容拉回输入框、该条移出队列（对齐 Codex「restore to composer」）|
| B6 | **到达回合边界** | agent 进入下一个 turn（`turn_started`/`turn_completed` trace）时，队列非空 + run 活跃 + 非强制 → **整批一起冲队**：`chat.send`（后端 Steer 注入活跃会话）→ agent 在**持续的同一 run** 里织入；幽灵**原地实心化**为真实发言气泡，后续 turn 作答 |
| B7 | **强制插队** | Esc（聚焦输入框时）/ ⚡按钮 → `chat.abort(active_run_id)` 中断 → 立即冲整批 → run 已取消，发言作为**新 run** 起跑（对话/任务自动重启）；幽灵实心化 |
| B8 | 普通 Stop（现有按钮） | 只 halt 当前 run，**不冲队**；幽灵保留待用户后续发送或强插（与 B7 区分）|
| B9 | run 自然结束仍有幽灵 | 空闲时冲队（**保留**现有 `should_auto_drain_on_settle` 作兜底）|
| B10 | 强插但无活跃 run | 退化为普通发送（B1）|

> **插入时机说明**：B6 冲队发生在回合边界 trace 到达时；`chat.send` 落入活跃会话事件流，harness 在其**下一个** `get_events()` 读到。故可编辑窗口 ≈ 当前 turn 时长；插入延迟 ≤ 1 个回合，符合用户选择的"下个回合边界"语义。强插（B7）为"不想等"的提速逃生口。

---

## 4. 架构 / 连线（How）

### 4.1 数据流
```
用户运行中回车
  └─> ChatState::prompt_queue 追加 QueuedPrompt（客户端暂存，渲为幽灵气泡 @ transcript 尾部）
        │  (可 ✕ 删 / 点开拉回 composer 编辑)
        ▼
  回合边界 trace 到达 (events.rs: turn_started/turn_completed)
     且 queue 非空 && run 活跃 && !force
  └─> 冲整批: 逐条 ChatApi::send（运行中 => 后端 BusyInputMode::Steer 注入活跃会话）
        └─> 幽灵从 prompt_queue 移除 + 真实用户气泡 append 进流（实心化）
        └─> harness 下个 turn 织入并作答（同一 run_id）

强制插队 (Esc / ⚡)
  └─> ChatApi::abort(active_run_id)  // 绕过 user_interrupted 抑制
        └─> 冲整批 ChatApi::send  // run 已取消 => 作为新 run 起跑
```

### 4.2 关键设计点
- **暂存区**：复用 `ChatState::prompt_queue`（已是纯客户端 `RwSignal<Vec<QueuedPrompt>>`）。`QueuedPrompt` 若缺 id 则补一个稳定 key（供 `<For>` 渲染 + 删除/编辑定位）。
- **冲队触发改写**：当前唯一触发是 `busy→idle`。**新增**回合边界触发——在 `events.rs` 的 trace 分发里，于回合边界（`turn_completed`，或 `turn_started` 且 `iteration >= 2`）置一个 `flush_request` 信号 / 直接调用冲队入口；`busy→idle` 兜底保留。
- **冲队机制**：复用现有 `ChatApi::send`（`api/chat.rs`）。运行中发送天然走后端 Steer，无需新 RPC、无需 BusyInputMode 选择参数。
- **强制插队**：新增 Esc keydown（composer textarea 聚焦时）+ ⚡按钮。流程 = `chat.abort` → 冲队。**必须绕过** `user_interrupted` 一次性抑制标志（该标志保留给 B8 普通 Stop）。即区分两个动作：`Stop`(halt, 不冲) vs `ForceInsert`(abort + 冲)。
- **实心化**：冲队即调 send，send 本就把用户气泡 append 进 transcript。幽灵从 `prompt_queue` 移除、真实气泡在**同一位置**出现 → 天然连续。可选 dashed→solid CSS 过渡微动画提升"流入"观感。
- **冲队决策纯函数**：所有"是否冲/冲哪批/是否抑制"判定集中到 `shared/ui_logic/src/state/composer_queue.rs` 的纯函数（扩展现有 `should_auto_drain_on_settle`），便于单测。

### 4.3 改动文件（wide 单聊）
| 文件 | 改动 |
|---|---|
| `interfaces/webchat/src/platform/wide/views/chat/composer/queue_bar.rs` | 去除/改造：幽灵不再渲在输入框上方 |
| `.../chat/composer/mod.rs` | 运行中回车仍 enqueue；新增编辑（拉回 composer）、Esc 强插 keydown、⚡按钮；区分 Stop vs ForceInsert |
| `shared/ui_logic/src/state/composer_queue.rs` | 扩展冲队决策纯函数（回合边界冲 / 强制冲 / 空闲兜底 / 普通 Stop 抑制）+ 单测 |
| `.../chat/events.rs` | 回合边界 trace → 触发冲队信号 |
| `.../chat/transcript.rs`（+ `messages.rs` 如需） | 在消息流尾部渲染 pending 幽灵气泡（✕ / 点开编辑）|
| `.../chat/state.rs` | `prompt_queue`（已存在）+ 强制冲队一次性信号；**clear/clear_session/restore 重置幽灵队列** |

视觉规格（已 brainstorm 用可视化伴侣定稿，方向 A「幽灵气泡」）：右对齐用户气泡样式、虚线描边、降透明度/降饱和；可选小标 `排队 N`；hover/常驻 ✕；底部一行强插提示「⚡ 立即插入 / Esc」。配色沿用 Panel 深色（`#0d0d10` / `#17171c` / `#4f46e5` 靛蓝）。

---

## 5. 边界与风险

- 🔴 **会话切换 / 新建 / 恢复必须 reset `prompt_queue`**：置顶 Todo 面板那次曾踩"新增 ChatState 字段未在 `clear`/`clear_session`/`restore_from` 重置 → 跨 tab 残留泄漏"。本次镜像同样处理（与 `strip_open` 等 ephemeral 字段一致）。
- `active_run_id` 事件延迟（GAP#3）：后端 Steer 能优雅吃下"误判运行中"的发送（最坏 = 被 steer 而非起新 run），可接受；不引入额外同步。
- **冲队幂等**：冲队后 `prompt_queue` 立即清空 → 连续回合边界不重复冲；空白/纯空格幽灵守卫不入队。
- **强插竞态**：`abort` 与冲队 `send` 的顺序——先 abort 再 send；若 abort 尚未在后端落地即 send，最坏 = 该 send 被当作对旧 run 的 steer。可接受（仍会被处理）；如需更稳可等 `run_error/cancelled` 事件再冲，列为实现期可选加固。
- 多条幽灵冲队 = 逐条 `chat.send`；后端 coalesce 合并为一个 steering burst，由 harness 一次织入（与后端既有行为一致）。
- 🔴 **运行中冲队 send 的 run_id 语义**：运行中 `chat.send` 会**返回一个新 run_id**，但该 send 实际被 Steer 注入原 run（`AgentRunManager::start_run` 先生成 run_id，`execute()` 走 steering 后立即 `Ok(())`、**不产生流**）。Panel **必须不**把这个返回的 run_id 当作新活跃 run（不替换 `active_run_id`、不起新 spinner/run 跟踪）；真正的作答仍来自**原 run** 的事件流。实现时冲队 send 的响应应被吞掉/忽略其 run_id（仅 B7 强插因先 abort、原 run 已亡，返回的 run_id 才是新活跃 run）。

---

## 6. 测试

- **纯函数单测**（`composer_queue.rs`）：回合边界触发冲队、强制冲队、空闲兜底冲队、普通 Stop 抑制冲队、队空不冲、非活跃 run 退化为发送。
- **投影/渲染测试**：pending `QueuedPrompt` → transcript 尾部幽灵气泡（参照现有 `events::projection_tests` 模式）。
- **构建门**：`just wasm`（controller 批验，实现者不跑 cargo）。
- **运行时 QA（用户执行）**：full macOS app（不带 PANEL_URL，走配对屏连本地最新 server）。验：B2 幽灵落对话流底部不抢 Todo 槽 / B3 多条 / B4 删 / B5 编辑 / B6 回合边界自动实心化并被作答 / B7 Esc + ⚡ 中断后自动重启 / B8 普通 Stop 保留幽灵 / 切 tab 无残留。

---

## 7. 已定决策（brainstorm 结论）

1. 插入时机 = **下个回合边界（Steer）**，非"任务结束后 follow-up"。
2. 强制插队入口 = **键盘快捷键（Esc）+ ⚡按钮**双入口；动作 = 中断 + 全部插入 + 重启。
3. 排队视觉 = **方向 A 幽灵气泡（融入对话流）**；否决托盘卡（隔离感 + 与 Todo 面板抢底部固定槽）。
4. 排队语义 = **可撤回可编辑（客户端暂存）**，非"发即锁定"。
5. 范围 = **先做 wide 单聊**；phone 另起。
