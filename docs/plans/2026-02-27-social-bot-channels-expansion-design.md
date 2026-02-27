# Social Bot Channels Expansion Design

> 参考 OpenFang（OpenClaw 的 Rust 复刻版）的 40 个社交 Bot Channel 实现，扩充 Aleph 的社交 Bot 能力。
> 不照搬，而是充分融合 Aleph 现有架构思想和代码实现。学习，超越。

**日期**: 2026-02-27
**状态**: Approved

---

## 背景

### Aleph 现状

Aleph 已有成熟的 Channel 抽象层：

- **`Channel` trait** (`gateway/channel.rs`) — 定义 `start/stop/send/inbound_receiver` 等核心方法
- **`ChannelFactory`** — 运行时从配置创建 channel 实例
- **`ChannelRegistry`** — 中央注册表，统一 inbound stream
- **`InboundMessageRouter`** — 权限检查 + Agent 路由
- **`ChannelCapabilities`** — 精细的能力矩阵声明

已实现 channel：CLI、Telegram、Discord、iMessage、WhatsApp (stub)、BridgedChannel。

### OpenFang 资源

OpenFang 在 `crates/openfang-channels/` 中实现了 40 个 channel adapter，共 ~23,000 行 Rust 代码。核心组件：

- **`ChannelAdapter` trait** — 类似 Aleph 的 `Channel`，但 `start()` 返回 `Stream<ChannelMessage>`
- **`BridgeManager`** — 类似 Aleph 的 `ChannelRegistry`
- **`AgentRouter`** — 类似 Aleph 的 `InboundMessageRouter`，带 specificity scoring
- **`formatter.rs`** — Markdown → 平台特定 markup 转换（Aleph 缺少）

### 关键差异

| 维度 | Aleph | OpenFang |
|------|-------|---------|
| Inbound 模式 | `inbound_receiver()` → mpsc | `start()` → `Stream<ChannelMessage>` |
| 能力声明 | `ChannelCapabilities` 结构体（精细） | 无显式能力矩阵 |
| 消息类型 | `OutboundMessage`（支持 inline_keyboard） | `ChannelContent` enum（更简洁） |
| 格式化 | 各 channel 自行处理 | 统一 `formatter.rs` |
| 密钥安全 | 普通 `String` | `Zeroizing<String>` |

---

## 设计原则

1. **直接适配 Aleph Channel trait** — 每个新 channel 实现 Aleph 的 `Channel` trait，复用 `ChannelRegistry`/`InboundMessageRouter`
2. **从 OpenFang 借鉴协议对接逻辑** — API 调用、WebSocket 握手、Webhook 验证等"脏活"直接参考
3. **Aleph 架构优势保持** — `ChannelCapabilities`、`ChannelStatus` 五态、JSON-RPC 控制面
4. **超越 OpenFang** — 统一 formatter、智能分段、secret zeroization、精细能力声明

---

## 基础设施层 (Phase 0)

### 1. 统一消息格式化器 (`MessageFormatter`)

**位置：** `core/src/gateway/formatter.rs`

**职责：** Markdown → 平台特定 markup 的统一转换。

```rust
pub enum MarkupFormat {
    Markdown,        // 原始 (Matrix, Discourse)
    TelegramHtml,    // <b>, <i>, <code>, <a>
    SlackMrkdwn,     // *bold*, _italic_, `code`, <url|text>
    DiscordMarkdown, // **bold**, *italic*, `code` (近似标准 Markdown)
    IrcFormatting,   // \x02 bold, \x1D italic, \x03 color
    PlainText,       // 纯文本 (SMS, Viber 等)
}

pub struct MessageFormatter;

impl MessageFormatter {
    /// Convert Markdown to platform-specific markup
    pub fn format(markdown: &str, target: MarkupFormat) -> String;

    /// Smart message splitting (respects paragraph/code block boundaries)
    pub fn split(text: &str, max_len: usize) -> Vec<String>;

    /// Normalize platform markup to standard Markdown (inbound direction)
    pub fn normalize(platform_text: &str, source: MarkupFormat) -> String;
}
```

**从 OpenFang 借鉴：** `formatter.rs` 的 Markdown→HTML 和 Markdown→mrkdwn 转换逻辑。

**Aleph 超越：**
- `split()` 智能分段（按段落/代码块边界分割，不在代码块中间断开）
- `normalize()` 入站方向归一化（各平台 markup → 标准 Markdown）

### 2. Webhook 通用接收器

**位置：** `core/src/gateway/webhook_receiver.rs`

