> **SUPERSEDED** by `docs/plans/2026-02-23-server-centric-architecture-design.md`
> This document describes a deprecated Server-Client architecture that has been replaced.

---

# Server-Client 架构设计

> 将 Aleph 从"本地一体化"架构演进为"大脑在云端，手脚在身边"的分布式架构

## 1. 架构概述

### 1.1 核心目标

- **Server (Remote Brain)**：运行 Agent Loop、LLM 交互、任务调度、记忆系统
- **Client (Local Hands)**：执行本地工具（Shell、文件系统、UI 交互）、提供环境上下文

### 1.2 核心机制

| 机制 | 描述 |
|------|------|
| **Policy 驱动路由** | 工具声明 `ExecutionPolicy`，决定执行位置偏好 |
| **能力协商** | Client 在 `connect` 时声明 Manifest，Server 动态路由 |
| **反向 RPC** | Server 可向 Client 发起 `tool.call` 请求并等待响应 |
| **权限上下文** | Client 声明 `granted_scopes`，Server 提前拦截越权请求 |

### 1.3 路由决策矩阵

```
┌──────────────┬───────────────────┬─────────────────────────┐
│ Policy       │ Client 有能力+权限 │ Client 无能力或无权限    │
├──────────────┼───────────────────┼─────────────────────────┤
│ ServerOnly   │ Server 执行        │ Server 执行             │
│ ClientOnly   │ Client 执行        │ ❌ Error                │
│ PreferServer │ Server 执行        │ Server 执行             │
│ PreferClient │ Client 执行        │ Server 执行 (回退)      │
└──────────────┴───────────────────┴─────────────────────────┘
```

### 1.4 通信流程

```
┌─────────────────┐                    ┌─────────────────┐
│     Client      │                    │     Server      │
├─────────────────┤                    ├─────────────────┤
│                 │ ── agent.run ───→  │                 │
│                 │                    │  [Agent Loop]   │
│                 │                    │  [Tool决策]     │
│                 │ ←── tool.call ──── │                 │
│  [本地执行]      │                    │  [挂起等待]     │
│                 │ ── tool.result ──→ │                 │
│                 │                    │  [继续执行]     │
│                 │ ←── stream.chunk ─ │                 │
└─────────────────┘                    └─────────────────┘
```

---

## 2. 协议设计

### 2.1 ExecutionPolicy 枚举

```rust
/// 工具执行位置策略
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub enum ExecutionPolicy {
    /// 必须在 Server 执行（如：访问内部数据库）
    ServerOnly,

    /// 必须在 Client 执行（如：截图、系统通知）
    ClientOnly,

    /// 优先 Server，Client 无能力时不回退
    #[default]
    PreferServer,

    /// 优先 Client，无能力时回退到 Server
    PreferClient,
}
```

### 2.2 Client Manifest 结构

```rust
/// Client 能力声明 Manifest
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientManifest {
    /// 客户端类型标识
    pub client_type: String,  // "macos_native", "tauri", "cli", "web"

    /// 客户端版本，用于协议兼容性检查
    pub client_version: String,  // "1.2.0"

    /// 能力声明
    pub capabilities: ClientCapabilities,

    /// 运行环境信息
    pub environment: ClientEnvironment,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientCapabilities {
    /// 支持的工具类别
    pub tool_categories: Vec<String>,  // ["shell", "file_system", "ui"]

    /// 明确支持的具体工具
    pub specific_tools: Vec<String>,  // ["applescript:run"]

    /// 明确排除的工具
    pub excluded_tools: Vec<String>,  // ["shell:sudo"]

    /// 执行约束
    pub constraints: ExecutionConstraints,

    /// 权限上下文（可选）
    pub granted_scopes: Option<HashMap<String, Vec<String>>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionConstraints {
    /// 最大并发工具数
    pub max_concurrent_tools: u32,  // 3

    /// 单工具超时（毫秒）
    pub timeout_ms: u64,  // 30000
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientEnvironment {
    pub os: String,       // "macos", "windows", "linux", "web"
    pub arch: String,     // "arm64", "x86_64", "wasm"
    pub sandbox: bool,    // 是否在沙箱环境
}
```

---

## 3. 反向 RPC 机制

### 3.1 ReverseRpcManager

