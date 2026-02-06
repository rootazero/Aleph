# Aleph 系统架构重构计划：全量 Server-Client 转型

**文档版本**: 1.0
**创建日期**: 2026-02-06
**状态**: 设计定稿

## 1. 核心目标与架构哲学

### 1.1 战略目标

将 Aleph 从"本地一体化"架构演进为"大脑在云端，手脚在身边"的分布式架构：

- **解耦核心逻辑**：alephcore 彻底转变为独立的服务端（Gateway），客户端不再直接包含重型 AI 逻辑
- **统一客户端入口**：将分散在 `platforms/` 和 `clients/` 的代码整合，明确"前端实现"与"后端核心"的界限
- **提高开发效率**：复用现有的原生 UI 实现和平台特定代码（快捷键、剪贴板等），仅替换通信层
- **数据中心化**：所有持久化数据（会话历史、配置、记忆）保存在服务端，客户端仅作为"瘦壳"

### 1.2 架构原则

| 原则 | 实践 |
|------|------|
| **职责分离** | Server 负责智能决策，Client 负责 IO 和展示 |
| **状态无关** | 客户端不持有业务状态，可随时销毁重建 |
| **协议驱动** | 通过 WebSocket + JSON-RPC 2.0 定义交互契约 |
| **能力声明** | 客户端通过 Manifest 动态声明能力，Server 智能路由 |

---

## 2. 目录结构调整（Phasing Out platforms/）

### 2.1 现状与问题

**当前结构**：
```
aleph/
├── platforms/
│   ├── macos/          # Swift 原生客户端（已是 Thin Client）
│   └── tauri/          # Tauri 跨平台客户端（Fat Client）
├── clients/
│   └── cli/            # Rust CLI 客户端（Thin Client）
├── core/               # Rust 核心库 + Gateway
└── shared/protocol/    # 协议定义
```

**问题**：
- `platforms/` 与 `clients/` 并存，语义割裂
- `tauri` 作为 Fat Client，架构意图不明确
- 缺少客户端共享逻辑的统一抽象

### 2.2 目标结构

```
aleph/
├── clients/
│   ├── cli/            # Rust CLI 客户端
│   ├── macos/          # Swift 原生客户端
│   ├── desktop/        # Tauri 跨平台客户端
│   └── shared/         # Aleph Client SDK (Rust)
├── core/               # 服务端核心 + Gateway
└── shared/protocol/    # 协议定义
```

**优势**：
- 架构意图即时澄清："一切皆客户端"
- 为未来的 `mobile/` 或 `web/` 客户端预留空间
- `clients/shared` 成为客户端开发的核心依赖

### 2.3 迁移映射表

| 当前路径 | 目标路径 | 类型 |
|---------|---------|------|
| `platforms/macos` | `clients/macos` | 目录移动 |
| `platforms/tauri` | `clients/desktop` | 目录移动 + 重命名 |
| `clients/cli` | `clients/cli` | 保持不变 |
| (不存在) | `clients/shared` | 新建 crate |

### 2.4 Cargo Workspace 调整

**根目录 `Cargo.toml` 修改**：

```diff
 [workspace]
 resolver = "2"
-members = ["core", "shared/protocol", "clients/cli"]
-exclude = ["platforms/tauri/src-tauri"]
+members = [
+    "core",
+    "shared/protocol",
+    "clients/cli",
+    "clients/shared",
+    "clients/desktop/src-tauri",
+]
```

### 2.5 构建脚本更新

需要更新的脚本：
- `build-macos.sh` → 修改路径为 `clients/macos`
- 各客户端的 `Cargo.toml` 中的相对路径引用
- CI/CD 配置文件（如存在）

---

## 3. 分阶段实施路线

### 3.1 总体策略：先重组再瘦身

**核心原则**：每个阶段变更范围小、可独立验证、易于回滚

```
Phase 1: 目录重组        Phase 2: SDK 提取        Phase 3: Tauri 瘦身
┌──────────────┐        ┌──────────────┐        ┌──────────────┐
│ 物理移动目录  │        │ clients/shared│        │ Fat → Thin   │
│ 修复引用路径  │   →    │ CLI 重构验证  │   →    │ Clean Break  │
│ 编译验证     │        │ 集成测试     │        │ 功能验证     │
└──────────────┘        └──────────────┘        └──────────────┘
   风险：低              风险：中               风险：中
```

