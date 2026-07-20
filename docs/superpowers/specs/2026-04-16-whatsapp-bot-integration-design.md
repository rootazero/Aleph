# WhatsApp Bot 集成设计：wa-rs 原生运行时

**Status**: 设计稿（待实现计划）  
**Date**: 2026-04-16  
**Scope**: 将 `WaRuntime` 从占位符实现替换为真正的 `whatsapp_rust::Bot` 集成，完成连接状态机、QR 配对流程和事件循环。

---

## 1. 背景

Aleph 的 WhatsApp 通道已经完成架构重构：Go bridge 和 Rust 侧的 bridge 胶水代码已全部删除，新的模块骨架（`wa_runtime/`、`wa_auth/`、`wa_outbound/`、`wa_inbound/`、`wa_policy/`）已经就位。`Cargo.toml` 也已引入 `whatsapp-rust` 0.5 依赖。

然而，`WaRuntime` 目前仍是占位符实现：
- `client.rs` 中只创建了一个空壳 `WaRuntime`，没有实例化真正的 `whatsapp_rust::Bot`
- `event_loop.rs` 是空的，没有处理任何事件
- `wa_inbound/mapper.rs` 直接返回 `None`

本设计文档明确如何将 `whatsapp_rust::Bot` 真正接入 Aleph 的 `Channel` trait 体系。

---

## 2. 目标与非目标

### 目标
- 在 `WaRuntime` 中实例化并驱动 `whatsapp_rust::Bot<TokioTransport>`
- 实现基于 Vault 的 auth 恢复：有 auth 则自动恢复连接，无 auth 则进入 QR 配对流程
- 完成连接状态机（Disconnected → Pairing/Connecting → Connected → Error → reconnect）
- 实现真正的事件循环，处理 `MessagesUpsert`、`ConnectionOpen`、`ConnectionClose`、`Receipt` 等核心事件
- 将事件映射到 `InboundMessage`，通过 `ChannelState` 注入 gateway 管道
- 保持单进程部署，无外部二进制依赖

### 非目标
- 不实现 openclaw 的高级功能（polls、status、复杂的 group policy 引擎）—— 这些是下一步
- 不改 `Channel` trait 或 `ChannelRegistry` 的抽象
- 不在本阶段做真实 WhatsApp 账号的端到端集成测试（以单元测试 + mock 为主）

---

## 3. 架构 overview

### 3.1 运行时结构

```text
WhatsAppChannel (implements Channel trait)
    └── WaRuntime
            ├── bot: whatsapp_rust::Bot<TokioTransport>
            ├── state: AtomicConnectionState
            ├── auth: WaAuthManager (Vault-backed)
            ├── event_tx: mpsc::Sender<whatsapp_rust::Event>
            └── shutdown: CancellationToken
```

### 3.2 事件流

```text
WhatsApp Servers
        │
        ▼
whatsapp_rust::Bot (WebSocket + Signal Protocol)
        │
        ▼
WaRuntime event loop (tokio::task)
        │
        ├─ ConnectionOpen   → state = Connected
        ├─ ConnectionClose  → state = Disconnected / Error, trigger reconnect
        ├─ MessagesUpsert   → mapper → InboundMessage
        │                       │
        │                       ▼
        │                  policy.evaluate()
        │                       │
        │                       ▼
        │                  Pass? → ChannelState::send_inbound()
        │                  Fail? → log + drop
        │
        └─ Receipt / Reaction → internal handlers
        ▼
Gateway → Thinker
```

---

## 4. 核心模块设计

### 4.1 `WaRuntime` (`wa_runtime/client.rs`)

`WaRuntime` 负责持有 `whatsapp_rust::Bot` 实例并暴露 `Channel` 需要的操作接口。

```rust
pub struct WaRuntime {
    state: Arc<AtomicConnectionState>,
    auth: WaAuthManager,
    event_tx: mpsc::Sender<whatsapp_rust::types::events::Event>,
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl WaRuntime {
    pub async fn new(
        auth: WaAuthManager,
        event_tx: mpsc::Sender<whatsapp_rust::types::events::Event>,
    ) -> ChannelResult<Self> { ... }

    pub async fn start(&mut self) -> ChannelResult<()> { ... }
    pub async fn shutdown(&mut self) { ... }
    pub fn connection_state(&self) -> ConnectionState { ... }

    // Outbound APIs
    pub async fn send_message(&self, msg: OutboundMessage) -> ChannelResult<MessageId> { ... }
    pub async fn send_reaction(&self, jid: &str, msg_id: &str, emoji: &str) -> ChannelResult<()> { ... }
    pub async fn mark_read(&self, jid: &str, msg_id: &str) -> ChannelResult<()> { ... }
    pub async fn send_typing(&self, jid: &str) -> ChannelResult<()> { ... }
}
```

