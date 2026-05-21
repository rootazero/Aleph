# Aleph → Rust Moltbot 全面重构设计

**日期**: 2026-01-28
**状态**: 规划阶段
**目标**: 将 Aleph 全面改造为 Rust 版 Moltbot，采用 Gateway 中心化架构

---

## 1. 改造愿景

### 1.1 为什么要全面重构？

Moltbot 的架构设计极其巧妙，核心优势包括：

1. **单一 WebSocket 控制平面 (Gateway)** - 所有组件通过 `ws://127.0.0.1:18789` 统一通信
2. **多通道整合** - WhatsApp/Telegram/Slack/Discord 等所有平台统一对话线程
3. **沙箱隔离执行** - 主会话 (完整权限) vs 非主会话 (Docker 容器隔离)
4. **本地浏览器控制** - Chrome DevTools Protocol 深度集成
5. **事件驱动自动化** - Cron 调度 + Webhook 监听
6. **Agent 间协作** - 通过 `sessions_list/send` 工具实现多 Agent 协调

**Aleph 当前问题**：
- 架构碎片化：UI 层、Rust 核心、Agent Loop 各自独立
- 缺乏统一控制平面，组件间通信复杂
- 多平台集成困难（WhatsApp/Telegram 等需要大量定制开发）
- 权限模型不够细粒度（无会话级沙箱隔离）

### 1.2 改造目标

**核心目标**：将 Aleph 改造为 **Rust 原生的 Moltbot**，保持 Rust 性能优势，采用 Moltbot 的架构模式。

**四大核心能力**（已确认）：
1. ✅ **Gateway 中心化** - WebSocket 控制平面协调所有组件
2. ✅ **多通道集成** - 跨平台消息聚合
3. ✅ **沙箱隔离执行** - 会话级别权限控制
4. ✅ **本地工具增强** - Chrome CDP、Cron、Webhook

**平台策略**：
- 阶段式开发：**macOS 优先**，稳定后再考虑 Tauri 跨平台
- 保持 Swift + SwiftUI native 优势，增加 WebSocket 客户端连接 Gateway

**能力策略**：
- **全新实现** Agent Loop 和所有组件
- 对标 Moltbot 的功能和设计模式
- 保留 Rust 语言优势（性能、类型安全、内存安全）

---

## 2. 架构设计

### 2.1 总体架构

```
┌─────────────────────────────────────────────────────────────────────┐
│                        Aleph Gateway (Rust)                         │
│                     WebSocket Server :18789                          │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │               Gateway Message Router                            │ │
│  │   - RPC dispatch (JSON-RPC 2.0 protocol)                       │ │
│  │   - Event broadcasting to connected clients                    │ │
│  │   - Session management (main vs sandbox)                       │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │               Pi Agent Runtime (Rust)                           │ │
│  │   - Observe-Think-Act-Feedback loop                            │ │
│  │   - Tool execution orchestration                               │ │
│  │   - Streaming responses (partial results)                      │ │
│  │   - Model failover and rotation                                │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │               Channel Connectors (Rust)                         │ │
│  │   - Telegram Bot API                                           │ │
│  │   - Discord Bot                                                │ │
│  │   - Slack Bot                                                  │ │
│  │   - WhatsApp Web (via browser automation)                      │ │
│  │   - Signal                                                     │ │
│  │   - iMessage (macOS native via AppleScript)                    │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │               Local Tools (Rust)                                │ │
│  │   - Chrome CDP Controller (browser automation)                 │ │
│  │   - Cron Scheduler (task scheduling)                           │ │
│  │   - Webhook Listener (event-driven workflows)                  │ │
│  │   - File operations                                            │ │
│  │   - System integration (AppleScript, shell)                    │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │               Sandbox Manager (Docker)                          │ │
│  │   - Main session: full tool access                             │ │
│  │   - Non-main session: Docker container isolation               │ │
│  │   - Permission escalation: /elevated on|off                    │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
                              │
                 ┌────────────┴────────────┐
                 │                         │
                 ▼                         ▼
    ┌────────────────────┐    ┌────────────────────┐
    │  macOS Native App  │    │    CLI Client      │
    │  (Swift + SwiftUI) │    │    (Rust)          │
    │  WebSocket Client  │    │  WebSocket Client  │
    └────────────────────┘    └────────────────────┘
                 │
                 ▼
         ┌────────────────────┐
         │  WebChat UI        │
         │  (Browser)         │
         │  WebSocket Client  │
         └────────────────────┘
```