```rust
/// 反向 RPC 请求管理器
pub struct ReverseRpcManager {
    /// 等待响应的请求表：request_id -> oneshot sender
    pending: DashMap<String, oneshot::Sender<JsonRpcResponse>>,

    /// 请求 ID 生成器
    id_counter: AtomicU64,

    /// 默认超时
    default_timeout: Duration,
}

impl ReverseRpcManager {
    /// 向 Client 发送请求并等待响应
    pub async fn call(
        &self,
        conn: &WebSocketConnection,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
    ) -> Result<Value> {
        let id = self.next_id();
        let (tx, rx) = oneshot::channel();

        self.pending.insert(id.clone(), tx);

        // 发送请求到 Client
        conn.send(JsonRpcRequest {
            jsonrpc: "2.0",
            method: method.to_string(),
            params: Some(params),
            id: Some(id.clone()),
        }).await?;

        // 等待响应（带超时）
        let timeout = timeout.unwrap_or(self.default_timeout);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(response)) => response.into_result(),
            Ok(Err(_)) => Err(Error::ConnectionClosed),
            Err(_) => Err(Error::Timeout),
        }
    }

    /// 处理来自 Client 的响应
    pub fn handle_response(&self, response: JsonRpcResponse) {
        if let Some(id) = &response.id {
            if let Some((_, tx)) = self.pending.remove(id) {
                let _ = tx.send(response);
            }
        }
    }
}
```

### 3.2 Client 端工具执行

```rust
/// Client 需要实现的工具执行接口
pub trait LocalToolExecutor: Send + Sync {
    /// 执行本地工具
    fn execute(&self, tool_name: &str, args: Value)
        -> Pin<Box<dyn Future<Output = Result<Value>> + Send>>;
}

/// Client 消息处理循环
async fn client_message_loop(
    ws: WebSocketStream,
    executor: Arc<dyn LocalToolExecutor>,
) {
    while let Some(msg) = ws.next().await {
        match parse_message(msg) {
            // 处理 Server 的工具调用请求
            Message::Request(req) if req.method == "tool.call" => {
                let result = executor.execute(
                    &req.params["tool"],
                    req.params["args"].clone(),
                ).await;

                ws.send(JsonRpcResponse {
                    id: req.id,
                    result: Some(result),
                    error: None,
                }).await;
            }
            // 处理其他消息（事件流等）
            Message::Event(event) => handle_event(event),
            _ => {}
        }
    }
}
```

---

## 4. 路由决策引擎

### 4.1 ToolRouter

```rust
/// 工具路由决策器
pub struct ToolRouter {
    /// Server 端工具注册表
    server_registry: Arc<ToolRegistry>,

    /// 配置覆盖（优先级最高）
    config_overrides: HashMap<String, ExecutionPolicy>,
}

/// 路由决策结果
pub enum RoutingDecision {
    /// 在 Server 本地执行
    ExecuteLocal,

    /// 路由到 Client 执行
    RouteToClient,

    /// 无法执行
    CannotExecute { reason: String },
}

impl ToolRouter {
    /// 决定工具执行位置
    pub fn resolve(
        &self,
        tool_name: &str,
        tool_policy: ExecutionPolicy,
        client_manifest: Option<&ClientManifest>,
    ) -> RoutingDecision {
        // 1. 检查配置覆盖（最高优先级）
        let effective_policy = self.config_overrides
            .get(tool_name)
            .copied()
            .unwrap_or(tool_policy);

        // 2. 检查 Client 能力
        let client_capable = client_manifest
            .map(|m| self.check_client_capability(tool_name, m))
            .unwrap_or(false);

        // 3. 检查 Server 能力
        let server_capable = self.server_registry.has_tool(tool_name);

        // 4. 根据 Policy 决策
        match effective_policy {
            ExecutionPolicy::ServerOnly => {
                if server_capable {
                    RoutingDecision::ExecuteLocal
                } else {
                    RoutingDecision::CannotExecute {
                        reason: format!("Tool '{}' requires Server", tool_name)
                    }
                }
            }

            ExecutionPolicy::ClientOnly => {
                if client_capable {
                    RoutingDecision::RouteToClient
                } else {
                    RoutingDecision::CannotExecute {
                        reason: format!("Tool '{}' requires Client", tool_name)
                    }
                }
            }

            ExecutionPolicy::PreferServer => {
                if server_capable {
                    RoutingDecision::ExecuteLocal
                } else if client_capable {
                    RoutingDecision::RouteToClient
                } else {
                    RoutingDecision::CannotExecute {
                        reason: format!("Tool '{}' unavailable", tool_name)
                    }
                }
            }

            ExecutionPolicy::PreferClient => {
                if client_capable {
                    RoutingDecision::RouteToClient
                } else if server_capable {
                    RoutingDecision::ExecuteLocal
                } else {
                    RoutingDecision::CannotExecute {
                        reason: format!("Tool '{}' unavailable", tool_name)
                    }
                }
            }
        }
    }

    fn check_client_capability(&self, tool_name: &str, manifest: &ClientManifest) -> bool {
        let caps = &manifest.capabilities;

        if caps.excluded_tools.contains(&tool_name.to_string()) {
            return false;
        }

        let category = tool_name.split(':').next().unwrap_or(tool_name);

        caps.specific_tools.contains(&tool_name.to_string())
            || caps.tool_categories.contains(&category.to_string())
    }
}
```

