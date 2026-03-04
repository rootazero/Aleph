# OpenAI 订阅 Provider 设计

> 日期: 2026-03-04
> 状态: Approved

## 概述

为 Aleph 添加 ChatGPT 订阅账户作为 AI Provider 的支持。用户通过 OAuth 浏览器登录 OpenAI 账户，使用 ChatGPT 订阅额度（Plus/Pro）访问 GPT-4o、o3 等模型，而非通过 API Key 按量付费。

## 动机

- API Key 按 token 付费成本较高，订阅用户已有固定月费额度
- 订阅包含 ChatGPT 内置工具（Code Interpreter、DALL-E、Web Browsing）
- 扩展 Aleph 的 Provider 覆盖面，让订阅用户零额外成本使用

## 技术方案

### 方案选择

| 方案 | 描述 | 结论 |
|------|------|------|
| A. 原生协议适配器 | 纯 Rust 实现 ChatGPT backend-api 协议 | **采纳** |
| B. Extension Sidecar | Node.js 扩展处理认证，暴露本地 API | 备选 |
| C. 外部服务适配 | 用户自运行第三方服务 | 否决 |

采纳方案 A 的理由：与现有 Protocol Adapter 模式完美契合；一等公民体验；OAuth 浏览器流未来可复用。

## 架构设计

### 新增模块

```
core/src/providers/
├── protocols/
│   └── chatgpt.rs              # ChatGptAdapter: ProtocolAdapter 实现
├── chatgpt/
│   ├── mod.rs                  # 类型重导出
│   ├── types.rs                # 请求/响应结构体
│   ├── auth.rs                 # OAuth 浏览器流 + token 管理
│   └── security.rs             # CSRF / Requirements / Proof-of-Work
```

### 在 Provider 体系中的位置

```
                    ┌─────────────────┐
                    │   AiProvider    │  (trait)
                    └────────┬────────┘
                             │
                    ┌────────┴────────┐
                    │  HttpProvider   │  (generic wrapper)
                    └────────┬────────┘
                             │
        ┌────────────────────┼────────────────────┐
        │                    │                    │                    │
   OpenAiAdapter      AnthropicAdapter     GeminiAdapter      ChatGptAdapter ← NEW
        │                    │                    │                    │
   /v1/chat/          /v1/messages        /v1beta/models     /backend-api/
   completions                             :generateContent   conversation
```

### 配置方式

```toml
[providers.chatgpt-sub]
protocol = "chatgpt"
model = "gpt-4o"                    # ChatGPT 订阅模型
color = "#10a37f"
timeout_seconds = 120
enabled = true
# 无需 api_key — 通过 OAuth 认证
```

### Preset 条目

```rust
"chatgpt" => ProviderPreset {
    base_url: "https://chatgpt.com",
    protocol: "chatgpt",
    color: "#10a37f",
    default_model: "gpt-4o",
}
```

## OAuth 认证流程

### 浏览器 OAuth 流

```
用户点击"连接 ChatGPT"
        │
        ▼
Aleph 启动本地 HTTP 服务器 (localhost:随机端口)
        │
        ▼
打开系统浏览器 → https://auth0.openai.com/authorize
  ├── client_id: (OpenAI web client ID)
  ├── redirect_uri: http://localhost:{port}/callback
  ├── response_type: code
  ├── scope: openid profile email
  └── state: {random_nonce}
        │
        ▼
用户在浏览器中完成 OpenAI 登录（邮箱/Google/Microsoft/Apple）
        │
        ▼
浏览器重定向到 localhost:port/callback?code=xxx&state=xxx
        │
        ▼
Aleph 用 authorization_code 换取 access_token + refresh_token
        │
        ▼
关闭本地 HTTP 服务器，存储 token 到安全存储
```

### Token 管理 (`auth.rs`)

```rust
pub struct ChatGptAuth {
    access_token: SecureString,      // JWT, ~1h 有效期
    refresh_token: Option<SecureString>,
    expires_at: SystemTime,
    session_id: String,
}

impl ChatGptAuth {
    /// Launch OAuth browser flow, return auth info
    pub async fn authorize_via_browser() -> Result<Self>;

    /// Restore saved token from secure storage
    pub fn from_stored(config: &ProviderConfig) -> Option<Self>;

    /// Check token expiry, auto-refresh if needed
    pub async fn ensure_valid(&mut self) -> Result<&str>;

    /// Get current access_token
    pub fn access_token(&self) -> &str;
}
```

### Token 持久化

- 存储在 Aleph 的 Vault 系统中（`secret_name` 引用）
- 加密存储 access_token 和 refresh_token
- 启动时自动恢复，过期时自动刷新

## ChatGPT 协议适配器

### 请求格式 (`types.rs`)

```rust
/// ChatGPT backend-api conversation request
#[derive(Serialize)]
pub struct ChatGptRequest {
    pub action: String,                    // "next" | "continue"
    pub messages: Vec<ChatGptMessage>,
    pub model: String,                     // "gpt-4o", "o3", etc.
    pub conversation_id: Option<String>,   // for continued conversations
    pub parent_message_id: String,         // UUID, message chain
    pub timezone_offset_min: i32,
    pub conversation_mode: ConversationMode,
}

#[derive(Serialize)]
pub struct ChatGptMessage {
    pub id: String,                        // UUID
    pub author: Author,
    pub content: ChatGptContent,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct ConversationMode {
    pub kind: String,                      // "primary_assistant"
    pub plugin_ids: Option<Vec<String>>,   // code_interpreter, etc.
}
```