### 2.2 核心组件

#### 2.2.1 Gateway (WebSocket Server)

**职责**：
- 监听 `ws://127.0.0.1:18789`
- JSON-RPC 2.0 协议处理
- 客户端连接管理
- 事件广播机制
- 会话状态管理

**技术栈**：
- Rust + `tokio-tungstenite` (异步 WebSocket)
- `serde_json` (JSON 序列化)
- `dashmap` (并发安全的 HashMap)

**核心 RPC 方法**：
```rust
// Agent execution
agent.message.send { message, channel, session_id }
agent.thinking.stream { delta }
agent.action.execute { tool, args, session_id }

// Session management
sessions.list {}
sessions.history { session_id, limit }
sessions.send { session_id, message }

// Channel routing
channels.status {}
channels.send { channel, recipient, message }

// Sandbox control
sandbox.create { session_id, mode }  // "main" | "docker"
sandbox.elevate { session_id, enabled }
```

#### 2.2.2 Pi Agent Runtime

**职责**：
- Agent Loop 执行（Observe → Think → Act → Feedback）
- 工具调用编排
- 流式响应处理
- 模型故障切换

**与 Moltbot 对齐**：
- RPC 模式：客户端通过 Gateway 发送消息，Agent 返回流式响应
- 工具流式执行：工具调用结果增量返回
- Thinking 流：推理过程实时流式输出

**核心流程**：
```
User Message (via Gateway)
    │
    ▼
┌──────────────────────────────────────────┐
│  1. Observe: Collect context             │
│     - Session history                    │
│     - Channel context                    │
│     - Tool outputs                       │
└──────────────────────────────────────────┘
    │
    ▼
┌──────────────────────────────────────────┐
│  2. Think: LLM Decision Making           │
│     - Stream thinking process            │
│     - Select tools to call               │
│     - Generate reasoning                 │
└──────────────────────────────────────────┘
    │
    ▼
┌──────────────────────────────────────────┐
│  3. Act: Execute Tools                   │
│     - Check permissions                  │
│     - Execute in sandbox if needed       │
│     - Stream tool outputs                │
└──────────────────────────────────────────┘
    │
    ▼
┌──────────────────────────────────────────┐
│  4. Feedback: Return to user             │
│     - Route to originating channel       │
│     - Update session history             │
└──────────────────────────────────────────┘
```

#### 2.2.3 Channel Connectors

**职责**：
- 连接外部消息平台
- 双向消息路由
- 统一消息格式转换

**支持平台**：
| 平台 | 实现方式 | 优先级 |
|------|---------|-------|
| Telegram | Bot API (HTTP) | P0 |
| Discord | Discord Bot (Gateway + HTTP API) | P0 |
| Slack | Bolt SDK (WebSocket + HTTP) | P0 |
| WhatsApp | Browser automation (Chrome CDP) | P1 |
| Signal | Signal CLI (subprocess) | P1 |
| iMessage | AppleScript (macOS native) | P1 |
| WebChat | HTTP Server (内置) | P0 |

**消息路由**：
```
External Message (Telegram/Discord/etc.)
    │
    ▼
Channel Connector (parse message)
    │
    ▼
Gateway (route to Pi Agent)
    │
    ▼
Pi Agent (process message)
    │
    ▼
Gateway (broadcast response)
    │
    ▼
Channel Connector (send to original channel)
```

#### 2.2.4 Sandbox Manager

**职责**：
- 会话级别权限控制
- Docker 容器生命周期管理
- 主会话 vs 非主会话区分

**权限模型**：
```
┌────────────────────────────────────────┐
│  Main Session (DM with user)           │
│  - Full tool access                    │
│  - No Docker isolation                 │
│  - Elevated mode: /elevated on         │
└────────────────────────────────────────┘

┌────────────────────────────────────────┐
│  Non-Main Session (Groups/Channels)    │
│  - Docker container isolated           │
│  - Limited tool access                 │
│  - Network restricted                  │
└────────────────────────────────────────┘
```

**Docker 容器管理**：
- 基于 `bollard` crate (Docker API)
- 每个非主会话创建独立容器
- 容器自动清理机制
- 资源限制（CPU、内存、网络）

