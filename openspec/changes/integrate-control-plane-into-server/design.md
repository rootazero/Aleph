# Design: ControlPlane Integration Architecture

**Change ID**: `integrate-control-plane-into-server`
**Date**: 2026-02-09

## Architecture Overview

### Current Architecture (Before)

```
┌─────────────────┐                    ┌─────────────────┐
│  Dashboard      │ ←── WebSocket ───→ │  Server         │
│  (Leptos WASM)  │     JSON-RPC       │  (Rust Core)    │
│  Port 8081      │                    │  Port 18789     │
└─────────────────┘                    └─────────────────┘
        ↑                                       ↑
        │                                       │
   shared_ui_logic                         gateway/
   (40% 网络代码)                           handlers/
```

**问题**：
- 两个独立进程，需要复杂的网络通信
- 版本不一致风险
- 大量的状态同步代码
- CORS 和安全配置复杂

### Target Architecture (After)

```
┌─────────────────────────────────────────────────────────────┐
│                    aleph-server (单一二进制)                  │
│                                                               │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  Gateway (Port 18789)                               │    │
│  │  ├── WebSocket (/ws)                                │    │
│  │  └── HTTP Static (/cp/*)                            │    │
│  └─────────────────────────────────────────────────────┘    │
│                          │                                    │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  ControlPlane (Embedded)                            │    │
│  │  ├── Leptos WASM (rust-embed)                       │    │
│  │  ├── Server Functions (#[server])                   │    │
│  │  └── Direct Core Access                             │    │
│  └─────────────────────────────────────────────────────┘    │
│                          │                                    │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  Core Business Logic                                │    │
│  │  ├── Agent Loop                                     │    │
│  │  ├── Memory System                                  │    │
│  │  ├── Tool Server                                    │    │
│  │  └── Config Manager                                 │    │
│  └─────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```

**优势**：
- 单一进程，零网络延迟
- 版本强一致性
- 同源策略，安全简化
- 直接内存访问，性能提升

## Component Design

### 1. Build System (`core/build.rs`)

```rust
// core/build.rs
use std::process::Command;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=ui/control_plane/src");
    println!("cargo:rerun-if-changed=ui/control_plane/Cargo.toml");

    // 编译 ControlPlane
    let control_plane_dir = Path::new("ui/control_plane");
    if control_plane_dir.exists() {
        println!("Building ControlPlane...");

        let status = Command::new("trunk")
            .args(&["build", "--release"])
            .current_dir(control_plane_dir)
            .status()
            .expect("Failed to build ControlPlane");

        if !status.success() {
            panic!("ControlPlane build failed");
        }

        println!("ControlPlane built successfully");
    }
}
```

### 2. Asset Embedding (`core/src/gateway/control_plane/assets.rs`)

```rust
// core/src/gateway/control_plane/assets.rs
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "ui/control_plane/dist/"]
#[prefix = "cp/"]
pub struct ControlPlaneAssets;

impl ControlPlaneAssets {
    pub fn get_index_html() -> Option<Vec<u8>> {
        Self::get("cp/index.html").map(|f| f.data.to_vec())
    }
}
```

### 3. HTTP Server Integration (`core/src/gateway/control_plane/server.rs`)

```rust
// core/src/gateway/control_plane/server.rs
use axum::{
    Router,
    routing::get,
    response::{Html, IntoResponse},
    http::{StatusCode, header},
};
use rust_embed::RustEmbed;

use super::assets::ControlPlaneAssets;

pub fn create_control_plane_router() -> Router {
    Router::new()
        .route("/cp", get(serve_index))
        .route("/cp/*path", get(serve_static))
}

async fn serve_index() -> impl IntoResponse {
    match ControlPlaneAssets::get_index_html() {
        Some(content) => Html(content).into_response(),
        None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

async fn serve_static(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> impl IntoResponse {
    let path = format!("cp/{}", path);

    match ControlPlaneAssets::get(&path) {
        Some(content) => {
            let mime = mime_guess::from_path(&path)
                .first_or_octet_stream();

            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.as_ref())],
                content.data,
            ).into_response()
        }
        None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}
```

### 4. Server Functions (`core/ui/control_plane/src/api/`)

```rust
// core/ui/control_plane/src/api/config.rs
use leptos::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub api_key: String,
    pub endpoint: String,
    pub enabled: bool,
}

#[server(GetProviders, "/api")]
pub async fn get_providers() -> Result<Vec<ProviderConfig>, ServerFnError> {
    // 直接访问 core 的配置管理器
    let config_manager = use_context::<ConfigManager>()
        .ok_or_else(|| ServerFnError::ServerError("Config manager not found".into()))?;

    Ok(config_manager.get_providers().await)
}

#[server(UpdateProvider, "/api")]
pub async fn update_provider(config: ProviderConfig) -> Result<(), ServerFnError> {
    let config_manager = use_context::<ConfigManager>()
        .ok_or_else(|| ServerFnError::ServerError("Config manager not found".into()))?;

    config_manager.update_provider(config).await
        .map_err(|e| ServerFnError::ServerError(e.to_string()))
}
```

### 5. Client Integration (macOS)

```swift
// clients/macos/Aleph/Sources/SettingsButton.swift
import SwiftUI

struct SettingsButton: View {
    var body: some View {
        Button(action: openControlPlane) {
            Label("设置", systemImage: "gear")
        }
    }

    private func openControlPlane() {
        if let url = URL(string: "http://127.0.0.1:18789/cp") {
            NSWorkspace.shared.open(url)
        }
    }
}
```

