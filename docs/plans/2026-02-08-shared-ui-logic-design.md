# Shared UI Logic Design

**Date**: 2026-02-08
**Status**: Approved
**Author**: Architecture Team

## Overview

`shared_ui_logic` 是 Aleph 的**端侧 SDK**，负责封装所有 Client 与 Server 交互的逻辑。它解决了"类型漂移"和"逻辑重写"的问题，实现真正的"一次构建，随处使用"。

## 核心价值

- **类型安全**：所有 RPC 调用在编译期验证，消除类型漂移
- **跨平台**：同时支持 WASM（浏览器）和原生（Tauri/CLI）
- **响应式**：基于 Leptos Signals，UI 自动更新
- **可观测**：内置 Agent 行为追踪和指标收集

## 架构概览

### Crate 结构

```
shared_ui_logic/
├── Cargo.toml
├── src/
│   ├── lib.rs                    # 模块导出
│   ├── connection/               # 连接层（core feature）
│   │   ├── mod.rs
│   │   ├── connector.rs          # AlephConnector trait
│   │   ├── wasm.rs               # WASM 实现
│   │   ├── native.rs             # 原生实现
│   │   └── reconnect.rs          # 重连策略
│   ├── protocol/                 # 协议层（core feature）
│   │   ├── mod.rs
│   │   ├── rpc.rs                # JSON-RPC 封装
│   │   ├── streaming.rs          # 流式数据
│   │   └── events.rs             # 事件分发
│   ├── state/                    # 状态层（leptos feature）
│   │   ├── mod.rs
│   │   ├── agent_state.rs        # Agent 状态机
│   │   ├── ui_state.rs           # UI 状态
│   │   └── cache.rs              # 本地缓存
│   ├── api/                      # API 层（leptos feature）
│   │   ├── mod.rs
│   │   ├── memory.rs
│   │   ├── plugins.rs
│   │   ├── providers.rs
│   │   └── config.rs
│   └── observability/            # 可观测性（observability feature）
│       ├── mod.rs
│       ├── trace.rs              # 时间轴追踪
│       ├── metrics.rs            # 指标收集
│       └── logs.rs               # 日志流
└── examples/
    ├── cli_client.rs             # CLI 示例（core only）
    └── dashboard.rs              # Dashboard 示例（full features）
```

### Feature Flags 设计

```toml
[features]
default = ["leptos"]

# 基础协议层（无 UI 依赖）
core = [
    "dep:serde",
    "dep:serde_json",
    "dep:thiserror",
    "dep:aleph-protocol",
]

# UI 状态层（依赖 Leptos）
leptos = [
    "core",
    "dep:leptos",
    "dep:leptos_reactive",
]

# 可观测性增强（Command Center 专用）
observability = [
    "leptos",
    "dep:tracing",
]

# WASM 支持
wasm = [
    "dep:wasm-bindgen",
    "dep:wasm-bindgen-futures",
    "dep:web-sys",
]

# 原生支持
native = [
    "dep:tokio",
    "dep:tokio-tungstenite",
]
```

## 详细设计

### 1. 连接层（Connection Layer）

#### 1.1 核心挑战

连接层需要同时支持两种完全不同的运行时环境：

- **WASM 环境**：浏览器中的 `web_sys::WebSocket`，单线程事件循环
- **原生环境**：Tokio 异步运行时的 `tokio_tungstenite`，多线程

#### 1.2 AlephConnector Trait

```rust
use async_trait::async_trait;
use serde_json::Value;

/// 统一的 WebSocket 连接抽象
#[async_trait(?Send)]  // WASM 不支持 Send
pub trait AlephConnector {
    /// 连接到 Gateway
    async fn connect(&mut self, url: &str) -> Result<(), ConnectionError>;

    /// 断开连接
    async fn disconnect(&mut self) -> Result<(), ConnectionError>;

    /// 发送消息
    async fn send(&mut self, message: Value) -> Result<(), ConnectionError>;

    /// 接收消息（返回 Stream）
    fn receive(&mut self) -> impl Stream<Item = Result<Value, ConnectionError>>;

    /// 检查连接状态
    fn is_connected(&self) -> bool;
}
```