### 4.2 ExecutionEngine 集成

```rust
impl ExecutionEngine {
    pub async fn execute_tool(
        &self,
        tool_name: &str,
        args: Value,
        session: &Session,
    ) -> Result<Value> {
        let tool = self.registry.get_tool(tool_name)?;
        let client_manifest = session.client_manifest();

        match self.router.resolve(tool_name, tool.policy, client_manifest) {
            RoutingDecision::ExecuteLocal => {
                self.local_executor.execute(tool_name, args).await
            }

            RoutingDecision::RouteToClient => {
                self.reverse_rpc.call(
                    session.connection(),
                    "tool.call",
                    json!({ "tool": tool_name, "args": args }),
                    Some(Duration::from_millis(
                        client_manifest.unwrap().capabilities.constraints.timeout_ms
                    )),
                ).await
            }

            RoutingDecision::CannotExecute { reason } => {
                Err(Error::ToolUnavailable(reason))
            }
        }
    }
}
```

---

## 5. 实施计划

### 5.1 分阶段路线

```
Phase 1: 协议基础          Phase 2: 反向 RPC         Phase 3: 路由集成
┌─────────────────┐       ┌─────────────────┐       ┌─────────────────┐
│ ExecutionPolicy │       │ ReverseRpcMgr   │       │ ToolRouter      │
│ ClientManifest  │  ──→  │ PendingRequests │  ──→  │ ExecutionEngine │
│ ConnectParams   │       │ tool.call 协议   │       │ Shadow Registry │
└─────────────────┘       └─────────────────┘       └─────────────────┘
```

### 5.2 Phase 1: 协议基础

| 任务 | 文件 | 描述 |
|------|------|------|
| 1.1 | `core/src/dispatcher/types.rs` | 定义 `ExecutionPolicy` 枚举 |
| 1.2 | `core/src/gateway/protocol.rs` | 定义 `ClientManifest` 结构 |
| 1.3 | `core/src/gateway/handlers/auth.rs` | 扩展 `connect` 参数解析 |
| 1.4 | `core/src/gateway/server.rs` | `ConnectionState` 存储 Manifest |

### 5.3 Phase 2: 反向 RPC

| 任务 | 文件 | 描述 |
|------|------|------|
| 2.1 | `core/src/gateway/reverse_rpc.rs` | 新建 `ReverseRpcManager` |
| 2.2 | `core/src/gateway/server.rs` | 集成反向 RPC 到消息循环 |
| 2.3 | `core/src/gateway/protocol.rs` | 定义 `tool.call` / `tool.result` 消息 |
| 2.4 | 测试 | 单元测试：请求/响应匹配、超时处理 |

### 5.4 Phase 3: 路由集成

| 任务 | 文件 | 描述 |
|------|------|------|
| 3.1 | `core/src/executor/router.rs` | 新建 `ToolRouter` |
| 3.2 | `core/src/executor/engine.rs` | 集成路由决策到执行流程 |
| 3.3 | `core/src/tools/mod.rs` | `UnifiedTool` 增加 `policy` 字段 |
| 3.4 | `core/src/config/` | 配置文件支持 policy 覆盖 |
| 3.5 | 集成测试 | 端到端测试：Server-Client 工具调用 |

