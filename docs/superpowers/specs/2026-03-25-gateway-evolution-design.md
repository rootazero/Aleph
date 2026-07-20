# Gateway Evolution — 学习 OpenClaw，超越 OpenClaw

> 对标 OpenClaw Gateway 核心能力，结合 Aleph Rust 架构优势的全面优化方案。

## 背景

OpenClaw 是当前最成熟的开源 AI 智能体框架之一，其 Gateway 网关是核心价值所在：
统一的多渠道接入、灵活的多 Provider 路由、精细的安全模型。

本设计对比分析 OpenClaw Gateway 与 Aleph 的差距，制定分层渐进的优化方案。
关键原则：**不照搬，而是充分融合 Aleph 现有架构思想和 Rust 语言优势**。

## 现状分析：已实现 vs 缺失

经过深入代码审查，Aleph 已实现约 70% 的 OpenClaw Gateway 能力：

### 已实现（无需重复建设）

| 能力 | Aleph 实现 | 文件 |
|------|-----------|------|
| 连接分层（Lane Manager） | 4 车道 Semaphore 限流（Query/Execute/Mutate/System） | `gateway/lane.rs` |
| 渠道消息去重 | InboundDedupTracker，5 分钟窗口，10k 上限 | `gateway/inbound_router/dedup.rs` |
| 状态版本追踪 | 3 域 AtomicU64（presence/health/config） | `gateway/state_version.rs` |
| Presence 追踪 | DashMap 无锁并发，heartbeat 更新 | `gateway/presence.rs` |
| 层级路由解析 | peer→guild→team→account→channel→default 优先级链 | `routing/resolve.rs` |
| DmScope + IdentityLinks | PerPeer/PerChannelPeer/Main 三种隔离策略 | `routing/session_key.rs` |
| CompositeRouter | 规则 + LLM fallback 任务分类 | `routing/composite_router.rs` |
| Auth Profile 轮换 | 多凭证、Round-robin、Cooldown 指数退避 | `providers/auth_profiles/` |
| Retry 逻辑 | 指数退避、rate limit header 感知 | `providers/retry.rs` |
| DM Policy 枚举 | Open/Allowlist/Pairing/Disabled | `gateway/inbound_router/types.rs` |
| Pairing 流程 | 8-char Base32 码，5 分钟过期 | `gateway/security/pairing.rs` |
| Owner/Guest 身份 | IdentityContext 冻结快照 + PolicyEngine | `shared/protocol/src/auth.rs` |
| WASM Capabilities | default-deny 4 种能力 | `extension/runtime/wasm/capabilities.rs` |

### Aleph 已超越 OpenClaw 的点

| 维度 | Aleph 优势 | OpenClaw 做法 |
|------|-----------|--------------|
| Auth Profile | 多凭证类型 + Round-robin + Cooldown 指数退避 + Billing 禁用 | 简单 token rotation |
| 路由解析 | 编译期类型安全纯函数 + IdentityLinks 跨渠道身份归并 | Runtime string 匹配 + WeakMap cache |
| 协议适配 | Rust trait 零成本抽象 + YAML 动态热加载 | 每 provider 硬编码 TypeScript adapter |
| 连接管理 | Semaphore 真并行 + AtomicU64 无锁版本号 | Node.js event loop + Map setTimeout |
| 安全模型 | WASM default-deny capability + 编译期不可变 IdentityContext | Runtime pattern scanning |

### 需要补齐的差距（本设计范围）

| # | 差距 | 重要性 |
|---|------|--------|
| G1 | InboundRouter 未接入新路由系统（仍用旧 AgentRouter） | 高 |
| G2 | RPC 级幂等去重缺失 | 中 |
| G3 | /btw 旁白对话缺失 | 低 |
| G4 | Presence 无连接角色分类 | 低 |
| G5 | 跨 Provider 降级链缺失 | 高 |
| G6 | OAuth Token 无自动刷新 | 中 |
| G7 | 动态模型发现缺失（Ollama 等） | 低 |
| G8 | DM Policy 无持久化 | 中 |
| G9 | Guest 审批门未接入 | 中 |
| G10 | 安全事件无实时告警 | 低 |
| G11 | Plugin 安装无完整性校验 | 低 |

---

## 分层渐进方案

### Phase 1：Gateway 核心加固

**范围**：G2 — RPC IdempotencyGuard

**交付物**：

