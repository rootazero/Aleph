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
│   │   ├── connector.rs # AlephConnector trait
│   │   ├── reconnect.rs # 重连策略
│   │   ├── wasm.rs      # WASM 实现（TODO）
│   │   └── native.rs    # 原生实现（TODO）
│   ├── protocol/        # 协议层（core feature）
│   │   ├── rpc.rs       # JSON-RPC 封装（TODO）
│   │   ├── streaming.rs # 流式数据（TODO）
│   │   └── events.rs    # 事件分发（TODO）
│   ├── state/           # 状态层（leptos feature）
│   │   ├── agent_state.rs # Agent 状态机（TODO）
│   │   ├── ui_state.rs    # UI 状态（TODO）
│   │   └── cache.rs       # 本地缓存（TODO）
│   ├── api/             # API 层（leptos feature）
│   │   ├── memory.rs    # Memory API（TODO）
│   │   ├── plugins.rs   # Plugins API（TODO）
│   │   ├── providers.rs # Providers API（TODO）
│   │   └── config.rs    # Config API（TODO）
│   └── observability/   # 可观测性（observability feature）
│       ├── trace.rs     # 时间轴追踪（TODO）
│       ├── metrics.rs   # 指标收集（TODO）
│       └── logs.rs      # 日志流（TODO）
└── examples/
    ├── cli_client.rs    # CLI 示例（TODO）
    └── dashboard.rs     # Dashboard 示例（TODO）
```

## 当前状态

### ✅ 已完成

- [x] Crate 基础结构
- [x] Feature Flags 配置
- [x] 连接层 trait 定义（`AlephConnector`）
- [x] 重连策略实现（指数退避）
- [x] 原生连接器实现（`connection/native.rs`）
  - [x] tokio-tungstenite WebSocket 连接
  - [x] 后台任务管理 stream
  - [x] 双向通道通信
  - [x] JSON 自动序列化/反序列化
- [x] 单元测试（5 个测试全部通过）
- [x] 文档测试（2 个测试通过）

### 🚧 进行中

- [ ] 连接层平台实现
  - [ ] WASM 实现（`connection/wasm.rs`）

### 📋 待实施

- [ ] 协议层（Protocol Layer）
  - [ ] RPC 客户端
  - [ ] 流式数据处理
  - [ ] 事件分发系统
- [ ] 状态层（State Layer）
  - [ ] Agent 状态机
  - [ ] UI 状态管理
- [ ] API 层（API Layer）
  - [ ] Memory API
  - [ ] Plugins API
  - [ ] Providers API
  - [ ] Config API
- [ ] 可观测性层（Observability Layer）
  - [ ] 时间轴追踪
  - [ ] 指标收集器
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
