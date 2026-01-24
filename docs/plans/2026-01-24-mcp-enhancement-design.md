# MCP Enhancement Design

> 完整增强 Aether MCP 实现，达到与 OpenCode（Claude Code 开源实现）同等级功能

## 背景

对比分析 OpenCode 和 Aether 的 MCP 实现后，发现以下关键差距：

| 功能 | OpenCode | Aether | 重要性 |
|------|----------|--------|--------|
| 远程服务器支持 | StreamableHTTP + SSE | 仅 Stdio | 高 |
| OAuth 认证 | 完整实现 | 无 | 高 |
| 资源管理 | resources/list, read | 仅类型定义 | 中 |
| 提示模板 | prompts/list, get | 仅类型定义 | 中 |
| 工具变更通知 | ToolListChangedNotification | 未实现 | 中 |
| 每服务器超时配置 | 支持 | 全局固定 30s | 低 |
| HTTP 头支持 | 自定义头 | N/A | 高 |
| 重连机制 | 显式重连 API | 无 | 中 |

## 设计决策

- **传输层实现**：自行实现 HTTP/SSE 传输，保持与 Aether 自实现 AetherTool/AiProvider 风格一致
- **OAuth 回调服务器**：独立轻量级进程，避免主进程复杂化

---

## 模块结构

```
core/src/mcp/
├── mod.rs                    # 模块入口（更新）
├── types.rs                  # 类型定义（扩展）
├── client.rs                 # 客户端注册表（扩展）
├── jsonrpc.rs                # JSON-RPC 协议（保持）
├── transport/
│   ├── mod.rs                # 传输层抽象（新增 trait）
│   ├── stdio.rs              # Stdio 传输（保持）
│   ├── http.rs               # HTTP 传输（新增）
│   └── sse.rs                # SSE 传输（新增）
├── external/
│   ├── mod.rs                # 外部服务器管理
│   ├── connection.rs         # 连接管理（重构，支持多传输）
│   └── runtime.rs            # 运行时检测（保持）
├── auth/                     # OAuth 认证（新增目录）
│   ├── mod.rs                # 认证模块入口
│   ├── provider.rs           # OAuth 提供者实现
│   ├── storage.rs            # 凭证存储
│   └── callback.rs           # 回调服务器启动器
├── resources.rs              # 资源管理（新增）
├── prompts.rs                # 提示模板（新增）
└── notifications.rs          # 通知处理（新增）
```

---

## 详细设计

### 1. 传输层抽象

```rust
// transport/mod.rs
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// 发送请求并等待响应
    async fn request(&self, req: JsonRpcRequest) -> Result<JsonRpcResponse>;

    /// 发送通知（无响应）
    async fn notify(&self, notification: JsonRpcNotification) -> Result<()>;

    /// 检查连接是否存活
    async fn is_alive(&self) -> bool;

    /// 关闭连接
    async fn close(&self) -> Result<()>;

    /// 设置通知处理器（用于接收服务器推送的通知）
    fn set_notification_handler(&self, handler: Box<dyn NotificationHandler>);
}

pub trait NotificationHandler: Send + Sync {
    fn handle(&self, notification: JsonRpcNotification);
}
```

**三种传输实现**：

| 传输 | 请求方式 | 通知接收 | 适用场景 |
|------|---------|---------|---------|
| Stdio | stdin/stdout | stdout 轮询 | 本地进程 |
| HTTP | POST 请求 | 响应流/轮询 | 远程无状态 |
| SSE | POST + EventSource | SSE 事件流 | 远程有状态 |

HTTP 和 SSE 使用 `reqwest` 客户端（Aether 已依赖）。

### 2. OAuth 认证系统

#### 2.1 OAuth 提供者

```rust
// auth/provider.rs
pub struct McpOAuthProvider {
    server_name: String,
    server_url: String,
    config: OAuthConfig,
    storage: Arc<OAuthStorage>,
}

impl McpOAuthProvider {
    /// 获取/刷新访问令牌
    async fn get_access_token(&self) -> Result<String>;

    /// 启动授权流程，返回授权 URL
    async fn start_authorization(&self) -> Result<AuthorizationUrl>;

    /// 完成授权（用授权码换取令牌）
    async fn finish_authorization(&self, code: &str) -> Result<()>;

    /// 动态客户端注册（若服务器支持）
    async fn register_client(&self) -> Result<ClientInfo>;
}
```

