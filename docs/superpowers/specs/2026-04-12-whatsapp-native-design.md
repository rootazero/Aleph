# WhatsApp Native Rust Baileys 实现设计

**状态**: 已批准
**日期**: 2026-04-12
**范围**: 消息核心 (消息收发、基本事件处理、媒体处理)

---

## 1. 背景

Aleph 当前 WhatsApp 实现通过 Go bridge 中间层连接 Baileys，存在以下问题：
- 跨语言通信开销
- 事件同步复杂性
- 无法充分利用 Rust 异步优势
- 维护两套代码库

openclaw (TypeScript) 直接使用 `@whiskeysockets/baileys`，架构更简洁。

---

## 2. 目标

1. 实现原生 Rust Baileys 客户端
2. 保留 Go bridge 作为透明降级
3. 消息收发核心功能优先
4. 集成 Aleph Vault 认证存储

---

## 3. 架构设计

### 3.1 目录结构

```
src/gateway/interfaces/whatsapp/
├── native_baileys/           # 新增：原生Rust实现
│   ├── mod.rs                 # 模块入口
│   ├── client.rs              # Baileys协议客户端
│   ├── event.rs               # 事件类型映射
│   ├── message.rs             # 消息处理
│   ├── media.rs               # 媒体处理
│   ├── auth.rs                # Vault认证集成
│   └── errors.rs              # 错误类型
│
├── bridge_fallback.rs         # Go bridge降级逻辑
├── native_client.rs           # 统一客户端接口
├── factory.rs                 # 客户端工厂
├── mod.rs                     # 主模块 (修改)
│
└── [existing files...]        # 保留，逐步迁移

features:
  native-baileys = ["dep:baileys"]
```

### 3.2 核心组件

#### NativeBaileysClient
- 封装 `baileys` crate
- 实现 `WhatsAppClient` trait
- 管理连接生命周期

#### AuthManager
- Vault 存储加密认证数据
- 认证状态序列化/反序列化

#### EventMapper
- `native_event_to_bridge_event()`
- 保持与现有 event_loop 兼容

#### FallbackManager
- 检测 native 连接失败
- 自动切换到 Go bridge
- 对用户透明

### 3.3 事件流

```
Inbound:
  WhatsApp Server → Baileys → NativeBaileysClient → EventMapper → Channel event_loop

Outbound:
  Channel event_loop → NativeBaileysClient → WhatsApp Server
```

---

## 4. Feature Flag 设计

```toml
# Cargo.toml
[features]
default = []
native-baileys = ["dep:baileys"]
```

```rust
#[cfg(feature = "native-baileys")]
mod native_baileys;

#[cfg(feature = "native-baileys")]
pub use native_baileys::{NativeBaileysClient, AuthManager};
```

---

## 5. Vault 集成

### 5.1 认证数据存储

```rust
struct WaAuthData {
    creds: Creds,           // WhatsApp credentials
    keys: Keys,             // Encryption keys
    app_state_sync: Vec<u8>, // App state
}

// Vault storage path
const VAULT_PATH = "whatsapp/auth/{account_id}";
```

### 5.2 认证流程

1. 首次连接 → 创建新认证 → 存储到 Vault
2. 后续连接 → 从 Vault 加载 → 恢复会话
3. 认证失效 → 重新扫码 → 更新 Vault

---

## 6. 透明降级机制

### 6.1 降级触发条件

- Native 连接超时
- 认证失败
- 协议版本不兼容
- 运行时 panic

### 6.2 降级流程

```rust
async fn connect_with_fallback(config: &WhatsAppConfig) -> Result<Box<dyn WhatsAppClient>> {
    // 优先尝试 native
    if cfg!(feature = "native-baileys") {
        match NativeBaileysClient::new(config).await {
            Ok(client) => return Ok(client),
            Err(e) => {
                warn!("Native Baileys failed: {}, falling back to bridge", e);
            }
        }
    }
    
    // 降级到 Go bridge
    BridgeClient::new(config).await
}
```

---

## 7. 消息处理

### 7.1 Inbound 消息映射

| Baileys Event | Aleph InboundEvent |
|----------------|-------------------|
| messages.upsert | Message |
| message.update | MessageUpdate |
| reactions.update | ReactionUpdate |
| presence.update | PresenceUpdate |
| connection.open | Connected |
| connection.close | Disconnected |

### 7.2 Outbound 消息

```rust
impl WhatsAppClient for NativeBaileysClient {
    async fn send_message(&self, msg: OutboundMessage) -> Result<MessageId> {
        // Use baileys send_message
    }
    
    async fn send_reaction(&self, target: MessageId, emoji: &str) -> Result<()> {
        // Use baileys send_reaction
    }
    
    async fn mark_read(&self, message_id: &MessageId) -> Result<()> {
        // Use baileys chat.modify
    }
}
```

---

## 8. 媒体处理

### 8.1 下载流程 (Rust)

```rust
async fn download_media(&self, msg: &Message) -> Result<MediaContent> {
    // 1. Extract media URL from message
    // 2. Download via baileys
    // 3. Decode if needed (image/video)
    // 4. Return content
}
```

### 8.2 上传流程

复用现有 Go bridge 上传逻辑，通过 RPC 调用。

---

## 9. 迁移策略

### Phase 1: 独立模块 (不破坏现有代码)
- 新增 `native_baileys/` 模块
- Feature flag 控制编译
- 独立测试

### Phase 2: 并行运行
- 同一账号可切换 native/bridge
- 生产环境验证稳定性
- 收集问题

### Phase 3: 逐步接管
- Native 稳定后默认启用
- 减少 bridge 代码维护
- 最终废弃 bridge (可选)

---

## 10. 风险与缓解

| 风险 | 缓解措施 |
|------|----------|
| baileys crate 不稳定 | feature flag隔离，可禁用 |
| 协议版本落后 | 定期更新 baileys 版本 |
| Vault 集成复杂性 | 先实现文件存储，V2再集成Vault |
| 降级逻辑复杂 | 充分测试回退路径 |

---

## 11. 待确定

- [ ] baileys crate 版本锁定
- [ ] 错误类型细化
- [ ] 测试策略
- [ ] 具体实现任务分解

---

## 12. 参考

- openclaw WhatsApp 实现: `extensions/whatsapp/`
- Aleph 现有 WhatsApp: `src/gateway/interfaces/whatsapp/`
- baileys crate: https://crates.io/crates/baileys
