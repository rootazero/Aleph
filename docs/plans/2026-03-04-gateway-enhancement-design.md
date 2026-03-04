# Gateway Enhancement Design

> 参考 OpenClaw Gateway 实现，分析 Aleph 的差距，融合 Aleph 架构思想设计全面改进方案。
> 策略：增强现有架构，按架构层分切为三阶段推进。

**日期**: 2026-03-04
**状态**: Approved

---

## 背景

Aleph Gateway 已有 70+ RPC 方法、14 种社交通道、完整的安全体系。但与 OpenClaw 对比后，发现在安全韧性、协议效率、生产级能力三个维度存在系统性差距。

本设计采用"增强现有架构"策略，不推倒重建，而是按架构层分切为三阶段填充：
- **P1: 连接生命周期** — 都是连接建立/维持/断开的不同阶段
- **P2: 流控与安全** — 都是消息处理管道中的拦截层
- **P3: API 面扩展** — 都是对外 API 面的新增

---

## P1: 连接生命周期增强

### 1.1 Challenge-Response 握手

**问题**: 当前 `connect` RPC 直接携带 token，无防重放保护。

**协议流程**:

```
客户端                              服务端
  │                                   │
  ├──── WebSocket 升级 ──────────────▶│
  │                                   │
  │◀──── connect.challenge ──────────┤  {nonce: 随机32字节hex, ts: unix_ms, server_id}
  │                                   │
  │      sig = HMAC-SHA256(           │
  │        key=token,                 │
  │        msg=nonce+ts+device_id     │
  │      )                            │
  │                                   │
  ├──── connect.authorize ───────────▶│  {device_id, signature, client_info}
  │                                   │
  │      验证:                         │
  │      1. nonce 未被用过 (防重放)     │
  │      2. ts 在 ±30s 窗口内          │
  │      3. HMAC 签名正确              │
  │      4. device 已注册              │
  │                                   │
  │◀──── connect.ok ─────────────────┤  {hello snapshot}
  │                                   │
```

**实现要点**:
- 复用现有 `TokenManager` 的 HMAC-SHA256 (不引入新密码学依赖)
- `ConnectionState` 新增 `challenge_nonce: Option<String>`
- 配置 `auth.challenge_response: bool` (默认 false，渐进开启)
- Nonce 存储: 内存 `HashSet<String>` + 5min TTL 自动清理
- 兼容现有简单 token 模式

### 1.2 Hello Snapshot

**问题**: 连接成功后仅返回 auth 确认，客户端需多次 RPC 获取状态。

**Payload 定义**:

```rust
pub struct HelloSnapshot {
    pub server_id: String,
    pub uptime_ms: u64,
    pub state_version: StateVersion,
    pub presence: Vec<PresenceEntry>,
    pub limits: ConnectionLimits,
    pub capabilities: Vec<String>,        // HandlerRegistry.list_methods()
    pub active_workspace: Option<String>, // WorkspaceManager
}

pub struct ConnectionLimits {
    pub max_connections: u32,
    pub current_connections: u32,
    pub rate_limits: HashMap<String, RateLimit>,
}
```

**数据来源**: 从 `GatewayContext` 汇聚。

### 1.3 Presence 追踪

**问题**: 多设备连接时设备之间无法感知彼此。

**数据模型**:

```rust
pub struct PresenceEntry {
    pub conn_id: String,
    pub device_id: Option<String>,
    pub device_type: DeviceType,      // 复用现有 enum
    pub role: DeviceRole,             // 复用现有 enum
    pub connected_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
    pub client_info: ClientInfo,
}

pub struct ClientInfo {
    pub name: String,        // "Aleph macOS", "Aleph CLI"
    pub version: String,
    pub platform: String,
}
```

**事件**:
- `presence.joined` — 新设备连接
- `presence.left` — 设备断开
- `presence.heartbeat` — 30s 间隔更新

**实现**: 新建 `core/src/gateway/presence.rs` (~150行)，通过 `GatewayEventBus` 广播。心跳复用 WebSocket ping/pong。

### 1.4 Graceful Shutdown

**问题**: 关闭时不通知客户端，无法区分崩溃和正常重启。

**流程**:
1. 广播 `system.shutdown` 事件 (含 reason, grace_period_ms)
2. 等待 grace period (默认 5s)
3. 发送 WebSocket Close frame (1001 Going Away)
4. 清理资源

**触发**: 绑定 `tokio::signal::ctrl_c()` + SIGTERM handler。

---

## P2: 流控与安全

### 2.1 滑动窗口限流

**问题**: 有 `RATE_LIMITED` 错误码但无实现，无法防御请求洪泛。

**设计**:

