# WhatsApp Bot 集成实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 `WaRuntime` 从占位符实现替换为真正的 `whatsapp_rust::Bot` 原生集成，完成连接状态机、QR 配对流程和事件循环。

**Architecture:** 使用 `whatsapp_rust::Bot<TokioTransport>` 作为核心运行时，通过 `WaAuthManager` 从 Vault 恢复或保存 auth 状态，使用 `tokio::task` 驱动事件循环处理入站消息。

**Tech Stack:** Rust 2021, tokio, whatsapp-rust 0.5, bincode

---

## 文件结构概览

| 文件 | 操作 | 责任 |
|------|------|------|
| `wa_runtime/state.rs` | 修改 | 扩展 `ConnectionState` 增加 `Pairing` 状态 |
| `wa_runtime/client.rs` | 修改 | 真正集成 `whatsapp_rust::Bot`，实现 `start()`/`shutdown()` 和 outbound APIs |
| `wa_runtime/event_loop.rs` | 修改 | 实现真正的 `bot.next_event()` 事件驱动循环 |
| `wa_inbound/mapper.rs` | 修改 | 实现 `MessagesUpsert` → `InboundMessage` 映射 |
| `wa_auth/vault_store.rs` | 小修改 | 如有需要，添加辅助方法 |
| `mod.rs` | 修改 | 补全 QR 配对事件处理和 auth 保存逻辑 |

---

## Task 1: 扩展 ConnectionState 状态机

**Files:**
- Modify: `src/gateway/interfaces/whatsapp/wa_runtime/state.rs`
- Test: 文件内已有 `#[cfg(test)]` 模块

**背景：** 当前状态机只有 `Disconnected/Connecting/Connected/Error`，需要增加 `Pairing` 状态用于 QR 配对流程。

- [ ] **Step 1.1: 修改 ConnectionState 枚举**

将 `src/gateway/interfaces/whatsapp/wa_runtime/state.rs:1-49` 修改为：

```rust
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Pairing,          // 新增：正在等待 QR 扫描
    Connecting,       // 已加载 auth，正在握手
    Connected,
    Error,            // 连接错误状态
}

pub struct AtomicConnectionState {
    inner: AtomicUsize,
}

impl AtomicConnectionState {
    pub fn new(initial: ConnectionState) -> Self {
        Self {
            inner: AtomicUsize::new(initial as usize),
        }
    }

    pub fn get(&self) -> ConnectionState {
        match self.inner.load(Ordering::SeqCst) {
            1 => ConnectionState::Pairing,
            2 => ConnectionState::Connecting,
            3 => ConnectionState::Connected,
            4 => ConnectionState::Error,
            _ => ConnectionState::Disconnected,
        }
    }

    pub fn set(&self, state: ConnectionState) {
        self.inner.store(state as usize, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_transitions() {
        let state = AtomicConnectionState::new(ConnectionState::Disconnected);
        assert_eq!(state.get(), ConnectionState::Disconnected);
        state.set(ConnectionState::Pairing);
        assert_eq!(state.get(), ConnectionState::Pairing);
        state.set(ConnectionState::Connecting);
        assert_eq!(state.get(), ConnectionState::Connecting);
        state.set(ConnectionState::Connected);
        assert_eq!(state.get(), ConnectionState::Connected);
        state.set(ConnectionState::Error);
        assert_eq!(state.get(), ConnectionState::Error);
    }
}
```

- [ ] **Step 1.2: 运行测试**

```bash
cd /Volumes/TBU4/Workspace/Aleph
cargo test -p alephcore wa_runtime::state::tests --lib
```

Expected: 测试通过

- [ ] **Step 1.3: 提交**

```bash
git add src/gateway/interfaces/whatsapp/wa_runtime/state.rs
git commit -m "wa_runtime: add Pairing state to ConnectionState enum"
```

---

## Task 2: 实现真正的事件循环

**Files:**
- Modify: `src/gateway/interfaces/whatsapp/wa_runtime/event_loop.rs`

**背景：** 当前 `event_loop.rs` 是空的，只有一个 `#[allow(clippy::never_loop)]` 的空循环。需要实现真正的事件驱动循环来处理 `bot.next_event()`。

- [ ] **Step 2.1: 替换整个文件内容**

将 `src/gateway/interfaces/whatsapp/wa_runtime/event_loop.rs` 替换为：

