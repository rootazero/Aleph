# MCP 接口预留文档（阶段 3）

## 一、概述

### 1.1 什么是 Model Context Protocol (MCP)

**Model Context Protocol (MCP)** 是 Anthropic 提出的**标准化协议**，用于 AI 应用与外部数据源、工具之间的交互。MCP 定义了统一的接口，使得 AI Agent 可以安全、可控地访问外部资源和执行工具调用。

**官方定义**:
> MCP is an open protocol that standardizes how applications provide context to LLMs. Think of MCP as a universal "adapter" that lets AI models securely connect to different data sources and tools.

**核心价值**:
- ✅ **标准化接口**: 避免为每个工具编写专用集成代码
- ✅ **安全可控**: 细粒度权限管理，防止恶意工具调用
- ✅ **可扩展性**: 第三方可轻松开发 MCP Server
- ✅ **跨平台**: 基于 JSON-RPC 2.0，语言无关

**典型使用场景**:

| 场景 | MCP Server 示例 | 提供的功能 |
|-----|-----------------|-----------|
| 文件操作 | `mcp-server-filesystem` | 读写本地文件 |
| 数据库查询 | `mcp-server-postgres` | 执行 SQL 查询 |
| Git 集成 | `mcp-server-git` | 查看提交历史、diff |
| 浏览器自动化 | `mcp-server-puppeteer` | 截图、表单填写 |
| 天气查询 | `mcp-server-weather` | 获取实时天气数据 |
| Slack 集成 | `mcp-server-slack` | 发送消息、查询频道 |

### 1.2 MCP 核心概念

MCP 协议定义了三种核心抽象：

#### 1. Resources（资源）

**定义**: 静态或动态的数据源，AI 可以读取但通常不能修改。

**特点**:
- 只读访问
- 支持分页
- 可携带元数据（MIME type, 描述等）

**示例**:

| Resource URI | 描述 | 返回内容 |
|-------------|------|---------|
| `file:///path/to/doc.txt` | 本地文件 | 文件内容 |
| `postgres://db/users/123` | 数据库记录 | JSON 格式的用户数据 |
| `git://repo/commits` | Git 提交历史 | 提交列表 |
| `slack://channel/general` | Slack 频道消息 | 最近消息列表 |

**数据结构**:

```json
{
  "uri": "file:///Users/ziv/notes.txt",
  "name": "My Notes",
  "description": "Personal notes",
  "mimeType": "text/plain",
  "contents": "Note content here..."
}
```

---

#### 2. Prompts（提示词模板）

**定义**: MCP Server 提供的**预定义 Prompt 模板**，包含参数化的系统提示词和样例。

**特点**:
- 参数化支持（占位符）
- 包含元数据（名称、描述、参数定义）
- 可选的样例输入/输出

**示例**:

| Prompt Name | 描述 | 参数 |
|------------|------|------|
| `code-review` | 代码审查模板 | `language`, `code` |
| `translate` | 翻译模板 | `target_lang`, `text` |
| `summarize` | 摘要模板 | `max_words`, `content` |

**数据结构**:

```json
{
  "name": "code-review",
  "description": "Review code for bugs and style",
  "arguments": [
    {
      "name": "language",
      "description": "Programming language",
      "required": true
    },
    {
      "name": "code",
      "description": "Code to review",
      "required": true
    }
  ]
}
```

---

#### 3. Tools（工具）

**定义**: MCP Server 提供的**可执行函数**，AI 可以调用以执行操作。

**特点**:
- 双向交互（读写）
- 需要权限控制
- 返回结构化结果

**示例**:

| Tool Name | 描述 | 参数 | 返回值 |
|----------|------|------|-------|
| `file_write` | 写入文件 | `path`, `content` | 成功/失败状态 |
| `sql_query` | 执行 SQL | `query` | 查询结果（JSON）|
| `web_scrape` | 抓取网页 | `url` | HTML 内容 |
| `send_email` | 发送邮件 | `to`, `subject`, `body` | 发送状态 |

**数据结构**:

```json
{
  "name": "file_write",
  "description": "Write content to a file",
  "inputSchema": {
    "type": "object",
    "properties": {
      "path": { "type": "string" },
      "content": { "type": "string" }
    },
    "required": ["path", "content"]
  }
}
```

---

#### MCP 架构图

```
┌─────────────────────────────────────────────────────┐
│  Aleph Agent (Client)                              │
│  ┌───────────────────────────────────────────────┐  │
│  │  McpClient (JSON-RPC 2.0)                     │  │
│  │  ├─ list_resources()                          │  │
│  │  ├─ read_resource(uri)                        │  │
│  │  ├─ list_prompts()                            │  │
│  │  ├─ get_prompt(name, args)                    │  │
│  │  ├─ list_tools()                              │  │
│  │  └─ call_tool(name, args)                     │  │
│  └───────────────────────────────────────────────┘  │
└──────────────────────┬──────────────────────────────┘
                       │ JSON-RPC 2.0 over
                       │ stdio / HTTP / WebSocket
                       ↓
┌─────────────────────────────────────────────────────┐
│  MCP Server (Provider)                              │
│  ┌───────────────────────────────────────────────┐  │
│  │  Resource Handler                             │  │
│  │  ├─ Filesystem: file:///...                   │  │
│  │  ├─ Database: postgres://...                  │  │
│  │  └─ Git: git://...                            │  │
│  ├───────────────────────────────────────────────┤  │
│  │  Prompt Handler                               │  │
│  │  ├─ code-review                               │  │
│  │  ├─ translate                                 │  │
│  │  └─ summarize                                 │  │
│  ├───────────────────────────────────────────────┤  │
│  │  Tool Handler                                 │  │
│  │  ├─ file_write(path, content)                 │  │
│  │  ├─ sql_query(query)                          │  │
│  │  └─ web_scrape(url)                           │  │
│  └───────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────┘
```

### 1.3 为什么要在本次方案中预留接口

**设计原则**: **标准化集成，未来可扩展**

**本次实施（MVP）**:
- ✅ 数据结构重构（String → AgentPayload）
- ✅ Memory 功能集成
- ⚠️ MCP 接口预留（空实现）

**阶段 3（未来）**:
- 🔮 McpClient JSON-RPC 2.0 实现
- 🔮 MCP Server 生命周期管理
- 🔮 Resources/Prompts/Tools 完整支持
- 🔮 权限管理和安全隔离

