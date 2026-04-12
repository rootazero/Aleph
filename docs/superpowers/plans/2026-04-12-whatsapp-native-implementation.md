# WhatsApp Native Rust Baileys 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现原生 Rust Baileys 客户端，替换现有 Go bridge，支持消息收发核心功能

**Architecture:** 新增 `native_baileys/` 模块，通过 feature flag 集成到现有架构。保留 Go bridge 作为透明降级方案。

**Tech Stack:** Rust, baileys crate, tokio, Vault API

---

## 文件结构

```
src/gateway/interfaces/whatsapp/
├── native_baileys/           # NEW
│   ├── mod.rs                 # 模块入口，导出公开类型
│   ├── client.rs              # NativeBaileysClient
│   ├── event.rs               # 事件映射
│   ├── message.rs             # 消息处理
│   ├── media.rs               # 媒体处理
│   ├── auth.rs                # Vault认证集成
│   └── errors.rs              # 错误类型
│
├── bridge_fallback.rs         # NEW: 降级逻辑
├── native_client.rs           # NEW: 统一接口
├── factory.rs                 # MODIFY
└── mod.rs                     # MODIFY

Cargo.toml                     # MODIFY: 添加 baileys 依赖
```

---

## Task 1: 初始化 native_baileys 模块

**Files:**
- Create: `src/gateway/interfaces/whatsapp/native_baileys/mod.rs`
- Create: `src/gateway/interfaces/whatsapp/native_baileys/errors.rs`
- Modify: `src/gateway/interfaces/whatsapp/Cargo.toml`

- [ ] **Step 1: 添加 baileys 依赖到 Cargo.toml**

```toml
# 在 aleph/Cargo.toml 或 gateway/Cargo.toml 中添加
[dependencies]
baileys = { version = "0.20", optional = true }
```

- [ ] **Step 2: 创建 errors.rs**

```rust
// src/gateway/interfaces/whatsapp/native_baileys/errors.rs
use thiserror::Error;

#[derive(Error, Debug)]
pub enum NativeBaileysError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    
    #[error("Authentication failed: {0}")]
    AuthFailed(String),
    
    #[error("Message send failed: {0}")]
    SendFailed(String),
    
    #[error("Vault error: {0}")]
    VaultError(String),
    
    #[error("Event mapping error: {0}")]
    EventMappingError(String),
    
    #[error("Media error: {0}")]
    MediaError(String),
}
```

- [ ] **Step 3: 创建 mod.rs**

```rust
// src/gateway/interfaces/whatsapp/native_baileys/mod.rs

mod auth;
mod client;
mod errors;
mod event;
mod media;
mod message;

pub use errors::NativeBaileysError;
pub use client::NativeBaileysClient;
pub use auth::AuthManager;
```

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/whatsapp/native_baileys/
git commit -m "feat(whatsapp): add native_baileys module structure"
```

---

## Task 2: 实现 AuthManager (Vault认证集成)

**Files:**
- Create: `src/gateway/interfaces/whatsapp/native_baileys/auth.rs`
- Test: `src/gateway/interfaces/whatsapp/native_baileys/tests/auth_test.rs`

- [ ] **Step 1: 编写 AuthManager 测试**

```rust
// src/gateway/interfaces/whatsapp/native_baileys/tests/auth_test.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_auth_manager_save_load() {
        // Mock Vault for testing
        let manager = AuthManager::new("test_account");
        
        let auth_data = WaAuthData {
            creds: test_creds(),
            keys: test_keys(),
            app_state_sync: vec![],
        };
        
        manager.save_auth(&auth_data).await.unwrap();
        
        let loaded = manager.load_auth().await.unwrap();
        assert_eq!(loaded.creds, auth_data.creds);
    }
}
```

- [ ] **Step 2: 实现 AuthManager 结构体**

```rust
// src/gateway/interfaces/whatsapp/native_baileys/auth.rs