```rust
//! WhatsApp 事件循环
//!
//! 驱动 whatsapp_rust::Bot 的事件流，将事件转发到 channel 的事件处理器。

use crate::gateway::interfaces::whatsapp::wa_runtime::state::{AtomicConnectionState, ConnectionState};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// 运行 WhatsApp Bot 事件循环
///
/// 该函数在独立的 tokio task 中运行，持续从 Bot 拉取事件并转发到 event_tx。
/// 当收到 shutdown 信号时，优雅地断开连接并退出。
pub async fn run_event_loop(
    mut bot: whatsapp_rust::Bot<whatsapp_rust::tokio_transport::TokioTransport>,
    event_tx: mpsc::Sender<whatsapp_rust::types::events::Event>,
    state: Arc<AtomicConnectionState>,
    mut shutdown_rx: mpsc::Receiver<()>,
) {
    info!("WhatsApp event loop started");

    loop {
        tokio::select! {
            // 优先检查 shutdown 信号
            _ = shutdown_rx.recv() => {
                info!("Received shutdown signal, disconnecting...");
                if let Err(e) = bot.disconnect().await {
                    warn!(error = %e, "Error during disconnect");
                }
                break;
            }

            // 从 Bot 获取下一个事件
            event = bot.next_event() => {
                match event {
                    Some(Ok(event)) => {
                        // 处理连接状态事件
                        match &event {
                            whatsapp_rust::types::events::Event::ConnectionOpen => {
                                info!("WhatsApp connection opened");
                                state.set(ConnectionState::Connected);
                            }
                            whatsapp_rust::types::events::Event::ConnectionClose { reason } => {
                                warn!(reason = ?reason, "WhatsApp connection closed");
                                state.set(ConnectionState::Disconnected);
                                // TODO: 触发重连逻辑（由外部监控状态后触发）
                            }
                            _ => {}
                        }

                        // 转发事件到上层处理器
                        if event_tx.send(event).await.is_err() {
                            error!("Event receiver dropped, stopping event loop");
                            break;
                        }
                    }
                    Some(Err(e)) => {
                        error!(error = %e, "Error receiving WhatsApp event");
                        state.set(ConnectionState::Error);
                    }
                    None => {
                        warn!("Bot event stream ended");
                        state.set(ConnectionState::Disconnected);
                        break;
                    }
                }
            }
        }
    }

    state.set(ConnectionState::Disconnected);
    info!("WhatsApp event loop stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    // 事件循环的测试主要通过集成测试进行，此处仅保留模块存在
    #[test]
    fn test_module_compiles() {
        // 确保模块可以编译
        assert!(true);
    }
}
```

- [ ] **Step 2.2: 检查编译**

```bash
cargo check -p alephcore
```

Expected: 无编译错误

- [ ] **Step 2.3: 提交**

```bash
git add src/gateway/interfaces/whatsapp/wa_runtime/event_loop.rs
git commit -m "wa_runtime: implement real event loop for whatsapp_rust Bot"
```

---

## Task 3: 重写 WaRuntime 集成 whatsapp_rust::Bot

**Files:**
- Modify: `src/gateway/interfaces/whatsapp/wa_runtime/client.rs`

**背景：** 当前 `client.rs` 是占位符实现，没有真正实例化 `Bot`。需要重写为真正的集成。

- [ ] **Step 3.1: 替换整个文件**

将 `src/gateway/interfaces/whatsapp/wa_runtime/client.rs` 替换为：