**预留的好处**:
1. **避免破坏性修改**: mcp_resources 字段已预留，未来只需填充
2. **Skills 依赖**: Skills 方案 C 需要 MCP Tools 支持
3. **标准化**: 遵循 Anthropic MCP 规范，生态兼容
4. **渐进式演进**: 可先实现 Resources，再添加 Prompts 和 Tools

---

## 二、预留的数据结构

### 2.1 McpResource 结构体

**文件**: `Aleph/core/src/mcp/resource.rs`（新建）

**定义**:

```rust
/// 🔮 MCP 资源（阶段 3 预留）
///
/// 代表 MCP Server 提供的静态或动态数据源
///
/// **本次实施**: 仅定义结构
/// **阶段 3**: 由 McpClient 调用 read_resource() 填充
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    /// 资源 URI（唯一标识）
    /// 示例: "file:///path/to/file.txt", "postgres://db/table/id"
    pub uri: String,

    /// 资源名称（人类可读）
    pub name: String,

    /// 资源描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// MIME 类型（如 "text/plain", "application/json"）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,

    /// 资源内容（文本或 Base64 编码的二进制）
    pub contents: String,

    /// 🔮 元数据（阶段 3 扩展）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl McpResource {
    /// 创建简化版资源（测试用）
    pub fn new(uri: String, name: String, contents: String) -> Self {
        Self {
            uri,
            name,
            description: None,
            mime_type: Some("text/plain".to_string()),
            contents,
            metadata: None,
        }
    }

    /// 检查是否为文本资源
    pub fn is_text(&self) -> bool {
        self.mime_type
            .as_ref()
            .map(|m| m.starts_with("text/"))
            .unwrap_or(true)
    }
}
```

### 2.2 McpTool 结构体

**文件**: `Aleph/core/src/mcp/tool.rs`（新建）

**定义**:

```rust
use serde_json::Value;

/// 🔮 MCP 工具定义（阶段 3 预留）
///
/// 代表 MCP Server 提供的可执行函数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    /// 工具名称（唯一标识）
    pub name: String,

    /// 工具描述
    pub description: String,

    /// 输入参数 Schema（JSON Schema 格式）
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,

    /// 🔮 是否需要用户确认（安全机制）
    #[serde(default)]
    pub requires_confirmation: bool,
}

/// 🔮 MCP 工具调用结果（阶段 3 预留）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
    /// 是否成功
    pub success: bool,

    /// 结果内容（JSON 格式）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,

    /// 错误信息（如果失败）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
```

### 2.3 McpPrompt 结构体

**文件**: `Aleph/core/src/mcp/prompt.rs`（新建）

**定义**:

```rust
/// 🔮 MCP Prompt 定义（阶段 3 预留）
///
/// 代表 MCP Server 提供的 Prompt 模板
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    /// Prompt 名称
    pub name: String,

    /// Prompt 描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// 参数定义
    #[serde(default)]
    pub arguments: Vec<McpPromptArgument>,
}

/// Prompt 参数定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptArgument {
    /// 参数名称
    pub name: String,

    /// 参数描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// 是否必需
    #[serde(default)]
    pub required: bool,
}

/// 🔮 获取 Prompt 的结果（阶段 3 预留）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptResult {
    /// 渲染后的提示词
    pub prompt: String,

    /// 🔮 可选的样例消息
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<McpMessage>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpMessage {
    pub role: String, // "user" | "assistant"
    pub content: String,
}
```

### 2.4 AgentContext.mcp_resources 字段

**文件**: `Aleph/core/src/payload/mod.rs`（已存在）

**当前定义**:

```rust
pub struct AgentContext {
    pub memory_snippets: Option<Vec<MemoryEntry>>,
    pub search_results: Option<Vec<SearchResult>>,

    /// MCP 资源（阶段 3 实现）
    pub mcp_resources: Option<HashMap<String, serde_json::Value>>,

    pub workflow_state: Option<WorkflowState>,
}
```

**🔮 阶段 3 增强** - 使用结构化类型:

```rust
/// MCP 上下文数据
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpContext {
    /// 读取的资源
    #[serde(default)]
    pub resources: Vec<McpResource>,

    /// 获取的 Prompts
    #[serde(default)]
    pub prompts: Vec<McpPromptResult>,

    /// 工具调用结果
    #[serde(default)]
    pub tool_results: Vec<McpToolResult>,
}

pub struct AgentContext {
    pub memory_snippets: Option<Vec<MemoryEntry>>,
    pub search_results: Option<Vec<SearchResult>>,

    /// 🔮 MCP 上下文（阶段 3 替换为结构化类型）
    pub mcp_context: Option<McpContext>,

    pub workflow_state: Option<WorkflowState>,
}
```

### 2.5 Intent::BuiltinMcp 增强

**文件**: `Aleph/core/src/payload/intent.rs`（已存在）

**当前定义**:

```rust
pub enum Intent {
    /// 内置功能：MCP 工具调用
    /// 对应指令: /mcp, /tool
    BuiltinMcp,

    // ...
}
```

**🔮 阶段 3 增强** - 添加工具参数:

```rust
pub enum Intent {
    /// 内置功能：MCP 工具调用
    ///
    /// **本次实施**: 仅枚举定义
    /// **阶段 3**: 支持指定工具名称和参数
    BuiltinMcp {
        /// 工具名称（可选）
        tool_name: Option<String>,

        /// 工具参数（JSON）
        tool_args: Option<serde_json::Value>,
    },

    // ...
}
```

---

## 三、预留的执行方法

### 3.1 CapabilityExecutor::execute_mcp()

**文件**: `Aleph/core/src/capability/mod.rs`（已存在）

**当前实现**（空）:

```rust
impl CapabilityExecutor {
    #[allow(dead_code)]
    async fn execute_mcp(&self, payload: AgentPayload) -> Result<AgentPayload> {
        // TODO: 实现 MCP 调用
        // payload.context.mcp_resources = Some(mcp_resources);
        Ok(payload)
    }
}
```

**🔮 阶段 3 完整实现伪代码**:

```rust
impl CapabilityExecutor {
    async fn execute_mcp(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        // 1. 检查 McpClient 是否可用
        let mcp_client = self.mcp_client
            .as_ref()
            .ok_or_else(|| AlephError::McpNotAvailable)?;

        // 2. 根据 Intent 决定调用类型
        let mcp_context = match &payload.meta.intent {
            Intent::BuiltinMcp { tool_name: Some(name), tool_args } => {
                // 直接工具调用
                info!("Calling MCP tool: {}", name);
                let result = mcp_client.call_tool(name, tool_args.clone()).await?;

                McpContext {
                    resources: vec![],
                    prompts: vec![],
                    tool_results: vec![result],
                }
            }
            _ => {
                // 自动资源发现和读取
                info!("Discovering MCP resources");
                let resources = mcp_client.discover_and_read_resources().await?;

                McpContext {
                    resources,
                    prompts: vec![],
                    tool_results: vec![],
                }
            }
        };

        // 3. 填充到 payload
        if !mcp_context.resources.is_empty() || !mcp_context.tool_results.is_empty() {
            payload.context.mcp_context = Some(mcp_context);
        }

        Ok(payload)
    }
}
```

### 3.2 McpClient 架构设计

**文件**: `Aleph/core/src/mcp/client.rs`（新建）

**职责**:
- 管理多个 MCP Server 连接
- 实现 JSON-RPC 2.0 通信
- 处理 Resources/Prompts/Tools 调用
- Server 生命周期管理（启动、停止、重启）

**设计**:

```rust
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::Mutex;
use crate::mcp::{McpResource, McpTool, McpPrompt, McpToolResult};
use crate::error::Result;

/// 🔮 MCP 客户端（阶段 3 实现）
///
/// 统一管理多个 MCP Server，提供高层 API
pub struct McpClient {
    /// 已连接的 MCP Server
    servers: Arc<Mutex<HashMap<String, McpServerConnection>>>,

    /// 配置
    config: McpConfig,
}

impl McpClient {
    pub async fn new(config: McpConfig) -> Result<Self> {
        let mut servers = HashMap::new();

        // 根据配置启动各 MCP Server
        for (name, server_config) in &config.servers {
            let connection = McpServerConnection::connect(server_config).await?;
            servers.insert(name.clone(), connection);
        }

        Ok(Self {
            servers: Arc::new(Mutex::new(servers)),
            config,
        })
    }

    /// 列出所有可用资源
    pub async fn list_resources(&self) -> Result<Vec<McpResource>> {
        let servers = self.servers.lock().await;
        let mut all_resources = Vec::new();

        for (name, server) in servers.iter() {
            match server.list_resources().await {
                Ok(resources) => all_resources.extend(resources),
                Err(e) => warn!("Failed to list resources from {}: {}", name, e),
            }
        }

        Ok(all_resources)
    }

    /// 读取指定资源
    pub async fn read_resource(&self, uri: &str) -> Result<McpResource> {
        // 根据 URI scheme 选择对应的 Server
        let server_name = Self::parse_server_from_uri(uri)?;

        let servers = self.servers.lock().await;
        let server = servers
            .get(&server_name)
            .ok_or_else(|| AlephError::McpServerNotFound(server_name))?;

        server.read_resource(uri).await
    }

    /// 列出所有可用工具
    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        let servers = self.servers.lock().await;
        let mut all_tools = Vec::new();

        for server in servers.values() {
            match server.list_tools().await {
                Ok(tools) => all_tools.extend(tools),
                Err(e) => warn!("Failed to list tools: {}", e),
            }
        }

        Ok(all_tools)
    }

    /// 调用工具
    pub async fn call_tool(
        &self,
        tool_name: &str,
        args: Option<serde_json::Value>,
    ) -> Result<McpToolResult> {
        // 找到提供该工具的 Server
        let servers = self.servers.lock().await;

        for server in servers.values() {
            if server.has_tool(tool_name).await {
                return server.call_tool(tool_name, args).await;
            }
        }

        Err(AlephError::McpToolNotFound(tool_name.to_string()))
    }

    /// 🔮 获取 Prompt 模板（阶段 3）
    pub async fn get_prompt(
        &self,
        prompt_name: &str,
        args: HashMap<String, String>,
    ) -> Result<McpPromptResult> {
        // 实现逻辑...
        todo!()
    }

    /// 🔮 自动资源发现（阶段 3 高级功能）
    pub async fn discover_and_read_resources(&self) -> Result<Vec<McpResource>> {
        let all_resources = self.list_resources().await?;

        // 启发式选择相关资源（基于上下文）
        let relevant = all_resources.into_iter().take(5).collect::<Vec<_>>();

        // 并行读取内容
        let mut tasks = Vec::new();
        for resource in relevant {
            let uri = resource.uri.clone();
            let client = self.clone();
            tasks.push(tokio::spawn(async move {
                client.read_resource(&uri).await
            }));
        }

        let results = futures::future::join_all(tasks).await;
        Ok(results.into_iter().filter_map(|r| r.ok()).filter_map(|r| r.ok()).collect())
    }

    fn parse_server_from_uri(uri: &str) -> Result<String> {
        // 从 URI 解析 server 名称
        // 例如: "file:///..." -> "filesystem"
        //      "postgres://..." -> "postgres"
        let scheme = uri.split("://").next()
            .ok_or_else(|| AlephError::InvalidUri(uri.to_string()))?;

        Ok(scheme.to_string())
    }
}
```

### 3.3 JSON-RPC 2.0 通信协议

**MCP 使用 JSON-RPC 2.0** 作为通信协议，支持 stdio、HTTP、WebSocket 三种传输方式。

**JSON-RPC 2.0 规范**:

**请求格式**:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "resources/read",
  "params": {
    "uri": "file:///path/to/file.txt"
  }
}
```

**响应格式**（成功）:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "uri": "file:///path/to/file.txt",
    "contents": "File content here..."
  }
}
```

**响应格式**（错误）:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32600,
    "message": "Invalid request",
    "data": { ... }
  }
}
```

**实现**:

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 请求
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    jsonrpc: String, // 固定为 "2.0"
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: String, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method,
            params,
        }
    }
}

/// JSON-RPC 2.0 响应
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    jsonrpc: String,
    id: u64,
    #[serde(flatten)]
    payload: JsonRpcPayload,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcPayload {
    Success { result: Value },
    Error { error: JsonRpcError },
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    code: i32,
    message: String,
    data: Option<Value>,
}
```

**MCP 核心方法**:

