# MCP Orchestration Layer Design

> **Date**: 2026-02-03
> **Status**: Draft
> **Scope**: P0 (基础设施) + P1 (能力对齐)

## 1. 背景与目标

### 1.1 现状分析

Aleph 的 MCP 实现目前处于"接口定义"阶段：

| 组件 | 状态 | 问题 |
|------|------|------|
| `mcp/client.rs` | ✅ 完整 | McpClient 功能齐全 |
| `mcp/transport/*` | ✅ 完整 | Stdio/HTTP/SSE 三种传输 |
| `mcp/auth/*` | ✅ 完整 | OAuth 完整流程 |
| `gateway/handlers/mcp.rs` | ⚠️ TODO | 所有 handler 返回空/硬编码 |
| 配置持久化 | 🔴 缺失 | 重启后连接丢失 |
| Resources/Prompts 集成 | 🔴 缺失 | LLM 只能调用 Tools |

### 1.2 设计目标

**P0 - 基础设施补全**：
- 实现 `McpManager` Actor 管理所有 Server 生命周期
- 配置持久化到 `~/.aleph/mcp_config.json`
- 完善 Gateway handlers，对接 McpManager
- 健康检查与自动重启

**P1 - 能力对齐**：
- Agent Context 注入 MCP Resources
- System Prompt 展示 MCP Prompts 作为 Slash Commands
- 新增 `mcp_read_resource` 和 `mcp_get_prompt` 内置工具

---

## 2. 架构设计

### 2.1 整体架构

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              UI / Clients                                │
│                    (macOS App, Tauri, CLI, WebSocket)                   │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │ JSON-RPC
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                           Gateway Layer                                  │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │ HandlerRegistry                                                  │    │
│  │  ├── mcp.list, mcp.add, mcp.delete, mcp.status ...              │    │
│  │  ├── mcp.listTools, mcp.listResources, mcp.listPrompts (P1)     │    │
│  │  └── mcp.start, mcp.stop, mcp.restart                           │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│                                    │                                     │
│                         McpManagerHandle (Clone)                         │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │ mpsc::channel
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                     McpManager (Actor)                                   │
│  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐       │
│  │ Command Loop     │  │ Health Monitor   │  │ Config Watcher   │       │
│  │ (mpsc receiver)  │  │ (interval task)  │  │ (file watcher)   │       │
│  └────────┬─────────┘  └────────┬─────────┘  └────────┬─────────┘       │
│           │                     │                     │                  │
│           └─────────────────────┼─────────────────────┘                  │
│                                 ▼                                        │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │ clients: HashMap<String, Arc<McpClient>>                        │    │
│  │ config: McpPersistentConfig (~/.aleph/mcp_config.json)         │    │
│  │ health_states: HashMap<String, ServerHealth>                    │    │
│  │ event_tx: broadcast::Sender<McpEvent>                           │    │
│  └─────────────────────────────────────────────────────────────────┘    │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │ Arc<McpClient>
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                        McpClient (现有)                                  │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │ tool_location_map: RwLock<HashMap<String, ToolLocation>>        │    │
│  │ external_servers: RwLock<HashMap<String, McpServerConnection>>  │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│                                    │                                     │
│            ┌───────────────────────┼───────────────────────┐            │
│            ▼                       ▼                       ▼            │
│  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐       │
│  │ StdioTransport   │  │ HttpTransport    │  │ SseTransport     │       │
│  │ (子进程)          │  │ (HTTP POST)      │  │ (HTTP + SSE)     │       │
│  └──────────────────┘  └──────────────────┘  └──────────────────┘       │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                        External MCP Servers                              │
│              (github-mcp, linear-mcp, postgres-mcp, ...)                │
└─────────────────────────────────────────────────────────────────────────┘
```

### 2.2 控制面/数据面分离

**设计决策**：`McpManager` 只负责**控制面**（增删改查 Server、获取 Client 引用），**数据面**（实际工具调用）由调用方直接与 `Arc<McpClient>` 交互。

**理由**：
- 避免 Manager 成为所有工具调用的串行瓶颈
- 点对点通信，并发性能更好
- Manager 专注于生命周期和状态管理

```
数据面调用流程：

Gateway/AgentLoop                McpManager                McpClient
      │                              │                         │
      │──GetClient("github")────────►│                         │
      │◄─────Arc<McpClient>──────────│                         │
      │                              │                         │
      │──client.call_tool("search")──┼────────────────────────►│
      │◄─────McpToolResult───────────┼─────────────────────────│