#### 1.1 IdempotencyGuard

Lock-free RPC 幂等去重，防止客户端重连后重发导致重复执行。

```rust
// 新文件: gateway/idempotency.rs

/// Cache entry state: in-flight (awaiting) or complete (result cached).
enum CacheEntry {
    /// Request is in-flight; duplicates await this receiver.
    InFlight(watch::Receiver<Option<Value>>),
    /// Request completed; result cached until TTL expires.
    Complete(Value, Instant),
}

/// Lock-free RPC idempotency guard with TTL-based expiry.
/// Handles both completed results AND in-flight deduplication.
pub struct IdempotencyGuard {
    cache: DashMap<String, CacheEntry>,
    ttl: Duration,
}

impl IdempotencyGuard {
    pub fn new(ttl: Duration) -> Self;

    /// Try to acquire an idempotency slot.
    /// Returns:
    /// - Ok(None) → first request, caller should execute and call `complete()`
    /// - Ok(Some(value)) → cached result, return immediately
    /// - Err(Receiver) → in-flight duplicate, await the receiver for result
    pub fn try_acquire(&self, key: &str) -> Result<Option<Value>, watch::Receiver<Option<Value>>>;

    /// Mark a key as complete with its result.
    pub fn complete(&self, key: String, result: Value);

    /// Remove expired entries. Called periodically by background task.
    pub fn prune(&self) -> usize;
}
```

**集成点**：
- RPC 请求新增可选 `idempotency_key: Option<String>` 字段
- `handler.rs` 在 LaneManager acquire 之前检查 IdempotencyGuard
- 只对 Execute/Mutate lane 的方法启用，Query 跳过
- TTL：5 分钟
- 后台清理：复用 GatewayServer 已有的 `spawn_background_tasks()` 周期

**文件变更**：
- 新增 `gateway/idempotency.rs`
- 修改 `gateway/server/handler.rs` — 分发前检查
- 修改 `gateway/server/mod.rs` — GatewaySharedState 新增字段 + 后台 prune

---

### Phase 2：智能路由

**范围**：G1 + G3 + G4

**交付物**：

#### 2.1 InboundRouter 接入新路由系统

用 `resolve_route()` 替换旧的 `AgentRouter.route()` 调用。

**路由流程变更**：
```
旧: InboundMessage → AgentRouter.route(channel, peer) → workspace active_agent
新: InboundMessage → resolve_route(bindings, session_cfg, default_agent,
                      RouteInput{channel, account, peer, guild, team})
                    → ResolvedRoute{agent_id, session_key, workspace}
```

**文件变更**：
- 修改 `gateway/inbound_router/agent_resolver.rs` — 改用 resolve_route()
- 修改 `gateway/inbound_router/mod.rs` — 注入 Vec<RouteBinding> 和 SessionConfig
- 修改 `gateway/inbound_router/executor.rs` — 使用 ResolvedRoute.workspace

**向后兼容 & 迁移策略**：
- 当无 `RouteBinding` 配置时，自动 fallback 到 `workspace_manager.get_active_agent()`，确保现有部署零配置升级
- InboundRouter 构造时：如果 `Vec<RouteBinding>` 为空，agent 解析逻辑降级为旧路径（workspace active_agent）
- 所有现有渠道即使没有显式 binding 也能正常工作（走 MatchedBy::Default 路径）

**旧代码清理**：
- 删除 `AgentRouter` struct 及其 `route()`/`resolve_agent()` 方法
- 删除 `gateway/router.rs` 中的 `RoutingBinding` struct（与 routing/config.rs 的 RouteBinding 重复）
- 保留 `SessionKey` enum 和 `to_new()`/`from_new()` 转换（广泛引用，后续独立迁移）

#### 2.2 /btw 旁白对话

在对话中插入不影响上下文的旁白问答。

**实现方式**（遵循 R8 LLM 主权 + R9 工具即一切）：
- `/btw` 注册为内置斜杠命令
- 收到 `/btw <message>` 时创建 `SessionKey::Ephemeral`（已有！）
- 用 Ephemeral session 执行 agent.run（不保存历史，不污染上下文）
- 响应通过 `RunComplete` 事件的 `metadata` 字段标记 `"btw": true`
- Panel UI 根据 metadata 渲染为旁白气泡样式
- 非 Panel 渠道（Telegram/Discord）：响应文本前缀 `[btw]` 标记，无需渠道 adapter 变更
- Panel WASM 的渲染变更不在本 Phase 范围内（P2 只做后端，UI 适配独立跟进）

