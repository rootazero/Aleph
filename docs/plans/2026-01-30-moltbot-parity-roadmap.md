# Moltbot 功能对齐路线图

> **目标**: 以 Moltbot 为参考，完成 Aleph 的核心功能闭环
> **优先级**: Gateway 加固 → Agent 增强 → 工具生态

---

## 总体进度概览

| 领域 | 当前完成度 | 目标 | 状态 |
|------|-----------|------|------|
| **Gateway 控制面** | 90% | 100% | 🔄 进行中 |
| **Block Streaming** | 0% | 100% | 📋 待开始 |
| **Auth Profiles** | 0% | 100% | 📋 待开始 |
| **Canvas (A2UI)** | 0% | 80% | 📋 待开始 |
| **Message Tools** | 30% | 80% | 📋 待开始 |
| **Webhooks** | 0% | 100% | 📋 待开始 |

---

## Phase 1: Gateway 控制面加固

**OpenSpec**: `openspec/changes/harden-gateway-control-plane/`

### 1.1 连接级认证门控

**目标**: 当 `require_auth` 启用时，强制执行 `connect` 握手

**实现要点**:
```rust
// core/src/gateway/server.rs

struct ConnectionState {
    authenticated: bool,
    permissions: Vec<String>,
    device_id: Option<String>,
}

async fn handle_ws_message(
    state: &mut ConnectionState,
    msg: JsonRpcRequest,
    config: &GatewayConfig,
) -> JsonRpcResponse {
    // 认证门控
    if config.require_auth && !state.authenticated {
        if msg.method != "connect" {
            return JsonRpcResponse::error(
                msg.id,
                ErrorCode::AuthRequired,
                "Connection requires authentication",
            );
        }
    }

    // 分发到 handler
    dispatch_to_handler(msg).await
}
```

**任务清单**:
- [ ] `GatewayServer` 添加 `ConnectionState` 追踪
- [ ] 实现 `connect` handshake handler
- [ ] 非认证请求返回 `AUTH_REQUIRED` 后关闭连接
- [ ] 单元测试: 认证流程

### 1.2 事件订阅过滤

**目标**: 支持 `events.subscribe` / `events.unsubscribe` / `events.list`

**实现要点**:
```rust
// core/src/gateway/subscription_manager.rs

struct SubscriptionManager {
    // connection_id -> 订阅的 topic patterns
    subscriptions: HashMap<ConnectionId, HashSet<String>>,
}

impl SubscriptionManager {
    fn should_deliver(&self, conn_id: &ConnectionId, event: &TopicEvent) -> bool {
        // 无订阅 = 接收全部
        let Some(patterns) = self.subscriptions.get(conn_id) else {
            return true;
        };

        // 模式匹配: "stream.*" 匹配 "stream.text_delta"
        patterns.iter().any(|p| glob_match(p, &event.topic))
    }
}
```

**任务清单**:
- [ ] 实现 `SubscriptionManager`
- [ ] 注册 `events.subscribe` / `events.unsubscribe` / `events.list` RPC
- [ ] EventBus 集成订阅过滤
- [ ] 测试: 订阅过滤行为

### 1.3 Inbound Router 启动

**目标**: Gateway 启动时自动启动 `InboundMessageRouter` 和配置的 channels

**实现要点**:
```rust
// core/src/bin/aleph_gateway.rs

async fn main() -> Result<()> {
    let config = load_config()?;

    // 创建 Gateway
    let gateway = GatewayServer::new(config.clone()).await?;

    // 启动 Inbound Router
    let inbound_router = InboundMessageRouter::new(
        gateway.execution_adapter(),
        gateway.reply_emitter(),
        config.routing.clone(),
    );

    // Auto-start channels
    if config.gateway.auto_start_channels {
        for channel_config in &config.channels {
            inbound_router.start_channel(channel_config).await?;
        }
    }

    // 运行服务
    gateway.run_with_router(inbound_router).await
}
```