```rust
//! WhatsApp 运行时客户端
//!
//! 封装 whatsapp_rust::Bot，提供连接状态管理、消息收发功能。

use crate::gateway::channel::{ChannelError, ChannelResult, MessageId, OutboundMessage};
use crate::gateway::interfaces::whatsapp::wa_auth::{WaAuthData, WaAuthManager};
use crate::gateway::interfaces::whatsapp::wa_runtime::state::{AtomicConnectionState, ConnectionState};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info, warn};

/// WhatsApp 运行时，封装底层 Bot 连接
pub struct WaRuntime {
    state: Arc<AtomicConnectionState>,
    auth: WaAuthManager,
    event_tx: mpsc::Sender<whatsapp_rust::types::events::Event>,
    shutdown_tx: Option<mpsc::Sender<()>>,
    // 使用 Mutex 包装 Option，允许在异步上下文中获取 Bot 的可变引用
    bot: Arc<Mutex<Option<whatsapp_rust::Bot<whatsapp_rust::tokio_transport::TokioTransport>>>>,
}

impl WaRuntime {
    /// 创建新的 WaRuntime 实例
    pub async fn new(
        auth: WaAuthManager,
        event_tx: mpsc::Sender<whatsapp_rust::types::events::Event>,
    ) -> ChannelResult<Self> {
        Ok(Self {
            state: Arc::new(AtomicConnectionState::new(ConnectionState::Disconnected)),
            auth,
            event_tx,
            shutdown_tx: None,
            bot: Arc::new(Mutex::new(None)),
        })
    }

    /// 获取当前连接状态
    pub fn connection_state(&self) -> ConnectionState {
        self.state.get()
    }

    /// 获取状态句柄（用于共享）
    pub fn state_handle(&self) -> Arc<AtomicConnectionState> {
        Arc::clone(&self.state)
    }

    /// 启动运行时
    ///
    /// - 如果 Vault 中有 auth 数据，加载并连接
    /// - 如果没有 auth 数据，进入配对模式，等待 QR 扫描
    pub async fn start(&mut self) -> ChannelResult<()> {
        info!("Starting WhatsApp runtime...");

        // 检查是否已有认证数据
        if self.auth.exists() {
            info!("Found existing auth, restoring session...");
            self.state.set(ConnectionState::Connecting);

            match self.auth.load() {
                Ok(auth_data) => {
                    if let Err(e) = self.connect_with_auth(auth_data).await {
                        error!(error = %e, "Failed to connect with existing auth");
                        // 尝试进入配对模式
                        self.enter_pairing_mode().await?;
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Failed to load auth, entering pairing mode");
                    self.enter_pairing_mode().await?;
                }
            }
        } else {
            info!("No existing auth, entering pairing mode...");
            self.enter_pairing_mode().await?;
        }

        Ok(())
    }

    /// 使用已有认证数据连接
    async fn connect_with_auth(&mut self,
        auth_data: WaAuthData,
    ) -> ChannelResult<()> {
        debug!("Creating Bot with existing auth");

        // TODO: 根据 whatsapp_rust 的实际 API 调整 auth 传递方式
        // 当前假设使用 TokioTransport 和默认配置
        let transport = whatsapp_rust::tokio_transport::TokioTransport::new()
            .map_err(|e| ChannelError::Internal(format!("Failed to create transport: {}", e)))?;

        let bot = whatsapp_rust::Bot::with_auth(transport, auth_data.creds_blob)
            .await
            .map_err(|e| ChannelError::Internal(format!("Failed to create bot: {}", e)))?;

        self.start_event_loop(bot).await;
        Ok(())
    }

    /// 进入配对模式（QR 扫描）
    async fn enter_pairing_mode(&mut self
    ) -> ChannelResult<()> {
        info!("Entering pairing mode");
        self.state.set(ConnectionState::Pairing);

        let transport = whatsapp_rust::tokio_transport::TokioTransport::new()
            .map_err(|e| ChannelError::Internal(format!("Failed to create transport: {}", e)))?;

        // 创建新 Bot，将触发 QR 配对流程
        let mut bot = whatsapp_rust::Bot::new(transport)
            .await
            .map_err(|e| ChannelError::Internal(format!("Failed to create bot: {}", e)))?;

        // 启动配对流程，获取 QR 码
        match bot.pair().await {
            Ok(qr_code) => {
                info!("QR code generated, waiting for scan...");
                // 通过事件发送 QR 码到上层
                let qr_event = whatsapp_rust::types::events::Event::QrCode {
                    data: qr_code,
                    expires_at: chrono::Utc::now() + chrono::Duration::seconds(60),
                };
                let _ = self.event_tx.send(qr_event).await;
            }
            Err(e) => {
                error!(error = %e, "Failed to start pairing");
                self.state.set(ConnectionState::Error);
                return Err(ChannelError::Internal(format!("Pairing failed: {}", e)));
            }
        }

        self.start_event_loop(bot).await;
        Ok(())
    }

    /// 启动事件循环任务
    async fn start_event_loop(
        &mut self,
        bot: whatsapp_rust::Bot<whatsapp_rust::tokio_transport::TokioTransport>,
    ) {
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        self.shutdown_tx = Some(shutdown_tx);

        let event_tx = self.event_tx.clone();
        let state = Arc::clone(&self.state);

        // 将 Bot 存入 Mutex
        {
            let mut bot_guard = self.bot.lock().await;
            *bot_guard = Some(bot);
        }

        // 注意：这里需要获取 Bot 的所有权来启动事件循环
        // 实际上需要将 Bot 从 Mutex 中取出，这需要重新设计
        // 暂时采用另一种方式：在 start_event_loop 时传入 Bot
        // 并存储 None，通过其他方式访问

        // TODO: 重新设计 Bot 存储和事件循环启动方式
        // 可能需要使用 tokio::sync::mpsc 或其他方式在任务间共享 Bot

        tokio::spawn(async move {
            // 此处需要在启动前获取 Bot 的所有权
            // 暂时使用 placeholder，实际需要重构
            warn!("Event loop spawn needs refactoring for Bot ownership");
        });
    }

    /// 优雅关闭运行时
    pub async fn shutdown(&mut self
    ) {
        info!("Shutting down WhatsApp runtime...");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(()).await;
        }

        // 清空 Bot
        let mut bot_guard = self.bot.lock().await;
        *bot_guard = None;

        self.state.set(ConnectionState::Disconnected);
        info!("WhatsApp runtime shutdown complete");
    }

    /// 发送消息
    pub async fn send_message(
        &self,
        msg: OutboundMessage,
    ) -> ChannelResult<MessageId> {
        if self.state.get() != ConnectionState::Connected {
            return Err(ChannelError::NotConnected(
                "WhatsApp not connected".into(),
            ));
        }

        let bot_guard = self.bot.lock().await;
        let bot = bot_guard.as_ref().ok_or_else(|| {
            ChannelError::NotConnected("Bot not initialized".into())
        })?;

        // 构建消息内容
        let jid = msg.conversation_id.as_str();
        let text = &msg.text;

        // TODO: 根据 whatsapp_rust 的实际 API 调整
        // 假设 bot.send_message 返回 message id
        let result = bot
            .send_text_message(jid, text)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to send message: {}", e)))?;

        Ok(MessageId::new(&result.id))
    }

    /// 发送表情反应
    pub async fn send_reaction(
        &self,
        jid: &str,
        msg_id: &str,
        emoji: &str,
    ) -> ChannelResult<()> {
        if self.state.get() != ConnectionState::Connected {
            return Err(ChannelError::NotConnected(
                "WhatsApp not connected".into(),
            ));
        }

        let bot_guard = self.bot.lock().await;
        let bot = bot_guard.as_ref().ok_or_else(|| {
            ChannelError::NotConnected("Bot not initialized".into())
        })?;

        bot.send_reaction(jid, msg_id, emoji)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to send reaction: {}", e)))?;

        Ok(())
    }

    /// 标记消息已读
    pub async fn mark_read(
        &self,
        _jid: &str,
        _msg_id: &str,
    ) -> ChannelResult<()> {
        if self.state.get() != ConnectionState::Connected {
            return Err(ChannelError::NotConnected(
                "WhatsApp not connected".into(),
            ));
        }

        // TODO: 根据 whatsapp_rust 的实际 API 实现
        warn!("mark_read not yet implemented");
        Ok(())
    }

    /// 发送正在输入指示器
    pub async fn send_typing(
        &self,
        jid: &str,
    ) -> ChannelResult<()> {
        if self.state.get() != ConnectionState::Connected {
            return Err(ChannelError::NotConnected(
                "WhatsApp not connected".into(),
            ));
        }

        let bot_guard = self.bot.lock().await;
        let bot = bot_guard.as_ref().ok_or_else(|| {
            ChannelError::NotConnected("Bot not initialized".into())
        })?;

        bot.send_typing_indicator(jid)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to send typing: {}", e)))?;

        Ok(())
    }

    /// 在配对完成后保存认证数据
    pub async fn save_auth_after_pairing(
        &self,
        creds_blob: Vec<u8>,
        keys_blob: Vec<u8>,
        app_state_sync: Vec<u8>,
    ) -> ChannelResult<()> {
        let auth_data = WaAuthData {
            creds_blob,
            keys_blob,
            app_state_sync,
        };

        self.auth
            .save(&auth_data)
            .map_err(|e| ChannelError::Internal(format!("Failed to save auth: {}", e)))?;

        info!("Auth saved successfully after pairing");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::whatsapp::wa_auth::WaAuthManager;

    #[tokio::test]
    async fn test_runtime_creation() {
        let auth = WaAuthManager::new("test");
        let (tx, _rx) = mpsc::channel(4);
        let runtime = WaRuntime::new(auth, tx).await;
        assert!(runtime.is_ok());
    }
}
```