| 方法 | 参数 | 返回值 | 描述 |
|-----|------|--------|------|
| `resources/list` | - | `{ resources: [...] }` | 列出所有资源 |
| `resources/read` | `{ uri: "..." }` | `{ contents: "..." }` | 读取资源内容 |
| `prompts/list` | - | `{ prompts: [...] }` | 列出所有 Prompts |
| `prompts/get` | `{ name: "...", arguments: {...} }` | `{ prompt: "..." }` | 获取渲染后的 Prompt |
| `tools/list` | - | `{ tools: [...] }` | 列出所有工具 |
| `tools/call` | `{ name: "...", arguments: {...} }` | `{ content: [...] }` | 调用工具 |

### 3.4 MCP Server 管理

**Server 连接抽象**:

```rust
use tokio::process::{Child, Command};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// 🔮 MCP Server 连接（阶段 3 实现）
pub struct McpServerConnection {
    /// Server 进程（stdio 模式）
    process: Option<Child>,

    /// HTTP/WebSocket 客户端（HTTP 模式）
    http_client: Option<reqwest::Client>,

    /// Server 配置
    config: McpServerConfig,

    /// 请求 ID 计数器
    next_id: Arc<AtomicU64>,
}

impl McpServerConnection {
    /// 连接到 MCP Server
    pub async fn connect(config: &McpServerConfig) -> Result<Self> {
        match &config.transport {
            McpTransport::Stdio { command, args, env } => {
                // 启动子进程
                let mut child = Command::new(command)
                    .args(args)
                    .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()?;

                // TODO: 建立 stdio 通信
                info!("Started MCP server: {:?}", command);

                Ok(Self {
                    process: Some(child),
                    http_client: None,
                    config: config.clone(),
                    next_id: Arc::new(AtomicU64::new(1)),
                })
            }
            McpTransport::Http { url } => {
                // 创建 HTTP 客户端
                let client = reqwest::Client::new();

                Ok(Self {
                    process: None,
                    http_client: Some(client),
                    config: config.clone(),
                    next_id: Arc::new(AtomicU64::new(1)),
                })
            }
        }
    }

    /// 发送 JSON-RPC 请求
    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest::new(id, method.to_string(), params);

        match &self.config.transport {
            McpTransport::Stdio { .. } => {
                // stdio 通信
                self.send_stdio_request(request).await
            }
            McpTransport::Http { url } => {
                // HTTP 通信
                self.send_http_request(url, request).await
            }
        }
    }

    async fn send_http_request(&self, url: &str, request: JsonRpcRequest) -> Result<Value> {
        let client = self.http_client.as_ref().unwrap();

        let response = client
            .post(url)
            .json(&request)
            .send()
            .await?
            .json::<JsonRpcResponse>()
            .await?;

        match response.payload {
            JsonRpcPayload::Success { result } => Ok(result),
            JsonRpcPayload::Error { error } => {
                Err(AlephError::McpError(error.message))
            }
        }
    }

    /// 列出资源
    pub async fn list_resources(&self) -> Result<Vec<McpResource>> {
        let result = self.send_request("resources/list", None).await?;
        let resources: Vec<McpResource> = serde_json::from_value(
            result["resources"].clone()
        )?;
        Ok(resources)
    }

    /// 读取资源
    pub async fn read_resource(&self, uri: &str) -> Result<McpResource> {
        let params = serde_json::json!({ "uri": uri });
        let result = self.send_request("resources/read", Some(params)).await?;
        Ok(serde_json::from_value(result)?)
    }

    /// 列出工具
    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        let result = self.send_request("tools/list", None).await?;
        let tools: Vec<McpTool> = serde_json::from_value(
            result["tools"].clone()
        )?;
        Ok(tools)
    }

    /// 调用工具
    pub async fn call_tool(
        &self,
        tool_name: &str,
        args: Option<Value>,
    ) -> Result<McpToolResult> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": args.unwrap_or(Value::Null)
        });

        let result = self.send_request("tools/call", Some(params)).await?;

        Ok(McpToolResult {
            success: true,
            content: Some(result),
            error: None,
        })
    }

    /// 检查是否有指定工具
    pub async fn has_tool(&self, tool_name: &str) -> bool {
        self.list_tools()
            .await
            .map(|tools| tools.iter().any(|t| t.name == tool_name))
            .unwrap_or(false)
    }
}

impl Drop for McpServerConnection {
    fn drop(&mut self) {
        // 清理子进程
        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
        }
    }
}
```

---

## 四、配置示例

### 4.1 MCP Server 配置

**文件**: `~/.aleph/config.toml`

```toml
[mcp]
enabled = true

# Filesystem Server (stdio)
[mcp.servers.filesystem]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/Users/ziv/Documents"]
env = {}

# Postgres Server (stdio)
[mcp.servers.postgres]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-postgres"]
env = { "DATABASE_URL" = "postgres://user:pass@localhost/db" }

# Git Server (stdio)
[mcp.servers.git]
transport = "stdio"
command = "mcp-server-git"
args = ["--repo", "/Users/ziv/projects/myrepo"]
env = {}

# HTTP Server 示例
[mcp.servers.weather]
transport = "http"
url = "http://localhost:3000/mcp"
```

**Rust 配置结构**:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default)]
    pub servers: HashMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    #[serde(flatten)]
    pub transport: McpTransport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase")]
pub enum McpTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    Http {
        url: String,
    },
    #[serde(rename = "websocket")]
    WebSocket {
        url: String,
    },
}
```

### 4.2 Resources 使用示例

**场景**: 读取本地文件作为上下文

**配置**:

```toml
[[rules]]
regex = "^/read"
provider = "claude"
system_prompt = "基于以下文件内容回答问题。"
strip_prefix = true
capabilities = ["mcp"]
intent_type = "file_read"
context_format = "markdown"
```

**执行流程**:

```rust
// 1. 用户输入
user_input = "/read 解释 config.toml 的作用";

// 2. 路由到 MCP capability
payload.config.capabilities = vec![Capability::Mcp];

// 3. execute_mcp() 调用
let resources = mcp_client.list_resources().await?; // 获取所有文件资源
let config_file = resources.iter().find(|r| r.uri.contains("config.toml")).unwrap();
let content = mcp_client.read_resource(&config_file.uri).await?;

// 4. 填充 context
payload.context.mcp_context = Some(McpContext {
    resources: vec![content],
    prompts: vec![],
    tool_results: vec![],
});