### 3.2 Phase 1：目录迁移与工作区对齐

**目标**：完成目录重组，所有客户端保持功能不变

#### 任务清单

| 任务 | 操作 | 验证方式 |
|------|------|---------|
| 1.1 | 移动 `platforms/macos` → `clients/macos` | `xcodegen generate && xcodebuild` |
| 1.2 | 移动 `platforms/tauri` → `clients/desktop` | `cd clients/desktop && npm run tauri build` |
| 1.3 | 更新根 `Cargo.toml` workspace members | `cargo metadata` 无错误 |
| 1.4 | 修复 `clients/desktop/src-tauri/Cargo.toml` 中的路径 | `cargo check -p aleph-desktop` |
| 1.5 | 更新构建脚本 `build-macos.sh` | 执行脚本，macOS App 正常启动 |
| 1.6 | Git 提交检查点 | `git status` 无 untracked 重要文件 |

#### 路径修复示例

**clients/desktop/src-tauri/Cargo.toml**：
```diff
 [dependencies]
-alephcore = { path = "../../../core" }
+alephcore = { path = "../../../core" }  # 路径层级不变
+aleph-protocol = { path = "../../../shared/protocol" }
```

**预期结果**：
- 所有客户端编译通过
- 功能无回退（macOS App、Tauri App、CLI 均可正常启动）
- 架构意图通过目录结构得到明确表达

---

### 3.3 Phase 2：建立 Aleph Client SDK

**目标**：从 CLI 提取共享逻辑，创建可复用的 SDK

#### 核心设计

**clients/shared/Cargo.toml**：
```toml
[package]
name = "aleph-client-sdk"
version = "0.1.0"
edition = "2021"

[features]
default = ["client", "native-tls"]

# 基础通信能力
transport = ["dep:tokio-tungstenite", "dep:futures-util"]
rpc = ["dep:serde_json", "aleph-protocol"]

# 核心客户端实例
client = ["transport", "rpc"]

# 本地工具执行
local-executor = ["dep:tokio", "tokio/process", "dep:async-trait"]

# TLS 选项（互斥）
native-tls = ["tokio-tungstenite/native-tls"]
rustls = ["tokio-tungstenite/rustls-tls-native-roots"]

# 可选日志
tracing = ["dep:tracing"]

[dependencies]
aleph-protocol = { path = "../../shared/protocol" }
tokio = { workspace = true, optional = true }
tokio-tungstenite = { version = "0.21", optional = true }
futures-util = { version = "0.3", optional = true }
serde_json = { workspace = true, optional = true }
async-trait = { workspace = true, optional = true }
tracing = { workspace = true, optional = true }
thiserror = { workspace = true }
```

#### SDK 模块结构

```
clients/shared/src/
├── lib.rs              # 公开 API
├── transport.rs        # WebSocket 连接管理
├── rpc.rs              # JSON-RPC 封装
├── auth.rs             # 认证协议 + ConfigStore trait
├── client.rs           # GatewayClient 主体
├── executor.rs         # LocalExecutor trait
└── error.rs            # 错误类型定义
```

#### 任务清单

| 任务 | 文件 | 描述 |
|------|------|------|
| 2.1 | `clients/shared/Cargo.toml` | 创建 crate 并定义 features |
| 2.2 | `clients/shared/src/transport.rs` | 提取 WebSocket 连接、心跳、重连逻辑 |
| 2.3 | `clients/shared/src/rpc.rs` | 提取 JSON-RPC 请求/响应匹配逻辑 |
| 2.4 | `clients/shared/src/auth.rs` | 实现 Managed Auth + ConfigStore trait |
| 2.5 | `clients/shared/src/client.rs` | 组装完整的 GatewayClient |
| 2.6 | `clients/shared/src/executor.rs` | 定义 LocalExecutor trait |
| 2.7 | 重构 CLI | 修改 `clients/cli` 使用新 SDK |
| 2.8 | 集成测试 | CLI 全功能验证（连接、认证、工具调用） |

#### 核心 API 设计

