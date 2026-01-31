# Telegram 审批集成实现计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 实现 Danger 级别命令通过 Telegram 进行远程审批，支持 inline keyboard 一键操作。

**Architecture:** 新增 `ApprovalBridge` 组件连接 `ExecApprovalManager` 和 `ChannelRegistry`，扩展 Telegram channel 支持 inline keyboard 和 callback query。审批消息带按钮，点击后自动解决审批请求。

**Tech Stack:** teloxide (InlineKeyboardMarkup, CallbackQuery), tokio channels

---

## 现有模块分析

**已实现：**
- ✅ `TelegramChannel` - 消息发送/接收 (text, media, reply)
- ✅ `ExecApprovalManager` - 审批生命周期管理 (create, wait, resolve)
- ✅ `ExecApprovalForwarder` - 消息格式化和解析
- ✅ `SecurityKernel` - 风险评估 (Blocked/Danger/Caution/Safe)

**缺失：**
- ❌ Telegram inline keyboard 支持
- ❌ Callback query 处理
- ❌ ApprovalManager ↔ Channel 连接
- ❌ 审批消息编辑（显示结果）

---

## Task 1: 添加 Telegram InlineKeyboard 支持

**Files:**
- Modify: `core/src/gateway/channels/telegram/mod.rs`
- Modify: `core/src/gateway/channel.rs` (添加 SendOptions)

**Step 1: 扩展 OutboundMessage 支持 inline keyboard**

在 `/Volumes/TBU4/Workspace/Aether/core/src/gateway/channel.rs` 添加：

```rust
/// Inline keyboard button
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineButton {
    /// Button text
    pub text: String,
    /// Callback data (sent back when clicked)
    pub callback_data: String,
}

/// Inline keyboard row (buttons in a row)
pub type InlineKeyboardRow = Vec<InlineButton>;

/// Inline keyboard markup
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InlineKeyboard {
    /// Rows of buttons
    pub rows: Vec<InlineKeyboardRow>,
}

impl InlineKeyboard {
    /// Create empty keyboard
    pub fn new() -> Self {
        Self { rows: Vec::new() }
    }

    /// Add a row of buttons
    pub fn row(mut self, buttons: Vec<InlineButton>) -> Self {
        self.rows.push(buttons);
        self
    }

    /// Add a single button as a new row
    pub fn button(self, text: impl Into<String>, callback_data: impl Into<String>) -> Self {
        self.row(vec![InlineButton {
            text: text.into(),
            callback_data: callback_data.into(),
        }])
    }
}
```

**Step 2: 扩展 OutboundMessage**

在 `OutboundMessage` 结构体中添加 `inline_keyboard` 字段：

```rust
pub struct OutboundMessage {
    // ... existing fields ...
    /// Optional inline keyboard
    pub inline_keyboard: Option<InlineKeyboard>,
}
```

**Step 3: 在 TelegramChannel.send() 中处理 inline keyboard**

在 `/Volumes/TBU4/Workspace/Aether/core/src/gateway/channels/telegram/mod.rs` 的 `send` 方法中添加：

```rust
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

// 在 send 方法内部，构建消息时
let mut request = bot.send_message(chat_id, &message.text);

// 添加 inline keyboard
if let Some(ref keyboard) = message.inline_keyboard {
    let markup = InlineKeyboardMarkup::new(
        keyboard.rows.iter().map(|row| {
            row.iter().map(|btn| {
                InlineKeyboardButton::callback(&btn.text, &btn.callback_data)
            }).collect::<Vec<_>>()
        }).collect::<Vec<_>>()
    );
    request = request.reply_markup(markup);
}
```

**Step 4: 添加测试**