#### 1.3 自动平台选择

```rust
#[cfg(target_arch = "wasm32")]
mod wasm;
#[cfg(target_arch = "wasm32")]
pub use wasm::WasmConnector as DefaultConnector;

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::NativeConnector as DefaultConnector;

/// 创建默认连接器
pub fn create_connector() -> DefaultConnector {
    DefaultConnector::new()
}
```

#### 1.4 重连策略

```rust
pub struct ReconnectStrategy {
    max_attempts: u32,
    current_attempt: u32,
    base_delay_ms: u64,
}

impl ReconnectStrategy {
    /// 计算下一次重连延迟（指数退避）
    pub fn next_delay(&mut self) -> Option<u64> {
        if self.current_attempt >= self.max_attempts {
            return None;
        }

        let delay = self.base_delay_ms * 2u64.pow(self.current_attempt);
        self.current_attempt += 1;
        Some(delay)
    }

    /// 重置重连计数
    pub fn reset(&mut self) {
        self.current_attempt = 0;
    }
}
```

### 2. 协议层（Protocol Layer）

#### 2.1 RPC 客户端封装

```rust
pub struct RpcClient {
    connector: Arc<RwLock<dyn AlephConnector>>,
    pending_requests: Arc<RwLock<HashMap<String, oneshot::Sender<JsonRpcResponse>>>>,
    next_id: Arc<RwLock<u64>>,
}

impl RpcClient {
    /// 发起 RPC 调用（类型安全）
    pub async fn call<P, R>(&self, method: &str, params: P) -> Result<R, RpcError>
    where
        P: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        // 生成请求 ID
        let id = self.generate_id().await;

        // 构造请求
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: id.clone(),
            method: method.to_string(),
            params: Some(serde_json::to_value(params)?),
        };

        // 创建响应通道
        let (tx, rx) = oneshot::channel();
        self.pending_requests.write().await.insert(id.clone(), tx);

        // 发送请求
        let mut connector = self.connector.write().await;
        connector.send(serde_json::to_value(&request)?).await?;
        drop(connector);

        // 等待响应（带超时）
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            rx
        ).await??;

        // 解析结果
        if let Some(error) = response.error {
            return Err(RpcError::ServerError(error.message));
        }

        let result = response.result.ok_or(RpcError::MissingResult)?;
        Ok(serde_json::from_value(result)?)
    }
}
```

#### 2.2 流式数据处理

```rust
pub struct StreamHandler {
    event_rx: mpsc::UnboundedReceiver<StreamEvent>,
}

impl StreamHandler {
    /// 将事件转换为 Stream
    pub fn into_stream(self) -> impl Stream<Item = StreamEvent> {
        futures::stream::unfold(self.event_rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
    }

    /// 过滤特定类型的事件
    pub fn filter_by_type(
        self,
        event_type: &str,
    ) -> impl Stream<Item = StreamEvent> {
        let event_type = event_type.to_string();
        self.into_stream()
            .filter(move |event| {
                futures::future::ready(event.event_type == event_type)
            })
    }
}
```

#### 2.3 事件分发系统

```rust
pub struct EventDispatcher {
    handlers: Arc<RwLock<HashMap<String, Vec<EventCallback>>>>,
}

impl EventDispatcher {
    /// 订阅事件
    pub async fn subscribe<F>(&self, topic: &str, callback: F)
    where
        F: Fn(Value) + Send + Sync + 'static,
    {
        let mut handlers = self.handlers.write().await;
        handlers
            .entry(topic.to_string())
            .or_insert_with(Vec::new)
            .push(Arc::new(callback));
    }

    /// 分发事件
    pub async fn dispatch(&self, topic: &str, payload: Value) {
        let handlers = self.handlers.read().await;

        // 分发到具体 topic
        if let Some(callbacks) = handlers.get(topic) {
            for callback in callbacks {
                callback(payload.clone());
            }
        }

        // 分发到通配符订阅者
        if let Some(callbacks) = handlers.get("*") {
            for callback in callbacks {
                callback(payload.clone());
            }
        }
    }
}
```