#### 2.2.5 Local Tools

**Chrome CDP Controller**：
- 管理专用 Chromium 实例
- 基于 `chromiumoxide` crate (CDP protocol)
- 支持页面截图、DOM 操作、JavaScript 执行

**Cron Scheduler**：
- 基于 `tokio-cron-scheduler` crate
- 持久化任务配置（SQLite）
- 支持自然语言任务定义

**Webhook Listener**：
- HTTP Server (基于 `axum`)
- 动态 endpoint 创建
- Webhook → Agent 消息转换

---

## 3. 功能对比与实现路径

### 3.1 Moltbot 核心功能 → Aleph 实现

| Moltbot 功能 | 对应 Aleph 实现 | 优先级 | 技术栈 |
|--------------|----------------|-------|-------|
| Gateway (WebSocket) | 全新实现 `aleph_gateway` crate | P0 | tokio-tungstenite |
| Pi Agent Runtime | 重写 Agent Loop | P0 | Custom Rust |
| Channel Connectors | 新增 `channels/` 模块 | P0 | 各平台 SDK/API |
| Sandbox Docker | 新增 `sandbox/` 模块 | P1 | bollard |
| Chrome CDP | 新增 `tools/browser/` 模块 | P1 | chromiumoxide |
| Cron Scheduler | 新增 `tools/cron/` 模块 | P2 | tokio-cron-scheduler |
| Webhook Listener | 新增 `tools/webhook/` 模块 | P2 | axum |
| Multi-agent Coordination | `sessions_*` tools | P1 | Custom RPC |
| Model Failover | Agent Loop 增强 | P1 | Custom logic |
| Skills System | 保留并增强 | P0 | 现有实现 |

### 3.2 阶段性实现计划

#### Phase 1: Gateway 基础设施 (2 周)

**目标**: 构建 WebSocket Gateway 和 RPC 框架

**交付物**：
- [ ] `aleph_gateway` crate (WebSocket Server)
- [ ] JSON-RPC 2.0 协议实现
- [ ] 客户端连接管理
- [ ] 基础 RPC 方法（`agent.message.send`, `sessions.list`）
- [ ] CLI 客户端（测试用）

**技术决策**：
- WebSocket: `tokio-tungstenite`
- RPC 框架: 自定义实现（基于 JSON-RPC 2.0 spec）
- 并发模型: `tokio` 异步运行时

#### Phase 2: Pi Agent Runtime (3 周)

**目标**: 重写 Agent Loop，支持 RPC 模式和流式执行

**交付物**：
- [ ] Agent Loop 核心循环（Observe-Think-Act-Feedback）
- [ ] 流式响应处理（delta streaming）
- [ ] 工具调用框架
- [ ] 会话历史管理
- [ ] 模型路由和故障切换

**关键挑战**：
- Rust 异步流式处理（`tokio::sync::mpsc` + `futures::Stream`）
- 工具调用增量输出
- 思维过程流式可视化

#### Phase 3: Channel Connectors (4 周)

**目标**: 实现多平台消息集成

**交付物**：
- [ ] Telegram Bot (基于 `teloxide`)
- [ ] Discord Bot (基于 `serenity`)
- [ ] Slack Bot (基于 `slack-morphism`)
- [ ] WebChat UI (基于 `axum` + React)
- [ ] 消息路由和格式转换

**优先级**：
1. WebChat (内部测试)
2. Telegram (易于集成)
3. Discord (社区常用)
4. Slack (企业场景)

#### Phase 4: Sandbox 和权限管理 (2 周)

**目标**: 实现会话级别权限控制和 Docker 隔离

**交付物**：
- [ ] 主会话 vs 非主会话识别
- [ ] Docker 容器管理（基于 `bollard`）
- [ ] 权限检查框架
- [ ] `/elevated` 命令

**安全要求**：
- 非主会话强制 Docker 隔离
- 网络访问限制
- 文件系统隔离
- 资源配额限制

#### Phase 5: 本地工具增强 (3 周)

**目标**: 添加 Chrome CDP、Cron、Webhook 支持

**交付物**：
- [ ] Chrome CDP Controller（基于 `chromiumoxide`）
- [ ] Cron 调度器（基于 `tokio-cron-scheduler`）
- [ ] Webhook Listener（基于 `axum`）
- [ ] 工具注册和发现机制