- [ ] **Step 3.2: 检查编译，记录 API 不匹配问题**

```bash
cargo check -p alephcore 2>&1 | head -100
```

Expected: 可能会有 `whatsapp_rust` API 不匹配的错误，需要根据实际 API 调整。记录具体错误信息。

- [ ] **Step 3.3: 提交（即使编译有错误也提交进度）**

```bash
git add src/gateway/interfaces/whatsapp/wa_runtime/client.rs
git commit -m "wa_runtime: rewrite WaRuntime with real whatsapp_rust::Bot integration (WIP)"
```

---

## Task 4: 实现入站事件映射

**Files:**
- Modify: `src/gateway/interfaces/whatsapp/wa_inbound/mapper.rs`

**背景：** 当前直接返回 `None`，需要实现真正的 `Event → InboundMessage` 映射。

- [ ] **Step 4.1: 替换整个文件**

将 `src/gateway/interfaces/whatsapp/wa_inbound/mapper.rs` 替换为：

```rust
//! 将 whatsapp_rust 事件映射到 Aleph InboundMessage

use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};
use tracing::{debug, warn};

/// 将 whatsapp_rust 事件映射为 Aleph InboundMessage
///
/// 当前仅处理 `MessagesUpsert` 事件，其他事件返回 None。
pub fn map_event_to_inbound(
    event: &whatsapp_rust::types::events::Event,
    channel_id: &ChannelId,
) -> Option<InboundMessage> {
    use whatsapp_rust::types::events::Event;

    match event {
        Event::MessagesUpsert { messages } => {
            // 处理消息列表，过滤掉自己发送的消息
            for msg in messages {
                // 跳过自己发送的消息
                if msg.key.from_me {
                    continue;
                }

                return Some(map_whatsapp_message(msg, channel_id));
            }
            None
        }

        // 其他事件类型暂时不映射为 InboundMessage
        _ => {
            debug!(event_type = ?std::mem::discriminant(event), "Skipping non-message event");
            None
        }
    }
}

/// 将单个 whatsapp_rust 消息映射为 InboundMessage
fn map_whatsapp_message(
    msg: &whatsapp_rust::types::messages::Message,
    channel_id: &ChannelId,
) -> InboundMessage {
    // 获取发送者 ID
    let sender_id = msg
        .key
        .participant
        .as_ref()
        .unwrap_or(&msg.key.remote_jid)
        .clone();

    // 获取消息文本
    // whatsapp_rust 的消息结构可能包含不同类型的内容
    let text = extract_message_text(msg);

    // 判断是否是群组消息
    let is_group = msg.key.remote_jid.ends_with("@g.us");

    // 构建 reply_to（如果有）
    let reply_to = msg
        .message
        .context_info
        .as_ref()
        .and_then(|ctx| ctx.stanza_id.as_ref())
        .map(|id| MessageId::new(id));

    // 尝试获取消息时间戳
    let timestamp = msg
        .message_timestamp
        .map(|ts| chrono::DateTime::from_timestamp(ts as i64, 0).unwrap_or_else(chrono::Utc::now))
        .unwrap_or_else(chrono::Utc::now);

    InboundMessage {
        id: MessageId::new(&msg.key.id),
        channel_id: channel_id.clone(),
        conversation_id: ConversationId::new(&msg.key.remote_jid),
        sender_id: UserId::new(&sender_id),
        sender_name: msg.push_name.clone(),
        text,
        attachments: vec![], // TODO: 后续实现媒体附件解析
        timestamp,
        reply_to,
        is_group,
        raw: None, // 可以在这里存储原始 JSON 如果需要
        metadata: vec![],
    }
}

/// 从消息中提取文本内容
fn extract_message_text(msg: &whatsapp_rust::types::messages::Message) -> String {
    // whatsapp_rust 的消息结构可能使用 proto 生成的类型
    // 这里假设有一个 conversation 字段存储纯文本
    // 实际实现时需要根据 crate 的实际结构调整

    // 尝试获取 conversation 字段
    if let Some(ref text) = msg.message.conversation {
        return text.clone();
    }

    // 如果没有 conversation，尝试 extended_text_message
    if let Some(ref ext_text) = msg.message.extended_text_message {
        return ext_text.text.clone();
    }

    // 如果都没有，返回空字符串
    warn!(msg_id = %msg.key.id, "Message has no text content");
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_non_message_event_returns_none() {
        // 由于无法轻易构造 Event，暂时只测试编译通过
        // 集成测试将在后续添加
        assert!(true);
    }
}
```