**GatewayClient**：
```rust
pub struct GatewayClient {
    // 内部状态（隐藏实现细节）
}

impl GatewayClient {
    /// 创建客户端实例
    pub fn new(url: &str) -> Self;

    /// 连接并认证（Managed Authentication）
    pub async fn connect_with_config(
        &self,
        config: &ClientConfig,
    ) -> Result<AuthToken>;

    /// 发送 RPC 请求
    pub async fn call(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value>;

    /// 订阅服务端事件流
    pub fn subscribe_events(&self) -> EventStream;

    /// 检查连接状态
    pub fn is_connected(&self) -> bool;

    /// 优雅关闭连接
    pub async fn close(&self) -> Result<()>;
}
```

**ConfigStore Trait**：
```rust
#[async_trait]
pub trait ConfigStore: Send + Sync {
    async fn load_token(&self) -> Result<Option<String>>;
    async fn save_token(&self, token: &str) -> Result<()>;
    async fn clear_token(&self) -> Result<()>;
    async fn get_or_create_device_id(&self) -> String;
}
```

**预期结果**：
- CLI 通过 SDK 连接 Gateway，功能完整
- SDK API 清晰、易用，适合其他客户端集成
- 集成测试覆盖核心场景（认证、重连、工具调用）

---

### 3.4 Phase 3：Tauri 客户端瘦身（Fat → Thin）

**目标**：采用 Clean Break 策略，将 Tauri 完全转变为 Thin Client

#### 核心转变

| 维度 | Fat Client（当前） | Thin Client（目标） |
|------|-------------------|-------------------|
| **依赖关系** | `alephcore` (直接代码调用) | `aleph-client-sdk` + `aleph-protocol` |
| **内存占用** | 数百 MB（包含 DB、模型） | 极轻量（仅 UI + 网络层） |
| **状态管理** | 核心状态在客户端内存 | 所有状态在 Gateway |
| **更新策略** | 核心逻辑更新需重新打包 App | 核心逻辑更新仅需更新服务端 |

#### Command Proxy 模式

**核心思想**：所有 `#[tauri::command]` 从"本地函数调用"转变为"WebSocket 消息转发"

**改造前** (`clients/desktop/src-tauri/src/core/mod.rs`):
```rust
#[tauri::command]
pub async fn process_input<R: Runtime>(
    app: AppHandle<R>,
    input: String,
    topic_id: Option<String>,
) -> Result<()> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;  // Arc<AlephCore>

    core.process(input, Some(options))?;  // 本地调用
    Ok(())
}
```

**改造后**：
```rust
#[tauri::command]
pub async fn process_input<R: Runtime>(
    app: AppHandle<R>,
    input: String,
    topic_id: Option<String>,
) -> Result<()> {
    let state = app.state::<ClientState>();
    let client = state.get_client()?;  // Arc<GatewayClient>

    // 转发到 Gateway
    client.call("agent.run", Some(json!({
        "input": input,
        "topic_id": topic_id,
    }))).await?;

    Ok(())
}
```

#### 任务清单

| 任务 | 文件 | 描述 |
|------|------|------|
| 3.1 | `clients/desktop/src-tauri/Cargo.toml` | 移除 `alephcore` 依赖，添加 `aleph-client-sdk` |
| 3.2 | `src-tauri/src/state.rs` | 创建 `ClientState`，替换 `CoreState` |
| 3.3 | `src-tauri/src/gateway_client.rs` | 实现 Tauri 专用的 `ConfigStore` |
| 3.4 | `src-tauri/src/core/mod.rs` | 重写所有 commands 为 Command Proxy 模式 |
| 3.5 | `src-tauri/src/event_handler.rs` | 适配 SDK 的事件流，转发到前端 |
| 3.6 | `src-tauri/src/main.rs` | 初始化 `GatewayClient`，建立连接 |
| 3.7 | 前端验证 | 确认 React 层无感知，UI 功能正常 |
| 3.8 | 集成测试 | 端到端测试：Tauri ↔ Gateway 完整交互 |

#### 核心文件改造示例

**clients/desktop/src-tauri/src/state.rs**（新建）：
```rust
use aleph_client_sdk::GatewayClient;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ClientState {
    client: Arc<GatewayClient>,
}

impl ClientState {
    pub fn new(client: Arc<GatewayClient>) -> Self {
        Self { client }
    }

    pub fn get_client(&self) -> Result<Arc<GatewayClient>> {
        Ok(self.client.clone())
    }
}
```

