# Channel 无实例测试架构设计

> 设计目标：在不连接真实平台实例的前提下，系统性地验证所有 Channel 实现的有效性。
> 
> 日期：2026-04-26

## 1. 问题背景

Aleph 支持 15+ 个消息平台 Channel。目前仅 Telegram 通过真实实例验证过，其余 Channel（Discord、Slack、WhatsApp、Matrix、Signal、LINE、Feishu、WeChat、QQ、IRC、XMPP、Nostr、Mattermost、MS Teams、Email、Webhook、iMessage）均缺乏实例化验证。需要一套无需真实平台连接即可验证 Channel 有效性的测试体系。

## 2. 设计原则

- **分层隔离**：接口契约、协议解析、业务逻辑三层独立测试
- **渐进验证**：从最简单（Webhook）到最复杂（WhatsApp/Signal）分批推进
- **零外部依赖**：不依赖真实平台账号、网络连接、或外部服务
- **可复用框架**：一次搭建，所有 Channel 复用

## 3. 核心架构

```
┌─────────────────────────────────────────────────────────────┐
│                    Channel 无实例测试框架                      │
├─────────────────────────────────────────────────────────────┤
│  Layer 3: 业务逻辑测试 (Integration)                          │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  fixture-based 消息流测试                                │  │
│  │  - 入站消息解析 → InboundMessage                         │  │
│  │  - 出站消息构造 → HTTP/WebSocket 请求                     │  │
│  │  - 状态机转换 (Disconnected → Connected → Error)         │  │
│  └───────────────────────────────────────────────────────┘  │
├─────────────────────────────────────────────────────────────┤
│  Layer 2: 协议解析测试 (Protocol)                             │
│  ┌─────────────────────┐  ┌───────────────────────────────┐ │
│  │  HTTP Mock 测试      │  │  WebSocket Mock 测试           │ │
│  │  (wiremock/mockito)  │  │  (tokio::net::TcpListener)     │ │
│  │  - 请求格式验证       │  │  - Gateway event 推送           │ │
│  │  - 响应解析验证       │  │  - 心跳/重连逻辑                │ │
│  │  - 错误处理路径       │  │  - 消息接收验证                 │ │
│  └─────────────────────┘  └───────────────────────────────┘ │
├─────────────────────────────────────────────────────────────┤
│  Layer 1: 接口契约测试 (Unit)                                 │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  Channel Trait 通用测试函数 test_channel_contract<C>()  │  │
│  │  - start()/stop() 状态转换                              │  │
│  │  - capabilities() 与实现一致性                           │  │
│  │  - send() 返回格式                                       │  │
│  │  - health() 初始状态                                     │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

## 4. 测试分层详细设计

### 4.1 Layer 1: 接口契约测试

**目标**：验证每个 Channel 对 `Channel` trait 的实现是否符合契约。

**实现**：

```rust
// tests/common/channel_contract.rs
pub async fn test_channel_contract<C: Channel>(mut channel: C) {
    // 初始状态
    assert_eq!(channel.status(), ChannelStatus::Disconnected);
    
    // start() 后状态
    match channel.start().await {
        Ok(()) => assert_eq!(channel.status(), ChannelStatus::Connected),
        Err(_) => assert!(matches!(
            channel.status(), 
            ChannelStatus::Error | ChannelStatus::Disabled
        )),
    }
    
    // capabilities 一致性检查
    let caps = channel.capabilities();
    if caps.typing_indicator {
        let result = channel.send_typing(&ConversationId::new("test")).await;
        assert!(!matches!(result, Err(ChannelError::UnsupportedFeature(_))));
    }
    
    // send() 返回格式
    let result = channel.send(OutboundMessage::text("test", "hello")).await;
    if let Ok(send_result) = result {
        assert!(!send_result.message_id.as_str().is_empty());
    }
    
    // stop() 后状态
    channel.stop().await.ok();
    assert_eq!(channel.status(), ChannelStatus::Disconnected);
}
```

**调用方式**：

```rust
#[tokio::test]
async fn test_slack_contract() {
    let channel = SlackChannel::new("test", mock_slack_config());
    test_channel_contract(channel).await;
}
```

### 4.2 Layer 2: 协议解析测试

#### 4.2.1 HTTP Mock 测试（REST-based Channel）

**适用 Channel**：Slack、LINE、Feishu、Mattermost、QQ、Webhook、MS Teams（REST 部分）

**关键改造**：为 Channel 的 HTTP client 添加可注入的 `base_url`：

```rust
// SlackChannel 新增构造函数
impl SlackChannel {
    pub fn with_client(
        id: impl Into<String>,
        config: SlackConfig,
        client: reqwest::Client,
        base_url: Option<String>,  // 测试时注入 mock server URL
    ) -> Self { ... }
}
```

**测试示例**：

```rust
#[tokio::test]
async fn test_slack_send_message_request_format() {
    let mock_server = MockServer::start().await;
    
    Mock::given(method("POST"))
        .and(path("/api/chat.postMessage"))
        .and(header("authorization", "Bearer xoxb-test"))
        .and(body_json(json!({
            "channel": "C12345",
            "text": "Hello"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "ts": "1234567890.123456"
        })))
        .mount(&mock_server)
        .await;
    
    let channel = SlackChannel::with_client(
        "test", test_config(), reqwest::Client::new(), 
        Some(mock_server.uri())
    );
    
    let result = channel.send(OutboundMessage::text("C12345", "Hello")).await;
    assert!(result.is_ok());
}
```

#### 4.2.2 WebSocket Mock 测试（WebSocket-based Channel）

**适用 Channel**：Discord、Matrix、Slack（Socket Mode）、Nostr

**实现**：基于 `tokio::net::TcpListener` 搭建 mock WebSocket server：

```rust
// tests/common/mock_ws.rs
pub struct MockWebSocket {
    events: Vec<WebSocketEvent>,
}

