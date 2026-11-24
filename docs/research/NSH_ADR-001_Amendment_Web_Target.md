# ADR-001 Amendment: Web Target Support

**Date:** 2026-11-05  
**Status:** Accepted  
**Supersedes:** None (extends ADR-001)  
**Related:** ADR-001 (Desktop UI), PROTOCOL_V1 (Transport)

## Context

原ADR-001确立了NSH Desktop的Mixed Runtime架构（Rust Native Components + TSX）。现需明确：Web不是Desktop的附属构建选项，而是Aleph产品架构的正式目标平台。

## Decision

### 1. 双目标平台架构

Aleph产品架构包含两个正式目标：

```text
Aleph Architecture
├── Aleph Server (standalone process)
├── NSH UI Codebase (single source)
│   ├── Rust Native Components
│   └── TSX Screens / Modules
├── Desktop Target
│   ├── Native Components: full support
│   ├── Transport: IPC (UDS/Named Pipe)
│   └── Host Capabilities: full desktop APIs
└── Web Target
    ├── Native Components: graceful fallback to TSX
    ├── Transport: WebSocket
    └── Host Capabilities: browser-safe subset
```

**重要原则：** 这不是"Desktop with optional Web build"，而是"一套UI代码库，两种宿主，两套Transport，统一Protocol"。

### 2. UI代码策略

**UI作为跨平台主体，Native Components作为平台增强：**

- **TSX Screens/Modules**: 跨平台共享，Desktop和Web均可运行
- **Rust Native Components**: Desktop优先实现，Web Target必须提供Fallback
- **组件API契约**: 统一接口定义，实现按平台分化

示例组件分层：

```text
Component: MessageList
├── API Contract (TypeScript interface)
├── Desktop Implementation (Rust Native)
│   └── 高性能虚拟列表、原生滚动、系统字体渲染
└── Web Fallback (TSX)
    └── 浏览器虚拟列表、Web滚动、Web字体
```

**规则：**
- 每个Rust Native Component必须声明其Web Fallback策略：
  - `fallback: "tsx"` - 有等效TSX实现
  - `fallback: "degraded"` - 降级到简化版TSX
  - `fallback: "unavailable"` - Web不可用，UI需适配
- TSX组件优先使用跨平台安全API
- 平台特定能力通过Host API Bridge抽象

### 3. Client API抽象

**统一Client API，Transport按平台分化：**

```rust
// aleph-client (统一API层)
pub trait AlephClient {
    async fn connect(&self) -> Result<Connection>;
    async fn call_rpc(&self, method: &str, params: Value) -> Result<Value>;
    async fn subscribe_events(&self, scope: EventScope) -> Result<EventStream>;
}

// Desktop实现
pub struct IpcClient {
    transport: UnixDomainSocket | NamedPipe
}

// Web实现  
pub struct WsClient {
    transport: WebSocket
}
```

**Transport要求：**
- Desktop: IPC (UDS on macOS/Linux, Named Pipe on Windows)
- Web: WebSocket over HTTPS
- 两者必须实现相同的Protocol v1消息语义
- 认证、握手、事件订阅、幂等、Snapshot/Replay语义统一

### 4. Build流程

**同一UI源码，两种产物：**

```bash
# Desktop构建
cargo build --bin aleph-nsh-desktop
# 产物包含Rust Native Components + TSX + IPC Client

# Web构建
npm run build:web
# 产物仅包含TSX + Web Fallback Components + WS Client
# 通过WASM或纯Web实现，无Rust Native Components
```

**约束：**
- 不维护两套独立UI代码库
- Native Component缺失时，构建工具自动选择Fallback
- Web构建拒绝依赖Desktop-only Host API

### 5. 协议扩展

PROTOCOL_V1必须明确支持WebSocket Transport：

**新增章节：PROTOCOL_V1 § 3.4 WebSocket Transport**

```markdown
### 3.4 WebSocket Transport (Web Target)

- URL: `wss://server-host:port/v1/ws`
- Subprotocol: `aleph.protocol.v1`
- Frame格式: 与IPC相同（Length-prefixed JSON）
- 认证: HTTP Bearer Token或Cookie，连接前由Web客户端获取
- 握手: 连接后立即调用`system.hello`，与IPC客户端相同
- 限制: 
  - WebSocket消息大小上限由Server配置
  - 浏览器跨域策略：Server必须配置CORS或同源部署
  - 二进制数据通道暂不支持，使用Base64或资源引用
```

**安全要求：**
- WebSocket必须over TLS (wss://)
- 认证Token通过HTTPS获取，不得嵌入页面
- Server配置CSP、CORS和Rate Limiting
- Web客户端受浏览器沙箱约束，不能访问任意本地文件

## Consequences

### 优点
- Web成为正式产品能力，不是技术演示
- Desktop和Web用户体验一致（共享UI逻辑）
- Server独立性更强（Desktop不是唯一客户端）
- 协作场景友好（Web链接分享、远程访问）

### 成本
- 必须为每个Native Component设计Fallback
- Transport层需维护IPC和WebSocket两套实现
- Web构建需独立测试和QA
- Server需部署HTTPS、认证和WebSocket支持

### 风险
- Native Component Fallback质量不一致
- Web性能与Desktop差距过大
- WebSocket连接稳定性（移动网络、代理）
- 浏览器兼容性（Safari、旧版Chrome）

## Implementation Notes

### Phase 1: Foundation (当前Phase 2之后)
- 补充PROTOCOL_V1 WebSocket章节
- 实现`aleph-client-ws` (WebSocket Client)
- Server增加WebSocket endpoint
- 验证握手、RPC、事件订阅

### Phase 2: UI Adaptation
- 审计现有Native Components，标记Fallback策略
- 实现核心组件的TSX Fallback
- 构建工具支持Web Target
- 验证Desktop/Web功能对等

### Phase 3: Production
- 部署HTTPS + WebSocket
- Web认证流程
- 跨浏览器QA
- 性能基准

## Verification

ADR-001 Amendment完成的标准：

- [ ] PROTOCOL_V1新增WebSocket Transport章节
- [ ] ADR-001更新"UI技术"和"Transport"部分
- [ ] `aleph-client-ws`实现并通过测试
- [ ] Server WebSocket endpoint可接受连接
- [ ] Desktop和Web客户端均可完成握手
- [ ] 至少一个核心UI Screen在Web运行
- [ ] 文档记录Native Component Fallback契约

## References

- ADR-001: Desktop UI采用Mixed Runtime
- ADR-002: Server采用Standalone独立进程
- PROTOCOL_V1: 传输、握手、RPC、事件
- Phase 2实施: NSH Desktop连接层（已完成Task 4-5）