## Data Flow

### Configuration Update Flow

```
┌─────────────────┐
│  ControlPlane   │
│  UI Component   │
└────────┬────────┘
         │ 1. User clicks "Save"
         ↓
┌─────────────────┐
│  Server Fn      │
│  update_config  │
└────────┬────────┘
         │ 2. Direct function call (no network)
         ↓
┌─────────────────┐
│  ConfigManager  │
│  (Core)         │
└────────┬────────┘
         │ 3. Write to disk
         ↓
┌─────────────────┐
│  Hot Reload     │
│  Watcher        │
└────────┬────────┘
         │ 4. Broadcast event
         ↓
┌─────────────────┐
│  All Clients    │
│  (WebSocket)    │
└─────────────────┘
```

### Real-time Sync Flow

```
┌─────────────────┐
│  Core Event     │
│  (Memory, Tool) │
└────────┬────────┘
         │ 1. Event emitted
         ↓
┌─────────────────┐
│  Event Bus      │
│  (Gateway)      │
└────────┬────────┘
         │ 2. Broadcast
         ├──────────────────┐
         ↓                  ↓
┌─────────────────┐  ┌─────────────────┐
│  ControlPlane   │  │  Clients        │
│  (Leptos Signal)│  │  (WebSocket)    │
└─────────────────┘  └─────────────────┘
```

## Migration Strategy

### Phase 1: Infrastructure Setup
1. 创建 `core/ui/control_plane/` 目录
2. 实现 `core/build.rs` 自动化构建
3. 添加 `rust-embed` 依赖
4. 实现 `control_plane/assets.rs` 和 `control_plane/server.rs`

### Phase 2: UI Migration
1. 将 `clients/dashboard/src/` 复制到 `core/ui/control_plane/src/`
2. 转换 RPC 调用为 Server Functions
3. 移除 `shared_ui_logic` 的网络层依赖
4. 更新路由配置

### Phase 3: Client Simplification
1. 从 macOS Client 移除设置 UI 文件
2. 添加 ControlPlane 跳转按钮
3. 更新配置同步逻辑

### Phase 4: Testing & Validation
1. 端到端测试
2. 性能基准测试
3. 内存占用测试

## Security Considerations

### Same-Origin Policy
- ControlPlane 和 API 在同一 Origin (`http://127.0.0.1:18789`)
- 无需 CORS 配置
- Cookie/Session 自动共享

### Authentication
```rust
// 简化的身份验证
pub async fn auth_middleware(
    req: Request<Body>,
    next: Next<Body>,
) -> Result<Response, StatusCode> {
    // 本地访问，信任 localhost
    if req.uri().host() == Some("127.0.0.1") {
        return Ok(next.run(req).await);
    }

    // 远程访问，检查 Token
    let token = req.headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok());

    if verify_token(token).await {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}
```

## Performance Optimization

### Asset Compression
```rust
#[derive(RustEmbed)]
#[folder = "ui/control_plane/dist/"]
#[prefix = "cp/"]
#[compression = "gzip"]  // 启用 gzip 压缩
pub struct ControlPlaneAssets;
```

### Caching Strategy
```rust
async fn serve_static(...) -> impl IntoResponse {
    // ...
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime.as_ref()),
            (header::CACHE_CONTROL, "public, max-age=31536000"), // 1 year
        ],
        content.data,
    ).into_response()
}
```

### Lazy Loading
```rust
// ControlPlane UI 使用代码分割
#[component]
pub fn App() -> impl IntoView {
    view! {
        <Router>
            <Route path="/cp" view=|| view! { <Home/> }/>
            <Route path="/cp/providers" view=|| {
                // 懒加载
                Suspense(fallback=|| view! { <Loading/> })
                    .children(|| view! { <ProvidersPage/> })
            }/>
        </Router>
    }
}
```

## Rollback Plan

如果集成失败，可以快速回滚：

1. **保留原 Dashboard**：在迁移期间保留 `clients/dashboard/`
2. **Feature Flag**：使用 `control-plane-embedded` feature flag
3. **双模式运行**：支持同时运行独立 Dashboard 和嵌入式 ControlPlane

```toml
[features]
default = ["control-plane-embedded"]
control-plane-embedded = ["rust-embed"]
control-plane-standalone = []  # 回退到独立 Dashboard
```

## Monitoring & Observability

### Metrics
- ControlPlane 页面加载时间
- Server Function 调用延迟
- 内存占用变化
- 静态资源缓存命中率

### Logging
```rust
#[server(UpdateProvider, "/api")]
pub async fn update_provider(config: ProviderConfig) -> Result<(), ServerFnError> {
    tracing::info!("Updating provider: {}", config.name);

    let start = Instant::now();
    let result = config_manager.update_provider(config).await;

    tracing::info!(
        "Provider update completed in {:?}",
        start.elapsed()
    );

    result.map_err(|e| ServerFnError::ServerError(e.to_string()))
}
```

## Future Enhancements

### Multi-Node Management
```rust
// 未来支持管理多个 Aleph 节点
#[server(ListNodes, "/api")]
pub async fn list_nodes() -> Result<Vec<NodeInfo>, ServerFnError> {
    // 从配置中读取节点列表
    // 支持切换当前管理的节点
}
```

### Remote Access
```rust
// 未来支持远程访问（需要身份验证）
pub fn create_control_plane_router() -> Router {
    Router::new()
        .route("/cp", get(serve_index))
        .route("/cp/*path", get(serve_static))
        .layer(middleware::from_fn(auth_middleware))  // 添加认证
}
```
