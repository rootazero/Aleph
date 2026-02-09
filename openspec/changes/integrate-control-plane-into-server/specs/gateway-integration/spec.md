# Spec: Gateway Integration

**Capability**: `gateway-integration`
**Status**: Draft
**Related Change**: `integrate-control-plane-into-server`

## Overview

定义 Gateway 如何集成 ControlPlane HTTP 服务，包括路由配置、Server Functions 支持和身份验证。

---

## ADDED Requirements

### Requirement: ControlPlane Router Integration

The Gateway SHALL integrate the ControlPlane router into the main router and handle routing priority correctly.

#### Scenario: Route priority

**Given** Server 正在运行

**When** 请求到达 Gateway

**Then** 路由优先级为：
1. WebSocket (`/ws`) - 最高优先级
2. API 端点 (`/api/*`) - 中等优先级
3. ControlPlane (`/cp/*`) - 低优先级
4. 404 - 默认

**Acceptance Criteria**:
- WebSocket 连接不受影响
- API 请求正常工作
- ControlPlane 静态文件正常服务
- 未匹配的路径返回 404

#### Scenario: Start Gateway with ControlPlane

**Given** Server 配置启用 ControlPlane

**When** 调用 `Gateway::start()`

**Then**
- Gateway 创建 ControlPlane 路由器
- 将 ControlPlane 路由器合并到主路由器
- 日志输出 "ControlPlane enabled at /cp"
- Server 正常启动

**Acceptance Criteria**:
- 启动时间增加 < 100ms
- 无错误日志
- 可以访问 `/cp`

---

### Requirement: Server Functions Support

The Gateway SHALL support Leptos Server Functions, allowing ControlPlane UI to directly call Server-side functions.

#### Scenario: Call Server Function from UI

**Given** ControlPlane UI 已加载

**When** UI 调用 `get_providers()` Server Function

**Then**
- 请求发送到 `/api/get_providers`
- Gateway 路由到 Server Function 处理器
- Server Function 访问 `ConfigManager`
- 返回 JSON 响应
- UI 接收并显示数据

**Acceptance Criteria**:
- 延迟 < 10ms（本地调用）
- 无需 WebSocket
- 类型安全（编译期检查）

#### Scenario: Server Function error handling

**Given** ControlPlane UI 调用 Server Function

**When** Server Function 抛出错误

**Then**
- 返回 HTTP 500
- 响应体包含错误信息（JSON 格式）
- UI 显示友好的错误提示
- 错误被记录到日志

**Acceptance Criteria**:
- 不会崩溃
- 错误信息清晰
- 支持错误恢复

---

### Requirement: Context Injection

The Gateway SHALL provide Server Functions with access to Core components through context injection.

#### Scenario: Access ConfigManager in Server Function

**Given** Server Function 需要访问配置

**When** Server Function 调用 `use_context::<ConfigManager>()`

**Then**
- 返回 `Some(ConfigManager)`
- ConfigManager 是线程安全的（Arc）
- 可以读写配置

**Acceptance Criteria**:
- 使用 Leptos 的 `provide_context`
- 在 Gateway 启动时注入
- 所有 Server Functions 可访问

#### Scenario: Access MemorySystem in Server Function

**Given** Server Function 需要访问知识库

**When** Server Function 调用 `use_context::<MemorySystem>()`

**Then**
- 返回 `Some(MemorySystem)`
- 可以查询和修改知识库

**Acceptance Criteria**:
- 支持异步操作
- 线程安全
- 无数据竞争

---

### Requirement: Authentication Middleware

The Gateway SHALL provide authentication middleware to protect ControlPlane access.

#### Scenario: Local access without authentication

**Given** 请求来自 `127.0.0.1`

**When** 访问 `/cp`

**Then**
- 无需身份验证
- 直接返回内容
- 响应时间 < 50ms

**Acceptance Criteria**:
- 本地访问零开销
- 无需 Token
- 用户体验流畅

#### Scenario: Remote access requires authentication

**Given** 请求来自远程 IP（非 127.0.0.1）

**When** 访问 `/cp` 且未提供 Token

**Then**
- 返回 HTTP 401 Unauthorized
- 响应体包含错误信息
- 日志记录未授权访问

**Acceptance Criteria**:
- 安全防护
- 清晰的错误提示
- 审计日志

#### Scenario: Remote access with valid token

**Given** 请求来自远程 IP

**When** 访问 `/cp` 并提供有效的 `Authorization: Bearer <token>`