**clients/desktop/src-tauri/src/gateway_client.rs**（新建）：
```rust
use aleph_client_sdk::ConfigStore;
use tauri::AppHandle;
use async_trait::async_trait;

/// Tauri-specific config store using app_data_dir
pub struct TauriConfigStore {
    app: AppHandle,
}

#[async_trait]
impl ConfigStore for TauriConfigStore {
    async fn load_token(&self) -> Result<Option<String>> {
        let config_path = self.app.path_resolver()
            .app_data_dir()
            .unwrap()
            .join("config.json");

        // 读取 token 逻辑
        // ...
    }

    async fn save_token(&self, token: &str) -> Result<()> {
        // 保存 token 逻辑
        // ...
    }

    // ... 其他方法
}
```

**clients/desktop/src-tauri/src/main.rs**（修改）：
```rust
use aleph_client_sdk::GatewayClient;
use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let client = GatewayClient::new("ws://127.0.0.1:18789");
            let config_store = TauriConfigStore::new(app.handle());

            // 异步初始化连接
            tauri::async_runtime::spawn(async move {
                if let Err(e) = client.connect_with_config(&config_store).await {
                    eprintln!("Failed to connect: {}", e);
                }
            });

            app.manage(ClientState::new(Arc::new(client)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            process_input,
            cancel_processing,
            // ... 其他 commands
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

#### 离线处理策略

**设计决策**：不降级，只重连

- WebSocket 断开时，SDK 自动尝试重连
- 前端通过监听连接状态事件显示 UI 提示
- 不在客户端维护备用的 `alephcore` 实例

**连接状态事件**：
```rust
// SDK 发出的事件
pub enum ConnectionEvent {
    Connected,
    Disconnected { reason: String },
    Reconnecting { attempt: u32 },
    AuthenticationFailed { error: String },
}

// Tauri 转发到前端
window.emit("connection-status", event);
```

#### 预期结果

- Tauri 客户端编译体积减少 50%+
- 内存占用从 300MB+ 降至 50MB 以内
- 前端 React 代码无需修改（API 透明）
- 支持连接远程 Gateway（实现真正的 Server-Client 分离）

---

## 4. 关键技术设计

### 4.1 Managed Authentication 设计

#### 认证状态机

```
┌─────────────┐
│ Disconnected│
└──────┬──────┘
       │ connect_with_config()
       ▼
┌─────────────┐    Token 有效
│Authenticating├──────────────┐
└──────┬──────┘               │
       │ Token 无效/不存在      │
       ▼                      ▼
┌─────────────┐         ┌──────────────┐
│   Pairing   │         │ Authenticated│
└──────┬──────┘         └──────┬───────┘
       │                       │
       │ 配对成功               │ 连接断开
       └───────────────────────┤
                               ▼
                        ┌──────────────┐
                        │ Reconnecting │
                        └──────┬───────┘
                               │
                               │ 自动重连
                               └──────────┐
                                          │
                                          ▼
                                   重新认证流程
```

#### 配对流程（Pairing Flow）

```rust
// SDK 提供的配对 API
impl GatewayClient {
    /// 启动配对流程，返回配对码和等待 Future
    pub async fn start_pairing(
        &self,
        device_name: &str,
        manifest: ClientManifest,
    ) -> Result<PairingSession> {
        // 1. 向服务端请求配对
        let response = self.call("pairing.request", Some(json!({
            "device_name": device_name,
            "manifest": manifest,
        }))).await?;

        // 2. 返回配对会话
        Ok(PairingSession {
            code: response["code"].as_str().unwrap().to_string(),
            expires_in: response["expires_in"].as_u64().unwrap(),
            completion: self.wait_for_pairing(response["request_id"].as_str().unwrap()),
        })
    }
}

pub struct PairingSession {
    /// 8 字符配对码（显示给用户）
    pub code: String,

    /// 过期时间（秒）
    pub expires_in: u64,