```

---

## 3. 核心组件设计

### 3.1 McpManager Actor

```rust
// Actor 内部状态
struct McpManagerActor {
    config_path: PathBuf,                              // ~/.aleph/mcp_config.json
    clients: HashMap<String, Arc<McpClient>>,          // server_id -> client
    health_states: HashMap<String, ServerHealth>,      // 健康状态
    event_tx: broadcast::Sender<McpEvent>,             // 事件广播
    health_config: HealthCheckConfig,                  // 健康检查配置
}

// 控制面命令（无 CallTool）
enum McpCommand {
    // === 生命周期 ===
    AddServer(McpServerConfig, oneshot::Sender<Result<()>>),
    RemoveServer(String, oneshot::Sender<Result<()>>),
    RestartServer(String, oneshot::Sender<Result<()>>),

    // === 查询 ===
    GetClient(String, oneshot::Sender<Option<Arc<McpClient>>>),
    ListServers(oneshot::Sender<Vec<McpServerInfo>>),
    GetStatus(String, oneshot::Sender<McpServerStatus>),

    // === 聚合查询（P1 能力对齐）===
    AggregateTools(oneshot::Sender<Vec<McpTool>>),
    AggregateResources(oneshot::Sender<Vec<McpResource>>),
    AggregatePrompts(oneshot::Sender<Vec<McpPrompt>>),

    // === 配置 ===
    ReloadConfig(oneshot::Sender<Result<()>>),
    Shutdown,
}

// 外部调用接口（克隆安全）
#[derive(Clone)]
pub struct McpManagerHandle {
    tx: mpsc::Sender<McpCommand>,
    event_tx: broadcast::Sender<McpEvent>,  // 用于 subscribe
}
```

### 3.2 配置持久化

**位置**：`~/.aleph/mcp_config.json`

```json
{
  "version": 1,
  "servers": {
    "github": {
      "transport": "stdio",
      "command": "node",
      "args": ["~/.mcp/github/index.js"],
      "env": { "GITHUB_TOKEN": "${GITHUB_TOKEN}" },
      "requires_runtime": "node",
      "auto_start": true,
      "timeout_seconds": 30
    },
    "linear": {
      "transport": "sse",
      "url": "https://mcp.linear.app/sse",
      "auth": {
        "type": "oauth",
        "provider_id": "linear"
      },
      "auto_start": true
    }
  }
}
```

### 3.3 启动流程

```
Aleph 启动
    │
    ▼
┌─────────────────────────────────────┐
│ 1. 加载 mcp_config.json             │
│    - 解析 servers 配置               │
│    - 展开环境变量 (${...})           │
└─────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────┐
│ 2. 过滤 auto_start = true 的 Server │
│    - 检查 runtime 可用性             │
│    - 检查 OAuth token 有效性         │
└─────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────┐
│ 3. 并发启动所有 Server (join_all)   │
│    - 成功: 注册到 clients HashMap    │
│    - 失败: 记录到 startup_report     │
└─────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────┐
│ 4. 启动后台任务                      │
│    - 健康检查循环 (每 30s)           │
│    - 配置文件监听 (热重载)           │
└─────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────┐
│ 5. 广播 McpEvent::ManagerReady      │
│    - Gateway 开始接受 MCP 请求       │
└─────────────────────────────────────┘
```

---

## 4. 事件系统

### 4.1 事件类型

```rust
#[derive(Clone, Debug)]
pub enum McpEvent {
    // === 生命周期事件 ===
    ManagerReady,                              // Manager 启动完成
    ManagerShutdown,                           // Manager 关闭

    // === Server 状态事件 ===
    ServerStarted { id: String },              // Server 启动成功
    ServerStopped { id: String },              // Server 正常停止
    ServerCrashed { id: String, error: String }, // Server 异常退出
    ServerRestarting { id: String, attempt: u32 }, // 正在重启

    // === 能力变更事件（来自 MCP notifications）===
    ToolsChanged { server_id: String },        // tools/list_changed
    ResourcesChanged { server_id: String },    // resources/list_changed
    PromptsChanged { server_id: String },      // prompts/list_changed

    // === 配置事件 ===
    ConfigReloaded,                            // 配置热重载完成
    ServerAdded { id: String },                // 新 Server 添加
    ServerRemoved { id: String },              // Server 移除
}
```

### 4.2 订阅机制

```rust
impl McpManagerHandle {
    /// 订阅所有事件
    pub fn subscribe(&self) -> broadcast::Receiver<McpEvent> {
        self.event_tx.subscribe()
    }