**职责：** 为 webhook 型 channel（WhatsApp、LINE、Viber 等）提供共享的 HTTP server。

```rust
pub struct WebhookReceiver {
    router: axum::Router,
    port: u16,
    handlers: HashMap<String, Arc<dyn WebhookHandler>>,
}

pub trait WebhookHandler: Send + Sync {
    /// Verify webhook signature (HMAC-SHA256, etc.)
    fn verify(&self, headers: &HeaderMap, body: &[u8]) -> bool;

    /// Parse webhook payload into InboundMessages
    async fn handle(&self, body: Bytes) -> ChannelResult<Vec<InboundMessage>>;

    /// URL path for this handler (e.g., "/webhook/whatsapp")
    fn path(&self) -> &str;
}
```

**设计要点：**
- 复用 Aleph 已有的 axum 依赖，不引入新 HTTP 框架
- 每个 webhook channel 只需实现 `WebhookHandler` trait
- 共享端口，路径隔离

### 3. 安全增强：Secret Zeroization

**借鉴 OpenFang：** 所有 channel token 使用 `Zeroizing<String>`。

**实施：**
- 新增 `zeroize` 依赖（已在 OpenFang 验证，轻量）
- 现有 channel 的 `bot_token: String` 改为 `bot_token: Zeroizing<String>`
- 新 channel 直接采用

---

## Channel 实现规划

### Phase 1：HTTP 轮询/REST 型（验证基础设施）

| # | Channel | 协议 | 从 OpenFang 借鉴 | Aleph 增强 | 预计代码量 |
|---|---------|------|------------------|-----------|-----------|
| 1 | **Slack** | WebSocket (Socket Mode) + REST | 完整 adapter 逻辑、Socket Mode 连接 | `ChannelCapabilities` 精细声明、线程支持 | ~500 行 |
| 2 | **Email** | IMAP + SMTP | 收发逻辑、MIME 解析 | 附件→InboundMessage 映射、HTML 邮件格式化 | ~600 行 |
| 3 | **Matrix** | HTTP REST (/sync 长轮询) | sync token 管理、Room 路由 | E2EE 预留接口、Room→ConversationId 映射 | ~450 行 |

### Phase 2：WebSocket/流式型（复杂度递增）

| # | Channel | 协议 | 从 OpenFang 借鉴 | Aleph 增强 | 预计代码量 |
|---|---------|------|------------------|-----------|-----------|
| 4 | **Signal** | REST (signal-cli wrapper) | API 对接逻辑 | 安全性（E2EE 天然）、群组支持 | ~350 行 |
| 5 | **Mattermost** | WebSocket + REST | WebSocket 事件处理 | 自动重连+心跳、Channel 过滤 | ~400 行 |
| 6 | **IRC** | Raw TCP (RFC 2812) | 协议解析、NICK/JOIN/PRIVMSG | 多 channel 订阅、NickServ 认证 | ~450 行 |

### Phase 3：Webhook 接收型（需要 WebhookReceiver）

| # | Channel | 协议 | 从 OpenFang 借鉴 | Aleph 增强 | 预计代码量 |
|---|---------|------|------------------|-----------|-----------|
| 7 | **WhatsApp** (完善) | HTTP Webhook | 完整 adapter、签名验证 | 补全 Aleph 现有 stub | ~500 行 |
| 8 | **Webhook** (通用) | HTTP POST + HMAC | 完整 bidirectional 逻辑 | 作为"万能适配器"，支持自定义集成 | ~300 行 |

### Phase 4：补充通讯类

| # | Channel | 协议 | 从 OpenFang 借鉴 | Aleph 增强 | 预计代码量 |
|---|---------|------|------------------|-----------|-----------|
| 9 | **XMPP** | TCP (XMPP protocol) | Stanza 处理 | MUC 群组支持 | ~400 行 |
| 10 | **Nostr** | WebSocket (NIP-01 Relay) | 私钥签名、Relay 连接 | 多 Relay 支持、NIP-04 加密 DM | ~350 行 |

---

## 每个 Channel 的标准实现结构

```
core/src/gateway/interfaces/{channel_name}/
├── mod.rs           # Channel trait 实现 + ChannelFactory
├── config.rs        # {Channel}Config (serde + schemars)
└── message_ops.rs   # 消息转换、API 调用、媒体处理
```

与 Aleph 现有 Telegram/Discord/iMessage 的文件结构保持一致。

---

## Feature Flag 策略

每个新 channel 独立 feature flag，与现有模式一致：