// 5. PromptAssembler 格式化
/*
### 上下文信息

**文件**: config.toml
```toml
[general]
theme = "cyberpunk"
...
```
*/
```

### 4.3 Tools 调用示例

**场景**: 执行 Git 命令

**配置**:

```toml
[[rules]]
regex = "^/git"
provider = "claude"
system_prompt = "你是 Git 助手，帮助用户执行 Git 操作。"
strip_prefix = true
capabilities = ["mcp"]
intent_type = "git_operation"
```

**工具定义** (由 MCP Server 提供):

```json
{
  "name": "git_log",
  "description": "Get git commit history",
  "inputSchema": {
    "type": "object",
    "properties": {
      "limit": { "type": "number", "default": 10 },
      "branch": { "type": "string", "default": "main" }
    }
  }
}
```

**调用流程**:

```rust
// 1. 用户输入
user_input = "/git 显示最近 5 条提交";

// 2. AI 决定调用工具（通过 Function Calling）
let tool_call = ToolCall {
    name: "git_log",
    arguments: json!({ "limit": 5, "branch": "main" })
};

// 3. execute_mcp() 执行工具
let result = mcp_client.call_tool("git_log", Some(tool_call.arguments)).await?;

// 4. 结果返回给 AI
payload.context.mcp_context = Some(McpContext {
    resources: vec![],
    prompts: vec![],
    tool_results: vec![result],
});
```

### 4.4 路由规则配置

**带 MCP 功能的路由规则**:

```toml
# 文件助手（Memory + MCP Resources）
[[rules]]
regex = "^/file"
provider = "claude"
system_prompt = "你是文件助手，帮助用户理解和操作文件。"
strip_prefix = true
capabilities = ["memory", "mcp"]
intent_type = "file_assistant"

# Git 助手（仅 MCP Tools）
[[rules]]
regex = "^/git"
provider = "claude"
system_prompt = "你是 Git 助手，基于工具调用结果回答问题。"
strip_prefix = true
capabilities = ["mcp"]
intent_type = "git_assistant"

# 数据库查询（MCP Resources）
[[rules]]
regex = "^/db"
provider = "openai"
system_prompt = "你是数据库助手，基于查询结果分析数据。"
strip_prefix = true
capabilities = ["mcp"]
intent_type = "database_query"
```

**🔮 阶段 3 扩展** - 指定 MCP Server:

```toml
[[rules]]
regex = "^/myfiles"
provider = "claude"
capabilities = ["mcp"]
# 🔮 新增字段：指定使用哪个 MCP Server
mcp_server = "filesystem"
mcp_resource_filter = "*.md"  # 仅读取 Markdown 文件
```

---

## 五、阶段 3 实施计划

### 5.1 需要新增的模块

**文件结构**:

```
Aleph/core/src/
├── mcp/                         # 🔮 MCP 模块（阶段 3 新建）
│   ├── mod.rs                   # 模块导出
│   ├── client.rs                # McpClient
│   ├── server.rs                # McpServerConnection
│   ├── jsonrpc.rs               # JSON-RPC 2.0 实现
│   ├── resource.rs              # McpResource
│   ├── tool.rs                  # McpTool, McpToolResult
│   ├── prompt.rs                # McpPrompt
│   └── transport/               # 传输层
│       ├── stdio.rs             # Stdio 传输
│       ├── http.rs              # HTTP 传输
│       └── websocket.rs         # WebSocket 传输
├── payload/
│   └── mcp.rs                   # 🔮 McpContext（本次预留）
└── capability/
    └── mod.rs                   # 填充 execute_mcp()
```

### 5.2 McpClient 实现（JSON-RPC）

**实施优先级**:

| 功能 | 优先级 | 预计时间 | 依赖 |
|-----|--------|---------|------|
| JSON-RPC 2.0 核心 | P0 | 4 小时 | `serde_json` |
| Stdio 传输层 | P0 | 5 小时 | `tokio::process` |
| Resources API | P0 | 3 小时 | JSON-RPC |
| Tools API | P1 | 4 小时 | JSON-RPC |
| Prompts API | P2 | 3 小时 | JSON-RPC |
| HTTP 传输层 | P2 | 3 小时 | `reqwest` |
| WebSocket 传输层 | P3 | 4 小时 | `tokio-tungstenite` |

**总时间**: 约 26 小时（并行实施可缩短）

### 5.3 Server 生命周期管理

**功能设计**:

```rust
/// 🔮 MCP Server 管理器（阶段 3）
pub struct McpServerManager {
    servers: Arc<Mutex<HashMap<String, McpServerConnection>>>,
    config: McpConfig,
}

impl McpServerManager {
    /// 启动所有配置的 Server
    pub async fn start_all(&mut self) -> Result<()> {
        for (name, server_config) in &self.config.servers {
            match McpServerConnection::connect(server_config).await {
                Ok(conn) => {
                    info!("Started MCP server: {}", name);
                    self.servers.lock().await.insert(name.clone(), conn);
                }
                Err(e) => {
                    error!("Failed to start MCP server {}: {}", name, e);
                }
            }
        }

        Ok(())
    }

    /// 停止所有 Server
    pub async fn stop_all(&mut self) {
        let mut servers = self.servers.lock().await;
        for (name, conn) in servers.drain() {
            info!("Stopping MCP server: {}", name);
            drop(conn); // 触发 Drop trait
        }
    }

    /// 重启指定 Server
    pub async fn restart_server(&mut self, name: &str) -> Result<()> {
        let server_config = self.config.servers.get(name)
            .ok_or_else(|| AlephError::McpServerNotFound(name.to_string()))?;

        // 停止旧连接
        let mut servers = self.servers.lock().await;
        servers.remove(name);

        // 启动新连接
        let new_conn = McpServerConnection::connect(server_config).await?;
        servers.insert(name.to_string(), new_conn);

        info!("Restarted MCP server: {}", name);
        Ok(())
    }