impl MockWebSocket {
    pub fn on_connect(self, event: WebSocketEvent) -> Self { ... }
    pub fn on_message(self, event: WebSocketEvent) -> Self { ... }
    
    pub async fn start(self) -> MockWebSocketServer { ... }
}

// 使用示例
#[tokio::test]
async fn test_discord_gateway_event() {
    let mock_ws = MockWebSocket::new()
        .on_connect(send_json!({"op": 10, "d": {"heartbeat_interval": 45000}}))
        .on_message(send_json!({
            "t": "MESSAGE_CREATE",
            "d": {"id": "123", "content": "Hello", "author": {"id": "456"}}
        }))
        .start().await;
    
    let mut channel = DiscordChannel::with_gateway_url(mock_ws.uri(), test_config());
    channel.start().await.unwrap();
    
    let msg = timeout(Duration::from_secs(5), 
        channel.inbound_subscribe().recv()
    ).await.unwrap().unwrap();
    
    assert_eq!(msg.text, "Hello");
    assert_eq!(msg.sender_id.as_str(), "456");
}
```

#### 4.2.3 TCP Mock 测试（Raw TCP Channel）

**适用 Channel**：IRC、XMPP、Email（IMAP/SMTP）

**实现**：使用 `tokio::net::TcpListener` 模拟服务器响应：

```rust
// tests/common/mock_tcp.rs
pub async fn mock_irc_server() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, format!("127.0.0.1:{}", port))
}

