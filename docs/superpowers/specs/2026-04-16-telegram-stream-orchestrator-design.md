# Telegram Stream Orchestrator 设计文档

> 方案 B：统一 Stream Orchestrator  
> 日期：2026-04-16  
> 目标：学习并超越 OpenClaw 的 Telegram 流式交互能力，充分利用 Rust 的类型安全与并发优势

---

## 1. 范围与架构

### 1.1 目标

- 补齐 Aleph Telegram 频道相对于 OpenClaw 的核心流式交互缺失
- 引入统一的 `StreamOrchestrator` 调度 Draft Lane、Reasoning Lane、Answer Lane
- 将 `StatusReactionController` 升级为与执行引擎联动的生命周期状态机
- 清理废弃的 bridge 代码，避免屎山堆积

### 1.2 纳入范围

1. **Draft Stream API 支持** — `sendMessageDraft` 实时预览，自动降级到 message-edit
2. **Reasoning Lane** — 从 `<think>` 标签提取推理内容，独立渲染为草稿/消息
3. **Answer Lane** — 最终答案交付，支持 edit-based streaming
4. **Status Reaction 生命周期** — `Queued → Thinking → ToolActive → Compacting → Done/Error`
5. **Lane Delivery Tracker** — 精确追踪每条 lane 的 preview/final message_id
6. **代码清理** — 删除 `src/gateway/bridge/` 及关联废弃代码

### 1.3 排除范围（YAGNI）

- Auto-topic-label（LLM 自动命名 topic）
- Thread bindings / ACP spawn from Telegram
- 通用频道插件 SDK

### 1.4 架构原则

- `Channel` trait **不得修改**
- 所有新能力收敛在 `src/gateway/interfaces/telegram/` 内，不泄漏到 gateway core
- 配置解析在 channel 边界完成
- 向后兼容：未启用新能力时完全降级到现有路径

---

## 2. 模块结构

```text
src/gateway/interfaces/telegram/
├── streaming/
│   ├── mod.rs                    # StreamOrchestrator 入口与公开 API
│   ├── orchestrator.rs           # 统一调度器与状态机
│   ├── state.rs                  # StreamState 枚举及转换规则
│   ├── tracker.rs                # LaneDeliveryTracker
│   └── lanes/
│       ├── mod.rs
│       ├── draft_lane.rs         # sendMessageDraft 预览 lane
│       ├── reasoning_lane.rs     # <think> 提取与渲染 lane
│       └── answer_lane.rs        # 最终答案 edit-based lane
├── status_reaction_controller.rs # 与 orchestrator 联动的状态反应
├── delivery.rs                   # 重构：委托给 orchestrator
└── config_v2.rs                  # 新增 streaming/status_reactions 配置
```

---

## 3. StreamOrchestrator 核心设计

### 3.1 统一状态机

每条对话流拥有一个 `StreamState`，非法转换在运行期被拒绝：

```rust
pub enum StreamState {
    Idle,
    Streaming {
        draft: DraftLaneState,
        reasoning: ReasoningLaneState,
        answer: AnswerLaneState,
    },
    Finalizing,
    Completed,
    Error { cooldown_until: Instant },
}
```

所有状态转换通过 `orchestrator.transition(new_state)` 完成。

### 3.2 三条 Lane 的职责

| Lane | 用途 | 最终产物 |
|------|------|---------|
| **Draft Lane** | 利用 `sendMessageDraft` API 在输入框实时显示生成内容 | 草稿（可删除或固化） |
| **Reasoning Lane** | 解析 `<think>...</think>`，渲染为独立草稿/消息，前缀 `🤔 …` | 独立 reasoning 草稿（默认删除） |
| **Answer Lane** | strip 掉 reasoning 后的实际答案，按 edit-based stream 交付 | 最终正式消息 |

### 3.3 关键交互规则

1. **Draft Lane 优先**：若聊天支持 `sendMessageDraft` 且配置启用，所有实时预览走 Draft Lane；Answer Lane 只在最终确定时发送一次真实消息。
2. **Reasoning Lane 与 Answer Lane 并行**：检测到 `<think>` 时，reasoning 进入 Reasoning Lane，answer 继续走 Answer Lane，互不阻塞。
3. **自动降级**：Draft API 不可用时，自动降级为现有的 message-edit 模式。
4. **速率限制**：每条 lane 独立 throttle（最大 1 次 API 调用/秒），共享 `ApiThrottler`。

### 3.4 数据流

```
LLM chunk → reply_emitter
                │
                ▼
        StreamOrchestrator::push_chunk(text)
                │
        ├─ contains <think>? ──► ReasoningLane::update()
        │
        ├─ answer text ───────► DraftLane::update() (if draft enabled)
        │                        └─ fallback ──► AnswerLane::edit()
        │
        └─ finalize() ─────────► DraftLane::materialize() ──► real message
                                 ReasoningLane::clear()      (默认删除)
                                 AnswerLane::finalize()
```

---

## 4. Status Reaction 生命周期

### 4.1 状态枚举

```rust
pub enum AgentStatus {
    Queued,              // 👀
    Thinking,            // 🧠
    ToolActive(String),  // 🔧 <tool_name>
    Compacting,          // 🗜️
    Done,                // ✅
    Error,               // ❌
}
```