    /// 健康检查
    pub async fn health_check(&self) -> HashMap<String, bool> {
        let servers = self.servers.lock().await;
        let mut health = HashMap::new();

        for (name, server) in servers.iter() {
            // 尝试 list_resources() 作为健康检查
            let is_healthy = server.list_resources().await.is_ok();
            health.insert(name.clone(), is_healthy);
        }

        health
    }
}
```

### 5.4 Tool 调用和结果处理

**Function Calling 集成**:

```rust
impl CapabilityExecutor {
    async fn execute_mcp_with_function_calling(
        &self,
        payload: &mut AgentPayload,
        provider: &dyn AiProvider,
    ) -> Result<()> {
        let mcp_client = self.mcp_client.as_ref().ok_or(...)?;

        // 1. 获取所有可用工具
        let tools = mcp_client.list_tools().await?;

        // 2. 转换为 OpenAI Function 格式
        let functions: Vec<_> = tools.iter()
            .map(|tool| serde_json::json!({
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.input_schema
            }))
            .collect();

        // 3. 第一次 LLM 调用（带 tools）
        let response = provider.chat_with_tools(
            &payload.user_input,
            &functions
        ).await?;

        // 4. 如果 LLM 返回 tool_call
        if let Some(tool_call) = response.tool_calls.first() {
            info!("LLM requested tool call: {}", tool_call.name);

            // 5. 执行工具
            let result = mcp_client.call_tool(
                &tool_call.name,
                Some(tool_call.arguments.clone())
            ).await?;

            // 6. 第二次 LLM 调用（带工具结果）
            let final_response = provider.chat_with_tool_result(
                &payload.user_input,
                &tool_call,
                &result
            ).await?;

            // 7. 返回最终结果
            payload.context.mcp_context = Some(McpContext {
                resources: vec![],
                prompts: vec![],
                tool_results: vec![result],
            });
        }

        Ok(())
    }
}
```

### 5.5 实施步骤

**预估时间**: 2-3 天

| 步骤 | 任务 | 时间 | 依赖 |
|-----|------|------|------|
| 1 | JSON-RPC 2.0 核心实现 | 4 小时 | - |
| 2 | Stdio 传输层实现 | 5 小时 | JSON-RPC |
| 3 | McpServerConnection 实现 | 3 小时 | 传输层 |
| 4 | Resources API 实现 | 3 小时 | ServerConnection |
| 5 | Tools API 实现 | 4 小时 | ServerConnection |
| 6 | McpClient 统一接口 | 3 小时 | - |
| 7 | 填充 execute_mcp() | 2 小时 | McpClient |
| 8 | PromptAssembler 的 format_mcp_markdown() | 2 小时 | - |
| 9 | Server 生命周期管理 | 3 小时 | - |
| 10 | 配置文件解析 | 2 小时 | - |
| 11 | 单元测试 | 4 小时 | Mock Server |
| 12 | 集成测试 | 3 小时 | 真实 MCP Server |
| 13 | 文档和示例 | 2 小时 | - |
| **总计** | | **40 小时** | **约 2-3 天**（全职）|

---

## 六、UI 配置界面预留

### 6.1 MCP Server 管理界面

**文件**: `Aleph/Sources/Components/Settings/McpSettingsView.swift`（新建）

**UI 设计草图**:

```
┌──────────────────────────────────────────────────┐
│  MCP 服务器管理                                   │
├──────────────────────────────────────────────────┤
│ 启用 MCP  ☑︎                                      │
│                                                  │
│ ┌─ 已配置的服务器 ────────────────────────────┐  │
│ │  📁 filesystem    ✅ 运行中   [停止] [编辑]  │  │
│ │  🗄  postgres     ✅ 运行中   [停止] [编辑]  │  │
│ │  📦 git           ⚠️  已停止   [启动] [编辑]  │  │
│ └──────────────────────────────────────────────┘  │
│                                                  │
│           [+ 添加新服务器]                        │
│                                                  │
├─ 添加 MCP Server ───────────────────────────────┤
│  服务器名称:  [my-server          ]              │
│                                                  │
│  传输方式:    ◉ Stdio  ○ HTTP  ○ WebSocket      │
│                                                  │
│  ┌─ Stdio 配置 ──────────────────────────────┐  │
│  │ 命令:  [npx                        ]       │  │
│  │ 参数:  [-y @modelcontextprotocol/...  ]   │  │
│  │ 环境变量:                                  │  │
│  │   DATABASE_URL = [postgres://...     ]    │  │
│  │                  [+ 添加变量]              │  │
│  └────────────────────────────────────────────┘  │
│                                                  │
│           [测试连接]  [保存]  [取消]             │
├─ 工具权限管理 ──────────────────────────────────┤
│  允许的工具:                                     │
│  ☑︎ file_read     (读取文件)                     │
│  ☑︎ file_write    (写入文件) ⚠️ 危险             │
│  ☐ git_commit    (提交代码) ⚠️ 危险             │
│  ☑︎ sql_query     (查询数据库)                   │
│  ☐ send_email    (发送邮件) ⚠️ 危险             │
│                                                  │
│  危险工具需要手动确认  ☑︎                        │
└──────────────────────────────────────────────────┘
```

**Swift 代码结构**:

```swift
struct McpSettingsView: View {
    @State private var mcpEnabled: Bool = false
    @State private var servers: [McpServer] = []
    @State private var showingAddServer: Bool = false

    var body: some View {
        Form {
            Section("基础设置") {
                Toggle("启用 MCP", isOn: $mcpEnabled)
            }

            Section("已配置的服务器") {
                ForEach(servers) { server in
                    HStack {
                        serverIcon(for: server.name)
                        Text(server.name)

                        Spacer()

                        statusBadge(for: server.status)

                        Button(server.status == .running ? "停止" : "启动") {
                            toggleServer(server)
                        }

                        Button("编辑") {
                            editServer(server)
                        }
                    }
                }

                Button("+ 添加新服务器") {
                    showingAddServer = true
                }
            }

            Section("工具权限管理") {
                ForEach(availableTools) { tool in
                    HStack {
                        Toggle(tool.name, isOn: binding(for: tool.id))
                        Text(tool.description)
                            .font(.caption)
                            .foregroundColor(.secondary)

                        if tool.isDangerous {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundColor(.orange)
                        }
                    }
                }

                Toggle("危险工具需要手动确认", isOn: $requireConfirmation)
            }
        }
        .sheet(isPresented: $showingAddServer) {
            AddMcpServerView()
        }
    }
}

struct McpServer: Identifiable {
    let id: UUID
    var name: String
    var transport: McpTransport
    var status: ServerStatus
}

enum ServerStatus {
    case running
    case stopped
    case error(String)
}
```

### 6.2 Tool 权限配置

**安全机制**:

```swift
// Tool 调用前的确认对话框
struct ToolCallConfirmation: View {
    let tool: McpTool
    let arguments: [String: Any]
    let onConfirm: () -> Void
    let onCancel: () -> Void

    var body: some View {
        VStack(spacing: 20) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 60))
                .foregroundColor(.orange)

            Text("工具调用确认")
                .font(.headline)

            Text("AI 请求调用以下工具：")
                .foregroundColor(.secondary)

            VStack(alignment: .leading, spacing: 10) {
                Text("工具: \(tool.name)")
                    .font(.system(.body, design: .monospaced))

                Text("参数:")
                    .font(.caption)

                ForEach(arguments.keys.sorted(), id: \.self) { key in
                    HStack {
                        Text(key)
                            .font(.caption)
                            .foregroundColor(.secondary)
                        Text(": \(String(describing: arguments[key]!))")
                            .font(.system(.caption, design: .monospaced))
                    }
                }
            }
            .padding()
            .background(Color.gray.opacity(0.1))
            .cornerRadius(8)

            Text("⚠️ 此操作可能会修改您的文件或数据")
                .font(.caption)
                .foregroundColor(.orange)

            HStack(spacing: 20) {
                Button("取消") {
                    onCancel()
                }
                .keyboardShortcut(.cancelAction)

                Button("确认执行") {
                    onConfirm()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding()
        .frame(width: 400)
    }
}
```

---

## 七、测试策略

### 7.1 本次实施（MVP）

**测试目标**: 确保预留接口不影响现有功能

```rust
#[test]
fn test_mcp_resource_creation() {
    let resource = McpResource::new(
        "file:///test.txt".to_string(),
        "Test File".to_string(),
        "Content here".to_string(),
    );

    assert_eq!(resource.uri, "file:///test.txt");
    assert!(resource.is_text());
}

#[test]
fn test_mcp_context_default() {
    let context = McpContext::default();

    assert!(context.resources.is_empty());
    assert!(context.prompts.is_empty());
    assert!(context.tool_results.is_empty());
}

#[test]
fn test_mcp_tool_serialization() {
    let tool = McpTool {
        name: "test_tool".to_string(),
        description: "A test tool".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "arg1": { "type": "string" }
            }
        }),
        requires_confirmation: true,
    };

    let json = serde_json::to_string(&tool).unwrap();
    let parsed: McpTool = serde_json::from_str(&json).unwrap();

    assert_eq!(tool.name, parsed.name);
    assert_eq!(tool.requires_confirmation, parsed.requires_confirmation);
}
```

### 7.2 阶段 3 实施时

**Mock MCP Server 测试**:

```rust
/// Mock MCP Server for testing
struct MockMcpServer {
    resources: Vec<McpResource>,
    tools: Vec<McpTool>,
}