// 使用示例
#[tokio::test]
async fn test_irc_connection() {
    let (listener, addr) = mock_irc_server().await;
    
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let n = socket.read(&mut buf).await.unwrap();
        let msg = String::from_utf8_lossy(&buf[..n]);
        assert!(msg.contains("NICK"));
        assert!(msg.contains("USER"));
        
        socket.write_all(b":server 001 bot :Welcome\r\n").await.unwrap();
    });
    
    let mut channel = IrcChannel::with_server_addr("test", test_config(), &addr);
    channel.start().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Connected);
}
```

### 4.3 Layer 3: 业务逻辑测试

#### 4.3.1 Fixture 样本库

**目录结构**：

```
tests/fixtures/
├── slack/
│   ├── event_callback.json
│   ├── app_mention.json
│   ├── message_changed.json
│   └── reaction_added.json
├── discord/
│   ├── message_create.json
│   ├── interaction.json
│   └── guild_member_add.json
├── line/
│   ├── text_message.json
│   ├── image_message.json
│   └── follow_event.json
└── ...
```

**测试示例**：

```rust
#[test]
fn test_slack_event_parsing() {
    let json = include_str!("fixtures/slack/event_callback.json");
    let event: SlackEvent = serde_json::from_str(json).unwrap();
    
    let inbound = slack_to_inbound(event);
    assert_eq!(inbound.text, "Hello bot");
    assert_eq!(inbound.channel_id.as_str(), "slack");
    assert!(inbound.metadata.contains(&MessageMeta::AppMention));
}
```

**Fixture 来源**：
- Slack：官方 Event API 文档示例
- Discord：Discord API 文档 + serenity 测试 fixture
- LINE：LINE Messaging API 文档
- 其他：各平台官方开发者文档

#### 4.3.2 状态机测试

验证 Channel 状态转换的正确性：

```rust
#[tokio::test]
async fn test_channel_state_machine() {
    let mut channel = create_test_channel();
    
    // Disconnected → Connecting → Connected
    assert_eq!(channel.status(), ChannelStatus::Disconnected);
    
    let start_fut = channel.start();
    // 启动过程中应该是 Connecting
    assert_eq!(channel.status(), ChannelStatus::Connecting);
    
    start_fut.await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Connected);
    
    // Connected → Disconnected
    channel.stop().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Disconnected);
}
```

#### 4.3.3 错误处理测试

```rust
#[tokio::test]
async fn test_rate_limit_handling() {
    let mock_server = MockServer::start().await;
    
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429)
            .insert_header("Retry-After", "10")
            .set_body_json(json!({"ok": false, "error": "rate_limited"})))
        .mount(&mock_server).await;
    
    let channel = SlackChannel::with_client(
        "test", test_config(), reqwest::Client::new(),
        Some(mock_server.uri())
    );
    
    let result = channel.send(test_message()).await;
    assert!(matches!(result, Err(ChannelError::RateLimited { retry_after_secs: 10 })));
}
```

## 5. 实施路线图

### Phase 1: 基础设施（Week 1-2）

| 任务 | 输出 | 验收标准 |
|------|------|---------|
| 搭建通用契约测试框架 | `tests/common/channel_contract.rs` | 所有 Channel 可通过 `test_channel_contract()` |
| 搭建 HTTP Mock 工具 | `tests/common/mock_http.rs` | 支持 wiremock server 生命周期管理 |
| 搭建 WebSocket Mock 工具 | `tests/common/mock_ws.rs` | 支持 JSON event 推送和接收验证 |
| 搭建 TCP Mock 工具 | `tests/common/mock_tcp.rs` | 支持 IRC/XMPP 协议模拟 |
| 创建 Fixture 目录结构 | `tests/fixtures/<channel>/` | 每个 Channel 至少 3 个 fixture 样本 |

### Phase 2: Batch 1 - REST-based Channel（Week 3-4）

**目标 Channel**：Slack、Webhook、Email

| Channel | 协议 | 关键验证点 | 难度 |
|---------|------|-----------|------|
| Webhook | HTTP POST | 请求格式、回调处理 | ⭐ |
| Slack | Socket Mode + REST | chat.postMessage、event callback | ⭐⭐ |
| Email | IMAP + SMTP | 邮件解析、附件处理 | ⭐⭐ |

### Phase 3: Batch 2 - SDK-based Channel（Week 5-6）

**目标 Channel**：Discord、LINE、Mattermost

| Channel | 协议 | 关键验证点 | 难度 |
|---------|------|-----------|------|
| LINE | Webhook + REST | Messaging API、rich menu | ⭐⭐ |
| Mattermost | WebSocket + REST | API v4、线程回复 | ⭐⭐ |
| Discord | serenity WebSocket | Gateway event、slash command | ⭐⭐⭐ |

### Phase 4: Batch 3 - 企业平台（Week 7-8）

**目标 Channel**：Feishu、Matrix、MS Teams

| Channel | 协议 | 关键验证点 | 难度 |
|---------|------|-----------|------|
| Feishu | WebSocket + REST | 事件订阅、卡片消息 | ⭐⭐⭐ |
| Matrix | Matrix SDK | 房间事件、加密消息 | ⭐⭐⭐⭐ |
| MS Teams | streaminfo | streaming protocol、自适应卡片 | ⭐⭐⭐⭐ |

### Phase 5: Batch 4-5 - 复杂/受限平台（Week 9-12）

**目标 Channel**：WhatsApp、Signal、WeChat、QQ、IRC、XMPP、Nostr、iMessage

| Channel | 主要难点 | 测试策略 |
|---------|---------|---------|
| WhatsApp | 需 QR 认证 | Mock auth 流程，跳过配对验证 |
| Signal | 依赖 signal-cli | 模拟 signal-cli 的 REST 接口 |
| WeChat | 国内限制 | 基于文档的 fixture 测试 |
| QQ | 需验证 | 基于文档的 fixture 测试 |
| IRC | raw TCP | TCP mock 模拟 RFC 2812 |
| XMPP | 复杂协议 | TCP mock 模拟核心握手 |
| Nostr | WebSocket + 加密 | Mock relay，验证 NIP-01/04 |
| iMessage | macOS only | 仅在 macOS CI 上运行 |

## 6. 代码改造清单

为使 Channel 可测试，需进行以下最小化改造：

### 6.1 可注入 HTTP Client

```rust
// 为所有 HTTP-based Channel 添加
pub fn with_client(
    id: impl Into<String>,
    config: XxxConfig,
    client: reqwest::Client,
    base_url: Option<String>,  // 测试时注入 mock URL
) -> Self
```

### 6.2 可注入 WebSocket Gateway URL

```rust
// 为 WebSocket-based Channel 添加
pub fn with_gateway_url(
    gateway_url: String,
    config: XxxConfig,
) -> Self
```

### 6.3 可注入 TCP Server Address

```rust
// 为 TCP-based Channel 添加
pub fn with_server_addr(
    id: impl Into<String>,
    config: XxxConfig,
    server_addr: &str,
) -> Self
```

### 6.4 避免 unwrap() / expect()

确保 `start()`、`send()` 等方法在测试环境下不会因 unwrap 导致 panic，而是返回 `ChannelError`。

## 7. CI 集成

```yaml
# .github/workflows/channel-tests.yml
name: Channel Tests
on: [push, pull_request]

