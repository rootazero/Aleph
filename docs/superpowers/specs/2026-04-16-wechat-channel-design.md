# WeChat Channel Implementation Design

**Date:** 2026-04-16
**Status:** Approved
**Reference:** hermes-agent `gateway/platforms/weixin.py`

---

## 1. Overview

Implement a WeChat channel for Aleph using the Tencent iLink Bot API, aligned with hermes-agent's implementation while leveraging Rust's type safety, concurrency model, and architecture patterns.

**Scope:** Full feature parity with hermes-agent WeChat channel (DMs, group messages, media support, markdown conversion, context token management).

---

## 2. Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    ChannelRegistry                            │
│  (统一管理所有 Channel 的生命周期)                           │
└─────────────────────┬───────────────────────────────────────┘
                      │ create_channel("wechat")
                      ▼
┌─────────────────────────────────────────────────────────────┐
│                 WeChatChannel                                │
│  ┌─────────────────────────────────────────────────────┐  │
│  │              WeChatRuntime                            │  │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────────────┐   │  │
│  │  │ ILinkApi │  │PollingMgr│  │ MediaProcessor   │   │  │
│  │  │(HTTP客户端)│  │(长轮询)  │  │(AES-128解密)    │   │  │
│  │  └──────────┘  └──────────┘  └──────────────────┘   │  │
│  └─────────────────────────────────────────────────────┘  │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐     │
│  │ContextTokenStore│  │SyncBufManager│  │TypingCache  │     │
│  │(会话上下文)   │  │(增量同步)   │  │(打字状态)    │     │
│  └──────────────┘  └──────────────┘  └──────────────┘     │
└─────────────────────┬─────────────────────────────────────┘
                      │ inbound_subscribe()
                      ▼
┌─────────────────────────────────────────────────────────────┐
│              Unified Inbound Bus                             │
│  (ChannelRegistry 统一接收所有 Channel 消息)                  │
└─────────────────────────────────────────────────────────────┘
```

---

## 3. Module Structure

```
src/gateway/interfaces/wechat/
├── mod.rs                  # 模块入口，导出公开类型
├── config.rs               # WeChatConfig 配置结构
├── runtime.rs              # WeChatRuntime 核心运行时
├── api.rs                  # iLink API HTTP 客户端
├── types.rs                # iLink 消息类型映射
├── inbound/
│   ├── mod.rs
│   ├── mapper.rs           # iLink 消息 → InboundMessage
│   └── policy.rs           # DM/群聊访问策略
├── outbound/
│   ├── mod.rs
│   ├── mapper.rs           # OutboundMessage → iLink 格式
│   └── markdown.rs         # Markdown → 微信格式转换
├── media.rs                # 媒体下载/上传 + AES 解密
├── auth.rs                 # QR 登录、token 管理
├── sync_buf.rs             # 增量同步状态持久化
└── tests/
    └── *.rs                # 单元测试
```

---

## 4. Core Types

### 4.1 WeChatConfig

```rust
pub struct WeChatConfig {
    pub account_id: String,
    pub token: String,
    pub base_url: String,           // iLink 服务器地址
    pub cdn_base_url: String,        // 媒体 CDN 地址
    pub dm_policy: DmPolicy,         // open/allowlist/disabled
    pub group_policy: GroupPolicy,    // disabled/allowlist/open
    pub allow_from: Vec<String>,      // 允许的用户 ID 列表
    pub group_allow_from: Vec<String>, // 允许的群 ID 列表
    pub split_multiline_messages: bool,
}
```

### 4.2 WeChatChannel

Implements `Channel` trait:

| Method | Description |
|--------|-------------|
| `info()` | 返回 ChannelInfo |
| `status()` | Connected/Connecting/Disconnected/Error |
| `start()` | 启动轮询、恢复 token store |
| `stop()` | 优雅关闭 |
| `send(message)` | 发送 OutboundMessage |
| `inbound_subscribe()` | 返回消息订阅流 |
| `health()` | 健康检查 |

### 4.3 WeChatChannelFactory

```rust
pub struct WeChatChannelFactory;