### 3. 状态层（State Layer）

#### 3.1 Agent 状态机（自动驱动）

状态层的核心理念是：**UI 不应该手动管理状态，而是订阅状态变化**。

```rust
use leptos::*;

/// Agent 运行状态
#[derive(Debug, Clone, PartialEq)]
pub enum AgentPhase {
    Idle,
    Observing,           // 观察阶段
    Thinking,            // 思考阶段
    Acting,              // 行动阶段
    WaitingConfirmation, // 等待用户确认
    Error(String),
}

/// Agent 状态机（Leptos Signal 驱动）
pub struct AgentStateMachine {
    phase: RwSignal<AgentPhase>,
    current_tool: RwSignal<Option<String>>,
    thinking_content: RwSignal<String>,
    tool_calls: RwSignal<Vec<ToolCallInfo>>,
}

impl AgentStateMachine {
    /// 处理流式事件（自动更新状态）
    pub fn handle_event(&self, event: StreamEvent) {
        match event.event_type.as_str() {
            "agent.observe" => {
                self.phase.set(AgentPhase::Observing);
            }

            "agent.thinking" => {
                self.phase.set(AgentPhase::Thinking);
                if let Some(content) = event.payload.get("content") {
                    self.thinking_content.update(|c| {
                        c.push_str(content.as_str().unwrap_or(""));
                    });
                }
            }

            "tool.call_started" => {
                self.phase.set(AgentPhase::Acting);
                if let Some(tool_name) = event.payload.get("tool_name") {
                    let tool_name = tool_name.as_str().unwrap_or("").to_string();
                    self.current_tool.set(Some(tool_name.clone()));

                    // 添加到工具调用列表
                    self.tool_calls.update(|calls| {
                        calls.push(ToolCallInfo {
                            tool_name,
                            status: ToolStatus::Running,
                            started_at: event.timestamp.unwrap_or(0),
                            completed_at: None,
                        });
                    });
                }
            }

            "tool.call_completed" => {
                if let Some(tool_name) = event.payload.get("tool_name") {
                    let tool_name = tool_name.as_str().unwrap_or("");
                    self.tool_calls.update(|calls| {
                        if let Some(call) = calls.iter_mut()
                            .find(|c| c.tool_name == tool_name && c.status == ToolStatus::Running)
                        {
                            call.status = ToolStatus::Success;
                            call.completed_at = Some(event.timestamp.unwrap_or(0));
                        }
                    });
                }
                self.current_tool.set(None);
            }

            _ => {}
        }
    }

    /// 获取当前阶段（只读 Signal）
    pub fn phase(&self) -> ReadSignal<AgentPhase> {
        self.phase.read_only()
    }
}
```

#### 3.2 UI 状态管理

```rust
pub struct UiState {
    is_loading: RwSignal<bool>,
    error_message: RwSignal<Option<String>>,
    toast_message: RwSignal<Option<ToastMessage>>,
}

impl UiState {
    /// 显示加载状态
    pub fn show_loading(&self) {
        self.is_loading.set(true);
    }

    /// 显示错误
    pub fn show_error(&self, message: impl Into<String>) {
        self.error_message.set(Some(message.into()));
    }

    /// 显示 Toast
    pub fn show_toast(&self, content: impl Into<String>, level: ToastLevel) {
        self.toast_message.set(Some(ToastMessage {
            content: content.into(),
            level,
            duration_ms: 3000,
        }));
    }
}
```