**工具设计**：
```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> ToolSchema;
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput>;
}
```

#### Phase 6: macOS Native 集成 (2 周)

**目标**: 将 Swift UI 连接到 Gateway

**交付物**：
- [ ] Swift WebSocket Client
- [ ] RPC 方法 Swift 封装
- [ ] 流式响应 UI 渲染
- [ ] 菜单栏 App 集成

**技术栈**：
- Swift Concurrency (async/await)
- SwiftUI + Combine
- URLSession (WebSocket)

---

## 4. 技术架构细节

### 4.1 Gateway 协议设计

#### 4.1.1 连接流程

```
Client                        Gateway
  │                             │
  ├─ ws://127.0.0.1:18789 ─────▶│
  │                             │
  │◀─ { type: "welcome" } ──────┤
  │                             │
  ├─ { method: "auth", ... } ───▶│
  │                             │
  │◀─ { result: "success" } ────┤
  │                             │
```

#### 4.1.2 RPC 消息格式

**Request**:
```json
{
  "jsonrpc": "2.0",
  "id": "req-123",
  "method": "agent.message.send",
  "params": {
    "message": "Hello",
    "session_id": "main",
    "channel": "telegram",
    "stream": true
  }
}
```

**Streaming Response**:
```json
{
  "jsonrpc": "2.0",
  "id": "req-123",
  "result": {
    "type": "stream",
    "event": "thinking.delta",
    "data": {
      "delta": "I'm analyzing your request..."
    }
  }
}
```

**Final Response**:
```json
{
  "jsonrpc": "2.0",
  "id": "req-123",
  "result": {
    "type": "final",
    "message": "Here's the answer...",
    "tool_calls": [
      { "tool": "search", "status": "completed", "output": "..." }
    ]
  }
}
```

#### 4.1.3 Event Broadcasting

**Event Format**:
```json
{
  "jsonrpc": "2.0",
  "method": "event.broadcast",
  "params": {
    "event_type": "tool.completed",
    "session_id": "main",
    "data": {
      "tool": "search",
      "output": "..."
    }
  }
}
```

### 4.2 Agent Loop 详细设计

#### 4.2.1 Loop State Machine

```
┌─────────┐
│  IDLE   │
└────┬────┘
     │ message received
     ▼
┌─────────┐
│ OBSERVE │ ─────┐
└────┬────┘      │ context insufficient
     │           │
     │           ▼
     │      ┌─────────┐
     │      │  WAIT   │
     │      └────┬────┘
     │           │ context arrived
     │           │
     │◀──────────┘
     │
     ▼
┌─────────┐
│  THINK  │ ─────┐
└────┬────┘      │ need clarification
     │           │
     │           ▼
     │      ┌─────────┐
     │      │   ASK   │
     │      └────┬────┘
     │           │ user replied
     │           │
     │◀──────────┘
     │
     ▼
┌─────────┐
│   ACT   │ ─────┐
└────┬────┘      │ tool failed
     │           │
     │           ▼
     │      ┌─────────┐
     │      │ RECOVER │
     │      └────┬────┘
     │           │ retry
     │           │
     │◀──────────┘
     │
     ▼
┌─────────┐
│FEEDBACK │
└────┬────┘
     │
     ▼
┌─────────┐
│  IDLE   │
└─────────┘
```

#### 4.2.2 工具执行模型

**同步工具**（阻塞执行）：
```rust
#[async_trait]
impl Tool for FileReadTool {
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let path: String = serde_json::from_value(args["path"].clone())?;
        let content = tokio::fs::read_to_string(path).await?;
        Ok(ToolOutput::Text(content))
    }
}
```

**流式工具**（增量输出）：
```rust
#[async_trait]
impl Tool for SearchTool {
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let query: String = serde_json::from_value(args["query"].clone())?;
        let (tx, rx) = tokio::sync::mpsc::channel(32);

        // Spawn background task for streaming
        tokio::spawn(async move {
            for result in search_stream(query).await {
                tx.send(ToolOutputChunk::Result(result)).await.ok();
            }
        });

        Ok(ToolOutput::Stream(rx))
    }
}
```

### 4.3 Sandbox 隔离机制

#### 4.3.1 Docker 容器配置