```rust
#[test]
fn test_inline_keyboard_builder() {
    let keyboard = InlineKeyboard::new()
        .row(vec![
            InlineButton { text: "Allow Once".into(), callback_data: "approve:abc:once".into() },
            InlineButton { text: "Allow Always".into(), callback_data: "approve:abc:always".into() },
        ])
        .button("Deny", "approve:abc:deny");

    assert_eq!(keyboard.rows.len(), 2);
    assert_eq!(keyboard.rows[0].len(), 2);
    assert_eq!(keyboard.rows[1].len(), 1);
}
```

**Step 5: Commit**

```bash
git add core/src/gateway/channel.rs core/src/gateway/channels/telegram/mod.rs
git commit -m "feat(telegram): add inline keyboard support"
```

---

## Task 2: 添加 Callback Query 处理

**Files:**
- Modify: `core/src/gateway/channels/telegram/mod.rs`
- Modify: `core/src/gateway/channel.rs` (添加 CallbackQuery 类型)

**Step 1: 定义 CallbackQuery 类型**

在 `/Volumes/TBU4/Workspace/Aether/core/src/gateway/channel.rs` 添加：

```rust
/// Callback query from inline keyboard button click
#[derive(Debug, Clone)]
pub struct CallbackQuery {
    /// Unique query ID
    pub id: String,
    /// User who clicked
    pub user_id: UserId,
    /// Chat where button was clicked
    pub chat_id: ConversationId,
    /// Message containing the button
    pub message_id: MessageId,
    /// Callback data from the button
    pub data: String,
}

/// Channel trait extension for callback handling
#[async_trait]
pub trait CallbackHandler: Send + Sync {
    /// Handle callback query and return response text
    async fn answer_callback(&self, query_id: &str, text: Option<&str>) -> ChannelResult<()>;
}
```

**Step 2: 扩展 TelegramChannel 处理 callback query**

在 Telegram channel 的 message handler 中添加 callback query 分支：

```rust
use teloxide::types::CallbackQuery as TgCallbackQuery;

// 在 start_polling 的 dispatcher 中添加
.branch(
    Update::filter_callback_query().endpoint(
        |bot: Bot, q: TgCallbackQuery, callback_tx: mpsc::Sender<CallbackQuery>| async move {
            if let Some(data) = q.data {
                let query = CallbackQuery {
                    id: q.id.clone(),
                    user_id: UserId::new(q.from.id.to_string()),
                    chat_id: q.message.as_ref()
                        .map(|m| ConversationId::new(m.chat.id.to_string()))
                        .unwrap_or_default(),
                    message_id: q.message.as_ref()
                        .map(|m| MessageId::new(m.id.to_string()))
                        .unwrap_or_default(),
                    data,
                };
                let _ = callback_tx.send(query).await;
            }
            // Answer callback to remove loading state
            bot.answer_callback_query(&q.id).await?;
            Ok(())
        },
    ),
)
```

**Step 3: 添加 callback receiver 到 TelegramChannel**

```rust
pub struct TelegramChannel {
    // ... existing fields ...
    /// Callback query sender
    callback_tx: mpsc::Sender<CallbackQuery>,
    /// Callback query receiver (taken on first call)
    callback_rx: Option<mpsc::Receiver<CallbackQuery>>,
}

impl TelegramChannel {
    /// Take the callback receiver (can only be called once)
    pub fn take_callback_receiver(&mut self) -> Option<mpsc::Receiver<CallbackQuery>> {
        self.callback_rx.take()
    }
}
```

**Step 4: 实现 CallbackHandler trait**

```rust
#[async_trait]
impl CallbackHandler for TelegramChannel {
    async fn answer_callback(&self, query_id: &str, text: Option<&str>) -> ChannelResult<()> {
        #[cfg(feature = "telegram")]
        {
            let bot = self.bot.as_ref().ok_or(ChannelError::NotConnected)?;
            let mut req = bot.answer_callback_query(query_id);
            if let Some(t) = text {
                req = req.text(t);
            }
            req.await.map_err(|e| ChannelError::SendFailed(e.to_string()))?;
        }
        Ok(())
    }
}
```