#### 3.3 Dashboard 使用示例（零手动状态管理）

```rust
#[component]
pub fn AgentMonitor() -> impl IntoView {
    let agent_state = use_context::<AgentStateMachine>()
        .expect("AgentStateMachine not provided");

    // 自动响应状态变化
    let phase = agent_state.phase();
    let current_tool = agent_state.current_tool();

    view! {
        <div class="agent-monitor">
            // 阶段指示器（自动更新）
            <div class="phase-indicator">
                {move || match phase.get() {
                    AgentPhase::Idle => "待机中",
                    AgentPhase::Observing => "观察中...",
                    AgentPhase::Thinking => "思考中...",
                    AgentPhase::Acting => "执行中...",
                    AgentPhase::WaitingConfirmation => "等待确认",
                    AgentPhase::Error(ref e) => e.as_str(),
                }}
            </div>

            // 当前工具（自动显示/隐藏）
            {move || current_tool.get().map(|tool| view! {
                <div class="current-tool">
                    "正在执行: " {tool}
                </div>
            })}
        </div>
    }
}
```

### 4. 可观测性层（Observability Layer）

#### 4.1 时间轴模型

```rust
/// Agent 行为追踪节点
#[derive(Debug, Clone)]
pub struct TraceNode {
    pub id: String,
    pub timestamp: u64,
    pub node_type: TraceNodeType,
    pub duration_ms: Option<u64>,
    pub parent_id: Option<String>,
    pub children: Vec<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TraceNodeType {
    AgentRun,           // Agent 运行会话
    Observation,        // 观察阶段
    Thinking,           // 思考阶段
    ToolCall,           // 工具调用
    MemoryRetrieval,    // 记忆检索
    UserInteraction,    // 用户交互
}

/// 时间轴追踪器
pub struct TraceTimeline {
    nodes: RwSignal<VecDeque<TraceNode>>,
    max_nodes: usize,
    active_runs: RwSignal<Vec<String>>,
}

impl TraceTimeline {
    /// 开始一个 Agent 运行
    pub fn start_run(&self, run_id: String) {
        self.active_runs.update(|runs| runs.push(run_id.clone()));

        self.add_node(TraceNode {
            id: run_id.clone(),
            timestamp: current_timestamp(),
            node_type: TraceNodeType::AgentRun,
            duration_ms: None,
            parent_id: None,
            children: Vec::new(),
            metadata: serde_json::json!({"status": "running"}),
        });
    }

    /// 记录工具调用
    pub fn record_tool_call(
        &self,
        run_id: &str,
        tool_name: &str,
        params: serde_json::Value,
    ) -> String {
        let tool_id = format!("{}-tool-{}", run_id, uuid::Uuid::new_v4());

        self.add_node(TraceNode {
            id: tool_id.clone(),
            timestamp: current_timestamp(),
            node_type: TraceNodeType::ToolCall,
            duration_ms: None,
            parent_id: Some(run_id.to_string()),
            children: Vec::new(),
            metadata: serde_json::json!({
                "tool_name": tool_name,
                "params": params,
                "status": "running"
            }),
        });

        tool_id
    }

    /// 获取树状结构（用于可视化）
    pub fn get_tree(&self) -> Vec<TraceNode> {
        self.nodes.get().iter().cloned().collect()
    }
}
```

#### 4.2 指标收集器