    /// 等待配对完成的 Future
    pub completion: Pin<Box<dyn Future<Output = Result<AuthToken>>>>,
}
```

**客户端使用示例**：
```rust
// 在 Tauri 中
#[tauri::command]
async fn start_pairing(app: AppHandle) -> Result<PairingInfo> {
    let client = app.state::<ClientState>().get_client()?;
    let session = client.start_pairing("My Desktop", manifest).await?;

    // 返回配对码给前端显示
    let info = PairingInfo {
        code: session.code,
        expires_in: session.expires_in,
    };

    // 异步等待配对完成
    tauri::async_runtime::spawn(async move {
        match session.completion.await {
            Ok(token) => {
                // 保存 token
                config_store.save_token(&token).await;
                // 通知前端配对成功
                app.emit_all("pairing-success", ());
            }
            Err(e) => {
                app.emit_all("pairing-failed", e.to_string());
            }
        }
    });

    Ok(info)
}
```

#### 自动重连逻辑

```rust
// SDK 内部实现
impl GatewayClient {
    async fn reconnect_loop(&self) {
        let mut backoff = ExponentialBackoff::default();

        loop {
            if !self.is_connected() {
                info!("Attempting to reconnect...");

                // 尝试加载缓存的 token
                if let Some(token) = self.config_store.load_token().await {
                    match self.reconnect_with_token(token).await {
                        Ok(_) => {
                            info!("Reconnected successfully");
                            self.emit_event(ConnectionEvent::Connected);
                            backoff.reset();
                            continue;
                        }
                        Err(e) => {
                            warn!("Reconnect failed: {}", e);
                        }
                    }
                }

                // Token 失效，需要重新认证
                self.emit_event(ConnectionEvent::AuthenticationFailed {
                    error: "Token expired or invalid".into(),
                });
            }

            tokio::time::sleep(backoff.next()).await;
        }
    }
}
```

---

### 4.2 Feature Flags 详细配置

#### CLI 使用配置

**clients/cli/Cargo.toml**：
```toml
[dependencies]
aleph-client-sdk = {
    path = "../shared",
    features = ["client", "local-executor", "tracing", "native-tls"]
}
```

**使用场景**：
- `client`：完整的 WebSocket + RPC 能力
- `local-executor`：执行本地 Shell 工具
- `tracing`：完整的日志输出（CLI 需要详细诊断）
- `native-tls`：企业环境兼容性

#### Tauri 使用配置

**clients/desktop/src-tauri/Cargo.toml**：
```toml
[dependencies]
aleph-client-sdk = {
    path = "../../shared",
    features = ["client", "local-executor", "rustls"]
}
```

**使用场景**：
- `client`：核心通信能力
- `local-executor`：可能需要执行平台特定工具
- `rustls`：纯 Rust 实现，交叉编译友好
- 不启用 `tracing`：Tauri 有自己的日志系统

#### macOS (Swift) 对应实现

**策略**：参照 SDK 逻辑，用 Swift 实现

```swift
// clients/macos/Aleph/Sources/Gateway/GatewayClient.swift
class GatewayClient {
    private let url: URL
    private var webSocket: URLSessionWebSocketTask?
    private var authToken: String?

    func connect(with config: ClientConfig) async throws -> String {
        // 实现与 SDK 等价的认证逻辑
    }