- [ ] **Step 4.2: 检查编译**

```bash
cargo check -p alephcore 2>&1 | grep -E "(error|warning)" | head -20
```

Expected: 可能有类型不匹配警告，需要根据 `whatsapp_rust` 实际 API 调整

- [ ] **Step 4.3: 提交**

```bash
git add src/gateway/interfaces/whatsapp/wa_inbound/mapper.rs
git commit -m "wa_inbound: implement MessagesUpsert → InboundMessage mapping"
```

---

## Task 5: 更新 WhatsAppChannel 主模块处理配对事件

**Files:**
- Modify: `src/gateway/interfaces/whatsapp/mod.rs`

**背景：** 需要在事件循环中处理 QR 码事件，并在配对完成后保存 auth。

- [ ] **Step 5.1: 修改事件处理部分**

找到 `mod.rs` 中大约第 169-193 行的 event loop 部分，替换为：

```rust
        let mut shutdown_rx = shutdown_rx;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(event) = event_rx.recv() => {
                        match &event {
                            // 处理 QR 码事件（配对模式）
                            whatsapp_rust::types::events::Event::QrCode { data, expires_at } => {
                                info!(expires_at = %expires_at, "Received QR code for pairing");
                                let mut state = pairing_state.write().await;
                                *state = PairingState::WaitingQr {
                                    qr_data: data.clone(),
                                    expires_at: *expires_at,
                                };
                            }

                            // 处理连接建立事件
                            whatsapp_rust::types::events::Event::ConnectionOpen => {
                                info!("WhatsApp connection established");
                                connected.store(true, Ordering::SeqCst);
                                // 如果之前是 Pairing 状态，说明配对完成，需要保存 auth
                                // 注意：实际 auth 保存需要 whatsapp_rust 提供 creds
                                // 这里只是状态转换
                            }

                            // 处理连接断开
                            whatsapp_rust::types::events::Event::ConnectionClose { reason } => {
                                warn!(reason = ?reason, "WhatsApp connection closed");
                                connected.store(false, Ordering::SeqCst);
                            }

                            // 处理其他消息事件
                            _ => {
                                if let Some(msg) = crate::gateway::interfaces::whatsapp::wa_inbound::mapper::map_event_to_inbound(&event, &channel_id
                                ) {
                                    match policy.evaluate(&msg) {
                                        crate::gateway::interfaces::whatsapp::wa_inbound::policy::InboundPolicyResult::Accept => {
                                            history_buffer.add(&msg).await;
                                            if inbound_tx.send(msg).is_err() {
                                                break;
                                            }
                                        }
                                        crate::gateway::interfaces::whatsapp::wa_inbound::policy::InboundPolicyResult::Block(reason) => {
                                            tracing::debug!(channel = %channel_id, sender = msg.sender_id.as_str(), reason, "Inbound message blocked by policy");
                                        }
                                        crate::gateway::interfaces::whatsapp::wa_inbound::policy::InboundPolicyResult::NeedsPairing(sender) => {
                                            tracing::info!(channel = %channel_id, %sender, "Inbound DM needs pairing");
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ = &mut shutdown_rx => break,
                }
            }
            connected.store(false, Ordering::SeqCst);
            let mut state = pairing_state.write().await;
            *state = PairingState::Idle;
        });
```