**Base Image**:
```dockerfile
FROM rust:1.85-slim
RUN apt-get update && apt-get install -y \
    curl \
    git \
    python3 \
    nodejs \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /workspace
```

**容器启动参数**:
```rust
use bollard::container::{Config, CreateContainerOptions};

let config = Config {
    image: Some("aleph-sandbox:latest"),
    network_disabled: Some(false),  // 允许网络但限制出站
    host_config: Some(HostConfig {
        memory: Some(512 * 1024 * 1024),  // 512MB
        cpu_quota: Some(50000),  // 50% CPU
        readonly_rootfs: Some(true),
        ..Default::default()
    }),
    ..Default::default()
};
```

#### 4.3.2 权限矩阵

| Tool Category | Main Session | Non-Main Session | Elevated |
|--------------|-------------|------------------|----------|
| File Read | ✅ | ✅ (limited paths) | ✅ |
| File Write | ✅ | ❌ | ✅ |
| Shell Execution | ✅ | ❌ | ✅ |
| Browser Control | ✅ | ❌ | ✅ |
| Network Access | ✅ | ✅ (restricted) | ✅ |
| System Info | ✅ | ✅ | ✅ |

### 4.4 Channel Connector 接口

**统一消息格式**:
```rust
pub struct UnifiedMessage {
    pub id: String,
    pub channel: String,
    pub sender: Sender,
    pub content: MessageContent,
    pub timestamp: i64,
    pub metadata: HashMap<String, Value>,
}

pub enum MessageContent {
    Text(String),
    Image { url: String, caption: Option<String> },
    File { url: String, filename: String },
    Audio { url: String, duration: Option<u64> },
    Video { url: String, duration: Option<u64> },
}
```

**Connector Trait**:
```rust
#[async_trait]
pub trait ChannelConnector: Send + Sync {
    fn name(&self) -> &str;
    async fn start(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn send(&self, recipient: &str, message: &UnifiedMessage) -> Result<()>;
    async fn receive(&self) -> impl Stream<Item = UnifiedMessage>;
}
```

---

## 5. 数据模型

### 5.1 Session 管理

**Session 结构**:
```rust
pub struct Session {
    pub id: String,
    pub mode: SessionMode,
    pub channel: String,
    pub user_id: String,
    pub history: Vec<ChatMessage>,
    pub metadata: SessionMetadata,
    pub created_at: i64,
    pub last_active: i64,
}

pub enum SessionMode {
    Main { elevated: bool },
    NonMain { container_id: String },
}
```

**持久化**:
- SQLite 存储（`~/.aleph/sessions.db`）
- 自动压缩历史（保留最近 50 条消息）
- 过期会话清理（7 天未活动）

### 5.2 Tool Registry

**Tool 注册**:
```rust
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    categories: HashMap<String, Vec<String>>,
}

impl ToolRegistry {
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.insert(name.clone(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }
}
```

### 5.3 配置管理

**统一配置文件** (`~/.aleph/config.toml`):
```toml
[gateway]
host = "127.0.0.1"
port = 18789
max_connections = 100

[agent]
default_model = "claude-sonnet-4-5"
fallback_models = ["claude-opus-4-5", "gpt-4-turbo"]
max_loops = 20

[channels.telegram]
enabled = true
token = "${TELEGRAM_BOT_TOKEN}"

[channels.discord]
enabled = true
token = "${DISCORD_BOT_TOKEN}"

[sandbox]
enabled = true
docker_image = "aleph-sandbox:latest"
memory_limit_mb = 512
cpu_quota_percent = 50

[tools.chrome]
enabled = true
executable_path = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
```

---

## 6. 性能和可靠性

### 6.1 性能目标

| 指标 | 目标值 | 测量方式 |
|-----|-------|---------|
| Gateway 延迟 | < 10ms | P99 RPC 响应时间 |
| Agent Loop 首字节 | < 500ms | TTFB (Time To First Byte) |
| 工具执行开销 | < 50ms | 工具调用到执行时间 |
| 并发连接数 | 100+ | WebSocket 连接 |
| 内存占用 | < 500MB | Gateway + Agent 总内存 |

### 6.2 可靠性设计

**故障恢复**:
- Agent Loop 崩溃自动重启
- Docker 容器异常自动清理
- Channel Connector 重连机制
- 工具执行超时保护