**文件变更**：
- 修改 `gateway/inbound_router/command_handler.rs` — 注册 /btw 命令
- 修改命令执行逻辑 — 创建 Ephemeral session + metadata 标记
- 修改 `gateway/event_emitter/types.rs` — RunComplete metadata 类型支持

**关键优势**：利用已有的 SessionKey::Ephemeral，零新增抽象。

#### 2.3 Presence 角色增强

区分连接类型，为 UI 提供设备感知信息。

```rust
// 修改 gateway/presence.rs

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum ConnectionRole {
    User,      // Panel UI, CLI
    Node,      // Mobile device bridge
    Webhook,   // External webhook
    Channel,   // Messaging channel (Telegram, Discord, etc.)
}
```

**文件变更**：
- 修改 `gateway/presence.rs` — PresenceEntry 新增 `role: ConnectionRole`
- 修改 `gateway/server/handler.rs` — 认证时根据连接来源设置 role
- 修改 `gateway/hello_snapshot.rs` — 序列化包含 role

---

### Phase 3：Provider 进化

**范围**：G5 + G6 + G7

**交付物**：

#### 3.1 FallbackChain — 跨 Provider 降级

当主 provider 整体宕机时，自动降级到备用 provider。

```rust
// 新文件: providers/fallback.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackChain {
    /// Ordered list of (provider_name, model_id) to try
    pub entries: Vec<FallbackEntry>,
    /// Which errors trigger fallback
    pub trigger_on: Vec<FallbackTrigger>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackEntry {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FallbackTrigger {
    /// Provider is completely unreachable
    Unreachable,
    /// All profiles in cooldown
    AllProfilesCooled,
    /// Specific HTTP status
    HttpStatus(u16),
}
```

**架构决策**：FallbackChain 是**编排层中间件**，不是 HttpProvider 内部逻辑。
HttpProvider 保持单一职责（HTTP 传输），不感知其他 provider 的存在。

**集成点**：
- FallbackChain 作为 wrapper 位于调用方（Thinker/ExecutionEngine）与 provider dispatch 之间
- 调用方通过 `FallbackDispatcher.process(payload)` 代替直接调用 provider
- FallbackDispatcher 内部：尝试 provider A → 失败 → 检查 trigger → 从 ProtocolRegistry 获取 provider B → 重试
- 通过 EventBus 通知 UI："已降级到 {backup_provider}"
- 配置在 agent-level config 中

```rust
// 新文件: providers/fallback.rs

pub struct FallbackDispatcher {
    chain: FallbackChain,
    registry: &'static ProtocolRegistry,
    event_bus: Arc<GatewayEventBus>,
}

impl FallbackDispatcher {
    /// Execute with fallback: try each provider in chain until success.
    pub async fn process(&self, payload: RequestPayload<'_>) -> Result<ProviderResponse>;
    pub async fn stream(&self, payload: RequestPayload<'_>) -> Result<BoxStream<ProviderDelta>>;
}
```

**文件变更**：
- 新增 `providers/fallback.rs` — FallbackDispatcher + FallbackChain + FallbackTrigger
- 修改 Thinker 层的 provider 调用路径 — 使用 FallbackDispatcher 包装
- 修改 config types — 新增 FallbackChain 配置项
- **不修改** `providers/http_provider.rs`（保持单一职责）

**不做**：跨 provider 自动模型映射（太主观），model 由用户在 fallback config 指定。

#### 3.2 OAuth Token Auto-Refresh

自动刷新即将过期的 OAuth access token。

```rust
// 新文件: providers/oauth_refresh.rs

pub struct OAuthRefresher {
    client: reqwest::Client,
    /// provider_id → token_endpoint_url
    endpoints: HashMap<String, String>,
    /// Refresh before expiry (default: 5 min)
    refresh_margin: Duration,
}

impl OAuthRefresher {
    /// Check if credential needs refresh, refresh if needed.
    pub async fn maybe_refresh(
        &self,
        cred: &OAuthCredential,
    ) -> Result<Option<OAuthCredential>>;
}
```

**集成点**：
- AuthProfileProviderRegistry 在使用 credential 之前调用 maybe_refresh
- 刷新成功更新 AuthProfileStore
- 刷新失败 mark_failure + cooldown，走下一个 profile