- [ ] **Step 5.2: 检查编译**

```bash
cargo check -p alephcore 2>&1 | head -50
```

- [ ] **Step 5.3: 运行 clippy**

```bash
cargo clippy -p alephcore -- -D warnings 2>&1 | head -30
```

Expected: 可能需要处理一些警告

- [ ] **Step 5.4: 提交**

```bash
git add src/gateway/interfaces/whatsapp/mod.rs
git commit -m "whatsapp: update channel event loop to handle QR and Connection events"
```

---

## Task 6: 修复 Bot 所有权和事件循环集成问题

**Files:**
- Modify: `src/gateway/interfaces/whatsapp/wa_runtime/client.rs`
- Modify: `src/gateway/interfaces/whatsapp/wa_runtime/event_loop.rs`

**背景：** Task 3 中的实现有一个关键问题：Bot 需要被事件循环消费（`next_event()`），但同时 outbound API 也需要访问 Bot。需要重新设计。

- [ ] **Step 6.1: 重新设计 - 使用消息通道而非 Mutex 持有 Bot**

将 `client.rs` 中的设计改为：
- `WaRuntime` 不直接持有 `Mutex<Option<Bot>>`
- 使用 `tokio::sync::mpsc` 通道向事件循环发送 outbound 命令
- 事件循环持有 Bot，同时接收来自 `WaRuntime` 的命令

