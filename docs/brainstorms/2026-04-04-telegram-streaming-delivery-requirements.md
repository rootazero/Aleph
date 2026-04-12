---
date: 2026-04-04
topic: telegram-streaming-delivery
---

# Telegram 流式草稿投递 (Edit-Based Streaming)

## Problem Frame

Aleph 当前对 Telegram 消息采用"缓冲-一次性发送"模式：LLM 生成完整回复后才发送给用户。对于长回复（数百 token），用户需要等待数秒到数十秒才能看到任何内容，体验远逊于 ChatGPT / OpenClaw 等支持流式输出的产品。

基础设施已完备：`StreamingController` 状态机、`ReplyEmitter` 的 edit-based 流式路径、Telegram `edit_message()` 均已实现。**唯一缺失的是启用开关和 Telegram 特有的边缘处理。**

## User Flow

```
用户发送 "解释 Rust 的所有权机制"
          │
          ▼
   ExecutionEngine → LLM streaming
          │ token chunks
          ▼
   ReplyEmitter (stream_enabled=true)
          │
          ├─ 30 chars accumulated → SendInitial
          │     bot.send_message("Rust 的所有权...")
          │     → message_id = 42
          │
          ├─ debounce 800ms elapsed → Edit
          │     bot.edit_message_text(42, "Rust 的所有权机制是...")
          │
          ├─ ... 多次 edit ...
          │
          └─ LLM 完成 → EditFinal
                bot.edit_message_text(42, "完整回复 + 附件")
```

## Requirements

**启用 Edit-Based Streaming**

- R1. Telegram `ChannelCapabilities` 的 `stream_protocol` 从 `Default::default()` (None) 改为 `StreamProtocol::EditBased`，准确声明能力
- R2. `TelegramConfig` 新增 `streaming: StreamingOptions` 字段，包含 `enabled: bool` (默认 true)、`debounce_ms: u64` (默认 800)、`min_initial_chars: usize` (默认 30)
- R3. 当 Telegram channel 的 streaming 开启时，构建 `ReplyEmitter` 时强制 `stream_enabled = true`，覆盖全局 `output_mode` 设置。这允许 Telegram 独立控制流式行为而不影响其他 channel

**Telegram 边缘处理**

- R4. `edit_message()` 捕获 Telegram `MESSAGE_NOT_MODIFIED` (400 Bad Request) 错误，静默忽略而非返回错误。这在 debounce 窗口内未产生新 token 时会触发
- R5. 流式 edit 过程中，如果累积文本超过 Telegram 4096 字符限制：停止 edit 当前消息，发送新消息继续流式，后续 edit 切换到新 message_id。StreamingController 需新增 `overflow` 状态支持此场景
- R6. 流式 edit 开始后（SendInitial 成功），在 `ReplyEmitter` 的 `StreamAction::SendInitial` 分支中调用 `self.typing_cancel.cancel()` 取消 typing indicator。用户已能看到文字逐步出现，typing indicator 变得多余且会在 UI 上闪烁

**UX 增强**

- R7. 流式 edit 的中间文本末尾追加光标符号 "▍"（U+258D），final edit 时移除。给用户"正在生成"的视觉反馈
- R8. 首次发送（SendInitial）时追加尾部 "▍"；每次 Edit 时更新文本 + "▍"；EditFinal 时发送纯净文本（无光标）

**Debounce 调优**

- R9. Telegram 流式的 debounce 默认 800ms（而非 StreamingController 的全局默认 300ms）。Telegram 的 edit API 比 send 更严格限速，800ms 在实测中是安全阈值
- R10. SendInitial 的 min_initial_chars 默认 30，与当前 StreamingController 一致。过小会导致首条消息太短（一两个词），过大会延迟首次可见时间

## Success Criteria

- `output_mode: "typewriter"` 或 Telegram streaming 启用时，用户看到消息逐步出现而非一次性弹出
- 长回复（>4096 字符）不会因 edit 失败而丢失内容
- 流式过程中无 `MESSAGE_NOT_MODIFIED` 错误日志
- 流式过程中无 typing indicator 闪烁

## Scope Boundaries

- **不包含**: Lane 模式（reasoning lane / draft lane 分离）— 属于更大的 UI 重构
- **不包含**: 流式语音（TTS streaming）— voice mode 独立处理
- **不包含**: StreamingController 核心状态机重写 — 仅新增 `Overflow` 变体和对应的 `max_message_length` 感知逻辑（R5 的定向扩展，非核心重构）
- **不包含**: 其他 channel 的流式启用 — 本次只针对 Telegram

## Key Decisions

- **Per-channel streaming 配置覆盖全局**: Telegram 的 `streaming.enabled` 优先于全局 `output_mode`。原因：不同 channel 对 edit 的支持和限速差异巨大，全局一刀切不合适
- **Debounce 800ms 而非 300ms**: Telegram Bot API 的 edit 限速比普通 send 更严格。300ms 在高速生成时会频繁触发 429。800ms 是 OpenClaw 验证过的安全值
- **光标符号用 "▍" 而非 "..."**: "▍" 是业界标准的流式光标（ChatGPT、OpenClaw 均使用），视觉上更像"正在打字"而非"省略"
- **超长溢出发新消息**: 不截断、不中断流式。用户看到完整内容，只是分成多条消息

## Dependencies / Assumptions

- `StreamingController` 的 `push_chunk` / `poll_action` / `finalize` 接口不变，R5 通过新增 `overflow` 变体扩展 `StreamAction` enum
- `ReplyEmitter` 的 `channel_registry.edit()` 调用链已通过 `ChannelRegistry → Channel::edit() → delivery::edit_message()` 完整连接
- Telegram Bot API 的 `editMessageText` 在相同内容时返回 400 "Bad Request: message is not modified" — 这是已知行为，非 bug

## Outstanding Questions

### Resolve Before Planning

（无阻塞性问题 — 所有产品决策已确认）

### Deferred to Planning

- [Affects R3][Technical] ReplyEmitter 构建时如何获取 channel 的 streaming config — 需要在 `session_scheduler.rs` 或 `executor.rs` 的构建路径中注入
- [Affects R5][Technical] StreamingController overflow 状态机设计 — `StreamAction::Overflow(String)` 还是在 `Edit` 变体中增加溢出标记
- [Affects R4][Needs research] teloxide 的 `edit_message_text` 返回 `MESSAGE_NOT_MODIFIED` 时是 `ApiError` 的哪个变体 — 需要检查 teloxide 源码确认匹配方式。注意：当前 `classify_error` 会将所有 "Bad Request" 归类为 `Rejected`（永久失败），需为 MESSAGE_NOT_MODIFIED 增加特殊处理
- [Affects R9][Technical] `ReplyEmitterConfig` 当前无 `debounce_ms` 和 `min_initial_chars` 字段 — 需新增这两个字段（默认 300/30），per-channel 覆盖时由 executor 从 `TelegramConfig.streaming` 注入
- [Affects R7/R8][Technical] 光标符号 "▍" 的追加时机 — 需在 HTML 转换之后追加（避免被 `MessageFormatter` 转义），在 `ReplyEmitter` 的 send/edit 调用前拼接

## Next Steps

→ `/ce:plan` for structured implementation planning