**Step 5: Commit**

```bash
git add core/src/gateway/channel.rs core/src/gateway/channels/telegram/mod.rs
git commit -m "feat(telegram): add callback query handling"
```

---

## Task 3: 创建 ApprovalBridge 连接组件

**Files:**
- Create: `core/src/exec/bridge.rs`
- Modify: `core/src/exec/mod.rs`

**Step 1: 创建 bridge.rs**

创建 `/Volumes/TBU4/Workspace/Aether/core/src/exec/bridge.rs`：

```rust
//! Approval Bridge - connects ExecApprovalManager with chat channels.
//!
//! Listens for new approval requests and forwards them to configured channels.
//! Handles callback responses to resolve approvals.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use crate::gateway::channel::{
    CallbackQuery, ChannelRegistry, ConversationId, InlineButton, InlineKeyboard,
    MessageId, OutboundMessage,
};

use super::forwarder::{ExecApprovalForwarder, ForwardTarget};
use super::manager::{ExecApprovalManager, ExecApprovalRecord};
use super::socket::ApprovalDecisionType;

/// Tracks sent approval messages for editing
#[derive(Debug, Clone)]
pub struct SentApprovalMessage {
    pub approval_id: String,
    pub channel: String,
    pub chat_id: ConversationId,
    pub message_id: MessageId,
}

/// Bridge between ExecApprovalManager and chat channels
pub struct ApprovalBridge {
    manager: Arc<ExecApprovalManager>,
    forwarder: ExecApprovalForwarder,
    channel_registry: Arc<ChannelRegistry>,
    /// Track sent messages for editing
    sent_messages: Arc<RwLock<HashMap<String, Vec<SentApprovalMessage>>>>,
}

impl ApprovalBridge {
    /// Create a new bridge
    pub fn new(
        manager: Arc<ExecApprovalManager>,
        forwarder: ExecApprovalForwarder,
        channel_registry: Arc<ChannelRegistry>,
    ) -> Self {
        Self {
            manager,
            forwarder,
            channel_registry,
            sent_messages: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Build inline keyboard for approval
    pub fn build_approval_keyboard(approval_id: &str) -> InlineKeyboard {
        InlineKeyboard::new()
            .row(vec![
                InlineButton {
                    text: "✅ Allow Once".into(),
                    callback_data: format!("approve:{}:once", approval_id),
                },
                InlineButton {
                    text: "✅ Allow Always".into(),
                    callback_data: format!("approve:{}:always", approval_id),
                },
            ])
            .button("❌ Deny", format!("approve:{}:deny", approval_id))
    }

    /// Parse callback data into (approval_id, decision)
    pub fn parse_callback(data: &str) -> Option<(String, ApprovalDecisionType)> {
        let parts: Vec<&str> = data.split(':').collect();
        if parts.len() != 3 || parts[0] != "approve" {
            return None;
        }

        let approval_id = parts[1].to_string();
        let decision = match parts[2] {
            "once" => ApprovalDecisionType::AllowOnce,
            "always" => ApprovalDecisionType::AllowAlways,
            "deny" => ApprovalDecisionType::Deny,
            _ => return None,
        };

        Some((approval_id, decision))
    }

    /// Send approval request to a target
    pub async fn send_approval_request(
        &self,
        record: &ExecApprovalRecord,
        target: &ForwardTarget,
    ) -> Result<SentApprovalMessage, String> {
        let channel = self
            .channel_registry
            .get(&target.channel)
            .await
            .ok_or_else(|| format!("Channel not found: {}", target.channel))?;

        let message = self.forwarder.format_request(record);
        let keyboard = Self::build_approval_keyboard(&record.id);

        let outbound = OutboundMessage {
            conversation_id: ConversationId::new(&target.target),
            text: message.text,
            inline_keyboard: Some(keyboard),
            ..Default::default()
        };

        let result = channel.send(outbound).await
            .map_err(|e| format!("Failed to send: {}", e))?;

        Ok(SentApprovalMessage {
            approval_id: record.id.clone(),
            channel: target.channel.clone(),
            chat_id: ConversationId::new(&target.target),
            message_id: result.message_id,
        })
    }

    /// Handle callback query (button click)
    pub async fn handle_callback(&self, query: CallbackQuery) -> Result<String, String> {
        let (approval_id, decision) = Self::parse_callback(&query.data)
            .ok_or_else(|| "Invalid callback data".to_string())?;

        // Resolve the approval
        self.manager
            .resolve(&approval_id, decision.clone())
            .await
            .map_err(|e| format!("Failed to resolve: {}", e))?;

        let response = match decision {
            ApprovalDecisionType::AllowOnce => "✅ Allowed (once)",
            ApprovalDecisionType::AllowAlways => "✅ Allowed (always)",
            ApprovalDecisionType::Deny => "❌ Denied",
        };

        info!(
            approval_id = %approval_id,
            decision = ?decision,
            user = %query.user_id,
            "Approval resolved via callback"
        );

        Ok(response.to_string())
    }

    /// Update approval message after resolution
    pub async fn update_approval_message(
        &self,
        approval_id: &str,
        decision: ApprovalDecisionType,
        resolved_by: &str,
    ) -> Result<(), String> {
        let messages = self.sent_messages.read().await;
        let sent = messages.get(approval_id).ok_or("No sent messages found")?;

        let status_line = match decision {
            ApprovalDecisionType::AllowOnce => format!("✅ **Allowed** (once) by {}", resolved_by),
            ApprovalDecisionType::AllowAlways => format!("✅ **Allowed** (always) by {}", resolved_by),
            ApprovalDecisionType::Deny => format!("❌ **Denied** by {}", resolved_by),
        };

        for msg in sent {
            if let Some(channel) = self.channel_registry.get(&msg.channel).await {
                // Edit message to show result (remove keyboard)
                let _ = channel.edit(
                    msg.chat_id.clone(),
                    msg.message_id.clone(),
                    Some(status_line.clone()),
                    None, // Remove inline keyboard
                ).await;
            }
        }

        Ok(())
    }

    /// Track a sent approval message
    pub async fn track_sent_message(&self, msg: SentApprovalMessage) {
        let mut messages = self.sent_messages.write().await;
        messages
            .entry(msg.approval_id.clone())
            .or_insert_with(Vec::new)
            .push(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_callback_allow_once() {
        let result = ApprovalBridge::parse_callback("approve:abc123:once");
        assert!(result.is_some());
        let (id, decision) = result.unwrap();
        assert_eq!(id, "abc123");
        assert!(matches!(decision, ApprovalDecisionType::AllowOnce));
    }

    #[test]
    fn test_parse_callback_allow_always() {
        let result = ApprovalBridge::parse_callback("approve:xyz789:always");
        assert!(result.is_some());
        let (id, decision) = result.unwrap();
        assert_eq!(id, "xyz789");
        assert!(matches!(decision, ApprovalDecisionType::AllowAlways));
    }

    #[test]
    fn test_parse_callback_deny() {
        let result = ApprovalBridge::parse_callback("approve:test:deny");
        assert!(result.is_some());
        let (_, decision) = result.unwrap();
        assert!(matches!(decision, ApprovalDecisionType::Deny));
    }

    #[test]
    fn test_parse_callback_invalid() {
        assert!(ApprovalBridge::parse_callback("invalid").is_none());
        assert!(ApprovalBridge::parse_callback("approve:only_two").is_none());
        assert!(ApprovalBridge::parse_callback("other:id:once").is_none());
    }

    #[test]
    fn test_build_approval_keyboard() {
        let keyboard = ApprovalBridge::build_approval_keyboard("test123");
        assert_eq!(keyboard.rows.len(), 2);
        assert_eq!(keyboard.rows[0].len(), 2); // Allow Once, Allow Always
        assert_eq!(keyboard.rows[1].len(), 1); // Deny
        assert!(keyboard.rows[0][0].callback_data.contains("test123"));
    }
}
```