### 4.2 集成方式

- `StatusReactionController` 持有 `mpsc::Receiver<AgentStatus>`，监听 `execution_engine/run_loop.rs` 的事件回调。
- 每次状态变更调用 `bot.api.setMessageReaction()`，**限流 1 次/秒**。
- 若 `setMessageReaction` 失败（如群组无权限），降级为 `sendChatAction("typing")`。
- 当启用 status reactions 时，`Queued` 覆盖简单 ACK reaction。

### 4.3 与 Orchestrator 的联动

- `Streaming` 状态 → 发送 `Thinking`
- 执行引擎 tool start → 发送 `ToolActive("read_file")`
- finalize 完成 → 发送 `Done` 或 `Error`

---

## 5. Lane Delivery Tracker

用于精确追踪每条 lane 的交付状态：

```rust
pub struct LaneDeliveryTracker {
    lanes: HashMap<LaneName, LaneDeliverySnapshot>,
}

pub struct LaneDeliverySnapshot {
    pub lane: LaneName,
    pub preview_message_id: Option<MessageId>,
    pub final_message_id: Option<MessageId>,
    pub status: LaneDeliveryStatus,
    pub last_error: Option<String>,
}

pub enum LaneDeliveryStatus {
    Pending,
    Previewing,
    Materialized,
    Failed,
    Cleared,
}
```

**用途**：
- `delivery.rs` 需要 `reply_to` 时，准确找到 Answer Lane 的 final `message_id`
- 入站 `message_reaction` 事件反查原始消息
- 为调试和可观测性提供结构化日志

---

## 6. 错误处理与降级策略

### 6.1 Draft API 降级

`DraftLane` 首次调用 `sendMessageDraft` 时捕获错误：
- 若匹配 `unsupported|not available|can't be used`，标记 `draft_unavailable = true`
- 后续预览流量自动切到 `AnswerLane` 的 edit-based 模式
- 降级是会话级别，下次新对话会重新尝试

### 6.2 Error Policy

复用现有 `ErrorCooldown`，增强为 policy-aware：

```rust
pub enum ErrorPolicy {
    Reply,  // 向用户发送错误文本
    Silent, // 静默丢弃
    Once,   // 同作用域 60s 内只发一次
}
```

**作用域优先级**：`topic → group → account → global`

**去重 key**：`(account_id, chat_id, optional_thread_id)`

### 6.3 Rate Limit 与网络错误

- 所有 lane 共享 `ApiThrottler`（1 次/秒）
- `429` 时按 `retry_after` 指数退避
- 网络断开时 `StreamOrchestrator` 进入 `Error` 状态，恢复后自动重连

### 6.4 清理保证

`StreamOrchestrator::abort()` 被调用时：
- Draft Lane：调用 `clear()` 删除草稿
- Reasoning Lane：删除已发送的 reasoning 消息
- Answer Lane：若消息不完整，追加 `"[生成中断]"`

---

## 7. 代码清理计划

### 7.1 待清理内容

- `src/gateway/bridge/bridged_channel.rs`
- `src/gateway/bridge/mod.rs`
- `src/gateway/bridge/supervisor.rs`
- `src/gateway/bridge/types.rs`
- 残留引用：`justfile`、CI workflow、任何 Cargo workspace 中的 bridge 引用

### 7.2 安全清理原则

- **先验证后删除**：`cargo check -p alephcore` 通过后再删
- **分阶段进行**：清理放在实现完成后
- **保留 git 历史**：普通 git delete，方便回滚

---

## 8. 实施阶段

### Phase 1：StreamOrchestrator 骨架（1 周）

- 新增 `streaming/` 目录与 `StreamState` 状态机
- 实现 `AnswerLane`（基于现有 edit-based stream）
- `delivery.rs` 接入 orchestrator，保持向后兼容
- **验收**：`cargo test -p alephcore --lib telegram` 全绿

### Phase 2：Draft Lane + Reasoning Lane（1 周）

- 实现 `DraftLane`（含 Draft API 探测与降级）
- 实现 `ReasoningLane`（`<think>` 解析与渲染）
- `reply_emitter.rs` 不再丢弃 reasoning 内容
- **验收**：端到端在测试 bot 上跑通

### Phase 3：Status Reaction + Lane Tracker（1 周）

- 实现 `StatusReactionController` 并与执行引擎 hook 对接
- 实现 `LaneDeliveryTracker`
- **验收**：执行 tool 时能看到 🔧；reaction 事件可反查消息

### Phase 4：代码清理（3 天）

- 删除 `gateway/bridge/` 及残留引用
- **验收**：`cargo check -p alephcore` 通过；git diff 显示净删除

---

## 9. 关键设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| `Channel` trait 是否修改？ | **不修改** | 零影响其他 channel |
| Draft API 不可用时 | **自动降级 message-edit** | 兼容所有聊天场景 |
| Reasoning 最终处理 | **默认删除** | 保持聊天界面整洁 |
| 清理时机 | **最后阶段** | 防止过早删除导致调试困难 |
| 状态机实现 | **Rust enum** | 编译期+运行期双重安全 |

---

*本文档待审批，审批后进入实施计划阶段。*