**任务清单**:
- [ ] `InboundMessageRouter` 启动集成
- [ ] Channel auto-start 逻辑
- [ ] 统一 bindings: channel inbound 使用 `AgentRouter`
- [ ] 测试: inbound 路由到正确 agent

### 1.4 Agent Run Control RPCs

**目标**: 暴露 `agent.status` 和 `agent.cancel`

**实现要点**:
```rust
// core/src/gateway/handlers/agent.rs

async fn handle_agent_status(
    engine: &ExecutionEngine,
    params: AgentStatusParams,
) -> Result<AgentStatusResult> {
    let run = engine.get_run(&params.run_id)?;
    Ok(AgentStatusResult {
        state: run.state,
        started_at: run.started_at,
        elapsed_ms: run.elapsed_ms(),
        tool_calls: run.tool_call_count,
    })
}

async fn handle_agent_cancel(
    engine: &ExecutionEngine,
    params: AgentCancelParams,
) -> Result<AgentCancelResult> {
    engine.cancel_run(&params.run_id).await?;
    Ok(AgentCancelResult { cancelled: true })
}
```

**任务清单**:
- [ ] 实现 `agent.status` handler
- [ ] 实现 `agent.cancel` handler
- [ ] `ExecutionEngine` 添加 run 状态查询
- [ ] 测试: 取消正在执行的 run

---

## Phase 2: Agent 能力增强

### 2.1 Block Streaming (流式分块)

**参考**: Moltbot `pi-embedded-block-chunker.ts`

#### 2.1.1 核心分块器

**架构**:
```
Provider Stream → BlockChunker → BlockCoalescer → Channel Delivery
     ↓                ↓                ↓                ↓
  text_delta    智能分块         消息合并         渠道发送
```

**实现要点**:
```rust
// core/src/gateway/streaming/block_chunker.rs

pub struct BlockChunker {
    buffer: String,
    config: BlockChunkingConfig,
}

pub struct BlockChunkingConfig {
    pub min_chars: usize,      // 默认 800
    pub max_chars: usize,      // 默认 1200
    pub break_preference: BreakPreference,  // paragraph/newline/sentence
}

impl BlockChunker {
    pub fn append(&mut self, text: &str) {
        self.buffer.push_str(text);
    }

    pub fn drain<F>(&mut self, force: bool, mut emit: F)
    where
        F: FnMut(&str),
    {
        while self.buffer.len() >= self.config.min_chars || force {
            let break_result = self.pick_break_index(force);

            if break_result.index == 0 {
                if force && !self.buffer.is_empty() {
                    emit(&self.buffer);
                    self.buffer.clear();
                }
                return;
            }

            let (chunk, remainder) = self.split_at_break(break_result);
            emit(&chunk);
            self.buffer = remainder;
        }
    }

    fn pick_break_index(&self, force: bool) -> BreakResult {
        let fence_spans = parse_fence_spans(&self.buffer);

        // 优先级: paragraph > newline > sentence > hard cut
        if let Some(idx) = self.find_paragraph_break(&fence_spans) {
            return BreakResult::new(idx);
        }
        if let Some(idx) = self.find_newline_break(&fence_spans) {
            return BreakResult::new(idx);
        }
        if let Some(idx) = self.find_sentence_break(&fence_spans) {
            return BreakResult::new(idx);
        }

        // 硬截断 (带 fence 处理)
        if self.buffer.len() >= self.config.max_chars {
            return self.hard_break_with_fence(&fence_spans);
        }

        BreakResult::none()
    }
}
```

**任务清单**:
- [ ] 实现 `BlockChunker` 核心逻辑
- [ ] 实现 `parse_fence_spans` Markdown 解析
- [ ] 实现 fence 边界分割 (关闭旧 fence, 重开新 fence)
- [ ] 单元测试: 各种分割场景

#### 2.1.2 Markdown Fence 感知

**问题**: 代码块中间截断会破坏 Markdown 渲染