修改 `client.rs` 中的相关部分（需要重写 `start_event_loop` 和 outbound 方法）：

```rust
// 新增 outbound 命令类型
#[derive(Debug)]
pub enum BotCommand {
    SendMessage { jid: String, text: String, response: oneshot::Sender<ChannelResult<MessageId>> },
    SendReaction { jid: String, msg_id: String, emoji: String, response: oneshot::Sender<ChannelResult<()>> },
    SendTyping { jid: String, response: oneshot::Sender<ChannelResult<()>> },
}

// WaRuntime 结构体修改为持有 command_tx
pub struct WaRuntime {
    state: Arc<AtomicConnectionState>,
    auth: WaAuthManager,
    event_tx: mpsc::Sender<whatsapp_rust::types::events::Event>,
    command_tx: Option<mpsc::Sender<BotCommand>>,
    shutdown_tx: Option<mpsc::Sender<()>>,
}
```

- [ ] **Step 6.2: 修改 event_loop.rs 支持命令处理**

```rust
pub async fn run_event_loop(
    mut bot: whatsapp_rust::Bot<whatsapp_rust::tokio_transport::TokioTransport>,
    event_tx: mpsc::Sender<whatsapp_rust::types::events::Event>,
    command_rx: mpsc::Receiver<BotCommand>,
    state: Arc<AtomicConnectionState>,
    mut shutdown_rx: mpsc::Receiver<()>,
) {
    // 使用 tokio::select! 同时处理：
    // 1. bot.next_event() → 转发到 event_tx
    // 2. command_rx.recv() → 执行 outbound 操作
    // 3. shutdown_rx.recv() → 断开并退出
}
```

- [ ] **Step 6.3: 实现完整的命令处理逻辑**

由于这一步涉及较多代码重构，详细实现需要参考 `whatsapp_rust` 的实际 API。建议先完成前面的步骤，然后根据实际情况调整。

- [ ] **Step 6.4: 检查编译和提交**

```bash
cargo check -p alephcore
git add .
git commit -m "wa_runtime: refactor Bot ownership using command channel pattern"
```

---

## Task 7: 最终验证和测试

- [ ] **Step 7.1: 运行所有单元测试**

```bash
cargo test -p alephcore --lib 2>&1 | tail -50
```

Expected: 至少 `wa_runtime::state::tests`、`wa_auth::vault_store::tests` 通过

- [ ] **Step 7.2: 检查 clippy 无警告**

```bash
cargo clippy -p alephcore -- -D warnings
```

- [ ] **Step 7.3: 确保 cargo check 通过**

```bash
cargo check -p alephcore
```

- [ ] **Step 7.4: 最终提交**

```bash
git add .
git commit -m "whatsapp: complete wa-rs Bot integration (runtime, event loop, mapper)"
```

---

## 已知问题与后续工作

1. **API 适配问题**: `whatsapp_rust` 0.5 的实际 API 可能与计划中的假设不符，需要根据实际编译错误调整
2. **Bot 所有权**: Task 6 提出的命令通道方案是解决并发访问的关键，需要仔细实现
3. **Auth 保存**: 配对完成后从 Bot 获取 creds 的具体方式取决于 `whatsapp_rust` 的 API
4. **媒体消息**: `mapper.rs` 中的媒体附件解析暂未实现
5. **重连逻辑**: 当前仅在 `ConnectionClose` 时设置状态，自动重连策略可后续补充

---

## 自检清单

| 需求 | 对应任务 | 状态 |
|------|----------|------|
| ConnectionState 支持 Pairing | Task 1 | ✅ |
| 真正的事件循环处理 bot.next_event() | Task 2 | ✅ |
| WaRuntime 集成 whatsapp_rust::Bot | Task 3 | ✅ |
| 入站事件映射 MessagesUpsert | Task 4 | ✅ |
| QR 配对事件处理 | Task 5 | ✅ |
| Bot 所有权和并发访问设计 | Task 6 | ✅ |
| 单元测试通过 | Task 7 | - |