    /// 订阅特定 Server 的事件（过滤）
    pub fn subscribe_server(&self, server_id: String) -> impl Stream<Item = McpEvent> {
        let rx = self.event_tx.subscribe();
        BroadcastStream::new(rx).filter_map(move |event| {
            match &event {
                Ok(e) if e.relates_to(&server_id) => Some(e.clone()),
                _ => None,
            }
        })
    }
}
```

---

## 5. 健康检查与故障恢复

### 5.1 健康检查配置

```rust
struct HealthCheckConfig {
    interval: Duration,          // 默认 30s
    timeout: Duration,           // 单次检查超时 5s
    max_failures: u32,           // 连续失败阈值 3
    restart_delay: Duration,     // 重启前等待 2s
    max_restarts: u32,           // 5 分钟内最大重启次数 3
}

struct ServerHealth {
    consecutive_failures: u32,
    last_check: Instant,
    restart_count: u32,
    restart_window_start: Instant,
    status: HealthStatus,
}

enum HealthStatus {
    Healthy,
    Degraded { failures: u32 },
    Unhealthy,
    Restarting { attempt: u32 },
    Dead,  // 超过重启限制，需人工干预
}
```

### 5.2 状态暴露

```rust
#[derive(Serialize)]
pub struct McpServerStatus {
    pub id: String,
    pub running: bool,
    pub health: HealthStatus,
    pub uptime_seconds: u64,
    pub restart_count: u32,
    pub last_error: Option<String>,
    pub tools_count: usize,
    pub resources_count: usize,
}
```

---

## 6. Gateway Handler 实现

### 6.1 Handler 注册

```rust
// gateway/server.rs - 启动时注册
impl GatewayServer {
    pub fn with_mcp_manager(mut self, handle: McpManagerHandle) -> Self {
        let registry = self.handlers_mut();

        let h = handle.clone();
        registry.register("mcp.list", move |req| {
            let handle = h.clone();
            async move { mcp::handle_list(req, handle).await }
        });

        // ... 其他 handlers
        self
    }
}
```

### 6.2 完整 RPC 方法列表

| 方法 | 参数 | 返回 | 描述 |
|------|------|------|------|
| `mcp.list` | - | `Vec<McpServerInfo>` | 列出所有配置的 Server |
| `mcp.add` | `McpServerConfig` | `Result<()>` | 添加新 Server（持久化） |
| `mcp.update` | `{id, config}` | `Result<()>` | 更新 Server 配置 |
| `mcp.delete` | `{id}` | `Result<()>` | 删除 Server（持久化） |
| `mcp.status` | `{id}` | `McpServerStatus` | 获取单个 Server 状态 |
| `mcp.start` | `{id}` | `Result<()>` | 手动启动 Server |
| `mcp.stop` | `{id}` | `Result<()>` | 手动停止 Server |
| `mcp.restart` | `{id}` | `Result<()>` | 重启 Server |
| `mcp.listTools` | `{server_id?}` | `Vec<McpTool>` | 聚合所有工具 |
| `mcp.listResources` | `{server_id?}` | `Vec<McpResource>` | **P1**: 聚合所有资源 |
| `mcp.listPrompts` | `{server_id?}` | `Vec<McpPrompt>` | **P1**: 聚合所有 Prompts |
| `mcp.logs` | `{id, lines?}` | `Vec<String>` | 获取 Server 日志 |

---

## 7. P1: AgentLoop 集成

### 7.1 PromptBuilder 扩展

```rust
impl PromptBuilder {
    /// P1: 注入 MCP Resources 作为上下文
    fn append_mcp_resources(&self, prompt: &mut String, resources: &[McpResource]) {
        if resources.is_empty() { return; }

        prompt.push_str("\n## Available Context Resources\n");
        prompt.push_str("You can read these resources using `mcp_read_resource` tool:\n\n");

        for res in resources {
            prompt.push_str(&format!(
                "- `{}` - {} ({})\n",
                res.uri, res.name, res.mime_type.as_deref().unwrap_or("text/plain")
            ));
        }
    }