### 5.5 配置文件示例

```toml
# config.toml
[tool_routing]
# 配置覆盖（优先级最高）
overrides = [
    { tool = "shell:exec", policy = "ServerOnly" },
    { tool = "file:read", policy = "PreferClient" },
    { tool = "data_export", policy = "ServerOnly" },
]

# 默认策略
default_policy = "PreferServer"
```

---

## 6. 设计决策记录

| 决策 | 选择 | 理由 |
|------|------|------|
| 路由策略 | Policy 驱动 + 能力协商 | 精细控制，符合工具本质 |
| Manifest 格式 | 完整能力描述 | 环境感知、约束传递、版本演进 |
| 能力 vs 权限 | 分离设计 | Server 可提前拦截越权请求 |
| 反向 RPC | oneshot channel | 标准异步请求/响应模式 |

---

## 7. 验证测试

### 7.1 debug.tool_call 端点

用于验证反向 RPC 机制的测试端点：

**请求：**
```json
{
  "jsonrpc": "2.0",
  "method": "debug.tool_call",
  "params": {
    "tool": "shell:exec",
    "args": {"command": "echo hello"},
    "timeout_ms": 30000
  },
  "id": 1
}
```

**响应：**
```json
{
  "jsonrpc": "2.0",
  "result": {
    "success": true,
    "result": {
      "exit_code": 0,
      "stdout": "hello\n",
      "stderr": "",
      "duration_ms": 15
    },
    "duration_ms": 20,
    "executed_on": "client"
  },
  "id": 1
}
```

### 7.2 测试步骤

1. **启动 Gateway Server**
   ```bash
   cargo run -p alephcore --features gateway
   ```

2. **使用 CLI 连接**
   ```bash
   cargo run -p aleph-cli -- connect
   ```

3. **发送测试请求**（使用 websocat 或其他 WebSocket 客户端）
   ```bash
   echo '{"jsonrpc":"2.0","method":"debug.tool_call","params":{"tool":"shell:exec","args":{"command":"echo hello"}},"id":1}' | websocat ws://127.0.0.1:18789
   ```

4. **验证结果**
   - CLI 应该执行本地 shell 命令
   - Server 应该收到执行结果
   - 调用者应该收到完整响应

### 7.3 CLI 端实现

CLI 的 `LocalExecutor` 实现了 `shell:exec` 工具：

```rust
// clients/cli/src/executor.rs
pub struct LocalExecutor;

impl LocalExecutor {
    pub async fn execute(tool_name: &str, args: Value) -> Result<Value, String> {
        match tool_name {
            "shell:exec" | "shell_exec" | "exec" => {
                Self::execute_shell(args).await
            }
            _ => Err(format!("Unknown tool: {}", tool_name))
        }
    }
}
```

### 7.4 实现状态

| 组件 | 状态 | 文件 |
|------|------|------|
| `aleph-protocol` | ✅ 完成 | `shared/protocol/` |
| `ExecutionPolicy` | ✅ 完成 | `core/src/dispatcher/types.rs` |
| `ClientManifest` | ✅ 完成 | `core/src/gateway/client_manifest.rs` |
| `ReverseRpcManager` | ✅ 完成 | `core/src/gateway/reverse_rpc.rs` |
| `ToolRouter` | ✅ 完成 | `core/src/executor/router.rs` |
| `LocalExecutor` (CLI) | ✅ 完成 | `clients/cli/src/executor.rs` |
| CLI 反向 RPC 处理 | ✅ 完成 | `clients/cli/src/client.rs` |
| `debug.tool_call` | ✅ 完成 | `core/src/gateway/server.rs` |
| Agent Loop 集成 | ⏳ 待完成 | - |

---

## 8. 未来演进方向

### 8.1 多端拓扑

当多个 Client 在线时，Server 根据 Capability Manifest 的"环境权重"自动选择执行端。

### 8.2 Zero-Trust 安全

- 敏感工具签名
- Local UI Confirmation
- Scoped Capability

### 8.3 二进制流与大文件

- 远程文件句柄
- 二进制 Side-Channel

### 8.4 Ghost-SDK

- 跨语言协议代码生成
- Headless Runner 核心库