```rust
// core/src/gateway/rate_limiter.rs (~200行)

pub struct RateLimiter {
    buckets: DashMap<RateLimitKey, SlidingWindow>,
    config: RateLimitConfig,
    _cleanup_handle: JoinHandle<()>,  // 60s 周期清理
}

#[derive(Hash, Eq, PartialEq)]
pub struct RateLimitKey {
    pub identity: String,         // IP 或 device_id
    pub scope: RateLimitScope,
}

pub enum RateLimitScope {
    Auth,          // 认证: 10次/60s → 锁定5min
    RpcDefault,    // 普通 RPC: 100次/60s
    RpcWrite,      // 写操作: 3次/60s
    RpcHeavy,      // 重操作: 5次/60s
    WebhookAuth,   // Webhook: 10次/60s → 锁定5min
}

pub struct SlidingWindow {
    timestamps: VecDeque<Instant>,
    lockout_until: Option<Instant>,
}

pub struct RateLimitConfig {
    pub auth: WindowConfig,
    pub rpc_default: WindowConfig,
    pub rpc_write: WindowConfig,
    pub rpc_heavy: WindowConfig,
    pub exempt_loopback: bool,      // 默认 true
    pub prune_interval_secs: u64,   // 默认 60s
}
```

**集成点**: 在 `server.rs` 消息处理中，`HandlerRegistry` 调度前检查。

**与 OpenClaw 的区别**:
- 增加 per-device 维度 (利用已有 device_id)
- 用 DashMap (无锁并发读) 替代 JavaScript Map
- Scope 分类贴合 Aleph 方法体系

### 2.2 Lane 并发控制

**问题**: 所有方法共享执行池，长时间 `agent.run` 可能阻塞轻量查询。

**设计**:

```rust
// core/src/gateway/lane.rs (~120行)

pub struct LaneManager {
    lanes: HashMap<Lane, Arc<Semaphore>>,
    method_mapping: HashMap<String, Lane>,
}

pub enum Lane {
    Query,     // health, echo, config.get, models.list — 并发 50
    Execute,   // agent.run, chat.send, poe.run — 并发 5
    Mutate,    // config.patch, memory.store — 并发 10
    System,    // plugins.install, skills.install — 并发 3
}
```

**集成**: `HandlerRegistry` 调度前 acquire permit，超时 30s 返回 `SERVICE_UNAVAILABLE`。

### 2.3 慢消费者检测

**问题**: 客户端消费慢时广播通道积压，内存增长。

**方案**: 发送超时 + 队列深度检测。

```rust
const SLOW_CONSUMER_THRESHOLD: Duration = Duration::from_secs(5);

// 在 tokio::select! 的事件发送分支:
match tokio::time::timeout(SLOW_CONSUMER_THRESHOLD, ws_sender.send(msg)).await {
    Ok(Ok(())) => { /* 正常 */ },
    Ok(Err(_)) => { break; /* 连接断开 */ },
    Err(_timeout) => {
        tracing::warn!(conn_id, "slow consumer, closing");
        break;
    }
}
```

**非关键事件**: 支持 `drop_if_slow` 标志，跳过而不断开。

### 2.4 Scoped 事件广播

**问题**: 敏感事件 (pairing, exec.approval) 可能被无权客户端收到。

**设计**:

```rust
// core/src/gateway/event_scope.rs (~80行)

pub struct EventScopeGuard {
    rules: HashMap<String, Vec<String>>,  // event_topic → required_permissions
}

// 默认规则:
// "pairing.*"        → ["admin", "pairing"]
// "poe.sign.*"       → ["admin", "poe.approver"]
// "guest.*"          → ["admin", "guest.manager"]
// "exec.approval.*"  → ["admin", "exec.approver"]
// "config.changed"   → ["admin", "config.viewer"]
```

**集成**: 在事件分发循环中，`can_receive()` 检查后再发送。

---

## P3: API 面扩展

### 3.1 OpenAI-Compatible HTTP API

**问题**: 无法与 Cursor/Continue/Cody 等 OpenAI-compatible 工具对接。

**端点**:

| 方法 | 路径 | 功能 |
|------|------|------|
| POST | `/v1/chat/completions` | Chat Completions (SSE + 同步) |
| GET | `/v1/models` | 列出可用模型 |
| GET | `/v1/models/:id` | 获取模型详情 |
| GET | `/v1/health` | 健康检查 |

**请求格式** (严格兼容 OpenAI API):

```json
{
  "model": "claude-3-opus",
  "messages": [{"role": "user", "content": "Hello"}],
  "stream": true,
  "temperature": 0.7,
  "max_tokens": 4096,
  "tools": [...]
}
```

**流式响应** (SSE):

```
data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","choices":[{"delta":{"content":"Hi"},"index":0}]}

data: [DONE]
```

**实现要点**:
- 新建 `core/src/gateway/openai_api.rs` (~350行)
- 挂载到现有 axum Router，无需新端口
- `req.model` → `ProviderFactory` 解析
- `req.tools` → 映射到 `AlephTool` trait
- Bearer token 认证中间件复用 `TokenManager`
- 流式桥接: 复用 `EventEmittingCallback` → SSE `data:` 帧

### 3.2 State Versioning

**问题**: 所有事件全量推送，多客户端场景带宽浪费。

**设计**:

```rust
// core/src/gateway/state_version.rs (~60行)

pub struct StateVersionTracker {
    presence_version: AtomicU64,
    health_version: AtomicU64,
    config_version: AtomicU64,
}

pub struct StateVersion {
    pub presence: u64,
    pub health: u64,
    pub config: u64,
}
```

**使用**:
- 每个广播事件携带 `state_version`
- `events.subscribe` 支持 `since_version` 参数
- Hello Snapshot 包含初始版本

### 3.3 Tick 心跳

**问题**: 客户端无法检测连接活性和状态漂移。

**设计**: 10s 间隔广播 `system.tick` 事件，携带 `ts`, `state_version`, `connections`, `uptime_ms`。

### 3.4 Tailscale 集成

**问题**: 远程访问需手动配置端口转发。

**设计**:

```rust
// core/src/gateway/tailscale.rs (~200行)

pub struct TailscaleIntegration {
    enabled: bool,
    socket_path: PathBuf,  // /var/run/tailscale/tailscaled.sock
}
```

**认证流程**:
1. 检测 Tailscale headers (`Tailscale-User-Login`, `Tailscale-User-Name`)
2. 通过 LocalAPI (`/localapi/v0/whois`) 验证 peer IP
3. 提取用户身份 → 映射到 Aleph 权限

**新增 AuthMode**: `Tailscale` 加入现有认证模式 enum。

**配置**: `[gateway.auth] tailscale = true`

### 3.5 多热重载模式

**问题**: 单一重载策略不适合所有场景。

**设计**:

```rust
pub enum ReloadMode {
    Off,      // 禁用
    Hot,      // 立即应用 (当前行为)
    Restart,  // 触发进程重启
    Hybrid,   // 安全变更热重载，危险变更走 restart
}
```

**Hybrid 判断**: UI/channels/skills/workspace → 热重载; auth/providers/gateway → restart。

### 3.6 Multi-Bind 模式

**设计**:

```rust
pub enum BindMode {
    Loopback,    // 127.0.0.1
    Lan,         // 0.0.0.0
    Tailnet,     // Tailscale IP
    Auto,        // 有 Tailscale → Tailnet; 否则 → Loopback
}
```

---

## 新增文件清单

| 阶段 | 文件 | 预估行数 | 职责 |
|------|------|---------|------|
| P1 | `core/src/gateway/presence.rs` | ~150 | Presence 追踪 |
| P1 | 修改 `server.rs` | +200 | Challenge-Response, Hello Snapshot, Graceful Shutdown |
| P2 | `core/src/gateway/rate_limiter.rs` | ~200 | 滑动窗口限流 |
| P2 | `core/src/gateway/lane.rs` | ~120 | Lane 并发控制 |
| P2 | `core/src/gateway/event_scope.rs` | ~80 | Scoped 事件广播 |
| P2 | 修改 `server.rs` | +100 | 慢消费者检测、限流/Lane 集成 |
| P3 | `core/src/gateway/openai_api.rs` | ~350 | OpenAI HTTP API |
| P3 | `core/src/gateway/state_version.rs` | ~60 | State Versioning |
| P3 | `core/src/gateway/tailscale.rs` | ~200 | Tailscale 集成 |
| P3 | 修改 `hot_reload.rs` | +50 | 多热重载模式 |
| P3 | 修改 `server.rs` | +80 | Tick, Bind Mode |

**总计**: 新增 ~1,160 行 + 修改 ~430 行

---

## 与 OpenClaw 的关键区别

| 维度 | OpenClaw | Aleph (本设计) |
|------|----------|---------------|
| **语言** | TypeScript/Node.js | Rust (性能+安全) |
| **并发模型** | 事件循环 + Promise | Tokio async/await + Semaphore |
| **限流存储** | JS Map | DashMap (无锁并发读) |
| **限流维度** | per-IP + per-scope | per-IP + per-device + per-scope |
| **Lane 实现** | 自定义调度器 | tokio::Semaphore (零成本抽象) |
| **认证** | Ed25519 签名 | HMAC-SHA256 (复用现有 TokenManager) |
| **HTTP API** | Express middleware | axum 原生 (零拷贝 + 编译期路由) |
| **事件广播** | JS broadcast + scope guards | EventBus + EventScopeGuard (pattern matching) |
| **慢消费者** | bufferedAmount 检测 | 发送超时 + drop_if_slow |

---

## 设计原则遵循

- **R1 (Brain-Limb Separation)**: 所有改动在 Core 内，不涉及 Desktop Bridge
- **R3 (Core Minimalism)**: 无新重依赖，DashMap 是唯一新 crate
- **R4 (I/O-Only Interfaces)**: OpenAI HTTP API 是纯 I/O 转换层
- **P1 (Low Coupling)**: 限流/Lane/Scope 都是独立模块，通过 trait 组合
- **P3 (Extensibility)**: Scope 规则、Lane 配置、Bind 模式均可扩展
- **P6 (Simplicity)**: 每个模块 60-350 行，功能单一
- **P7 (Defensive Design)**: 限流、慢消费者检测、Challenge-Response 都是防御性机制
