---
date: 2026-04-04
topic: telegram-reliability-enhancement
---

# Telegram Channel 可靠性增强

## Problem Frame

Aleph 的 Telegram Channel 在 2026-03-27 完成了模块化重构（1,355 行单体拆分为 8 个模块 ~2,875 行），但在实际使用中暴露了以下可靠性问题：

- 用户快速连发多条消息 → 每条触发独立 LLM 调用（浪费 token + 体验差）
- 服务重启 → pending updates 被 `drop_pending_updates` 丢弃（丢消息）
- 网络抖动 → polling 恢复策略简单（现有 watchdog 可改进）

参考了 OpenClaw（TypeScript AI 助手）的 Telegram 实现作为先行方案，但不直接移植 — OpenClaw 面向多租户 SaaS 场景，其复杂度不适合 Aleph 的单用户自托管模型。本次优化的目标是**从 Aleph 自身的失败场景出发，借鉴 OpenClaw 的设计智慧，用 Rust 的并发优势实现最小可靠方案**。

## User Flow

```
用户快速发送 "帮我" → "查一下" → "今天天气"
          │
          ▼
   Telegram Bot API (3 个 Update)
          │
          ▼
   TelegramChannel (pure I/O, 逐条转发)
          │ 3 条 InboundMessage
          ▼
   Gateway::MessageCoalescer [NEW]
          │ 防抖等待 800ms（可配置）
          │ 合并为 1 条: "帮我查一下今天天气"
          ▼
   ExecutionEngine → 1 次 LLM 调用
          │
          ▼
   用户收到 1 条完整回复
```

## Requirements

**消息合并缓冲 (Message Coalescing)**

- R1. Gateway 层新增 `MessageCoalescer` 组件，在 `InboundMessageRouter` 的 `run_loop` 中插入缓冲阶段（在 spawn 独立任务之前）。注：当前 `run_loop` 对每条消息立即 spawn 任务，需改为先进入 coalescer 缓冲，flush 后再 spawn
- R2. 统一防抖窗口：默认 800ms（可配置），窗口内新消息重置计时器；上限 12 条片段或 50KB 文本。增加"早期 flush"启发：如果消息以问号/句号结尾且无后续消息在 200ms 内到达，提前 flush
- R3. 媒体组消息：Telegram 的 `media_group_id` 由 Channel 层传递到 InboundMessage metadata 中，coalescer 按 group_id 聚合，500ms 超时。前置条件：R0（InboundMessage metadata 机制）
- R4. （延后）转发消息特殊防抖窗口 — 首版使用统一的 R2 防抖窗口处理所有消息类型，观察实际效果后再决定是否需要转发专用窗口
- R5. 合并后的消息保留所有附件，文本按发送顺序拼接（换行分隔）
- R6. 防抖参数（窗口时长、上限）可通过 `GatewayConfig` 配置。接口保持最小化 — 当前只有 Telegram 使用 coalescing，不预设其他 Channel 的扩展点

**前置条件**

- R0. `InboundMessage` 新增 `metadata: HashMap<String, serde_json::Value>` 字段（或 typed enum），用于传递平台特有信息（`media_group_id`、`forward` 标记等）。同步修改 TelegramChannel 的 `convert_message` 提取 `media_group_id` 和 `forward_origin` 字段

**Update 去重与 Offset 持久化**

- R7. TelegramChannel 实现内部维护一个 `update_id` 水位线（watermark），只处理 > watermark 的 update（水位线是 Telegram polling 特有概念，不涉及 Channel trait 变更）
- R8. 水位线持久化到数据库（SQLite），字段：`channel_id`, `last_update_id`, `bot_id`, `updated_at`
- R9. 水位线采用"完成后写入"策略：coalesced batch flush 并成功提交到 ExecutionEngine 后，将该 batch 中最大的 `update_id` 写入水位线。崩溃恢复时可能重复处理已 coalesce 但未 flush 的消息 — 这是可接受的，因为 LLM 调用本身是幂等的（相同输入产生新回复，不会产生副作用）
- R10. 启动时从数据库加载水位线，传入 `getUpdates` 的 `offset` 参数。不再使用 `drop_pending_updates`。首次启动（数据库无记录）时，执行一次 `drop_pending_updates` 并将返回的 offset 写入数据库作为初始水位线，避免处理历史垃圾消息

**Chat 级消息序列化（延后 — coalescing 提供事实上的序列化）**

- R11. （延后）coalescing（R1-R6）已将同一 conversation 的快速连续消息合并为一条，对单用户场景提供了事实上的序列化保障。显式的 per-conversation 序列化队列/信号量在观察到实际竞态问题后再实现
- R12. （延后）如需实现，在 Gateway 层使用 per-conversation_id 的有序任务队列
- R13. （延后）序列化粒度为 conversation_id（含 forum topic），不是 chat_id

**Polling 健壮性增强**

- R14. Stall 检测：如果 polling 循环连续 90 秒未收到任何 HTTP 响应（空 update 列表算作正常响应），且并发的 `get_me()` 健康探针也失败，才判定为 stall 并触发传输重建。注：现有 `polling.rs` 已有 watchdog（120s `get_me()` 探针 + `MAX_CONSECUTIVE_FAILURES=3` + 指数退避 `5*2^n` 上限 60s），本次是在此基础上增强，非从零构建
- R15. 指数退避重启：polling 异常退出后，2s → 3.6s → 6.5s → ... → 30s（factor 1.8, jitter 0.25），替代现有的 `5*2^n` 上限 60s 策略
- R16. 健康恢复重置：polling 正常运行超过 5 分钟后，退避计数器归零（保留现有 `maybe_reset_attempts` 逻辑）
- R17. 组合判断：stall 检测 + 健康探针双重确认，避免 idle 场景误判。只有两者同时失败才重建传输