**解决方案**:
```rust
// core/src/gateway/streaming/markdown.rs

#[derive(Debug)]
pub struct FenceSpan {
    pub start: usize,
    pub end: Option<usize>,  // None = unclosed
    pub open_line: String,   // e.g., "```rust"
    pub marker: String,      // "```" or "~~~"
    pub indent: String,      // leading whitespace
}

pub fn parse_fence_spans(text: &str) -> Vec<FenceSpan> {
    let fence_re = Regex::new(r"^(\s*)(```|~~~)(\w*)\s*$").unwrap();
    // ... 解析逻辑
}

pub fn is_safe_fence_break(spans: &[FenceSpan], index: usize) -> bool {
    !spans.iter().any(|span| {
        index > span.start && span.end.map_or(true, |end| index < end)
    })
}
```

**Fence 分割示例**:
```
原始: ```rust\nfn main() {\n    很长的代码...\n}\n```

截断后:
块1: ```rust\nfn main() {\n```
块2: ```rust\n    很长的代码...\n}\n```
```

#### 2.1.3 消息合并器

**目标**: 减少高频小消息

```rust
// core/src/gateway/streaming/coalescer.rs

pub struct BlockCoalescer {
    buffer: String,
    config: CoalescingConfig,
    idle_timer: Option<tokio::time::Instant>,
}

pub struct CoalescingConfig {
    pub min_chars: usize,   // 默认 800
    pub max_chars: usize,   // 默认 1200
    pub idle_ms: u64,       // 默认 1000
    pub joiner: String,     // "\n\n"
}

impl BlockCoalescer {
    pub async fn enqueue(&mut self, text: &str) {
        if self.buffer.is_empty() {
            self.buffer = text.to_string();
        } else {
            self.buffer = format!("{}{}{}", self.buffer, self.config.joiner, text);
        }

        // 超过 max_chars → 立即刷新
        if self.buffer.len() >= self.config.max_chars {
            self.flush(true).await;
            return;
        }

        // 重置空闲计时器
        self.schedule_idle_flush();
    }

    pub async fn flush(&mut self, force: bool) {
        if !force && self.buffer.len() < self.config.min_chars {
            return;
        }

        // 发送合并后的消息
        self.emit(&self.buffer).await;
        self.buffer.clear();
    }
}
```

**任务清单**:
- [ ] 实现 `BlockCoalescer`
- [ ] 空闲超时机制 (tokio timer)
- [ ] Media 直通 (图片/视频不参与合并)
- [ ] 渠道差异化配置

#### 2.1.4 `<think>` 标签过滤

**目标**: 实时剥离推理过程

```rust
// core/src/gateway/streaming/think_filter.rs

pub struct ThinkFilter {
    in_thinking: bool,
}

impl ThinkFilter {
    pub fn filter(&mut self, text: &str) -> String {
        let re = Regex::new(r"<\s*(/?)(?:think|thinking|thought)\s*>").unwrap();

        let mut result = String::new();
        let mut last_idx = 0;

        for cap in re.captures_iter(text) {
            let match_start = cap.get(0).unwrap().start();
            let match_end = cap.get(0).unwrap().end();
            let is_close = cap.get(1).map_or(false, |m| m.as_str() == "/");

            // 非 thinking 状态: 保留文本
            if !self.in_thinking {
                result.push_str(&text[last_idx..match_start]);
            }

            self.in_thinking = !is_close;
            last_idx = match_end;
        }

        // 保留剩余文本
        if !self.in_thinking {
            result.push_str(&text[last_idx..]);
        }

        result
    }
}
```

**任务清单**:
- [ ] 实现 `ThinkFilter`
- [ ] 集成到 `ExecutionEngine` 事件流
- [ ] 测试: 跨 delta 的 think 标签

#### 2.1.5 渠道配置

```rust
// core/src/config/streaming.rs

#[derive(Deserialize)]
pub struct StreamingConfig {
    pub chunking: BlockChunkingConfig,
    pub coalescing: CoalescingConfig,

    // 渠道覆盖
    pub channel_overrides: HashMap<String, StreamingOverride>,
}