    func call(method: String, params: [String: Any]?) async throws -> Any {
        // JSON-RPC 调用
    }
}
```

**文档规范**：在 `clients/shared/docs/protocol.md` 中详细记录认证流程和消息格式，作为 Swift 实现的权威参考。

---

### 4.3 错误处理与日志策略

#### SDK 错误类型

**clients/shared/src/error.rs**：
```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    #[error("RPC error: {0}")]
    RpcError(String),

    #[error("Pairing timeout")]
    PairingTimeout,

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("WebSocket error: {0}")]
    WebSocketError(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, ClientError>;
```

#### 日志层级设计

| 层级 | 使用场景 | 示例 |
|------|---------|------|
| **ERROR** | 连接失败、认证失败、严重错误 | `error!("Failed to connect: {}", e)` |
| **WARN** | 重连尝试、Token 即将过期 | `warn!("Connection lost, reconnecting...")` |
| **INFO** | 连接建立、认证成功、重要状态变化 | `info!("Authenticated as device {}", device_id)` |
| **DEBUG** | RPC 请求/响应详情、消息路由 | `debug!("Sending RPC: {}", method)` |
| **TRACE** | WebSocket 原始消息 | `trace!("WS recv: {}", msg)` |

#### 客户端日志集成

**CLI**：直接使用 `tracing-subscriber`
```rust
tracing_subscriber::fmt()
    .with_env_filter(EnvFilter::from_default_env())
    .init();
```

**Tauri**：集成到 Tauri 的日志系统
```rust
// 可选：关闭 SDK 内部日志，由 Tauri 统一管理
// 不启用 aleph-client-sdk 的 "tracing" feature
```

---

### 4.4 测试策略

#### 单元测试（SDK 层）

**clients/shared/src/rpc.rs**：
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_request_response_matching() {
        let rpc = RpcClient::new();
        let id = "test-123".to_string();

        // 模拟请求
        let fut = rpc.send_request(id.clone(), "test.method", None);

        // 模拟响应
        rpc.handle_response(JsonRpcResponse {
            jsonrpc: "2.0",
            id: Some(id),
            result: Some(json!({"success": true})),
            error: None,
        });

        // 验证匹配
        let result = fut.await.unwrap();
        assert_eq!(result["success"], true);
    }

    #[tokio::test]
    async fn test_request_timeout() {
        let rpc = RpcClient::new();
        let fut = rpc.send_request_with_timeout(
            "timeout-test".into(),
            "slow.method",
            None,
            Duration::from_millis(100),
        );

        assert!(fut.await.is_err());
    }
}
```

#### 集成测试（CLI 层）

**测试场景**：
1. **连接测试**：启动 Gateway，CLI 成功连接并认证
2. **配对测试**：模拟配对流程，验证 Token 保存
3. **重连测试**：人为断开连接，验证自动重连
4. **工具调用测试**：通过 CLI 触发本地工具执行
5. **长连接稳定性**：保持连接 10 分钟，验证心跳机制

**测试脚本**（`clients/cli/tests/integration_test.sh`）：
```bash
#!/bin/bash
set -e

# 1. 启动 Gateway
cargo run -p alephcore --features gateway &
GATEWAY_PID=$!
sleep 2

# 2. 运行 CLI 测试
cargo test -p aleph-cli --test integration -- --nocapture

# 3. 清理
kill $GATEWAY_PID
```

#### 端到端测试（Tauri 层）

**使用 Tauri 的测试框架**：
```rust
// clients/desktop/src-tauri/tests/e2e.rs
#[tauri::test]
async fn test_gateway_connection() {
    let app = tauri::test::mock_app();

    // 模拟连接命令
    let result = app.invoke("connect_to_gateway", json!({})).await;
    assert!(result.is_ok());

    // 验证状态
    let status: ConnectionStatus = app.invoke("get_connection_status", json!({})).await.unwrap();
    assert_eq!(status, ConnectionStatus::Connected);
}
```

---

## 5. 风险评估与应对

### 5.1 技术风险

| 风险 | 影响 | 概率 | 应对措施 |
|------|------|------|---------|
| **SDK 抽象不足** | CLI 和 Tauri 需要大量重复代码 | 中 | Phase 2 充分验证 CLI，提前暴露问题 |
| **Tauri 异步集成问题** | Tokio 运行时冲突 | 中 | 参考现有 Tauri 项目最佳实践，使用 `tauri::async_runtime` |
| **WebSocket 稳定性** | 弱网环境频繁断连 | 高 | 实现指数退避重连 + 心跳机制 |
| **认证流程复杂** | 配对失败率高 | 低 | 详细的错误提示 + 配对码有效期延长 |
| **macOS 客户端脱节** | Swift 实现与 Rust SDK 不一致 | 中 | 建立协议文档作为唯一真理源 |

### 5.2 项目风险

| 风险 | 影响 | 概率 | 应对措施 |
|------|------|------|---------|
| **重构周期过长** | 用户无新功能交付 | 中 | 分阶段发布，Phase 1 完成即可发布 beta |
| **功能回退** | Tauri 瘦身后部分功能丢失 | 低 | 完整的功能测试清单 + 用户验收 |
| **文档滞后** | 新开发者难以理解架构 | 中 | 重构过程中同步更新 ARCHITECTURE.md |

### 5.3 回滚策略

**各 Phase 独立可回滚**：

- **Phase 1**：Git 分支保护，确认编译通过再合并
- **Phase 2**：CLI 新旧版本共存，通过 feature flag 切换
- **Phase 3**：保留 Tauri 的 Fat Client 分支（`legacy/fat-client`），紧急情况可恢复

---

## 6. 实施时间线与里程碑

### 6.1 时间估算

```
┌─────────────────────────────────────────────────────────┐
│ Phase 1: 目录重组 (3-5 天)                               │
├─────────────────────────────────────────────────────────┤
│ Phase 2: SDK 提取 + CLI 重构 (10-15 天)                  │
├─────────────────────────────────────────────────────────┤
│ Phase 3: Tauri 瘦身 (8-12 天)                           │
├─────────────────────────────────────────────────────────┤
│ 文档与测试完善 (3-5 天)                                  │
└─────────────────────────────────────────────────────────┘
总计：24-37 天（约 5-8 周）
```

### 6.2 里程碑

| 里程碑 | 交付物 | 验收标准 |
|-------|--------|---------|
| **M1: 目录重组完成** | 新目录结构 + 编译通过 | 所有客户端正常启动，功能无回退 |
| **M2: SDK 发布** | `aleph-client-sdk` v0.1.0 | CLI 完全基于 SDK，集成测试通过 |
| **M3: Tauri 瘦身完成** | Thin Tauri 客户端 | 编译体积 < 60MB，内存 < 80MB，功能完整 |
| **M4: 文档完善** | 更新所有架构文档 | ARCHITECTURE.md、SDK API 文档齐全 |

### 6.3 验收标准

#### 功能验收

- [ ] CLI 客户端：连接、认证、发送消息、接收流式响应、工具调用
- [ ] Tauri 客户端：同上 + UI 交互无感知
- [ ] macOS 客户端：保持现有功能（已是 Thin Client）
- [ ] 配对流程：生成配对码、扫码确认、Token 保存
- [ ] 重连机制：断网后自动重连，Token 失效后提示重新认证

#### 性能验收

- [ ] Tauri 客户端编译体积减少 50% 以上
- [ ] Tauri 客户端内存占用 < 80MB
- [ ] WebSocket 连接稳定性：24 小时无异常断连
- [ ] RPC 调用延迟：本地 < 10ms，远程 < 100ms

#### 代码质量

- [ ] 所有 SDK 公开 API 有文档注释
- [ ] 单元测试覆盖率 > 70%
- [ ] 集成测试覆盖核心场景
- [ ] 无 Clippy warnings（`cargo clippy --all-features`）

---

## 7. 附录

### 7.1 配置文件示例

**CLI 配置** (`~/.config/aleph-cli/config.toml`)：
```toml
[gateway]
url = "ws://127.0.0.1:18789"

[device]
device_id = "cli-abc123"
device_name = "My Terminal"

[auth]
# Token 由 SDK 自动管理
# token = "..." (自动保存)

[manifest]
tool_categories = ["shell", "file_system"]
specific_tools = []
excluded_tools = ["shell:sudo"]
```

**Tauri 配置** (存储在 `app_data_dir/config.json`)：
```json
{
  "gateway": {
    "url": "ws://127.0.0.1:18789"
  },
  "device": {
    "device_id": "desktop-xyz789",
    "device_name": "My Desktop"
  },
  "auth": {
    "token": "..."
  }
}
```

### 7.2 关键依赖版本

```toml
tokio = "1.35"
tokio-tungstenite = "0.21"
serde = "1.0"
serde_json = "1.0"
async-trait = "0.1"
thiserror = "2.0"
tracing = "0.1"
```

### 7.3 参考资料

- [Server-Client 架构设计](docs/plans/2026-02-06-server-client-architecture-design.md)
- [Gateway 协议文档](docs/GATEWAY.md)
- [Aleph Protocol 规范](shared/protocol/README.md)
- Tauri 异步最佳实践：https://tauri.app/v1/guides/features/command/

---

## 8. 总结

本重构计划通过三个阶段（目录重组 → SDK 提取 → Tauri 瘦身），将 Aleph 系统从本地一体化架构演进为纯粹的 Server-Client 分布式架构。核心成果包括：

1. **统一客户端架构**：所有客户端（CLI、Tauri、macOS）统一位于 `clients/` 目录
2. **可复用 SDK**：`aleph-client-sdk` 提供 Managed Authentication、自动重连、RPC 封装等核心能力
3. **轻量化客户端**：Tauri 客户端体积和内存占用减少 50%+，成为真正的"瘦壳"
4. **数据中心化**：所有持久化数据保存在服务端，客户端无状态

通过这次重构，Aleph 将真正实现"大脑在云端，手脚在身边"的愿景，为未来的移动端、Web 端扩展奠定坚实基础。