impl ChannelFactory for WeChatChannelFactory {
    fn channel_type(&self) -> &str { "wechat" }
    async fn create(&self, config: Value) -> Result<Box<dyn Channel>>;
}
```

---

## 5. Key Components

### 5.1 ILink API Client (`api.rs`)

iLink API 端点：

| Endpoint | Purpose |
|----------|---------|
| `ilink/bot/getupdates` | 长轮询获取新消息 |
| `ilink/bot/sendmessage` | 发送消息 |
| `ilink/bot/sendtyping` | 发送打字状态 |
| `ilink/bot/getconfig` | 获取 typing ticket |
| `ilink/bot/getuploadurl` | 获取媒体上传 URL |
| `ilink/bot/get_bot_qrcode` | 获取 QR 码 |
| `ilink/bot/get_qrcode_status` | 查询 QR 扫码状态 |

请求头：
```
Content-Type: application/json
AuthorizationType: ilink_bot_token
X-WECHAT-UIN: <random_uin>
iLink-App-Id: bot
iLink-App-ClientVersion: <version>
Authorization: Bearer <token>
```

### 5.2 Context Token Store (`auth.rs`)

每个 (account_id, user_id) 独立存储 context_token，文件路径：
```
{hermes_home}/wechat/accounts/{account_id}.json
{hermes_home}/wechat/accounts/{account_id}.context-tokens.json
{hermes_home}/wechat/accounts/{account_id}.sync.json
```

### 5.3 Sync Buffer (`sync_buf.rs`)

持久化 `get_updates_buf` 增量同步状态，轮询时带上上一次的结果实现增量更新。

### 5.4 Media Processing (`media.rs`)

媒体类型常量：
```rust
MEDIA_IMAGE = 1
MEDIA_VIDEO = 2
MEDIA_FILE = 3
MEDIA_VOICE = 4
```

AES-128-ECB 解密流程：
1. 从 CDN 下载密文
2. 使用 `aeskey` 解密
3. 缓存到本地文件

### 5.5 Markdown Conversion (`outbound/markdown.rs`)

转换规则：
- `# 标题` → `【标题】`
- `## 标题` → `**标题**`
- `[链接](url)` → `链接 (url)`
- 表格 → 列表格式
- 代码块保持原样

消息分块：
- 最大 4000 字符
- 智能分块（代码块保持完整）
- 块间延迟 350ms

---

## 6. Policy

### 6.1 DM Policy

| Value | Behavior |
|-------|----------|
| `open` | 允许所有 DM |
| `allowlist` | 仅允许 allow_from 列表中的用户 |
| `disabled` | 禁用 DM |

### 6.2 Group Policy

| Value | Behavior |
|-------|----------|
| `open` | 允许所有群消息 |
| `allowlist` | 仅允许 group_allow_from 列表中的群 |
| `disabled` | 禁用群消息 |

---

## 7. Message Types

### 7.1 Inbound Message Types

```rust
ITEM_TEXT = 1      // 文本
ITEM_IMAGE = 2      // 图片
ITEM_VOICE = 3      // 语音
ITEM_FILE = 4       // 文件
ITEM_VIDEO = 5       // 视频
```

### 7.2 Message Type Mapping

| WeChat Type | Aleph MessageType |
|-------------|-------------------|
| text (starts with `/`) | COMMAND |
| text | TEXT |
| image | PHOTO |
| video | VIDEO |
| voice | VOICE |
| file | DOCUMENT |

---

## 8. Error Handling

| Error Code | Meaning | Recovery |
|-----------|---------|----------|
| -14 | Session expired | 暂停 10 分钟重试 |
| > MAX_FAILURES | 连续失败 | 进入退避模式 |

退避策略：
- 初始延迟：2 秒
- 最大延迟：30 秒
- 最大连续失败：3 次

---

## 9. Configuration

### 9.1 TOML Config