**监控和日志**:
- 结构化日志（JSON 格式）
- 指标采集（Prometheus 格式）
- 分布式追踪（OpenTelemetry）

**数据持久化**:
- SQLite WAL 模式（高并发）
- 定期自动备份
- 崩溃恢复机制

---

## 7. 安全性

### 7.1 威胁模型

**外部威胁**:
- 恶意消息注入（XSS、命令注入）
- 未授权访问 Gateway
- 恶意工具调用

**内部威胁**:
- 非主会话逃逸（Docker breakout）
- 资源耗尽攻击（DoS）
- 敏感数据泄露

### 7.2 安全措施

**输入验证**:
- 所有用户输入严格校验
- JSON Schema 验证 RPC 参数
- SQL 注入防护（参数化查询）

**权限控制**:
- 基于会话的权限模型
- 工具白名单机制
- 敏感操作二次确认

**网络安全**:
- Gateway 仅监听 localhost
- 远程访问通过 Tailscale/SSH 隧道
- HTTPS 强制（WebChat）

**数据安全**:
- 敏感配置加密存储（OS Keychain）
- Token 和密钥脱敏日志
- 定期安全审计

---

## 8. 测试策略

### 8.1 测试分层

**单元测试**:
- 覆盖率目标：80%+
- 工具：`cargo test`
- 重点：核心逻辑、RPC 协议、工具执行

**集成测试**:
- Gateway + Agent Loop 端到端
- Channel Connector 消息路由
- Sandbox 隔离有效性

**端到端测试**:
- 真实 Channel 集成（Telegram/Discord）
- 多会话并发场景
- 故障恢复流程

### 8.2 测试工具

**Mock 框架**:
- `mockall` - Rust mock library
- `wiremock` - HTTP mock server

**负载测试**:
- `k6` - WebSocket 压测
- `cargo-flamegraph` - 性能分析

---

## 9. 迁移路径

### 9.1 从现有 Aleph 迁移

**Phase 0: 兼容性保留**
- 保留现有 macOS App UI
- Swift 代码暂不改动
- FFI 层逐步替换为 WebSocket

**Phase 1: Gateway 并行部署**
- Gateway 和现有 Rust 核心共存
- UI 可选择连接方式

**Phase 2: 功能迁移**
- 逐步迁移功能到 Gateway 架构
- 保持 API 兼容性

**Phase 3: 切换和清理**
- 完全切换到 Gateway 模式
- 删除旧代码

### 9.2 数据迁移

**会话历史**:
- 从现有 SQLite 导出
- 转换为新 Session 格式
- 导入到新数据库

**配置迁移**:
- 自动检测旧配置
- 转换为新 TOML 格式
- 保留用户自定义设置

---

## 10. 时间表和里程碑

### 10.1 总体时间表

| 阶段 | 时长 | 交付内容 |
|-----|------|---------|
| Phase 1: Gateway | 2 周 | WebSocket Server + RPC |
| Phase 2: Agent Runtime | 3 周 | Agent Loop 重写 |
| Phase 3: Channel Connectors | 4 周 | Telegram/Discord/Slack/WebChat |
| Phase 4: Sandbox | 2 周 | Docker 隔离 |
| Phase 5: Local Tools | 3 周 | Chrome CDP/Cron/Webhook |
| Phase 6: macOS Integration | 2 周 | Swift UI 连接 |
| **总计** | **16 周** | **全功能 Rust Moltbot** |

### 10.2 里程碑定义

**M1: Gateway MVP** (Week 2)
- ✅ WebSocket Server 运行
- ✅ RPC 协议工作
- ✅ CLI 客户端连接

**M2: Agent Loop 可用** (Week 5)
- ✅ 基础 Agent Loop 运行
- ✅ 工具调用成功
- ✅ 流式响应工作

**M3: 多通道集成** (Week 9)
- ✅ Telegram Bot 正常
- ✅ Discord Bot 正常
- ✅ WebChat 可访问

**M4: 生产就绪** (Week 16)
- ✅ 所有功能完整
- ✅ 性能达标
- ✅ macOS App 集成
- ✅ 文档完善

---

## 11. 风险和缓解

### 11.1 技术风险