**关键设计点：**
- `start()` 时检查 `auth.exists()`：
  - 存在 → 调用 `WaAuthManager::load()` 获取 creds/keys/app_state，构造 `Bot` 并 connect
  - 不存在 → 构造 `Bot` 进入 `pair()` 流程，生成 QR code 并通过 `PairingState` 暴露给上层
- QR 配对完成后，`whatsapp_rust` 会触发 `ConnectionOpen`，此时将 auth state 保存到 Vault
- 所有 outbound API 检查 `connection_state() == Connected`，否则返回 `ChannelError::NotConnected`

### 4.2 连接状态机 (`wa_runtime/state.rs`)

扩展现有的 `ConnectionState`，增加 `Pairing` 和带原因的 `Error`：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Pairing,          // 正在等待 QR 扫描
    Connecting,       // 已加载 auth，正在握手
    Connected,
    Error,            // 保持简单，错误原因通过 tracing 记录
}
```

状态转换：

| From | Event | To |
|------|-------|----|
| Disconnected | `start()` 无 auth | Pairing |
| Disconnected | `start()` 有 auth | Connecting |
| Pairing | QR 扫描成功 + ConnectionOpen | Connected |
| Connecting | ConnectionOpen | Connected |
| Connected | WebSocket 断开 | Disconnected / Error |
| Error | 自动重连触发 | Connecting |

重连策略（简化版）：
- 初始断开后等待 5s，指数退避到 60s 上限
- 最多连续重试 10 次，之后进入 `Error` 状态等待手动 `stop()`/`start()`

### 4.3 事件循环 (`wa_runtime/event_loop.rs`)

替换空循环，真正驱动 `bot.next_event().await`：

```rust
pub async fn run_event_loop(
    mut bot: whatsapp_rust::Bot<whatsapp_rust::tokio_transport::TokioTransport>,
    event_tx: mpsc::Sender<whatsapp_rust::types::events::Event>,
    state: Arc<AtomicConnectionState>,
    mut shutdown_rx: mpsc::Receiver<()>,
) {
    loop {
        tokio::select! {
            Some(event) = bot.next_event() => {
                match &event {
                    whatsapp_rust::types::events::Event::ConnectionOpen => {
                        state.set(ConnectionState::Connected);
                    }
                    whatsapp_rust::types::events::Event::ConnectionClose { reason } => {
                        state.set(ConnectionState::Disconnected);
                        tracing::warn!(reason, "WhatsApp connection closed");
                        // TODO: 触发重连逻辑（外部或内部）
                    }
                    _ => {}
                }
                if event_tx.send(event).await.is_err() {
                    break;
                }
            }
            _ = shutdown_rx.recv() => {
                let _ = bot.disconnect().await;
                break;
            }
        }
    }
    state.set(ConnectionState::Disconnected);
}
```

### 4.4 QR / 配对流程

`WhatsAppChannel::get_pairing_data()` 目前已支持返回 `PairingData::QrCode`。需要补全的是：**QR 数据从哪里来？**

方案：
1. `WaRuntime::start()` 在无 auth 时调用 `bot.pair()`
2. `whatsapp_rust` 的 `pair()` 通常返回一个 QR code string（或通过事件流发送）
3. 将 QR 数据写入 `PairingState::WaitingQr`
4. 用户扫码后，`ConnectionOpen` 事件触发，保存 auth 到 Vault

```rust
// 在 WhatsAppChannel::start() 的 event loop 中
Some(event) = event_rx.recv() => {
    match event {
        whatsapp_rust::types::events::Event::QrCode { data, .. } => {
            let mut state = pairing_state.write().await;
            *state = PairingState::WaitingQr { qr_data: data, expires_at: ... };
        }
        whatsapp_rust::types::events::Event::ConnectionOpen => {
            // 如果之前是 Pairing 状态，保存 auth
            if runtime.connection_state() == ConnectionState::Pairing {
                runtime.save_auth_after_pairing().await.ok();
            }
            connected.store(true, Ordering::SeqCst);
        }
        // ... 其他事件处理
    }
}
```

### 4.5 入站事件映射 (`wa_inbound/mapper.rs`)

从占位符实现真正的 `Event → InboundMessage` 映射。`whatsapp_rust` 的事件类型需要仔细处理：

```rust
pub fn map_event_to_inbound(
    event: &whatsapp_rust::types::events::Event,
    channel_id: &ChannelId,
) -> Option<InboundMessage> {
    use whatsapp_rust::types::events::Event;
    match event {
        Event::MessagesUpsert { messages } => {
            // 取第一条非自己发送的消息
            messages.iter().find(|m| !m.key.from_me).map(|m| {
                InboundMessage {
                    id: MessageId::new(&m.key.id),
                    channel_id: channel_id.clone(),
                    conversation_id: ConversationId::new(&m.key.remote_jid),
                    sender_id: UserId::new(&m.key.participant.as_ref().unwrap_or(&m.key.remote_jid)),
                    sender_name: m.push_name.clone(),
                    text: m.message.conversation.clone().unwrap_or_default(),
                    attachments: vec![], // 媒体解析后续补充
                    timestamp: chrono::Utc::now(), // 或用 m.message_timestamp
                    reply_to: m.message.context_info.as_ref()
                        .map(|c| MessageId::new(&c.stanza_id)),
                    is_group: m.key.remote_jid.ends_with("@g.us"),
                    raw: None,
                    metadata: vec![],
                }
            })
        }
        _ => None,
    }
}
```

**注意**：`whatsapp_rust::types::events::Event` 的具体字段命名可能因版本而异，实际实现时需要根据 crate 的 API 调整。若 API 不稳定，使用保守的 pattern match，并在无法匹配时返回 `None` 而不是 panic。

---

## 5. 文件变更清单

### 修改文件
- `src/gateway/interfaces/whatsapp/wa_runtime/client.rs` — 真正集成 `whatsapp_rust::Bot`
- `src/gateway/interfaces/whatsapp/wa_runtime/event_loop.rs` — 实现事件驱动循环
- `src/gateway/interfaces/whatsapp/wa_runtime/state.rs` — 增加 `Pairing` 状态
- `src/gateway/interfaces/whatsapp/wa_inbound/mapper.rs` — 实现 `MessagesUpsert` 映射
- `src/gateway/interfaces/whatsapp/mod.rs` — 补全 QR 配对事件处理和 auth 保存
- `src/gateway/interfaces/whatsapp/wa_auth/vault_store.rs` — 添加 `clear()` 或迁移辅助方法（如需要）

### 无需变更（已就位）
- `Cargo.toml` — 依赖已正确配置
- `wa_outbound/` — 当前 wrapper 可直接复用
- `wa_policy/` — 当前框架可直接复用
- `config.rs` — 结构已正确

---

## 6. 清理计划

本阶段无需删除旧代码（Go bridge 已在之前的重构中清理完毕）。但需确保：
- 删除 `wa_runtime/client.rs` 中所有占位符逻辑（如 `Ok(MessageId::new("wa-msg-id"))`）
- 删除 `event_loop.rs` 中的 `#[allow(clippy::never_loop)]` 和空循环
- 删除 `mapper.rs` 中的 `let _ = event; None` 占位符