```toml
[[channels]]
id = "wechat"
channel_type = "wechat"
enabled = true

[channels.config]
account_id = "bot-account"
token = "your-token"
base_url = "https://ilinkai.weixin.qq.com"
cdn_base_url = "https://novac2c.cdn.weixin.qq.com/c2c"
dm_policy = "open"
group_policy = "disabled"
allow_from = []
group_allow_from = []
split_multiline_messages = false
```

### 9.2 Environment Variables

| Variable | Config Key | Default |
|----------|------------|---------|
| `WEIXIN_ACCOUNT_ID` | account_id | - |
| `WEIXIN_TOKEN` | token | - |
| `WEIXIN_BASE_URL` | base_url | iLink 默认地址 |
| `WEIXIN_CDN_BASE_URL` | cdn_base_url | 微信 CDN 默认地址 |
| `WEIXIN_DM_POLICY` | dm_policy | "open" |
| `WEIXIN_GROUP_POLICY` | group_policy | "disabled" |
| `WEIXIN_ALLOWED_USERS` | allow_from | - |
| `WEIXIN_GROUP_ALLOWED_USERS` | group_allow_from | - |

---

## 10. Capabilities

```rust
ChannelCapabilities {
    attachments: true,
    images: true,
    audio: true,
    video: true,
    reactions: false,       // 微信不支持
    replies: false,        // 微信不支持
    editing: false,         // 微信不支持
    deletion: false,       // 微信不支持
    typing_indicator: true,
    read_receipts: false,
    rich_text: true,
    max_message_length: 4000,
    max_attachment_size: 100 * 1024 * 1024, // 100MB
}
```

---

## 11. Differences from hermes-agent

| Aspect | hermes-agent (Python) | Aleph (Rust) |
|--------|----------------------|---------------|
| 类型安全 | dict, runtime errors | struct, compile-time |
| 并发 | asyncio | tokio async/await |
| 错误处理 | exceptions | thiserror Result |
| 资源清理 | GC | RAII + Drop |
| 多账号 | 单账号 | 原生多账号隔离 |
| 健康检查 | 无 | 内置 health() |
| 配置 | dict + env | ChannelConfig + env |

---

## 12. Dependencies

```toml
[dependencies]
# HTTP client (reuse existing)
reqwest = { version = "0.12", features = ["json"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# AES decryption (for media)
aes = "0.8"
cipher = "0.4"
base64 = "0.22"

# QR code rendering (for login)
qrcode = "0.22"
image = { version = "0.25", default-features = false }

# Error handling
thiserror = "2"
anyhow = "1"

# Async traits
async_trait = "0.1"
```

---

## 13. Implementation Checklist

- [ ] Create module structure under `src/gateway/interfaces/wechat/`
- [ ] Implement `WeChatConfig` with validation
- [ ] Implement `WeChatChannel` with `Channel` trait
- [ ] Implement `WeChatChannelFactory`
- [ ] Implement `api.rs` with iLink HTTP client
- [ ] Implement `auth.rs` with token persistence
- [ ] Implement `sync_buf.rs` for incremental sync
- [ ] Implement `media.rs` with AES decryption
- [ ] Implement `inbound/mapper.rs` for message mapping
- [ ] Implement `inbound/policy.rs` for access control
- [ ] Implement `outbound/mapper.rs` for outbound formatting
- [ ] Implement `outbound/markdown.rs` for markdown conversion
- [ ] Register factory in `interfaces/mod.rs`
- [ ] Add unit tests
- [ ] Update documentation

---

## 14. Reference

- hermes-agent: `/Volumes/TBU4/Github/hermes-agent/gateway/platforms/weixin.py`
- hermes-agent tests: `/Volumes/TBU4/Github/hermes-agent/tests/gateway/test_weixin.py`
- Aleph Channel trait: `/Volumes/TBU4/Workspace/Aleph/src/gateway/channel.rs`
- Aleph ChannelRegistry: `/Volumes/TBU4/Workspace/Aleph/src/gateway/channel_registry.rs`
- Aleph WhatsApp (similar pattern): `/Volumes/TBU4/Workspace/Aleph/src/gateway/interfaces/whatsapp/mod.rs`