**Step 2: 更新 mod.rs 导出**

在 `/Volumes/TBU4/Workspace/Aether/core/src/exec/mod.rs` 添加：

```rust
pub mod bridge;

pub use bridge::{ApprovalBridge, SentApprovalMessage};
```

**Step 3: 运行测试**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test exec::bridge::tests
```

**Step 4: Commit**

```bash
git add core/src/exec/bridge.rs core/src/exec/mod.rs
git commit -m "feat(exec): add ApprovalBridge for channel integration"
```

---

## Task 4: 实现 Gateway 审批事件监听

**Files:**
- Create: `core/src/gateway/handlers/approval_bridge.rs`
- Modify: `core/src/gateway/handlers/mod.rs`

**Step 1: 创建审批事件处理器**

创建 `/Volumes/TBU4/Workspace/Aether/core/src/gateway/handlers/approval_bridge.rs`：

```rust
//! Gateway handler for approval bridge events.
//!
//! Listens for new approval requests from ExecApprovalManager
//! and forwards them to configured Telegram channels.

use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::exec::{
    ApprovalBridge, ExecApprovalForwarder, ExecApprovalManager, ExecApprovalRecord,
    ForwardTarget, ForwarderConfig,
};
use crate::gateway::channel::{CallbackQuery, ChannelRegistry};