```toml
[features]
slack = ["gateway"]
email = ["gateway", "lettre", "imap"]
matrix = ["gateway"]
signal = ["gateway"]
mattermost = ["gateway"]
irc = ["gateway"]
whatsapp = ["gateway"]       # 已存在，完善
webhook = ["gateway"]
xmpp = ["gateway", "xmpp-parsers"]
nostr = ["gateway", "nostr-sdk"]
```

---

## 数据流

### 入站

```
[平台 API / WebSocket / Webhook]
         ↓
[Channel Adapter]          ← 协议特定逻辑（从 OpenFang 借鉴）
         ↓
[MessageFormatter::normalize()]  ← 入站归一化
         ↓ InboundMessage（Aleph 统一类型）
[ChannelRegistry.inbound_tx]     ← 已有机制
         ↓
[InboundMessageRouter]           ← 权限检查 + Agent 路由
         ↓
[Agent Loop]                     ← Observe-Think-Act-Feedback
```

### 出站

```
[Agent Loop]
         ↓ OutboundMessage（Markdown 格式）
[MessageFormatter::format(markdown, target_markup)]  ← 新增
         ↓
[MessageFormatter::split(text, max_len)]             ← 按平台限制分段
         ↓
[Channel.send()]                                     ← 平台特定发送
```

**关键决策：** Agent 输出统一为 Markdown，由 `MessageFormatter` 在最后一步转换为平台格式。

---

## 错误处理

| 错误类型 | 处理策略 | 来源 |
|---------|---------|------|
| 认证失败 | 立即停止 channel，status → `Error`，通知用户 | OpenFang: fail-fast |
| 网络断连 | 指数退避重连（1s → 2s → 4s → ... → 60s cap） | OpenFang: backoff |
| 速率限制 | 遵守平台 Retry-After，排队等待 | Aleph 增强 |
| 消息过长 | `MessageFormatter::split()` 自动分段 | OpenFang: split_message |
| API 错误 | 记录错误，更新 `ChannelStatus.last_error`，不 panic | 两者共有 |

### 优雅关闭

统一采用 `watch::channel(false)` 模式（从 OpenFang 借鉴）：

```rust
struct ChannelImpl {
    shutdown_tx: watch::Sender<bool>,
}

impl Channel for ChannelImpl {
    async fn stop(&mut self) -> ChannelResult<()> {
        let _ = self.shutdown_tx.send(true);
        Ok(())
    }
}
```

---

## 测试策略

### 测试分层

| 层级 | 覆盖内容 | 方法 |
|------|---------|------|
| 单元测试 | `MessageFormatter` 转换、Config 解析、消息类型映射 | `#[cfg(test)]` 模块 |
| 集成测试 | Channel 生命周期（start→send→stop）、Registry 注册 | Mock HTTP server (wiremock) |
| 协议测试 | WebSocket 握手、Webhook 签名验证 | Mock server + 真实协议帧 |

### 每个 Channel 的最低测试要求

1. Config 序列化/反序列化
2. 消息格式化（Markdown → 平台格式）
3. 认证流程（token 验证）
4. 发送成功路径
5. 错误处理（网络错误、认证失败、速率限制）

### Mock 策略

```rust
#[cfg(test)]
mod tests {
    use wiremock::{MockServer, Mock, ResponseTemplate};

    #[tokio::test]
    async fn test_send_message() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/send"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(&mock_server)
            .await;

        let channel = TestChannel::new(config_with_url(mock_server.uri()));
        let result = channel.send(test_message()).await;
        assert!(result.is_ok());
    }
}
```

---

## "学习，超越" 总结

| 维度 | 从 OpenFang 学习 | Aleph 超越 |
|------|-----------------|-----------|
| 协议对接 | 40 个平台的 API 调用、WebSocket 握手、Webhook 验证 | — |
| 格式化 | `formatter.rs` (Markdown→HTML, Markdown→mrkdwn) | 双向转换 + 智能分段 |
| 密钥安全 | `Zeroizing<String>` | 统一到现有 channel |
| 优雅关闭 | `watch::channel` 模式 | 与 Aleph 现有机制融合 |
| 路由 | `AgentRouter` specificity scoring | 已有 `InboundMessageRouter`，保持 |
| 能力声明 | 无 | `ChannelCapabilities` 精细矩阵 |
| 状态管理 | `ChannelStatus` (简单) | 五态状态机 + RPC 控制 |
| 控制面 | 无 | JSON-RPC 2.0 管理所有 channel |