**错误策略增强**

- R18. Per-conversation 错误冷却机制，区分错误类型：永久性错误（403 Forbidden、chat deleted）→ 冷却 4 小时；可重试错误（网络超时、429 rate limit）→ 指数退避（复用 R15 策略），不触发长冷却。单用户可通过 `/reset_cooldown` 工具命令手动清除冷却状态
- R19. SendChatAction 断路器：连续 10 次 401 错误后暂停 typing indicator 发送，成功后恢复
- R20. 错误冷却状态存储在内存中（不需要持久化），进程重启自然重置

**可观测性**

- R21. 所有新增子系统（coalescing、offset 持久化、错误冷却、stall 检测）的关键事件使用结构化日志（tracing）记录：coalesce flush（合并了几条、等待了多久）、watermark 更新、cooldown 激活/解除、stall 检测触发/恢复
- R22. 优雅关闭：收到 SIGTERM 时，flush 所有 pending coalescing buffer 后再停止 polling loop。硬崩溃（SIGKILL）依赖 R9 的 watermark 重放机制恢复

## Success Criteria

- 用户连发 3 条短消息，只触发 1 次 LLM 调用（R1-R6 验证）
- 服务重启后，重启前 pending 的消息被正确处理而非丢弃（R7-R10 验证）
- （延后）同一对话的消息通过 coalescing 获得事实上的顺序保障（R11-R13 延后，coalescing 提供间接验证）
- 模拟网络中断 2 分钟后恢复，polling 在 10 秒内自动恢复（R14-R17 验证，stall 检测窗口为 90 秒）
- 同一对话连续发送失败 5 次后，不再重试直到冷却期过（R18-R20 验证）

## Scope Boundaries

- **不包含**: 实时流式响应（Draft Stream）— 属于体验层优化，下一阶段处理
- **不包含**: 会话绑定（Thread Bindings）— 依赖多代理系统进一步成熟
- **不包含**: 执行审批流 — 需要先设计通用的审批 trait
- **不包含**: Webhook 模式完善 — polling 模式足够覆盖当前部署场景
- **不包含**: 多账户支持 — 当前只需要单 bot
- **不修改**: Channel trait 接口 — 所有变更向后兼容
- **不修改**: 现有的 AccessController / DmPolicy / GroupPolicy — 已经足够好

## Key Decisions

- **消息合并在 Gateway 层**: Channel 只负责传递 raw InboundMessage + metadata，Gateway 负责 coalescing。注意：coalescing 是"什么构成一次 LLM 请求"的边界判断，在严格意义上属于业务语义而非纯 I/O 路由。但将其放在 Channel 层会违反 R4（Interface 纯 I/O），放在 ExecutionEngine 又过于靠后（此时消息已分别进入处理管线）。Gateway 是当前最务实的位置。当前只有 Telegram 使用此能力 — 保持接口最小化，不预设通用扩展
- **Offset 存数据库**: 与 Aleph 现有的 SQLite 状态管理统一，事务安全，不额外引入文件系统状态
- **不照搬 OpenClaw 的 grammY middleware 模式**: Rust 的 tokio 并发原语（select!, JoinSet, Semaphore）比 JS 的 middleware 链更适合表达这些模式
- **Stall 检测 + 健康探针组合判断**: 避免 idle 场景误判（只有 stall + 探针失败才重建），比 OpenClaw 的单一 stall 检测更保守也更准确

## Dependencies / Assumptions

- Aleph 的 SQLite 数据库使用幂等函数作为 migration 机制（`src/resilience/database/migration.rs` 中的 `migrate_add_*` 系列函数，启动时调用）。新增 `channel_state` 表需编写 `migrate_add_channel_state()` 函数并接入 `StateDatabase` 初始化路径
- `InboundMessage` struct 当前**没有** metadata 字段 — R0 作为前置条件解决此问题
- Gateway 的 `InboundMessageRouter` 的 `run_loop` 当前立即 spawn 独立任务处理每条消息 — 插入 coalescing 需要改变此模式（先缓冲再 spawn），这是 R1 的核心架构变更

## Outstanding Questions

### Resolve Before Planning

（无阻塞性问题 — 所有产品决策已确认）

### Deferred to Planning

- [Affects R0][Technical] InboundMessage metadata 机制设计：`HashMap<String, serde_json::Value>` vs typed enum vs 两者混合
- [Affects R0][Needs research] teloxide 的 Message struct 是否暴露 `media_group_id` 和 `forward_origin` 字段，以及如何提取
- [Affects R1][Technical] `InboundMessageRouter::run_loop` 的 coalescer 插入方案 — 缓冲阶段如何与现有的 spawn-per-message 模式共存
- [Affects R8][Technical] SQLite `channel_state` 表结构和索引设计
- [Affects R14-R17][Technical] 如何在 teloxide 的 long-polling 循环中插入 stall 检测（可能需要 tokio::select! 包装）

## Next Steps

→ `/ce:plan` for structured implementation planning