**预配置 endpoint**：
- Google: `https://oauth2.googleapis.com/token`
- 其他：用户在 OAuthCredential 或 provider config 中指定 `token_endpoint`

**client_secret 处理**：
- `OAuthCredential` 新增 `client_secret: Option<String>` 字段（存储在 SecretVault 中，不明文持久化）
- `token_endpoint` 字段也新增到 `OAuthCredential`
- 标准 OAuth2 refresh_token grant 使用 client_id + client_secret
- 如果 client_secret 为 None，假定为 public client (PKCE) 流，只发送 client_id
- Google Vertex AI 场景使用 service account JSON key，走独立的 JWT bearer grant 路径

**文件变更**：
- 新增 `providers/oauth_refresh.rs`
- 修改 `providers/auth_profile_registry.rs` — 使用前调用 refresh
- 修改 `providers/auth_profiles/credentials.rs` — OAuthCredential 新增 `token_endpoint` 和 `client_secret` 字段

#### 3.3 ModelDiscovery — 动态模型发现

为支持本地模型列表 API 的 provider（Ollama、LM Studio 等）提供运行时发现。

```rust
// 新增到 providers/adapter.rs

#[async_trait]
pub trait ModelDiscovery: Send + Sync {
    async fn discover_models(&self) -> Result<Vec<DiscoveredModel>>;
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredModel {
    pub id: String,
    pub display_name: Option<String>,
    pub context_window: Option<u32>,
    pub capabilities: Vec<String>,
}
```

**集成点**：
- ProtocolAdapter 新增可选 `fn as_model_discovery(&self) -> Option<&dyn ModelDiscovery>`（返回 trait object 引用，不影响 ProtocolAdapter 的 object safety，因为 `&dyn Trait` 是 Sized）
- OllamaProvider 实现 ModelDiscovery（调用 `/api/tags`）
- `models.list` RPC handler 合并静态配置 + 动态发现结果
- 缓存 5 分钟

**文件变更**：
- 修改 `providers/adapter.rs` — 新增 trait + DiscoveredModel
- 修改 Ollama 相关 provider — 实现 ModelDiscovery
- 修改 `gateway/handlers/` 中的 models.list handler — 合并发现结果

**不做**：不为 OpenAI/Anthropic 实现（模型列表由 YAML 协议定义管理）。

---

### Phase 4：安全纵深

**范围**：G8 + G9 + G10 + G11

**交付物**：

#### 4.1 DM Policy 持久化

DM 策略从内存迁移到 SQLite，支持动态修改。

```sql
-- SecurityStore schema migration
CREATE TABLE channel_policies (
    channel_id TEXT NOT NULL,
    policy_type TEXT NOT NULL,  -- 'dm' | 'group'
    policy TEXT NOT NULL,       -- 'open' | 'allowlist' | 'pairing' | 'disabled'
    allowlist TEXT,             -- JSON array of allowed sender IDs (per-channel query only, no cross-channel lookup needed)
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (channel_id, policy_type)
);
```

**文件变更**：
- 修改 `gateway/security/store.rs` — 新增表 + CRUD 方法
- 修改 `gateway/inbound_router/permission.rs` — 从 DB 读取 policy
- 新增 `gateway/handlers/` 中的 channel.setDmPolicy / channel.getDmPolicy handler
- 新增 `channel_policy` builtin tool（R9：工具即一切）

#### 4.2 GuestApprovalGate

Guest 调用敏感工具时需 Owner 审批。

```rust
// 新文件: gateway/security/guest_approval.rs

pub struct GuestApprovalGate {
    /// Tool patterns requiring approval (supports wildcards)
    approval_required: Vec<String>,
    /// Pending approval requests
    pending: DashMap<String, ApprovalRequest>,
    /// Approval timeout
    timeout: Duration,
}

pub struct ApprovalRequest {
    pub request_id: String,
    pub guest_id: String,
    pub tool_name: String,
    pub arguments_summary: String,
    pub created_at: Instant,
    pub result_tx: oneshot::Sender<ApprovalDecision>,
}

pub enum ApprovalDecision {
    Approved,
    Denied { reason: String },
}
```