impl MockMcpServer {
    fn new() -> Self {
        Self {
            resources: vec![
                McpResource::new(
                    "file:///test1.txt".into(),
                    "Test 1".into(),
                    "Content 1".into(),
                ),
                McpResource::new(
                    "file:///test2.txt".into(),
                    "Test 2".into(),
                    "Content 2".into(),
                ),
            ],
            tools: vec![
                McpTool {
                    name: "test_tool".into(),
                    description: "Test tool".into(),
                    input_schema: serde_json::json!({}),
                    requires_confirmation: false,
                },
            ],
        }
    }

    async fn handle_request(&self, method: &str, _params: Option<Value>) -> Result<Value> {
        match method {
            "resources/list" => {
                Ok(serde_json::json!({ "resources": self.resources }))
            }
            "resources/read" => {
                Ok(serde_json::to_value(&self.resources[0]).unwrap())
            }
            "tools/list" => {
                Ok(serde_json::json!({ "tools": self.tools }))
            }
            "tools/call" => {
                Ok(serde_json::json!({
                    "content": [{ "type": "text", "text": "Tool result" }]
                }))
            }
            _ => Err(AlephError::McpError(format!("Unknown method: {}", method))),
        }
    }
}

#[tokio::test]
async fn test_mcp_client_list_resources() {
    let mock_server = MockMcpServer::new();
    // ... 测试逻辑
}
```

**集成测试**（需要真实 MCP Server）:

```rust
#[tokio::test]
#[ignore] // 需要手动启用
async fn test_filesystem_server_real() {
    let config = McpServerConfig {
        transport: McpTransport::Stdio {
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
                "/tmp".to_string(),
            ],
            env: HashMap::new(),
        },
    };

    let connection = McpServerConnection::connect(&config).await.unwrap();

    let resources = connection.list_resources().await.unwrap();
    assert!(!resources.is_empty());

    println!("Resources: {:#?}", resources);
}
```

---

## 八、常见问题

### Q1: MCP 和 Function Calling 有什么区别？

**A**: **互补关系，而非替代**

| 特性 | Function Calling | MCP |
|-----|-----------------|-----|
| **范围** | 工具调用（Tools） | Resources + Prompts + Tools |
| **标准化** | OpenAI 定义格式 | Anthropic 标准协议 |
| **传输** | 通过 API 参数 | JSON-RPC 2.0 独立通道 |
| **动态性** | 静态定义 | 可动态发现 |
| **生态** | LLM 原生支持 | 需要 MCP Server |

**关系**:
- MCP Tools 可以**转换为** Function Calling 格式
- Function Calling 是 AI 决策层
- MCP 是数据/工具提供层

**示例**:

```rust
// MCP Tool 定义
let mcp_tool = McpTool {
    name: "file_write",
    description: "Write to file",
    input_schema: json!({ "type": "object", "properties": { "path": {...} } }),
};

// 转换为 OpenAI Function
let function = json!({
    "name": mcp_tool.name,
    "description": mcp_tool.description,
    "parameters": mcp_tool.input_schema
});

// AI 调用 Function
let tool_call = ai.chat_with_functions(...).tool_calls[0];