// 配置示例
// ~/.aleph/config.json
{
    "streaming": {
        "chunking": {
            "min_chars": 800,
            "max_chars": 1200,
            "break_preference": "paragraph"
        },
        "coalescing": {
            "min_chars": 800,
            "max_chars": 1200,
            "idle_ms": 1000,
            "joiner": "\n\n"
        },
        "channel_overrides": {
            "telegram": {
                "coalescing": { "min_chars": 600, "idle_ms": 500 }
            },
            "discord": {
                "chunking": { "max_chars": 800 }
            }
        }
    }
}
```

---

### 2.2 Auth Profiles (API Key 轮换)

**参考**: Moltbot `auth-profiles/`

#### 2.2.1 数据模型

```rust
// core/src/auth/profiles.rs

#[derive(Serialize, Deserialize)]
pub struct AuthProfileStore {
    pub version: u32,
    pub profiles: HashMap<String, AuthProfileCredential>,
    pub order: Option<HashMap<String, Vec<String>>>,
    pub last_good: Option<HashMap<String, String>>,
    pub usage_stats: Option<HashMap<String, ProfileUsageStats>>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuthProfileCredential {
    #[serde(rename = "api_key")]
    ApiKey {
        provider: String,
        key: String,
        email: Option<String>,
    },
    #[serde(rename = "token")]
    Token {
        provider: String,
        token: String,
        expires: Option<i64>,
        email: Option<String>,
    },
    #[serde(rename = "oauth")]
    OAuth {
        provider: String,
        access: String,
        refresh: String,
        expires: i64,
        email: Option<String>,
    },
}

#[derive(Serialize, Deserialize, Default)]
pub struct ProfileUsageStats {
    pub last_used: Option<i64>,
    pub cooldown_until: Option<i64>,
    pub disabled_until: Option<i64>,
    pub disabled_reason: Option<FailureReason>,
    pub error_count: u32,
    pub failure_counts: HashMap<FailureReason, u32>,
    pub last_failure_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureReason {
    Auth,       // 401
    Format,     // Invalid request
    RateLimit,  // 429
    Billing,    // 402/403
    Timeout,
    Unknown,
}
```

**任务清单**:
- [ ] 定义数据类型
- [ ] JSON 序列化/反序列化
- [ ] 文件存储: `~/.aleph/agent:main/auth-profiles.json`

#### 2.2.2 Profile 排序算法

```rust
// core/src/auth/order.rs

pub fn resolve_auth_profile_order(
    cfg: &Config,
    store: &AuthProfileStore,
    provider: &str,
    preferred: Option<&str>,
) -> Vec<String> {
    // 1. 收集该 provider 的所有 profile
    let mut profiles: Vec<String> = store.profiles
        .iter()
        .filter(|(_, cred)| cred.provider() == provider)
        .map(|(id, _)| id.clone())
        .collect();

    // 2. 过滤无效/过期的
    profiles.retain(|id| is_profile_valid(store, id));

    // 3. 分区: 可用 vs 冷却中
    let (available, in_cooldown): (Vec<_>, Vec<_>) = profiles
        .into_iter()
        .partition(|id| !is_in_cooldown(store, id));

    // 4. 排序可用 (round-robin: 按 last_used 升序)
    let mut available = available;
    available.sort_by_key(|id| {
        store.usage_stats
            .as_ref()
            .and_then(|s| s.get(id))
            .and_then(|s| s.last_used)
            .unwrap_or(0)
    });

    // 5. 排序冷却中 (按 cooldown_until 升序)
    let mut in_cooldown = in_cooldown;
    in_cooldown.sort_by_key(|id| {
        store.usage_stats
            .as_ref()
            .and_then(|s| s.get(id))
            .and_then(|s| s.cooldown_until)
            .unwrap_or(i64::MAX)
    });

    // 6. 合并 (优先可用)
    let mut result = available;
    result.extend(in_cooldown);

    // 7. 如果有首选, 移到最前
    if let Some(pref) = preferred {
        if let Some(pos) = result.iter().position(|id| id == pref) {
            let item = result.remove(pos);
            result.insert(0, item);
        }
    }

    result
}
```

#### 2.2.3 指数退避

```rust
// core/src/auth/cooldown.rs

pub fn calculate_cooldown_ms(error_count: u32) -> u64 {
    let normalized = error_count.max(1);
    let backoff = 60_000u64 * 5u64.pow((normalized - 1).min(3));
    backoff.min(3_600_000)  // 1 hour cap
}

// Error 1: 60s
// Error 2: 300s (5 min)
// Error 3: 1500s (25 min)
// Error 4+: 3600s (1 hour)

pub fn calculate_billing_cooldown_ms(error_count: u32, cfg: &Config) -> u64 {
    let base_hours = cfg.auth.cooldowns.billing_backoff_hours.unwrap_or(5);
    let max_hours = cfg.auth.cooldowns.billing_max_hours.unwrap_or(24);

    let backoff_hours = base_hours * 2u64.pow((error_count - 1).min(3));
    backoff_hours.min(max_hours) * 3_600_000
}

// Error 1: 5h
// Error 2: 10h
// Error 3: 20h
// Error 4+: 24h (capped)
```

#### 2.2.4 运行时集成

```rust
// core/src/agents/runner.rs

pub async fn run_agent_with_profile_rotation(
    params: AgentParams,
) -> Result<AgentResponse, FailoverError> {
    let mut auth_store = load_auth_profile_store(&params.agent_dir)?;
    let profile_order = resolve_auth_profile_order(
        &params.cfg,
        &auth_store,
        &params.provider,
        params.preferred_profile.as_deref(),
    );

    let mut profile_index = 0;

    loop {
        // 跳过冷却中的 profile
        while profile_index < profile_order.len()
            && is_in_cooldown(&auth_store, &profile_order[profile_index])
        {
            profile_index += 1;
        }

        if profile_index >= profile_order.len() {
            return Err(FailoverError::new(
                "All profiles in cooldown",
                FailureReason::RateLimit,
            ));
        }

        let profile_id = &profile_order[profile_index];
        let api_key = resolve_api_key(&auth_store, profile_id)?;

        match execute_agent_run(&params, &api_key).await {
            Ok(response) => {
                mark_profile_used(&mut auth_store, profile_id).await?;
                mark_profile_good(&mut auth_store, &params.provider, profile_id).await?;
                return Ok(response);
            }
            Err(err) if is_rotatable_error(&err) => {
                mark_profile_failure(
                    &mut auth_store,
                    profile_id,
                    classify_error(&err),
                    &params.cfg,
                ).await?;
                profile_index += 1;
            }
            Err(err) => return Err(err.into()),
        }
    }
}
```

**任务清单**:
- [ ] 实现 profile 排序算法
- [ ] 实现指数退避计算
- [ ] 实现文件锁 (fs2)
- [ ] 集成到 `ExecutionEngine`
- [ ] 与 model fallback 联动
- [ ] 测试: 轮换和退避行为

---

## Phase 3: 工具生态

### 3.1 Canvas (A2UI)

**参考**: Moltbot `canvas-host/`, `vendor/a2ui/`

#### 3.1.1 Canvas Host HTTP Server

```rust
// core/src/canvas/server.rs

pub async fn start_canvas_host(config: &CanvasConfig) -> Result<()> {
    let app = Router::new()
        // A2UI 渲染器静态资源
        .route("/__moltbot__/a2ui/*path", get(serve_a2ui_static))
        // Canvas 用户内容
        .route("/__moltbot__/canvas/*path", get(serve_canvas_content))
        // WebSocket 实时重载
        .route("/__moltbot/ws", get(ws_handler));

    let addr = SocketAddr::from(([127, 0, 0, 1], config.port));
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
}

async fn serve_a2ui_static(Path(path): Path<String>) -> impl IntoResponse {
    // 服务 vendor/a2ui/renderers/lit/ 静态文件
    // 注入 Live Reload script
}
```

#### 3.1.2 Canvas Tool (7 个 Action)

```rust
// core/src/tools/canvas.rs

#[derive(Deserialize)]
#[serde(tag = "action")]
pub enum CanvasAction {
    Present { url: Option<String>, placement: Option<Placement> },
    Hide,
    Navigate { url: String },
    Eval { javascript: String },
    Snapshot { format: SnapshotFormat, max_width: Option<u32>, quality: Option<f32> },
    A2uiPush { jsonl: String },
    A2uiReset,
}

pub async fn handle_canvas_action(
    action: CanvasAction,
    node_registry: &NodeRegistry,
    target_node: &str,
) -> Result<CanvasResult> {
    let command = match &action {
        CanvasAction::Present { .. } => "canvas.present",
        CanvasAction::Hide => "canvas.hide",
        CanvasAction::Navigate { .. } => "canvas.navigate",
        CanvasAction::Eval { .. } => "canvas.eval",
        CanvasAction::Snapshot { .. } => "canvas.snapshot",
        CanvasAction::A2uiPush { .. } => "canvas.a2ui.pushJSONL",
        CanvasAction::A2uiReset => "canvas.a2ui.reset",
    };

    // 发送到 Node
    node_registry.invoke(target_node, command, action.params()).await
}
```

#### 3.1.3 A2UI 协议

**Server → Client (Agent → UI)**:
```jsonl
{"surfaceUpdate":{"surfaceId":"main","components":[...]}}
{"beginRendering":{"surfaceId":"main","root":"root"}}
{"dataModelUpdate":{"surfaceId":"main","updates":[...]}}
```

**Client → Server (UI → Agent)**:
```json
{"userAction":{"name":"submit","surfaceId":"main","sourceComponentId":"btn-1","context":{...}}}
```

**任务清单**:
- [ ] Canvas Host HTTP 服务器
- [ ] 复用 Moltbot 的 Lit 渲染器 (vendor/a2ui/)
- [ ] Canvas Tool 实现 (7 个 action)
- [ ] A2UI JSONL 验证
- [ ] userAction 回传机制
- [ ] macOS 集成 (WKWebView)

---

### 3.2 Message Tools

**目标**: 消息回复、编辑、表情反应

```rust
// core/src/tools/message.rs

#[derive(Deserialize)]
pub struct MessageReplyParams {
    pub reply_to_id: String,
    pub text: String,
    pub channel: Option<String>,
}

#[derive(Deserialize)]
pub struct MessageEditParams {
    pub message_id: String,
    pub new_text: String,
}

#[derive(Deserialize)]
pub struct MessageReactParams {
    pub message_id: String,
    pub emoji: String,
    pub remove: bool,
}

// Channel adapter 实现
pub trait MessageOperations {
    async fn reply(&self, params: MessageReplyParams) -> Result<MessageResult>;
    async fn edit(&self, params: MessageEditParams) -> Result<MessageResult>;
    async fn react(&self, params: MessageReactParams) -> Result<()>;
}
```

**Channel 支持矩阵**:

| Channel | Reply | Edit | React |
|---------|-------|------|-------|
| Telegram | ✅ | ✅ | ✅ |
| Discord | ✅ | ✅ | ✅ |
| iMessage | ✅ | ❌ | ✅ (tapback) |
| Slack | ✅ | ✅ | ✅ |
| WhatsApp | ✅ | ❌ | ✅ |

**任务清单**:
- [ ] 定义 `MessageOperations` trait
- [ ] 各 channel adapter 实现
- [ ] Tool schema 注册
- [ ] 测试: 各 channel 回复/编辑/反应

---

### 3.3 Webhooks

**目标**: 外部 HTTP 触发器

```rust
// core/src/webhooks/mod.rs

#[derive(Deserialize)]
pub struct WebhookConfig {
    pub id: String,
    pub path: String,           // e.g., "/webhooks/github"
    pub secret: Option<String>, // HMAC 验证
    pub agent: String,          // 目标 agent
    pub session_key_template: String, // e.g., "task:webhook:{webhook_id}"
}

pub async fn handle_webhook(
    Path(path): Path<String>,
    headers: HeaderMap,
    body: Bytes,
    config: &[WebhookConfig],
    router: &AgentRouter,
) -> Result<impl IntoResponse> {
    let webhook = config.iter().find(|w| w.path == path)
        .ok_or(StatusCode::NOT_FOUND)?;

    // HMAC 验证 (如果配置了 secret)
    if let Some(secret) = &webhook.secret {
        verify_hmac(secret, &headers, &body)?;
    }

    // 构建 session key
    let session_key = render_session_key_template(&webhook.session_key_template, &webhook.id);

    // 路由到 agent
    let result = router.route_message(
        &webhook.agent,
        &session_key,
        &body,
    ).await?;

    Ok(Json(result))
}
```

**任务清单**:
- [ ] Webhook 配置 schema
- [ ] HTTP handler 注册 (Axum)
- [ ] HMAC 验证
- [ ] Session key 模板
- [ ] 路由到 agent
- [ ] 测试: GitHub/Stripe webhook 集成

---

## 实施时间线

```
          Phase 1              Phase 2                  Phase 3
       Gateway 加固        Agent 增强               工具生态
    ┌─────────────────┐ ┌─────────────────────┐ ┌─────────────────┐
    │ • Auth gate     │ │ • Block Streaming   │ │ • Canvas Host   │
    │ • Event filter  │ │   - BlockChunker    │ │ • Canvas Tool   │
    │ • Inbound start │ │   - Fence 解析      │ │ • A2UI 集成     │
    │ • Run control   │ │   - Coalescer       │ │ • Message Tools │
    └────────┬────────┘ │   - Think filter    │ │ • Webhooks      │
             │          │ • Auth Profiles     │ └────────┬────────┘
             │          │   - 数据模型         │          │
             │          │   - 轮换算法         │          │
             │          │   - 指数退避         │          │
             ▼          └──────────┬──────────┘          ▼
         Week 1-2              Week 3-5              Week 6-8
```

---

## 验收标准

### Phase 1 完成标准
- [ ] `require_auth=true` 时, 非 `connect` 首请求返回 `AUTH_REQUIRED`
- [ ] `events.subscribe` 成功过滤事件
- [ ] Gateway 启动时自动启动配置的 channels
- [ ] `agent.status` 和 `agent.cancel` 可用

### Phase 2 完成标准
- [ ] 流式输出按 paragraph/newline/sentence 分块
- [ ] 代码块中间截断时正确重开 fence
- [ ] 消息合并减少发送频率 (idleMs 生效)
- [ ] `<think>` 标签被正确过滤
- [ ] API key 429 后自动轮换到下一个
- [ ] 指数退避符合预期 (1min → 5min → 25min → 1h)

### Phase 3 完成标准
- [ ] Canvas Host 在 `:18793` 可访问
- [ ] `canvas.a2ui_push` 成功渲染 A2UI 组件
- [ ] userAction 回传触发 agent 响应
- [ ] Message reply/edit/react 在支持的 channel 工作
- [ ] Webhook 请求正确路由到 agent

---

## 风险和缓解

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| A2UI Lit 渲染器兼容性 | Canvas 无法渲染 | 直接复用 Moltbot 的 vendor/a2ui/ |
| 文件锁跨平台差异 | Auth Profiles 损坏 | 使用 fs2 crate, 充分测试 |
| Markdown fence 边缘情况 | 输出格式错误 | 参考 Moltbot 测试用例 |
| 渠道 rate limit 差异 | 配置复杂 | 提供合理默认值 + 文档 |

---

## 参考资源

- **Moltbot 源码**: `/Users/zouguojun/Workspace/moltbot/`
- **Moltbot 文档**: https://docs.molt.bot/
- **A2UI 规范**: `vendor/a2ui/specification/0.8/`
- **当前 OpenSpec**: `openspec/changes/harden-gateway-control-plane/`