**流程**：
1. Guest 请求执行工具 → PolicyEngine 检查 scope → 允许
2. GuestApprovalGate 检查 → 工具匹配审批列表 → 需要审批
3. 向 Owner 发送审批通知（EventBus → Panel/Channel）
4. Owner approve/deny → Gate 放行或拒绝
5. 超时 5 分钟自动拒绝

**默认审批列表**：`shell:*`, `file:write`, `config:*`

**配置持久化**：审批列表从 `aleph.toml` 的 `[security.guest_approval]` section 加载。默认值硬编码为上述三项，用户可覆盖。重启后从配置文件恢复，无需额外 DB 表。

**文件变更**：
- 新增 `gateway/security/guest_approval.rs`
- 修改 `executor/single_step.rs` — 执行前检查审批门
- 新增审批相关 RPC handler（approval.list / approval.decide）

#### 4.3 安全事件实时告警

安全事件通过 EventBus 实时推送。

```rust
// 新增到 gateway/security/

#[derive(Debug, Clone, Serialize)]
pub enum SecurityEventKind {
    PairingAttempt { channel: String, sender: String, success: bool },
    PermissionDenied { identity: String, tool: String, reason: String },
    GuestSessionCreated { guest_id: String, channel: String },
    BruteForceDetected { channel: String, attempts: u32, blocked_until: i64 },
    ApprovalRequested { guest_id: String, tool: String },
}
```

**暴力检测**：
- 同一 (channel, sender) 5 分钟内 pairing 失败超 5 次 → 临时禁止 30 分钟
- 计数器用 DashMap + TTL（与 IdempotencyGuard 同 pattern）

**文件变更**：
- 新增 `gateway/security/security_events.rs`
- 修改 `gateway/security/pairing.rs` — 发布 PairingAttempt 事件
- 修改 `gateway/inbound_router/permission.rs` — 发布 PermissionDenied 事件
- EventBus topic: `security.*`

#### 4.4 Plugin 完整性校验（轻量版）

安装时校验插件包完整性（注意：这是 integrity check，不是 authenticity 签名验证。如果 marketplace index 本身被篡改，hash 也会被替换。真正的签名验证需要 Ed25519 + 已知公钥，留作未来扩展）。

**实现**：
- Marketplace index 为每个插件附带 SHA-256 hash
- 安装时下载后校验：`actual_hash == expected_hash`
- 不一致则拒绝安装并告警

**文件变更**：
- 修改 `extension/marketplace/` 中的安装逻辑 — 下载后校验 hash
- Marketplace index format 新增 `sha256` 字段

---

## 清理计划

| 删除目标 | 所在文件 | 理由 |
|----------|---------|------|
| `AgentRouter` struct | `gateway/router.rs` | 被 `resolve_route()` 完全替代 |
| `AgentRouter.route()` / `resolve_agent()` | `gateway/router.rs` | 同上 |
| `RoutingBinding` struct | `gateway/router.rs` | 与 `routing/config.rs` 的 `RouteBinding` 重复 |
| `AgentRouter.from_bindings()` 转换 | `gateway/router.rs` | 不再需要旧格式转换 |
| `ChannelConfig` 中内联的 DM policy 字段 | P4 完成后 | 被 `channel_policies` DB 表替代 |

**保留**：
- `gateway/router.rs` 中的 `SessionKey` enum + `to_new()`/`from_new()` 转换（广泛引用，后续独立迁移统一）

---

## 实施顺序

```
Phase 1 ─── IdempotencyGuard ──────────────────→ 独立可测试
                │
Phase 2 ─── 路由接入 + /btw + Presence ────────→ 用户可感知
                │
Phase 3 ─── FallbackChain + OAuth + Discovery ─→ Provider 弹性
                │
Phase 4 ─── DmPolicy 持久化 + 审批门 + 告警 ───→ 安全收口
```

每个 Phase 独立可发布，依赖方向单向流动。

## 设计原则

1. **不照搬** — 不移植 OpenClaw 的 TypeScript 实现，用 Rust 类型系统和零成本抽象做更好的方案
2. **融合现有** — 充分利用已有的 LaneManager、PresenceTracker、AuthProfile 等基础设施
3. **清理旧代码** — 每个 Phase 完成后删除被替代的代码，避免屎山堆积
4. **遵守红线** — 所有改动遵守 R1-R10 架构红线，特别是 R8（LLM 主权）和 R9（工具即一切）
5. **超越 OpenClaw** — 利用 Rust 编译期安全、无锁并发、trait 抽象等优势做更优实现
