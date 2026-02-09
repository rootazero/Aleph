# Aleph UI Logic

> Aleph 端侧 SDK - 统一的 Client 与 Server 交互逻辑

## 概述

`aleph-ui-logic` 是 Aleph 的客户端 SDK，负责封装所有 Client 与 Server 交互的逻辑。它解决了"类型漂移"和"逻辑重写"的问题，实现真正的"一次构建，随处使用"。

## 核心价值

- **类型安全**：所有 RPC 调用在编译期验证，消除类型漂移
- **跨平台**：同时支持 WASM（浏览器）和原生（Tauri/CLI）
- **响应式**：基于 Leptos Signals，UI 自动更新
- **可观测**：内置 Agent 行为追踪和指标收集

## Feature Flags

```toml
[features]
default = ["leptos"]

# 基础协议层（无 UI 依赖）
core = [...]

# UI 状态层（依赖 Leptos）
leptos = [...]

# 可观测性增强（Command Center 专用）
observability = [...]

# WASM 支持
wasm = [...]

# 原生支持
native = [...]
```

## 架构

```
shared_ui_logic/
├── src/
│   ├── connection/      # 连接层（core feature）
│   │   ├── connector.rs # AlephConnector trait ✅
│   │   ├── reconnect.rs # 重连策略 ✅
│   │   ├── wasm.rs      # WASM 实现 ✅
│   │   └── native.rs    # 原生实现 ✅
│   ├── protocol/        # 协议层（core feature）
│   │   ├── rpc.rs       # JSON-RPC 封装 ✅
│   │   ├── streaming.rs # 流式数据 ✅
│   │   └── events.rs    # 事件分发 ✅
│   ├── state/           # 状态层（leptos feature）
│   │   ├── agent_state.rs # Agent 状态机（TODO）
│   │   ├── ui_state.rs    # UI 状态（TODO）
│   │   └── cache.rs       # 本地缓存（TODO）
│   ├── api/             # API 层（leptos feature）
│   │   ├── memory.rs    # Memory API ✅
│   │   ├── plugins.rs   # Plugins API（TODO）
│   │   ├── providers.rs # Providers API（TODO）
│   │   └── config.rs    # Config API ✅
│   └── observability/   # 可观测性（observability feature）
│       ├── trace.rs     # 时间轴追踪（TODO）
│       ├── metrics.rs   # 指标收集 ✅
│       └── logs.rs      # 日志流（TODO）
└── examples/
    ├── cli_client.rs    # CLI 示例（TODO）
    └── dashboard.rs     # Dashboard 示例（TODO）
```

## 当前状态

### ✅ 已完成

- [x] Crate 基础结构
- [x] Feature Flags 配置
- [x] 连接层（Connection Layer）
  - [x] AlephConnector trait 定义
  - [x] 重连策略实现（指数退避）
  - [x] 原生连接器实现（tokio-tungstenite）
  - [x] WASM 连接器实现（web-sys WebSocket）
- [x] 协议层（Protocol Layer）
  - [x] RPC 客户端（类型安全的 JSON-RPC 2.0）
  - [x] 流式数据处理（StreamHandler, StreamBuffer）
  - [x] 事件分发系统（Pub/Sub 模式）
- [x] API 层（API Layer）
  - [x] Memory API（7个方法：stats, search, delete, clear, compress, app_list）
  - [x] Config API（配置管理：behavior, search, policies, shortcuts, security）
- [x] 可观测性层（Observability Layer - 基础）
  - [x] MetricsCollector（工具调用追踪）
  - [x] TraceNode（层级追踪结构）
  - [x] ToolMetrics（工具统计）
- [x] 单元测试（23 个测试全部通过）
- [x] 文档测试（9 个测试通过）

### 🚧 进行中

暂无

### 📋 待实施

- [ ] 状态层（State Layer - 需要 Leptos 集成）
  - [ ] Agent 状态机
  - [ ] UI 状态管理
  - [ ] 本地缓存
- [ ] API 层（剩余模块）
  - [ ] Plugins API
  - [ ] Providers API
- [ ] 可观测性层（高级功能）
  - [ ] 时间轴追踪（响应式）
  - [ ] 日志流处理

## 使用示例

```rust
use aleph_ui_logic::connection::{create_connector, AlephConnector};

#[tokio::main]
async fn main() {
    // 创建连接器（自动选择平台）
    let mut connector = create_connector();

    // 连接到 Gateway
    connector.connect("ws://127.0.0.1:18789").await.unwrap();

    // 发送消息
    connector.send(json!({"type": "req"})).await.unwrap();
}
```

## 测试

```bash
# 运行所有测试
cargo test -p aleph-ui-logic --features core

# 运行特定模块测试
cargo test -p aleph-ui-logic --features core connection::reconnect

# 运行文档测试
cargo test -p aleph-ui-logic --features core --doc
```

## 构建

```bash
# 构建核心功能
cargo build -p aleph-ui-logic --features core

# 构建完整功能
cargo build -p aleph-ui-logic --features leptos,observability

# 构建 WASM 版本
cargo build -p aleph-ui-logic --target wasm32-unknown-unknown --features wasm

# 构建原生版本
cargo build -p aleph-ui-logic --features native
```

## 设计文档

详见：[shared_ui_logic 设计文档](../docs/plans/2026-02-08-shared-ui-logic-design.md)

## 许可证

MIT