**Then**
- 验证 Token
- 如果有效，返回内容
- 如果无效，返回 401

**Acceptance Criteria**:
- 支持 JWT Token
- Token 过期检查
- 安全的 Token 存储

---

### Requirement: Real-time Event Broadcasting

The Gateway SHALL broadcast Core events to ControlPlane UI in real-time.

#### Scenario: Config updated event

**Given** ControlPlane UI 正在显示配置页面

**When** 配置文件被修改（通过 hot-reload）

**Then**
- Gateway 发送 `config_updated` 事件
- ControlPlane UI 接收事件
- UI 自动刷新显示
- 无需手动刷新页面

**Acceptance Criteria**:
- 使用 Server-Sent Events (SSE) 或 WebSocket
- 延迟 < 100ms
- 自动重连

#### Scenario: Memory fact added event

**Given** ControlPlane UI 正在显示知识库页面

**When** 新的 Memory Fact 被添加

**Then**
- Gateway 广播 `memory_fact_added` 事件
- UI 实时显示新的 Fact
- 无需刷新

**Acceptance Criteria**:
- 支持多种事件类型
- 事件过滤（仅订阅感兴趣的事件）
- 高效传输

---

## Implementation Notes

### Router Integration

```rust
// core/src/gateway/mod.rs
pub async fn start(&self) -> Result<()> {
    let mut router = Router::new()
        .route("/ws", get(websocket_handler))
        .nest("/api", api_router());

    #[cfg(feature = "control-plane")]
    {
        use control_plane::create_control_plane_router;
        router = router.nest("/cp", create_control_plane_router());
        tracing::info!("ControlPlane enabled at /cp");
    }

    router = router.fallback(not_found);

    // ...
}
```

### Server Function Context

```rust
// core/src/gateway/control_plane/context.rs
use leptos::*;

#[derive(Clone)]
pub struct ControlPlaneContext {
    pub config_manager: Arc<ConfigManager>,
    pub memory_system: Arc<MemorySystem>,
    pub plugin_registry: Arc<PluginRegistry>,
}

pub fn provide_control_plane_context(
    config_manager: Arc<ConfigManager>,
    memory_system: Arc<MemorySystem>,
    plugin_registry: Arc<PluginRegistry>,
) {
    provide_context(ControlPlaneContext {
        config_manager,
        memory_system,
        plugin_registry,
    });
}
```

### Authentication Middleware

```rust
// core/src/gateway/control_plane/auth.rs
use axum::{
    middleware::Next,
    http::{Request, StatusCode},
    response::Response,
};

pub async fn auth_middleware<B>(
    req: Request<B>,
    next: Next<B>,
) -> Result<Response, StatusCode> {
    // 本地访问自动信任
    if is_local_request(&req) {
        return Ok(next.run(req).await);
    }

    // 远程访问检查 Token
    let token = req.headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match token {
        Some(token) if verify_token(token).await => {
            Ok(next.run(req).await)
        }
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

fn is_local_request<B>(req: &Request<B>) -> bool {
    req.uri().host() == Some("127.0.0.1")
        || req.uri().host() == Some("localhost")
}
```

---

## Testing Strategy

### Unit Tests

```rust
#[tokio::test]
async fn test_local_access_no_auth() {
    let req = Request::builder()
        .uri("http://127.0.0.1:18789/cp")
        .body(Body::empty())
        .unwrap();

    let result = auth_middleware(req, next).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_remote_access_requires_auth() {
    let req = Request::builder()
        .uri("http://192.168.1.100:18789/cp")
        .body(Body::empty())
        .unwrap();

    let result = auth_middleware(req, next).await;
    assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
}
```

### Integration Tests

```rust
#[tokio::test]
async fn test_server_function_call() {
    let app = create_test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/get_providers")
                .header("Content-Type", "application/json")
                .body(Body::from("{}"))
                .unwrap()
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
```

---

## Performance Requirements

- **Server Function 延迟**: < 10ms
- **Event 广播延迟**: < 100ms
- **Auth 检查开销**: < 1ms
- **路由匹配时间**: < 1ms

---

## Security Considerations

- 本地访问自动信任（假设本地环境安全）
- 远程访问必须提供有效 Token
- Token 使用 JWT，包含过期时间
- 支持 Token 撤销（黑名单）
- 所有敏感操作记录审计日志

---

## Related Specs

- `control-plane-embedding`: ControlPlane 如何嵌入
- `client-simplification`: Client 如何访问 ControlPlane
