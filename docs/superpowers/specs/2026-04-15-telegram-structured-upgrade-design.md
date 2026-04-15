# Telegram 结构化升级设计文档

> 方案 B：结构化升级 + 核心追平
> 日期：2026-04-15
> 目标：学习 OpenClaw，追平核心体验，充分利用 Rust 类型安全与并发优势

---

## 1. 范围与架构

### 1.1 目标

- 补齐 Aleph Telegram 频道相对于 OpenClaw 的核心功能缺失
- 重构配置系统，支持多账号与层级继承
- 清理废弃的 bridge 代码，避免屎山堆积

### 1.2 纳入范围

1. **Telegram 功能升级**
   - Reasoning stream（实时推理预览 lane）
   - Sticker pipeline（缓存 + vision 描述 + 发送/搜索 action）
   - Poll support（创建 poll 的工具 action 与 poll answer 入站）
   - Status reaction controller（👀→🔧→✅ 状态机）
   - Error policy（`reply` vs `silent` + 按作用域覆盖 + cooldown）
   - Reaction notifications（入站 `message_reaction` 转系统事件）

2. **结构性升级**
   - 多账号支持（`accounts.*` 配置 + 运行时路由）
   - 配置层级：account → group → topic 继承

3. **代码清理**
   - 移除 `gateway/bridge/` 及关联的废弃代码
   - 移除 `approval_bridge.rs`

### 1.3 排除范围（YAGNI）

- Auto-topic-label（LLM 自动命名 topic）
- Thread bindings / ACP spawn from Telegram
- WhatsApp Business API
- 通用频道插件 SDK

### 1.4 架构原则

- `Channel` trait **不得修改**
- 所有 Telegram 新能力必须收敛在 `src/gateway/interfaces/telegram/` 内，不允许向 gateway core 泄漏
- 配置解析在 channel 边界完成，不在 gateway router 中处理
- 清理工作必须放在最后阶段，等新代码稳定后再执行

---

## 2. Telegram 功能升级

### 2.1 Reasoning Stream

**现状**：`reply_emitter.rs` 仅将 `<think>` 标签 strip 后丢弃。

**设计**：
- 在 `delivery.rs` 中引入 `ReasoningLane`，与现有的 answer stream 并行
- 当 `ChannelCapabilities` 标记 `stream_protocol: EditBased` 时，创建两个 `StreamingController` 实例：一个用于 answer，一个用于 reasoning
- 将 LLM 输出分段：reasoning block 进入 reasoning lane，answer 进入 answer lane
- reasoning 渲染为独立的 Telegram 消息（或就地编辑），前缀使用 `🤔 …`
- 最终化时，根据配置决定删除或保留 reasoning 消息（默认删除以保持聊天整洁）

**涉及文件**：`telegram/delivery.rs`、`telegram/handlers.rs`、`telegram/config.rs`

### 2.2 Sticker Pipeline

**现状**：`handlers.rs` 可能将 sticker file ID 作为附件传递，但缺少 vision 解释与缓存。

**设计**：
- 新增模块 `telegram/sticker.rs`
- 入站 sticker 时，通过 Telegram Bot API 下载文件，复用 Aleph 的 media/vision 基础设施获取描述，按 `file_unique_id` 缓存
- 当模型不支持 vision 时，将 sticker 附件替换为文本描述注入消息体
- 新增两个 tool action：`telegram_send_sticker`、`telegram_search_sticker`
- 缓存持久化在 `StateDatabase` 的 `sticker_descriptions` 表中

### 2.3 Poll Support

**现状**：无 poll 支持。

**设计**：
- 新增模块 `telegram/poll.rs`
- 暴露 tool `telegram_send_poll`，参数包括 `question`、`options`、`is_anonymous`、`allows_multiple_answers`、`open_period`
- 映射到 `teloxide::types::Poll` API
- 在 `StateDatabase` 存储 poll 元数据，供 agent 后续引用结果
- 入站 poll 回答（`PollAnswer` update）转换为 `InboundMessage`，metadata 带 `poll_answer` 变体

### 2.4 Status Reaction Controller

**现状**：Telegram channel 有 `react()` 但缺少工具生命周期到 emoji 反应的状态映射。

**设计**：
- 新增模块 `telegram/status_reaction.rs`
- 状态机：`Idle → Thinking(👀) → ToolActive(🔧 <tool_name>) → Compacting(🗜️) → Done(👍) / Error(👎)`
- 与 `execution_engine/run_loop.rs` 已有的事件回调对接
- 反应变更限流（最大 1 次/秒），避免 Telegram API spam
- API 失败时回退到 typing indicator（例如在缺少权限的群组中）

### 2.5 Error Policy

**现状**：仅有全局 `error_cooldown.rs`，无法按聊天或 topic 覆盖。

**设计**：
- 在 config 中增加 `ErrorPolicy` 枚举：`Reply`（发送错误文本）、`Silent`（静默丢弃）、`Once`（cooldown 窗口内仅发送一次）
- 解析优先级：topic config → group config → account config → 全局默认值
- 去重 key = `(account_id, chat_id, optional_thread_id)`
- 复用现有 `ErrorCooldown`，但使其具备 policy-aware 能力

### 2.6 Reaction Notifications

**现状**：`react()` 仅支持出站。

**设计**：
- 在 `mod.rs` 的 handler 树中增加 `Update::filter_message_reaction()` 分支
- 将入站反应转换为 `InboundMessage` 事件，metadata 携带 `reaction: Vec<String>`
- 使 agent 能够感知用户对历史消息的反应并做出回应