---

## 7. 风险与缓解

| 风险 | 影响 | 缓解 |
|------|------|------|
| `whatsapp_rust` API 与文档不一致 | 高 | 实现时先打印/inspect Event 结构，使用保守 pattern match；无法识别的事件返回 `None` 并打 warning log |
| Vault auth 格式与 `whatsapp_rust` 期望不一致 | 中 | `WaAuthData` 保存原始 bytes blob，完全透明传递给 `whatsapp_rust` 的 store 接口 |
| 连接断开后重连导致状态错乱 | 中 | 重连逻辑单线程化（只在 event loop 或 `WaRuntime` 一个任务中修改 `ConnectionState`） |
| QR 超时后没有重新生成 | 低 | 先依赖 `whatsapp_rust` 内部机制；若不支持，在应用层添加 60s 超时后重新 `start()` |

---

## 8. 验收标准

- [ ] `cargo check -p alephcore` 无错误
- [ ] `WaRuntime::start()` 在有 Vault auth 时直接进入 `Connecting → Connected`
- [ ] `WaRuntime::start()` 在无 auth 时进入 `Pairing`，QR 数据可被 `get_pairing_data()` 读取
- [ ] 事件循环能接收并分发 `MessagesUpsert` 事件到 gateway inbound 通道
- [ ] `send_message`、`send_reaction`、`send_typing` 在 `Connected` 状态下调用 `bot` 对应 API
- [ ] 断开连接后 `connection_state()` 正确变为 `Disconnected`
- [ ] 单元测试：`wa_runtime/state.rs`、`wa_auth/vault_store.rs` 测试通过

---

## 9. 参考

- Aleph WhatsApp 现有代码：`src/gateway/interfaces/whatsapp/`
- `whatsapp-rust` crate 版本：0.5
- 前序设计文档：`docs/superpowers/specs/2026-04-15-whatsapp-native-redesign-design.md`