/// Approval bridge handler for Gateway
pub struct ApprovalBridgeHandler {
    bridge: Arc<ApprovalBridge>,
    config: ForwarderConfig,
}

impl ApprovalBridgeHandler {
    /// Create a new handler
    pub fn new(
        manager: Arc<ExecApprovalManager>,
        channel_registry: Arc<ChannelRegistry>,
        config: ForwarderConfig,
    ) -> Self {
        let forwarder = ExecApprovalForwarder::new(config.clone(), manager.clone());
        let bridge = Arc::new(ApprovalBridge::new(manager, forwarder, channel_registry));

        Self { bridge, config }
    }

    /// Get the bridge reference
    pub fn bridge(&self) -> Arc<ApprovalBridge> {
        self.bridge.clone()
    }

    /// Forward an approval request to configured targets
    pub async fn forward_approval(&self, record: &ExecApprovalRecord) {
        // Get targets based on config
        let targets = self.get_forward_targets(record);

        for target in targets {
            match self.bridge.send_approval_request(record, &target).await {
                Ok(sent) => {
                    info!(
                        approval_id = %record.id,
                        channel = %target.channel,
                        target = %target.target,
                        "Approval request forwarded"
                    );
                    self.bridge.track_sent_message(sent).await;
                }
                Err(e) => {
                    warn!(
                        approval_id = %record.id,
                        channel = %target.channel,
                        error = %e,
                        "Failed to forward approval"
                    );
                }
            }
        }
    }

    /// Handle callback query from channel
    pub async fn handle_callback(&self, query: CallbackQuery) -> Result<String, String> {
        self.bridge.handle_callback(query).await
    }

    /// Get forward targets for a record
    fn get_forward_targets(&self, record: &ExecApprovalRecord) -> Vec<ForwardTarget> {
        use crate::exec::ForwardMode;

        let mut targets = Vec::new();

        match self.config.mode {
            ForwardMode::Session | ForwardMode::Both => {
                // Parse session key for channel/target
                if let Some((channel, target)) = parse_session_target(&record.session_key) {
                    targets.push(ForwardTarget { channel, target });
                }
            }
            _ => {}
        }

        match self.config.mode {
            ForwardMode::Targets | ForwardMode::Both => {
                targets.extend(self.config.targets.clone());
            }
            _ => {}
        }

        targets
    }
}