    /// P1: 注入 MCP Prompts 作为 Slash Commands 提示
    fn append_mcp_prompts(&self, prompt: &mut String, prompts: &[McpPrompt]) {
        if prompts.is_empty() { return; }

        prompt.push_str("\n## User Slash Commands (from MCP)\n");
        prompt.push_str("The user may invoke these commands. When they do, ");
        prompt.push_str("use `mcp_get_prompt` to retrieve the full prompt:\n\n");

        for p in prompts {
            prompt.push_str(&format!("- `/{}`", p.name));
            if let Some(desc) = &p.description {
                prompt.push_str(&format!(" - {}", desc));
            }
            prompt.push('\n');
        }
    }
}
```

### 7.2 新增内置工具

| 工具名 | 描述 | 参数 |
|--------|------|------|
| `mcp_read_resource` | 读取 MCP Resource 内容 | `uri: string` |
| `mcp_get_prompt` | 获取 MCP Prompt 完整内容 | `name: string, arguments?: object` |

---

## 8. 实现计划

### 8.1 新增/修改文件清单

| 文件路径 | 操作 | 描述 |
|----------|------|------|
| `core/src/mcp/manager.rs` | **新增** | McpManagerActor + McpManagerHandle |
| `core/src/mcp/manager/commands.rs` | **新增** | McpCommand 枚举定义 |
| `core/src/mcp/manager/health.rs` | **新增** | 健康检查逻辑 |
| `core/src/mcp/manager/config.rs` | **新增** | 配置持久化 (JSON) |
| `core/src/mcp/events.rs` | **新增** | McpEvent 定义 |
| `core/src/mcp/mod.rs` | **修改** | 导出新模块 |
| `core/src/gateway/handlers/mcp.rs` | **修改** | 实现 TODO handlers |
| `core/src/gateway/server.rs` | **修改** | 添加 `with_mcp_manager()` |
| `core/src/thinker/prompt_builder.rs` | **修改** | 添加 Resources/Prompts 注入 |
| `core/src/builtin_tools/mcp_read_resource.rs` | **新增** | P1: 读取资源工具 |
| `core/src/builtin_tools/mcp_get_prompt.rs` | **新增** | P1: 获取 Prompt 工具 |

### 8.2 实现优先级

| 阶段 | 任务 | 预期产出 |
|------|------|----------|
| **P0.1** | McpManager Actor 骨架 | 命令循环运行 |
| **P0.2** | 配置持久化 | 重启后恢复连接 |
| **P0.3** | Gateway handler 对接 | API 可调用 |
| **P0.4** | 健康检查 + 自动重启 | 故障自愈 |
| **P1.1** | aggregate_resources/prompts | 聚合查询 |
| **P1.2** | PromptBuilder 注入 | Agent 感知资源 |
| **P1.3** | 新增内置工具 | LLM 可读取资源 |

---

## 9. 设计决策记录

| 决策 | 选项 | 选择 | 理由 |
|------|------|------|------|
| Manager 架构 | Arc<RwLock> vs Actor | **Actor** | 进程监控需隔离，避免锁争用 |
| 配置格式 | TOML vs JSON | **JSON** | 避免嵌套复杂性，与现有 config.toml 分离 |
| 工具调用路径 | 经过 Manager vs 直接调用 | **直接调用** | 避免串行瓶颈，控制面/数据面分离 |
| 健康检查 | 轮询 vs 事件驱动 | **轮询** | 简单可靠，Stdio 进程无主动通知 |

---

## 10. 风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| OAuth Token 过期 | Server 连接失败 | 健康检查检测 401，触发 Token 刷新 |
| Stdio 进程僵死 | 资源泄漏 | 定期检查 `try_wait()`，强制 kill |
| 配置文件损坏 | Manager 启动失败 | 保留备份，启动时校验 JSON Schema |
| 事件风暴 | broadcast channel 溢出 | 设置合理 capacity，丢弃旧事件 |

---

## Appendix A: 与现有代码的关系

### A.1 复用现有组件

- `McpClient` (client.rs) - 完全复用，作为 Manager 管理的对象
- `McpServerConnection` (external/connection.rs) - 复用连接管理
- `StdioTransport/HttpTransport/SseTransport` - 复用传输层
- `OAuthProvider/OAuthStorage` (auth/*) - 复用认证流程
- `McpNotificationRouter` (notifications.rs) - 复用通知路由

### A.2 需要扩展的组件

- `PromptBuilder` - 添加 Resources/Prompts 注入方法
- `HandlerRegistry` - 注册 MCP handlers
- `GatewayServer` - 添加 `with_mcp_manager()` 方法