```rust
/// 工具调用统计
#[derive(Debug, Clone)]
pub struct ToolMetrics {
    pub tool_name: String,
    pub total_calls: u64,
    pub success_count: u64,
    pub failure_count: u64,
    pub avg_duration_ms: f64,
    pub last_called: u64,
}

/// 指标收集器
pub struct MetricsCollector {
    tool_metrics: RwSignal<HashMap<String, ToolMetrics>>,
    total_runs: RwSignal<u64>,
    total_tokens: RwSignal<u64>,
}

impl MetricsCollector {
    /// 记录工具调用
    pub fn record_tool_call(
        &self,
        tool_name: &str,
        duration_ms: u64,
        success: bool,
    ) {
        self.tool_metrics.update(|metrics| {
            let entry = metrics.entry(tool_name.to_string())
                .or_insert_with(|| ToolMetrics {
                    tool_name: tool_name.to_string(),
                    total_calls: 0,
                    success_count: 0,
                    failure_count: 0,
                    avg_duration_ms: 0.0,
                    last_called: current_timestamp(),
                });

            entry.total_calls += 1;
            if success {
                entry.success_count += 1;
            } else {
                entry.failure_count += 1;
            }

            // 更新平均耗时
            entry.avg_duration_ms = (entry.avg_duration_ms * (entry.total_calls - 1) as f64
                + duration_ms as f64) / entry.total_calls as f64;

            entry.last_called = current_timestamp();
        });
    }

    /// 获取最常用的工具（Top N）
    pub fn top_tools(&self, n: usize) -> Vec<ToolMetrics> {
        let mut tools: Vec<_> = self.tool_metrics.get().values().cloned().collect();
        tools.sort_by(|a, b| b.total_calls.cmp(&a.total_calls));
        tools.truncate(n);
        tools
    }
}
```

## 实施计划

### Phase 1: 基础层（Week 1-2）
- [ ] 创建 `shared_ui_logic` crate
- [ ] 实现连接层（Connection Layer）
  - [ ] `AlephConnector` trait
  - [ ] WASM 实现
  - [ ] 原生实现
  - [ ] 重连策略
- [ ] 实现协议层（Protocol Layer）
  - [ ] RPC 客户端
  - [ ] 流式数据处理
  - [ ] 事件分发系统

### Phase 2: 状态层（Week 3）
- [ ] 实现 Agent 状态机
- [ ] 实现 UI 状态管理
- [ ] 集成 Leptos Signals
- [ ] 编写单元测试

### Phase 3: 可观测性层（Week 4）
- [ ] 实现时间轴追踪
- [ ] 实现指标收集器
- [ ] 实现日志流处理
- [ ] 编写集成测试

### Phase 4: API 层（Week 5）
- [ ] 封装 Memory API
- [ ] 封装 Plugins API
- [ ] 封装 Providers API
- [ ] 封装 Config API

### Phase 5: Dashboard POC（Week 6-7）
- [ ] 使用 Rust/UI (Leptos) 创建 Dashboard 原型
- [ ] 集成 `shared_ui_logic`
- [ ] 实现核心可视化组件
- [ ] 验证流式输出性能

## 成功标准

1. **类型安全**：所有 RPC 调用在编译期验证，零运行时类型错误
2. **跨平台**：同一套代码在 WASM 和原生环境运行
3. **性能**：流式输出延迟 < 50ms，UI 更新帧率 > 60fps
4. **可维护性**：新增 RPC 方法只需修改一处代码
5. **可观测性**：完整的 Agent 行为追踪和指标收集

## 风险与缓解

### 风险 1：WASM 与原生环境差异
- **缓解**：通过 `AlephConnector` trait 抽象，隔离平台差异
- **验证**：在两个环境中运行相同的集成测试

### 风险 2：Leptos 学习曲线
- **缓解**：先实现核心层（connection + protocol），后集成 Leptos
- **验证**：通过简单的示例验证 Signals 机制

### 风险 3：编译时间增加
- **缓解**：通过 Feature Flags 按需编译
- **验证**：测量不同 feature 组合的编译时间

## 参考资料

- [Leptos 官方文档](https://leptos.dev/)
- [Rust/UI 组件库](https://rust-ui.com/)
- [Aleph Gateway 协议](../GATEWAY.md)
- [Aleph Server-Client 架构](2026-02-06-server-client-architecture-design.md)