/// Parse session key to extract channel and target
/// Example: "agent:main:telegram:dm:12345" -> ("telegram", "12345")
fn parse_session_target(session_key: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = session_key.split(':').collect();

    // Look for channel type in session key
    for (i, part) in parts.iter().enumerate() {
        match *part {
            "telegram" | "discord" | "imessage" | "slack" => {
                // Next parts should be type and target
                if i + 2 < parts.len() {
                    return Some((part.to_string(), parts[i + 2].to_string()));
                }
            }
            _ => continue,
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_session_target_telegram_dm() {
        let result = parse_session_target("agent:main:telegram:dm:12345");
        assert_eq!(result, Some(("telegram".into(), "12345".into())));
    }

    #[test]
    fn test_parse_session_target_discord_group() {
        let result = parse_session_target("agent:main:discord:group:guild123");
        assert_eq!(result, Some(("discord".into(), "guild123".into())));
    }

    #[test]
    fn test_parse_session_target_no_channel() {
        let result = parse_session_target("agent:main:main");
        assert_eq!(result, None);
    }
}
```

**Step 2: 更新 handlers/mod.rs**

在 `/Volumes/TBU4/Workspace/Aether/core/src/gateway/handlers/mod.rs` 添加：

```rust
pub mod approval_bridge;

pub use approval_bridge::ApprovalBridgeHandler;
```

**Step 3: 运行测试**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test gateway::handlers::approval_bridge::tests
```

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/approval_bridge.rs core/src/gateway/handlers/mod.rs
git commit -m "feat(gateway): add ApprovalBridgeHandler for event forwarding"
```

---

## Task 5: 集成 Callback 处理到 Telegram Channel

**Files:**
- Modify: `core/src/gateway/channels/telegram/mod.rs`

**Step 1: 在 TelegramChannel 中集成 callback 处理**

修改 `start_polling` 方法，添加 callback query 处理：

```rust
// 在 dispatcher 构建中添加 callback_query handler
let callback_tx = self.callback_tx.clone();

let handler = dptree::entry()
    .branch(Update::filter_message().endpoint(message_handler))
    .branch(Update::filter_callback_query().endpoint(
        move |bot: Bot, q: teloxide::types::CallbackQuery| {
            let tx = callback_tx.clone();
            async move {
                if let Some(data) = q.data.clone() {
                    let chat_id = q.message.as_ref()
                        .map(|m| m.chat.id.to_string())
                        .unwrap_or_default();
                    let msg_id = q.message.as_ref()
                        .map(|m| m.id.to_string())
                        .unwrap_or_default();

                    let query = CallbackQuery {
                        id: q.id.clone(),
                        user_id: UserId::new(q.from.id.to_string()),
                        chat_id: ConversationId::new(chat_id),
                        message_id: MessageId::new(msg_id),
                        data,
                    };

                    let _ = tx.send(query).await;
                }

                // Answer to remove loading indicator
                bot.answer_callback_query(&q.id).await?;
                ResponseResult::Ok(())
            }
        },
    ));
```

**Step 2: 添加 edit 方法支持移除 keyboard**

```rust
/// Edit a message (optionally remove inline keyboard)
pub async fn edit(
    &self,
    chat_id: ConversationId,
    message_id: MessageId,
    text: Option<String>,
    keyboard: Option<InlineKeyboard>,
) -> ChannelResult<()> {
    #[cfg(feature = "telegram")]
    {
        let bot = self.bot.as_ref().ok_or(ChannelError::NotConnected)?;
        let chat = ChatId(chat_id.as_str().parse().map_err(|_| {
            ChannelError::InvalidConversation(chat_id.to_string())
        })?);
        let msg_id = teloxide::types::MessageId(
            message_id.as_str().parse().map_err(|_| {
                ChannelError::SendFailed("Invalid message ID".into())
            })?
        );

        if let Some(text) = text {
            let mut req = bot.edit_message_text(chat, msg_id, text);

            // Set keyboard or remove it
            if let Some(kb) = keyboard {
                let markup = InlineKeyboardMarkup::new(
                    kb.rows.iter().map(|row| {
                        row.iter().map(|btn| {
                            InlineKeyboardButton::callback(&btn.text, &btn.callback_data)
                        }).collect::<Vec<_>>()
                    }).collect::<Vec<_>>()
                );
                req = req.reply_markup(markup);
            } else {
                // Remove keyboard by setting empty markup
                req = req.reply_markup(InlineKeyboardMarkup::default());
            }

            req.await.map_err(|e| ChannelError::SendFailed(e.to_string()))?;
        }
    }
    Ok(())
}
```

**Step 3: Commit**

```bash
git add core/src/gateway/channels/telegram/mod.rs
git commit -m "feat(telegram): integrate callback handling and message editing"
```

---

## Task 6: 添加 Gateway RPC 方法

**Files:**
- Modify: `core/src/gateway/handlers/exec_approvals.rs`

**Step 1: 添加 approval forwarding 触发 RPC**

在现有的 `exec.approval.request` handler 中，触发 forwarding：

```rust
// 在创建 approval 后，触发 forwarding
if let Some(bridge_handler) = ctx.approval_bridge_handler() {
    let record = manager.get(&id).await;
    if let Some(record) = record {
        bridge_handler.forward_approval(&record).await;
    }
}
```

**Step 2: 添加 callback 处理 endpoint**

```rust
/// Handle callback from channel (internal use)
pub async fn handle_exec_callback(
    ctx: &GatewayContext,
    query: CallbackQuery,
) -> Result<String, String> {
    let bridge = ctx.approval_bridge_handler()
        .ok_or("Approval bridge not configured")?;

    bridge.handle_callback(query).await
}
```

**Step 3: Commit**

```bash
git add core/src/gateway/handlers/exec_approvals.rs
git commit -m "feat(gateway): wire approval forwarding to RPC handler"
```

---

## Task 7: 最终验证和文档

**Step 1: 运行所有相关测试**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && \
cargo test exec::bridge::tests && \
cargo test gateway::handlers::approval_bridge::tests && \
cargo test telegram::
```

**Step 2: 编译验证**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo check --features "gateway,telegram"
```

**Step 3: 更新设计文档**

修改 `/Volumes/TBU4/Workspace/Aether/docs/plans/2026-01-31-aether-beyond-openclaw-design.md`：

```markdown
### Milestone 3: Telegram 审批集成

- [x] TelegramAdapter 增强 (inline keyboard)
- [x] 审批请求消息模板
- [x] 回调处理 (approve/reject)
- [x] PtySupervisor ↔ Telegram 联动

**验收**: ✅ Danger 命令触发 Telegram 弹窗，点击后放行
```

**Step 4: Final Commit**

```bash
git add docs/plans/
git commit -m "docs: mark Milestone 3 (Telegram approval) as complete"
```

---

## 验收标准

完成本计划后，应满足以下条件：

1. ✅ Telegram 消息支持 inline keyboard (InlineKeyboardMarkup)
2. ✅ Telegram 支持 callback query 处理 (按钮点击)
3. ✅ `ApprovalBridge` 连接 `ExecApprovalManager` 和 `ChannelRegistry`
4. ✅ Danger 级别命令自动发送 Telegram 审批消息
5. ✅ 点击 "Allow Once" / "Allow Always" / "Deny" 按钮自动解决审批
6. ✅ 审批完成后消息更新显示结果

---

## 依赖关系

```
Milestone 1 (PtySupervisor) ✅ 完成
    │
    └──► Milestone 2 (SecurityKernel) ✅ 完成
             │
             └──► Milestone 3 (Telegram 审批) ← 当前
                      │
                      └──► Milestone 4 (规格驱动开发闭环)
```

---

*生成时间: 2026-01-31*