#### 2.2 凭证存储

```rust
// auth/storage.rs
pub struct OAuthStorage {
    file_path: PathBuf,  // ~/.aether/data/mcp-auth.json
}

pub struct OAuthEntry {
    pub tokens: Option<OAuthTokens>,
    pub client_info: Option<ClientInfo>,
    pub code_verifier: Option<String>,   // PKCE
    pub oauth_state: Option<String>,      // CSRF 防护
    pub server_url: String,               // URL 绑定
}

pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,         // Unix 时间戳
    pub scope: Option<String>,
}

pub struct ClientInfo {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub client_id_issued_at: Option<i64>,
    pub client_secret_expires_at: Option<i64>,
}
```

- 文件权限：`0o600`（仅所有者可读写）
- 凭证与服务器 URL 绑定，URL 变更时凭证失效

#### 2.3 OAuth 回调服务器

- 独立轻量进程，监听 `127.0.0.1:19877`
- 接收授权码回调，通过 IPC 通知主进程
- 5 分钟超时自动退出
- CSRF 状态参数验证

#### 2.4 PKCE 支持

- 生成 code_verifier（128 字节随机）
- 计算 code_challenge（SHA256 + Base64URL）

### 3. 资源管理

```rust
// resources.rs
pub struct McpResourceManager {
    client: Arc<McpClient>,
}

impl McpResourceManager {
    /// 列出服务器的所有资源
    async fn list(&self, server: &str) -> Result<Vec<McpResource>>;

    /// 读取资源内容
    async fn read(&self, server: &str, uri: &str) -> Result<ResourceContent>;

    /// 列出所有服务器的资源（聚合）
    async fn list_all(&self) -> Result<HashMap<String, Vec<McpResource>>>;
}

pub enum ResourceContent {
    Text(String),
    Binary { data: Vec<u8>, mime_type: String },
    Image { data: Vec<u8>, mime_type: String },
}
```

### 4. 提示模板

```rust
// prompts.rs
pub struct McpPromptManager {
    client: Arc<McpClient>,
}

impl McpPromptManager {
    /// 列出服务器的所有提示模板
    async fn list(&self, server: &str) -> Result<Vec<McpPrompt>>;

    /// 获取提示内容（带参数替换）
    async fn get(
        &self,
        server: &str,
        name: &str,
        args: HashMap<String, String>
    ) -> Result<PromptContent>;

    /// 列出所有服务器的提示（聚合）
    async fn list_all(&self) -> Result<HashMap<String, Vec<McpPrompt>>>;
}

pub struct McpPrompt {
    pub name: String,
    pub description: Option<String>,
    pub arguments: Vec<PromptArgument>,
}

pub struct PromptArgument {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
}
```

### 5. 通知系统

```rust
// notifications.rs
pub struct McpNotificationRouter {
    event_bus: Arc<EventBus>,
}

impl NotificationHandler for McpNotificationRouter {
    fn handle(&self, notification: JsonRpcNotification) {
        match notification.method.as_str() {
            "notifications/tools/listChanged" => {
                self.event_bus.publish(McpEvent::ToolsChanged { server });
            }
            "notifications/resources/listChanged" => {
                self.event_bus.publish(McpEvent::ResourcesChanged { server });
            }
            "notifications/prompts/listChanged" => {
                self.event_bus.publish(McpEvent::PromptsChanged { server });
            }
            _ => tracing::debug!("Unknown MCP notification: {}", notification.method),
        }
    }
}
```

通知事件通过 Aether 的 `EventBus` 广播，UI 层和 Agent Loop 可订阅响应。

### 6. 配置增强

```rust
// config/types/tools.rs
pub enum McpServerConfig {
    Local(McpLocalConfig),
    Remote(McpRemoteConfig),
}

pub struct McpLocalConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub timeout_ms: Option<u64>,
    pub requires_runtime: Option<String>,
}

pub struct McpRemoteConfig {
    pub name: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub oauth: Option<OAuthConfig>,
    pub timeout_ms: Option<u64>,
    pub transport: TransportPreference,  // Http / Sse / Auto
}

pub struct OAuthConfig {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub scope: Option<String>,
}

pub enum TransportPreference {
    Auto,  // 先尝试 HTTP，失败则 SSE
    Http,
    Sse,
}
```