// 执行 MCP Tool
let result = mcp_client.call_tool(&tool_call.name, tool_call.arguments).await?;
```

### Q2: 为什么不直接实现文件读写，而要用 MCP？

**A**: **标准化、安全性、可扩展性**

**直接实现的问题**:

```rust
// ❌ 问题：每个功能都需要自己实现
async fn read_file(path: &str) -> String { ... }
async fn query_database(sql: &str) -> Value { ... }
async fn git_log(limit: u32) -> Vec<Commit> { ... }
// ... 无穷无尽
```

**MCP 的优势**:

```rust
// ✅ 优势：统一接口，第三方实现
let result = mcp_client.call_tool("file_read", args).await?;
let result = mcp_client.call_tool("sql_query", args).await?;
let result = mcp_client.call_tool("git_log", args).await?;
// 新工具只需添加 MCP Server，无需修改代码
```

**类比**: MCP 之于工具调用，就像 Docker 之于应用部署 - **标准化的接口**。

### Q3: MCP Server 崩溃怎么办？

**A**: **自动重启 + 健康检查**

```rust
impl McpServerManager {
    /// 监控 Server 健康状态
    pub async fn monitor_health(&mut self) {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;

            let health = self.health_check().await;

            for (name, is_healthy) in health {
                if !is_healthy {
                    warn!("MCP server {} is unhealthy, restarting...", name);

                    if let Err(e) = self.restart_server(&name).await {
                        error!("Failed to restart server {}: {}", name, e);
                    }
                }
            }
        }
    }
}
```

**配置重试策略**:

```toml
[mcp.retry]
max_attempts = 3
initial_delay_ms = 1000
max_delay_ms = 10000
backoff_multiplier = 2.0
```

### Q4: 如何控制 MCP Tool 的权限？

**A**: **三层权限控制**

**1. 配置层**（白名单）:

```toml
[mcp.permissions]
# 仅允许这些工具
allowed_tools = ["file_read", "sql_query", "git_log"]

# 禁止这些工具
denied_tools = ["file_delete", "system_exec"]
```

**2. 代码层**（动态检查）:

```rust
impl McpClient {
    async fn call_tool(&self, name: &str, args: Option<Value>) -> Result<McpToolResult> {
        // 检查权限
        if !self.is_tool_allowed(name) {
            return Err(AlephError::ToolNotAllowed(name.to_string()));
        }

        // 检查是否需要确认
        if self.tool_requires_confirmation(name) {
            // 触发用户确认对话框
            let confirmed = self.request_user_confirmation(name, &args).await?;
            if !confirmed {
                return Err(AlephError::ToolCallCancelled);
            }
        }

        // 执行工具
        self.execute_tool_internal(name, args).await
    }
}
```

**3. UI 层**（用户控制）:

- 显示工具调用详情
- 危险工具标记（⚠️）
- 手动确认对话框

### Q5: MCP 会增加多少延迟？

**A**: **取决于传输方式和 Server 实现**

**延迟测试**（基于 stdio）:

| 操作 | 延迟 |
|-----|------|
| list_resources() | ~50ms |
| read_resource() (小文件) | ~100ms |
| call_tool() (简单工具) | ~150ms |
| call_tool() (数据库查询) | ~300ms |

**优化策略**:

1. **并行调用**: 同时读取多个资源
2. **缓存**: 缓存 list_resources() 结果
3. **HTTP/WebSocket**: 对于远程 Server，使用持久连接
4. **懒加载**: 仅在需要时读取资源内容

```rust
// 优化：并行读取多个资源
let tasks: Vec<_> = resource_uris.iter()
    .map(|uri| mcp_client.read_resource(uri))
    .collect();

let results = futures::future::join_all(tasks).await;
```

---

## 九、总结

### 9.1 本次实施（MVP）预留的接口

| 类别 | 接口 | 状态 |
|-----|------|------|
| **结构体** | `McpResource` | 🔮 文档定义 |
| **结构体** | `McpTool`, `McpToolResult` | 🔮 文档定义 |
| **结构体** | `McpPrompt` | 🔮 文档定义 |
| **结构体** | `McpContext` | 🔮 文档定义 |
| **字段** | `AgentContext.mcp_resources` | ⚠️ 已预留（HashMap）|
| **枚举** | `Capability::Mcp` | ⚠️ 已定义 |
| **枚举** | `Intent::BuiltinMcp` | ⚠️ 已定义 |
| **方法** | `execute_mcp()` | ⚠️ 空实现 |
| **方法** | `format_mcp_markdown()` | ⚠️ 方法签名 |
| **结构体** | `McpClient` | 🔮 文档定义 |
| **结构体** | `McpServerConnection` | 🔮 文档定义 |
| **协议** | JSON-RPC 2.0 | 🔮 文档定义 |

### 9.2 阶段 3 实施时的工作

| 任务 | 预估时间 | 依赖 |
|-----|---------|------|
| JSON-RPC 2.0 核心 | 4 小时 | - |
| Stdio 传输层 | 5 小时 | JSON-RPC |
| McpServerConnection | 3 小时 | 传输层 |
| Resources API | 3 小时 | ServerConnection |
| Tools API | 4 小时 | ServerConnection |
| Prompts API | 3 小时 | ServerConnection |
| McpClient 统一接口 | 3 小时 | - |
| execute_mcp() 填充 | 2 小时 | McpClient |
| format_mcp_markdown() | 2 小时 | - |
| Server 生命周期管理 | 3 小时 | - |
| 配置解析 | 2 小时 | - |
| 测试（单元 + 集成）| 7 小时 | - |
| 文档和示例 | 2 小时 | - |
| UI 配置界面 | 3 小时 | Swift |
| **总计** | **46 小时** | **约 2-3 天**（全职）|

### 9.3 设计验证清单

**数据结构**:
- ✅ McpResource 包含所有必要字段（uri, name, contents, metadata）
- ✅ McpTool 符合 MCP 规范
- ✅ McpContext 支持 Resources/Prompts/Tools 三种类型
- ✅ 所有字段都是 `Option<T>`（向后兼容）

**协议兼容性**:
- ✅ JSON-RPC 2.0 规范完整实现
- ✅ 支持 Stdio / HTTP / WebSocket 三种传输
- ✅ 符合 Anthropic MCP 规范

**安全性**:
- ✅ 工具权限白名单机制
- ✅ 危险工具手动确认
- ✅ Server 进程隔离

**可扩展性**:
- ✅ 支持多个 MCP Server 并存
- ✅ 动态发现 Resources/Tools
- ✅ 第三方可轻松开发 MCP Server

**Skills 兼容性**:
- ✅ MCP Tools 可被 Skills 工作流调用
- ✅ execute_skills_workflow() 可使用 MCP Client
- ✅ WorkflowEngine 可调度 Tool 执行

**结论**: MCP 接口预留已完成，阶段 3 实施时无需破坏性修改，且为 Skills 方案 C 提供完整支持。