---

## 3. 配置层级与多账户支持

### 3.1 当前问题

现有 `TelegramConfig` 为扁平结构，无法同时运行多个 bot，也无法对单个群组或 topic 进行精细化配置。

### 3.2 新配置结构（向后兼容）

采用 **层级覆盖** 模型，Rust 中用 `Option<T>` + 合并函数实现：

```toml
[[channels]]
id = "telegram"
channel_type = "telegram"
enabled = true

[[channels.config.accounts]]
id = "main"
bot_token = "${TELEGRAM_BOT_TOKEN}"
default_agent = "default"

[[channels.config.accounts.groups]]
id = "my_group"
chat_id = -1001234567890
group_policy = "allowlist"

[[channels.config.accounts.groups.topics]]
id = "rust_topic"
thread_id = 42
agent = "rust_expert"
block_streaming = true
error_policy = "silent"
```

**继承规则**（由内而外，后者覆盖前者）：
1. Account 默认值
2. Group 值（如果匹配 `chat_id`）
3. Topic 值（如果匹配 `thread_id`）

任何字段为 `None` 时向上继承。

### 3.3 运行时解析器

新增 `telegram/config_resolver.rs`：
- `ResolvedConfig`：针对当前消息上下文的扁平化配置视图
- 启动时将层级树预解析为 `HashMap<(AccountId, ChatId, Option<ThreadId>), ResolvedConfig>`，运行时 O(1) 查表
- 避免每次消息都进行递归合并

### 3.4 多账户运行时

- `TelegramChannel` 内部维护 `Vec<BotInstance>`，每个实例 = `Bot + AccountConfig + 独立 polling loop`
- 入站消息路由：通过 `Update` 中的 `chat_id` 反查所属的 `BotInstance`
- 账号隔离：pairing 数据、offset 追踪、状态反应控制器均按 `account_id` 隔离存储

### 3.5 向后兼容

旧配置（无 `accounts`）自动提升为单个匿名 account：

```rust
if config.accounts.is_empty() {
    config.accounts.push(legacy_account_from_flat_config(config));
}
```

`allowed_users` / `allowed_groups` 迁移到该匿名 account 的 policy 字段中。

---

## 4. 清理与删除

### 4.1 待清理内容

- **`src/gateway/bridge/`**：`bridged_channel.rs`、`supervisor.rs`、`types.rs`、`mod.rs` — WhatsApp native 化后已无消费者
- **`src/gateway/handlers/approval_bridge.rs`**：bridge 时代的 approval 路由，Telegram 与 WhatsApp 均已使用原生 `ChannelApprovalCapability`
- **残留引用**：`justfile`、CI workflow、`Cargo.toml` workspace members 中可能仍存在的 `whatsapp-bridge` 引用

### 4.2 安全清理原则

- **先验证后删除**：删除后必须 `cargo check` 通过
- **分阶段进行**：清理放在 Phase 4，避免功能开发期间破坏既有结构
- **保留 git 历史**：通过普通 git delete，方便回滚

### 4.3 不清理的部分

- `gateway/transport/` — 可能被其他 channel 或调试工具使用
- `gateway/interfaces/webhook/` — 通用 webhook 通道，与本次无关

---

## 5. 实施阶段

### Phase 1：配置重构 + 多账户（3 周）

- 重构 `TelegramConfig` 为层级结构
- 实现 `config_resolver.rs`
- 多账户运行时：`Vec<BotInstance>` + 独立 polling loop
- 向后兼容：旧配置自动提升
- **验收标准**：`cargo test -p alephcore --lib telegram` 全绿；旧配置文件无需修改即可启动

### Phase 2：核心功能追平（4 周）

- Reasoning stream
- Sticker pipeline
- Poll support
- Error policy
- **验收标准**：每项功能都有单元测试；端到端在测试 bot 上跑通

### Phase 3：交互增强（3 周）

- Status reaction controller
- Reaction notifications
- 与 execution engine hook 对接
- **验收标准**：执行 tool 时能看到 🔧 反应；用户点 👍 时系统能收到事件

### Phase 4：清理旧代码（2 周）

- 确认 `gateway/bridge/` 无消费者后删除
- 删除 `approval_bridge.rs` 及相关引用
- 清理 `justfile`、CI、workspace 中的 bridge 残留
- **验收标准**：`cargo check -p alephcore` 通过；git diff 显示净删除代码行

---

## 6. 关键设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| `Channel` trait 是否修改？ | **不修改** | 保持对其他 channel 的零影响 |
| 配置解析放在哪？ | **Channel 边界** | 避免 router 逻辑膨胀 |
| 多账号 bot 实例化 | **Vec<BotInstance>** | 简单、线程安全、易于隔离失败 |
| 清理时机 | **最后阶段** | 防止过早删除导致开发期调试困难 |
| Reasoning 最终处理 | **默认删除** | 保持聊天界面整洁，用户一般不需要保留推理过程 |

---

## 7. 风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| `gateway/bridge/` 仍被隐藏代码引用 | 编译失败 | 删除前全局 grep + `cargo check` 验证 |
| 配置重构破坏旧用户配置 | 高 | 提供自动提升逻辑 + 启动期明确日志 |
| 多账号 polling 导致 rate limit | 中 | 使用 `apiThrottler` 模式，每个 bot 独立限流 |
| teloxide 版本不支持新 API | 中 | 升级前检查 `teloxide` changelog，必要时升级版本 |

---

*本文档已获设计审批，下一步将进入实施计划阶段。*