### 7. 连接状态与重连

```rust
pub enum McpConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    NeedsAuth,
    NeedsClientRegistration,
    Failed { error: String },
}

pub struct McpServerState {
    pub config: McpServerConfig,
    pub status: McpConnectionStatus,
    pub last_error: Option<String>,
    pub connected_at: Option<DateTime<Utc>>,
}

impl McpClient {
    /// 重新连接指定服务器
    pub async fn reconnect(&self, server_name: &str) -> Result<McpConnectionStatus>;

    /// 断开指定服务器
    pub async fn disconnect(&self, server_name: &str) -> Result<()>;

    /// 启动 OAuth 认证流程
    pub async fn authenticate(&self, server_name: &str) -> Result<AuthResult>;

    /// 获取所有服务器状态
    pub async fn server_statuses(&self) -> HashMap<String, McpServerState>;

    /// 热重载配置
    pub async fn reload_config(&self, new_config: McpConfig) -> Result<McpStartupReport>;
}

pub enum AuthResult {
    AlreadyAuthenticated,
    AuthorizationUrl(String),
    Failed { error: String },
}
```

### 8. FFI 接口扩展

为 Swift/Kotlin UI 层暴露：

```rust
impl AetherCore {
    // 现有接口保持不变...

    // 新增接口
    pub fn mcp_reconnect(&self, server_name: String) -> Result<McpConnectionStatus>;
    pub fn mcp_disconnect(&self, server_name: String) -> Result<()>;
    pub fn mcp_authenticate(&self, server_name: String) -> Result<AuthResult>;
    pub fn mcp_get_server_statuses(&self) -> Vec<McpServerState>;

    // 资源和提示
    pub fn mcp_list_resources(&self) -> Result<Vec<McpResourceInfo>>;
    pub fn mcp_read_resource(&self, server: String, uri: String) -> Result<String>;
    pub fn mcp_list_prompts(&self) -> Result<Vec<McpPromptInfo>>;
    pub fn mcp_get_prompt(&self, server: String, name: String, args: HashMap<String, String>) -> Result<String>;
}
```

---

## 实现阶段

### 阶段 1：传输层抽象（基础）

1. 抽取 `McpTransport` trait
2. 重构 `StdioTransport` 实现该 trait
3. 重构 `McpServerConnection` 使用 trait object
4. 确保现有功能不受影响
5. 单元测试验证

### 阶段 2：HTTP/SSE 传输（远程支持）

1. 实现 `HttpTransport`
2. 实现 `SseTransport`
3. 配置增强（远程服务器配置）
4. 传输自动选择逻辑
5. 集成测试

### 阶段 3：资源与提示模板

1. 实现 `McpResourceManager`
2. 实现 `McpPromptManager`
3. 通知处理系统
4. FFI 接口扩展
5. 与 Agent Loop 集成

### 阶段 4：OAuth 认证

1. 凭证存储（storage.rs）
2. OAuth 提供者（provider.rs）
3. 回调服务器（独立进程）
4. 动态客户端注册
5. PKCE 支持
6. 端到端测试

### 清理工作（贯穿各阶段）

- 移除旧的硬编码逻辑
- 统一错误处理
- 更新文档
- 清理未使用的代码

---

## 安全考虑

1. **OAuth 安全**
   - CSRF 状态参数验证
   - PKCE 防止授权码拦截
   - 凭证与服务器 URL 绑定

2. **凭证存储安全**
   - 文件权限 0o600
   - 敏感数据不写入日志

3. **传输安全**
   - 远程连接使用 HTTPS
   - 超时保护防止无限等待

---

## 测试策略

1. **单元测试**
   - 各传输层的协议正确性
   - OAuth 流程的各个步骤
   - 凭证存储的读写

2. **集成测试**
   - 本地 MCP 服务器连接
   - 远程 MCP 服务器连接（mock）
   - OAuth 完整流程（mock）

3. **手动测试**
   - 真实 MCP 服务器连接
   - OAuth 认证流程
   - UI 集成验证