| 风险 | 影响 | 概率 | 缓解措施 |
|-----|------|------|---------|
| Rust 异步流式处理复杂 | 高 | 中 | 早期原型验证，参考 `tokio` 最佳实践 |
| Docker 性能开销 | 中 | 高 | 基准测试，优化容器配置 |
| Channel API 变更 | 中 | 低 | 使用官方 SDK，版本锁定 |
| macOS 权限限制 | 高 | 中 | TCC 提前测试，用户文档说明 |

### 11.2 项目风险

| 风险 | 影响 | 概率 | 缓解措施 |
|-----|------|------|---------|
| 范围蔓延 | 高 | 中 | 严格功能冻结，阶段性交付 |
| 时间估算不准 | 中 | 高 | 每周复审，及时调整 |
| 依赖项不稳定 | 低 | 低 | 使用成熟库，版本锁定 |

---

## 12. 下一步行动

### 12.1 立即开始

1. **创建新 Crate 结构**:
   ```bash
   cargo new aleph_gateway --lib
   cargo new aleph_agent --lib
   cargo new aleph_channels --lib
   cargo new aleph_sandbox --lib
   cargo new aleph_tools --lib
   ```

2. **技术验证**:
   - WebSocket + RPC 原型
   - Agent Loop 流式响应 POC
   - Docker API 集成测试

3. **文档编写**:
   - Gateway RPC API 规范
   - Agent Loop 状态机设计
   - Channel Connector 接口定义

### 12.2 协作分工

**架构设计师** (你):
- Gateway 协议设计
- Agent Loop 状态机
- 整体架构审查

**Rust 开发**:
- Gateway Server 实现
- Agent Runtime 实现
- Tool 框架开发

**Platform 集成**:
- Channel Connectors
- macOS App 改造
- Docker 配置

### 12.3 沟通计划

**每周同步**:
- 进度复审
- 技术难点讨论
- 风险识别

**里程碑演示**:
- 每 2 周演示可运行功能
- 收集反馈
- 调整优先级

---

## 13. 成功标准

### 13.1 功能完整性

- ✅ Gateway 稳定运行 100+ 并发连接
- ✅ Agent Loop 流式响应 < 500ms TTFB
- ✅ 3+ Channel Connectors 正常工作
- ✅ Docker 沙箱有效隔离
- ✅ Chrome CDP 浏览器控制成功
- ✅ macOS App 完整集成

### 13.2 性能指标

- ✅ Gateway RPC 延迟 < 10ms (P99)
- ✅ 内存占用 < 500MB
- ✅ 工具执行开销 < 50ms
- ✅ 支持 100+ 并发会话

### 13.3 可靠性

- ✅ 99.9% Gateway 可用性
- ✅ 故障自动恢复 < 5s
- ✅ 零数据丢失（会话历史）

### 13.4 安全性

- ✅ 通过安全审计
- ✅ 无已知严重漏洞
- ✅ 敏感数据加密存储

---

## 14. 附录

### 14.1 参考资料

**Moltbot 源码分析**:
- GitHub: https://github.com/moltbot/moltbot
- 本地路径: `/Users/zouguojun/Workspace/moltbot`
- Gateway: `src/gateway/`
- Agent Runtime: `src/agents/`
- Channel Connectors: `src/telegram/`, `src/discord/`, etc.

**技术文档**:
- JSON-RPC 2.0 Spec: https://www.jsonrpc.org/specification
- Docker API: https://docs.docker.com/engine/api/
- Chrome DevTools Protocol: https://chromedevtools.github.io/devtools-protocol/

**Rust Crates**:
- `tokio-tungstenite` - WebSocket
- `bollard` - Docker API
- `chromiumoxide` - Chrome CDP
- `teloxide` - Telegram Bot
- `serenity` - Discord Bot
- `slack-morphism` - Slack SDK

### 14.2 术语表

| 术语 | 定义 |
|-----|------|
| Gateway | WebSocket 控制平面，协调所有组件通信 |
| Pi Agent | Agent Loop 运行时，执行用户请求 |
| Channel Connector | 外部消息平台集成模块 |
| Main Session | 主会话，完整工具权限 |
| Non-Main Session | 非主会话，Docker 沙箱隔离 |
| RPC | Remote Procedure Call，远程过程调用 |
| CDP | Chrome DevTools Protocol |

---

**文档版本**: v1.0
**最后更新**: 2026-01-28
**状态**: 等待审批

**下一步**: 创建 Phase 1 详细实现计划