jobs:
  channel-contract:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Run channel contract tests
        run: cargo test --test channel_contract_test
        
  channel-protocol:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Run channel protocol tests
        run: cargo test --test '*_protocol_test'
        
  channel-fixture:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Run channel fixture tests
        run: cargo test --test '*_fixture_test'
```

## 8. 成功标准

| 指标 | 目标值 |
|------|--------|
| Channel trait 契约覆盖率 | 100%（所有 Channel） |
| REST-based Channel HTTP mock 覆盖率 | 100% |
| WebSocket-based Channel mock 覆盖率 | ≥80% |
| Fixture 样本数 | 每个 Channel ≥3 个 |
| CI 执行时间 | ≤5 分钟 |

## 9. 风险评估

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| 某些 Channel 代码不完整（stub） | 无法测试 | 标记为 `#[ignore]`，待实现后启用 |
| 平台文档 JSON 样本不准确 | 测试通过但真实环境失败 | 定期用真实实例样本更新 fixture |
| Mock 行为与真实平台不一致 | 假阳性 | 在真实环境可用时运行对比测试 |
| 改造引入回归 | 破坏现有功能 | 所有改造保持向后兼容 |

## 10. 参考

- `src/gateway/channel.rs` - Channel trait 定义
- `src/gateway/channel_registry.rs` - Channel 生命周期管理
- `src/gateway/interfaces/mod.rs` - 所有 Channel 注册入口
- `tests/link_acl_probe/mock_channel.rs` - 现有 Mock Channel 示例
- `src/gateway/proptest_channel.rs` - 属性测试示例
- [wiremock](https://docs.rs/wiremock/) - HTTP mock 库
- [tokio-test](https://docs.rs/tokio-test/) - 异步测试工具