use crate::NativeBaileysError;
use aleph_vault::VaultClient;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaAuthData {
    pub creds: Creds,
    pub keys: Keys,
    pub app_state_sync: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Creds {
    pub device_identity: Vec<u8>,
    pub session_id: String,
    pub noise_key: Vec<u8>,
    pub identity_key: Vec<u8>,
    pub signed_identity_key: Vec<u8>,
    pub registration_id: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keys {
    pub chat_state: Vec<u8>,
    pub session: Vec<u8>,
    pub sender_key: Vec<u8>,
    pub app_state_sync_key: Vec<u8>,
}

pub struct AuthManager {
    account_id: String,
    vault: VaultClient,
}

impl AuthManager {
    pub fn new(account_id: String) -> Self {
        Self {
            account_id,
            vault: VaultClient::new(),
        }
    }

    pub async fn save_auth(&self, auth: &WaAuthData) -> Result<(), NativeBaileysError> {
        let path = format!("whatsapp/auth/{}", self.account_id);
        let data = serde_json::to_vec(auth)
            .map_err(|e| NativeBaileysError::VaultError(e.to_string()))?;
        
        self.vault.write(&path, &data)
            .await
            .map_err(|e| NativeBaileysError::VaultError(e.to_string()))
    }

    pub async fn load_auth(&self) -> Result<WaAuthData, NativeBaileysError> {
        let path = format!("whatsapp/auth/{}", self.account_id);
        let data = self.vault.read(&path)
            .await
            .map_err(|e| NativeBaileysError::VaultError(e.to_string()))?;
        
        serde_json::from_slice(&data)
            .map_err(|e| NativeBaileysError::AuthFailed(e.to_string()))
    }
}
```

- [ ] **Step 3: 运行测试验证**

Run: `cargo test -p aleph-core native_baileys::auth`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/whatsapp/native_baileys/auth.rs
git commit -m "feat(whatsapp): implement AuthManager with Vault integration"
```

---

## Task 3: 实现 NativeBaileysClient

**Files:**
- Create: `src/gateway/interfaces/whatsapp/native_baileys/client.rs`
- Test: `src/gateway/interfaces/whatsapp/native_baileys/tests/client_test.rs`

- [ ] **Step 1: 编写 WhatsAppClient trait 测试 (mock)**

```rust
// src/gateway/interfaces/whatsapp/native_baileys/tests/client_test.rs

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_client_connect() {
        let config = test_config();
        let client = NativeBaileysClient::new(config).await.unwrap();
        assert!(client.is_connected());
    }
}
```

- [ ] **Step 2: 实现 NativeBaileysClient**

```rust
// src/gateway/interfaces/whatsapp/native_baileys/client.rs

use crate::{NativeBaileysError, AuthManager};
use baileys::{Baileys, BaileysConfig, Event};
use tokio::sync::mpsc;

pub struct NativeBaileysClient {
    baileys: Baileys,
    auth: AuthManager,
    event_tx: mpsc::Sender<BridgeEvent>,
    connected: Arc<AtomicBool>,
}

impl NativeBaileysClient {
    pub async fn new(
        config: WhatsAppConfig,
        event_tx: mpsc::Sender<BridgeEvent>,
    ) -> Result<Self, NativeBaileysError> {
        let auth = AuthManager::new(config.account_id.clone());
        
        // Load auth from Vault or create new
        let auth_data = match auth.load_auth().await {
            Ok(data) => data,
            Err(_) => {
                // First time setup - will need QR code
                return Err(NativeBaileysError::AuthFailed("No existing auth".into()));
            }
        };

        let baileys_config = BaileysConfig {
            auth: auth_data,
            ..Default::default()
        };

        let baileys = Baileys::new(baileys_config)
            .await
            .map_err(|e| NativeBaileysError::ConnectionFailed(e.to_string()))?;

        Ok(Self {
            baileys,
            auth,
            event_tx,
            connected: Arc::new(AtomicBool::new(true)),
        })
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    pub async fn send_message(&self, msg: OutboundMessage) -> Result<MessageId, NativeBaileysError> {
        self.baileys.send_message(msg.into_baileys())
            .await
            .map_err(|e| NativeBaileysError::SendFailed(e.to_string()))
    }

    pub async fn send_reaction(&self, msg_id: &MessageId, emoji: &str) -> Result<(), NativeBaileysError> {
        self.baileys.send_reaction(msg_id.as_str(), emoji)
            .await
            .map_err(|e| NativeBaileysError::SendFailed(e.to_string()))
    }

    pub async fn mark_read(&self, msg_id: &MessageId) -> Result<(), NativeBaileysError> {
        self.baileys.mark_read(msg_id.as_str())
            .await
            .map_err(|e| NativeBaileysError::SendFailed(e.to_string()))
    }

    pub async fn handle_qr_code<F>(&self, on_qr: F) -> Result<(), NativeBaileysError>
    where
        F: Fn(String),
    {
        // Handle QR code for new auth
        // This would be called during auth flow
        Ok(())
    }
}
```

- [ ] **Step 3: 实现 Event 处理循环**

```rust
impl NativeBaileysClient {
    pub async fn event_loop(&mut self) {
        let mut events = self.baileys.take_event_stream();
        
        while let Some(event) = events.next().await {
            match event {
                Event::MessagesUpsert(messages) => {
                    let bridge_event = map_baileys_to_bridge(event);
                    let _ = self.event_tx.send(bridge_event).await;
                }
                Event::ConnectionOpen => {
                    self.connected.store(true, Ordering::SeqCst);
                }
                Event::ConnectionClose => {
                    self.connected.store(false, Ordering::SeqCst);
                }
                _ => { /* ignore other events */ }
            }
        }
    }
}
```

- [ ] **Step 4: 运行测试验证**

Run: `cargo test -p aleph-core native_baileys::client`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/whatsapp/native_baileys/client.rs
git commit -m "feat(whatsapp): implement NativeBaileysClient"
```

---

## Task 4: 实现事件映射 (event.rs)

**Files:**
- Modify: `src/gateway/interfaces/whatsapp/native_baileys/event.rs`

- [ ] **Step 1: 编写事件映射测试**

```rust
#[test]
fn test_native_event_to_bridge_event() {
    use baileys::Event;
    
    let native_event = Event::MessagesUpsert(vec![test_message()]);
    let bridge_event = native_event_to_bridge_event(native_event);
    
    match bridge_event {
        BridgeEvent::Message(msg) => {
            assert_eq!(msg.id, "test_id");
        }
        _ => panic!("Expected Message event"),
    }
}
```

- [ ] **Step 2: 实现事件映射**

```rust
// src/gateway/interfaces/whatsapp/native_baileys/event.rs

use crate::{BridgeEvent, InboundMessage};
use baileys::Event;

pub fn native_event_to_bridge_event(event: Event) -> BridgeEvent {
    match event {
        Event::MessagesUpsert(messages) => {
            let inbound = messages
                .into_iter()
                .map(baileys_message_to_inbound)
                .collect();
            BridgeEvent::Messages(inbound)
        }
        Event::MessageUpdate { key, update } => {
            BridgeEvent::MessageUpdate(MessageUpdate {
                message_id: key.id,
                chat_id: key.remote,
                update,
            })
        }
        Event::PresenceUpdate { id, presences } => {
            BridgeEvent::PresenceUpdate(PresenceUpdate {
                contact_id: id,
                presences,
            })
        }
        Event::ConnectionOpen => BridgeEvent::Connected,
        Event::ConnectionClose { error } => {
            BridgeEvent::Disconnected(error.map(|e| e.to_string()))
        }
        _ => BridgeEvent::Unknown,
    }
}

fn baileys_message_to_inbound(msg: baileys::Message) -> InboundMessage {
    InboundMessage {
        id: msg.key.id,
        chat_id: msg.key.remote,
        from_me: msg.key.from_me,
        content: msg.message.to_string(),
        timestamp: msg.message_timestamp,
        media_url: msg.media_url,
        mime_type: msg.mime_type,
    }
}
```

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/whatsapp/native_baileys/event.rs
git commit -m "feat(whatsapp): implement event mapping"
```

---

## Task 5: 实现消息处理 (message.rs)

**Files:**
- Modify: `src/gateway/interfaces/whatsapp/native_baileys/message.rs`

- [ ] **Step 1: 实现 OutboundMessage 转换**

```rust
// src/gateway/interfaces/whatsapp/native_baileys/message.rs

use crate::{OutboundMessage, MessageId};
use baileys::{OutboundMessage as BaileysOutbound, Message};

impl From<OutboundMessage> for BaileysOutbound {
    fn from(msg: OutboundMessage) -> Self {
        match msg {
            OutboundMessage::Text { chat_id, content } => {
                BaileysOutbound::Text {
                    chat: chat_id,
                    content,
                }
            }
            OutboundMessage::Media { chat_id, media, caption } => {
                BaileysOutbound::Media {
                    chat: chat_id,
                    media,
                    caption,
                }
            }
            OutboundMessage::Reply { chat_id, content, quote_id } => {
                BaileysOutbound::Reply {
                    chat: chat_id,
                    content,
                    quote_id,
                }
            }
        }
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/gateway/interfaces/whatsapp/native_baileys/message.rs
git commit -m "feat(whatsapp): implement message conversion"
```

---

## Task 6: 实现媒体处理 (media.rs)

**Files:**
- Modify: `src/gateway/interfaces/whatsapp/native_baileys/media.rs`

- [ ] **Step 1: 实现 MediaProcessor**

```rust
// src/gateway/interfaces/whatsapp/native_baileys/media.rs

use crate::{NativeBaileysError, MediaContent};
use baileys::Message;

pub struct MediaProcessor {
    http_client: reqwest::Client,
}

impl MediaProcessor {
    pub fn new() -> Self {
        Self {
            http_client: reqwest::Client::new(),
        }
    }

    pub async fn download_media(&self, msg: &Message) -> Result<MediaContent, NativeBaileysError> {
        let url = msg.media_url
            .ok_or(NativeBaileysError::MediaError("No media URL".into()))?;
        
        let data = self.http_client.get(&url)
            .send()
            .await
            .map_err(|e| NativeBaileysError::MediaError(e.to_string()))?
            .bytes()
            .await
            .map_err(|e| NativeBaileysError::MediaError(e.to_string()))?;
        
        let mime = msg.mime_type
            .as_ref()
            .ok_or(NativeBaileysError::MediaError("No mime type".into()))?;
        
        Ok(MediaContent {
            data: data.to_vec(),
            mime_type: mime.clone(),
            file_name: msg.media_name.clone(),
        })
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/gateway/interfaces/whatsapp/native_baileys/media.rs
git commit -m "feat(whatsapp): implement media processor"
```

---

## Task 7: 实现 bridge_fallback (降级逻辑)

**Files:**
- Create: `src/gateway/interfaces/whatsapp/bridge_fallback.rs`

- [ ] **Step 1: 实现 FallbackManager**

```rust
// src/gateway/interfaces/whatsapp/bridge_fallback.rs

use crate::{NativeBaileysError, WhatsAppConfig, Box<dyn WhatsAppClient>};

pub struct FallbackManager {
    config: WhatsAppConfig,
}

impl FallbackManager {
    pub async fn connect(&self) -> Result<Box<dyn WhatsAppClient>, NativeBaileysError> {
        // 优先尝试 native
        #[cfg(feature = "native-baileys")]
        {
            match NativeBaileysClient::new(self.config.clone(), self.event_tx.clone()).await {
                Ok(client) => {
                    info!("Using native Baileys client");
                    return Ok(Box::new(client));
                }
                Err(e) => {
                    warn!("Native Baileys failed: {}, falling back to bridge", e);
                }
            }
        }
        
        // 降级到 Go bridge
        info!("Using Go bridge client");
        let bridge = BridgeClient::new(self.config).await?;
        Ok(Box::new(bridge))
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/gateway/interfaces/whatsapp/bridge_fallback.rs
git commit -m "feat(whatsapp): implement bridge fallback mechanism"
```

---

## Task 8: 更新 factory.rs 和 mod.rs

**Files:**
- Modify: `src/gateway/interfaces/whatsapp/factory.rs`
- Modify: `src/gateway/interfaces/whatsapp/mod.rs`

- [ ] **Step 1: 更新 factory.rs**

```rust
// src/gateway/interfaces/whatsapp/factory.rs

pub async fn create_whatsapp_channel(
    config: WhatsAppConfig,
) -> Result<WhatsAppChannel, Error> {
    let fallback = FallbackManager::new(config.clone());
    let client = fallback.connect().await?;
    
    Ok(WhatsAppChannel::new(client, config))
}
```

- [ ] **Step 2: 更新 mod.rs**

```rust
// src/gateway/interfaces/whatsapp/mod.rs

#[cfg(feature = "native-baileys")]
mod native_baileys;

#[cfg(feature = "native-baileys")]
pub use native_baileys::{NativeBaileysClient, AuthManager};
```

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/whatsapp/factory.rs src/gateway/interfaces/whatsapp/mod.rs
git commit -m "feat(whatsapp): integrate native Baileys via factory"
```

---

## Task 9: 更新 Cargo.toml 添加 feature

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: 添加 native-baileys feature**

```toml
[features]
default = []
native-baileys = ["dep:baileys"]
```

- [ ] **Step 2: Commit**

```bash
git add Cargo.toml
git commit -m "feat(whatsapp): add native-baileys feature flag"
```

---

## Task 10: 集成测试

**Files:**
- Create: `src/gateway/interfaces/whatsapp/tests/integration_test.rs`

- [ ] **Step 1: 编写集成测试**

```rust
// src/gateway/interfaces/whatsapp/tests/integration_test.rs

#[tokio::test]
async fn test_native_client_connect_and_send() {
    // 需要 mock Vault 和 WebSocket
    // 简化版本测试
}
```

- [ ] **Step 2: 运行 cargo check**

Run: `cargo check -p aleph-core --features native-baileys`
Expected: 无编译错误

- [ ] **Step 3: 运行单元测试**

Run: `cargo test -p aleph-core native_baileys`
Expected: 所有测试 PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/whatsapp/tests/
git commit -m "test(whatsapp): add integration tests"
```

---

## 验证清单

| 任务 | 验证 |
|------|------|
| Task 1 | `cargo check --features native-baileys` 无错误 |
| Task 2 | `cargo test auth_test` PASS |
| Task 3 | `cargo test client_test` PASS |
| Task 4 | `cargo test event_test` PASS |
| Task 5 | Message 转换测试 PASS |
| Task 6 | Media 下载测试 PASS |
| Task 7 | Fallback 逻辑正确 |
| Task 8 | Factory 正确选择 client |
| Task 9 | Feature flag 工作正常 |
| Task 10 | 集成测试 PASS |

---

## 实施顺序

1. Task 1 → Task 2 → Task 3 → Task 4 → Task 5 → Task 6 → Task 7 → Task 8 → Task 9 → Task 10
2. 建议每个 Task 独立分支
3. Review 后合并到 feature 分支

**Plan complete and saved to `docs/superpowers/plans/2026-04-12-whatsapp-native-implementation.md`**

---

## 执行选项

**1. Subagent-Driven (recommended)** - 我dispatch fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** - 在本session执行，使用executing-plans, batch execution with checkpoints

Which approach?