### 响应解析（SSE EventStream）

ChatGPT 后端返回的 SSE 格式：

```
data: {"message":{"id":"...","author":{"role":"assistant"},"content":{"content_type":"text","parts":["Hello"]},...},"conversation_id":"...","error":null}

data: {"message":{"id":"...","content":{"content_type":"text","parts":["Hello world"]},...}}

data: [DONE]
```

### 安全层 (`security.rs`)

```rust
pub struct ChatGptSecurity {
    csrf_token: String,
    requirements_token: String,
}

impl ChatGptSecurity {
    /// Fetch CSRF token
    pub async fn fetch_csrf(client: &Client) -> Result<String>;

    /// Fetch chat-requirements (includes proof-of-work params)
    pub async fn fetch_requirements(
        client: &Client,
        access_token: &str,
    ) -> Result<ChatRequirements>;

    /// Compute Proof-of-Work
    pub fn solve_proof_of_work(seed: &str, difficulty: &str) -> Result<String>;
}
```

### ProtocolAdapter 实现

```rust
pub struct ChatGptAdapter;

#[async_trait]
impl ProtocolAdapter for ChatGptAdapter {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool,
    ) -> Result<RequestBuilder> {
        // 1. Get/refresh access_token
        // 2. Fetch CSRF + requirements + PoW
        // 3. Build ChatGptRequest
        // 4. POST to /backend-api/conversation
        //    Headers: Authorization: Bearer {token}
        //             X-Csrf-Token: {csrf}
        //             Openai-Sentinel-Chat-Requirements-Token: {req_token}
        //             Openai-Sentinel-Proof-Token: {pow_token}
    }

    async fn parse_response(&self, response: Response) -> Result<String> {
        // Parse final message.content.parts
    }

    async fn parse_stream(&self, response: Response) -> Result<BoxStream<'_, Result<String>>> {
        // Parse SSE line by line, extract incremental text parts
    }

    fn name(&self) -> &'static str { "chatgpt" }
}
```

### AiProvider 方法映射

| AiProvider 方法 | ChatGPT 实现 |
|---|---|
| `process` | 单次对话，新建 conversation |
| `process_with_image` | messages 中嵌入 image_url 类型 |
| `process_with_attachments` | 支持图片/文件附件 |
| `process_with_thinking` | 选择 o3/o4-mini 等推理模型 |
| `supports_vision` | true (GPT-4o) |
| `supports_thinking` | true (o3/o4-mini) |

## 工具支持

### ChatGPT 内置工具映射

ChatGPT 订阅包含的内置工具通过 `conversation_mode` 控制：

| ChatGPT 工具 | 映射到 Aleph |
|---|---|
| Code Interpreter | 自动启用，响应中含 `code` content_type |
| DALL-E | 响应中含 `image_asset_pointer` |
| Web Browsing | 响应中含 `tether_browsing_display_result` |
| File Upload | 通过 multipart 上传到 `/backend-api/files` |

**设计决策**：ChatGPT 的工具响应作为**纯文本/图片结果**返回给 Aleph Agent Loop，而非映射到 Aleph 自己的工具系统。ChatGPT 内置工具是黑盒执行，Aleph 只消费其结果。

## 会话管理

```rust
/// ChatGPT conversation tracker
pub struct ConversationTracker {
    /// conversation_id → parent_message_id mapping
    conversations: HashMap<String, String>,
}
```

- **新对话**：`conversation_id = None`, `parent_message_id = uuid::new_v4()`
- **续对话**：使用上次响应的 `conversation_id` 和 `message.id`
- **映射关系**：一个 Aleph session 对应一个 ChatGPT conversation

## 错误处理 & 弹性

| 场景 | 处理策略 |
|---|---|
| Token 过期 (401) | 自动刷新 token，重试一次 |
| 速率限制 (429) | 返回友好消息："订阅额度已用尽，请稍后再试" |
| PoW 参数变化 | 重新获取 requirements，重试 |
| 安全流程大改 | 记录详细错误，提示用户检查更新 |
| 网络错误 | 标准重试逻辑（已有 HttpProvider 支持） |

## 模型列表

通过 `GET /backend-api/models` 获取当前订阅可用的模型列表，用于：
- 配置校验
- UI 模型选择器
- 自动选择最佳模型

## 风险 & 缓解

| 风险 | 严重度 | 缓解措施 |
|------|--------|----------|
| OpenAI 改变后端 API | 高 | 安全层模块化，便于快速更新；错误信息明确提示版本不兼容 |
| TOS 合规性 | 中 | 仅供个人使用；配置中明确标注"非官方" |
| TLS 指纹检测 | 中 | 使用 reqwest 的标准 TLS，如不够可切换到 rustls 自定义指纹 |
| Proof-of-Work 算法变更 | 中 | PoW 求解独立模块，便于替换 |

## 参考资料

- [ChatGPTReversed](https://github.com/gin337/ChatGPTReversed) — 教育项目，逆向 ChatGPT 前端 API
- [reverse-engineered-chatgpt](https://github.com/Zai-Kun/reverse-engineered-chatgpt) — Python 实现（已归档）
- [acheong08/ChatGPT](https://github.com/acheong08/ChatGPT) — 早期逆向工程项目
