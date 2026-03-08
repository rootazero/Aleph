# A2A Protocol Design

> **Date**: 2026-03-08
> **Status**: Approved
> **Scope**: Full A2A v0.3 bidirectional support (Server + Client)

## Overview

Add built-in A2A (Agent-to-Agent) protocol support to Aleph, enabling:
1. **Server**: External agents call Aleph's capabilities via standard A2A protocol
2. **Client**: Aleph delegates tasks to remote A2A agents via SubAgent + built-in tools
3. **Smart Routing**: LLM-powered intent-based agent matching from natural language

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Role | Bidirectional (Server + Client) | Full interoperability |
| Implementation | Self-implemented (no a2a-rs dependency) | Maximum flexibility, avoid framework-over-framework |
| Integration | Hybrid: Server as axum routes, Client as SubAgent | A2A HTTP semantics ≠ Gateway WebSocket; SubAgent fits delegation |
| Spec scope | A2A v0.3 complete | Streaming essential for LLM workloads |
| Discovery | Static config + dynamic + LLM smart routing | Progressive capability |
| Security | Tiered trust (Local/Trusted/Public) | Defense in depth, frictionless local dev |
| Architecture | DDD Bounded Context | Aligns with Dispatcher/Memory/Intent pattern |

## Architecture

### Bounded Context: `core/src/a2a/`

A2A is an independent bounded context alongside Dispatcher, Memory, Intent, and POE.

```
core/src/a2a/
├── mod.rs
├── domain/                      # Pure types, zero I/O
│   ├── agent_card.rs            # AgentCard, AgentSkill, AgentInterface, AgentProvider
│   ├── task.rs                  # A2ATask (AggregateRoot), TaskState, TaskStatus
│   ├── message.rs               # A2AMessage, Part, Artifact, FileContent
│   ├── events.rs                # TaskStatusUpdateEvent, TaskArtifactUpdateEvent, UpdateEvent
│   ├── security.rs              # SecurityScheme, TrustLevel, Credentials
│   └── error.rs                 # A2AError (thiserror, JSON-RPC error codes)
├── port/                        # Capability contracts (async traits)
│   ├── task_manager.rs          # A2ATaskManager
│   ├── message_handler.rs       # A2AMessageHandler
│   ├── streaming.rs             # A2AStreamingHandler
│   ├── agent_resolver.rs        # AgentResolver, RegisteredAgent
│   └── authenticator.rs         # A2AAuthenticator, A2AAuthPrincipal
├── adapter/                     # I/O implementations
│   ├── server/
│   │   ├── routes.rs            # axum route definitions (3 endpoints)
│   │   ├── request_processor.rs # JSON-RPC method dispatch
│   │   ├── bridge.rs            # AgentLoopBridge (A2AMessageHandler impl)
│   │   ├── task_store.rs        # A2ATaskManager impl (SQLite/in-memory)
│   │   └── stream_hub.rs        # A2AStreamingHandler impl (broadcast)
│   ├── client/
│   │   ├── http_client.rs       # A2AClient (reqwest)
│   │   ├── sse_stream.rs        # SSE event stream parser
│   │   └── pool.rs              # A2AClientPool (connection pool + health check)
│   └── auth/
│       ├── tiered.rs            # TieredAuthenticator
│       ├── token_store.rs       # In-memory/file token management
│       └── oauth2.rs            # OAuth2 verification (optional)
├── service/
│   ├── card_registry.rs         # CardRegistry (discovery + cache + health)
│   ├── card_builder.rs          # Self AgentCard auto-generation
│   ├── smart_router.rs          # SmartRouter (exact + LLM semantic)
│   └── notification.rs          # NotificationService (push webhook)
└── sub_agent.rs                 # A2ASubAgent (SubAgent trait impl)

core/src/builtin_tools/
├── a2a_discover.rs              # a2a_discover tool
├── a2a_list_agents.rs           # a2a_list_agents tool
├── a2a_send_message.rs          # a2a_send_message tool
├── a2a_get_task.rs              # a2a_get_task tool
└── a2a_cancel_task.rs           # a2a_cancel_task tool
```

### Dependency Flow

```
aleph.toml [a2a] config
        │
        ▼
   A2A Module (core/src/a2a)
        │
   ┌────┼────────────┐
   │    │             │
   ▼    ▼             ▼
Server  Service    Client
   │    │             │
   │    ▼             │
   │  port/ traits    │
   │    │             │
   │    ▼             │
   │  domain/ types   │
   │                  │
   ▼                  ▼
Gateway (merge)   A2ASubAgent → SubAgentDispatcher [Skill, MCP, A2A]
```

---

## Domain Model

All types implement `Serialize + Deserialize + Clone + Debug`.

### Agent Card (Service Discovery)

```rust
pub struct AgentCard {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub provider: Option<AgentProvider>,
    pub documentation_url: Option<String>,
    pub interfaces: Vec<AgentInterface>,
    pub skills: Vec<AgentSkill>,
    pub security: Vec<SecurityScheme>,
    pub extensions: Vec<AgentExtension>,
    pub default_input_modes: Vec<String>,    // "text", "file", "data"
    pub default_output_modes: Vec<String>,
}

pub struct AgentSkill {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub examples: Option<Vec<String>>,       // Key for LLM routing
    pub input_types: Option<Vec<String>>,
    pub output_types: Option<Vec<String>>,
}

pub struct AgentInterface {
    pub url: String,
    pub protocol: TransportProtocol,         // JsonRpc, Grpc, HttpJson
}
```

### Task Lifecycle

```rust
/// Aggregate Root — A2A task with full lifecycle
pub struct A2ATask {
    pub id: String,
    pub context_id: String,
    pub status: TaskStatus,
    pub artifacts: Vec<Artifact>,
    pub history: Vec<A2AMessage>,
    pub metadata: Option<Map<String, Value>>,
}

pub struct TaskStatus {
    pub state: TaskState,
    pub message: Option<A2AMessage>,
    pub timestamp: DateTime<Utc>,
}

pub enum TaskState {
    Submitted,
    Working,
    InputRequired,
    Completed,
    Canceled,
    Failed,
    Rejected,
    AuthRequired,
}
```

### Message

```rust
pub struct A2AMessage {
    pub message_id: String,
    pub role: A2ARole,                       // User | Agent
    pub parts: Vec<Part>,
    pub session_id: Option<String>,
    pub timestamp: Option<DateTime<Utc>>,
    pub metadata: Option<Map<String, Value>>,
}

pub enum Part {
    Text { text: String, metadata: Option<Map<String, Value>> },
    File { file: FileContent, metadata: Option<Map<String, Value>> },
    Data { data: Map<String, Value>, metadata: Option<Map<String, Value>> },
}

pub struct Artifact {
    pub artifact_id: String,
    pub kind: String,
    pub parts: Vec<Part>,
    pub metadata: Option<Map<String, Value>>,
}
```

### Events (Streaming)

```rust
pub struct TaskStatusUpdateEvent {
    pub task_id: String,
    pub context_id: String,
    pub status: TaskStatus,
    pub is_final: bool,
}

pub struct TaskArtifactUpdateEvent {
    pub task_id: String,
    pub context_id: String,
    pub artifact: Artifact,
    pub append: bool,
    pub last_chunk: bool,
}

pub enum UpdateEvent {
    Status(TaskStatusUpdateEvent),
    Artifact(TaskArtifactUpdateEvent),
}
```

### Security

```rust
pub enum SecurityScheme {
    ApiKey { location: ApiKeyLocation, name: String },
    Http { scheme: String, bearer_format: Option<String> },
    OAuth2 { flows: OAuth2Flows },
    OpenIdConnect { connect_url: String },
}

pub enum TrustLevel {
    Local,      // localhost — no auth required
    Trusted,    // LAN / paired — token required
    Public,     // Internet — OAuth2/mTLS required
}
```

---

## Port Traits

Five independent async traits defining capability contracts.

### A2ATaskManager

```rust
#[async_trait]
pub trait A2ATaskManager: Send + Sync {
    async fn create_task(&self, task_id: &str, context_id: &str) -> Result<A2ATask>;
    async fn get_task(&self, task_id: &str, history_length: Option<usize>) -> Result<A2ATask>;
    async fn update_status(&self, task_id: &str, state: TaskState, message: Option<A2AMessage>) -> Result<A2ATask>;
    async fn cancel_task(&self, task_id: &str) -> Result<A2ATask>;
    async fn list_tasks(&self, params: ListTasksParams) -> Result<ListTasksResult>;
    async fn add_artifact(&self, task_id: &str, artifact: Artifact) -> Result<()>;
}
```

### A2AMessageHandler

```rust
/// Core bridge between A2A protocol and Aleph Agent Loop
#[async_trait]
pub trait A2AMessageHandler: Send + Sync {
    /// Synchronous: receive message, wait for agent completion, return final task
    async fn handle_message(&self, task_id: &str, message: A2AMessage, session_id: Option<&str>) -> Result<A2ATask>;

    /// Streaming: receive message, return event stream
    async fn handle_message_stream(&self, task_id: &str, message: A2AMessage, session_id: Option<&str>)
        -> Result<Pin<Box<dyn Stream<Item = Result<UpdateEvent>> + Send>>>;
}
```

### A2AStreamingHandler

```rust
#[async_trait]
pub trait A2AStreamingHandler: Send + Sync {
    async fn subscribe_status(&self, task_id: &str)
        -> Result<Pin<Box<dyn Stream<Item = Result<TaskStatusUpdateEvent>> + Send>>>;
    async fn subscribe_artifacts(&self, task_id: &str)
        -> Result<Pin<Box<dyn Stream<Item = Result<TaskArtifactUpdateEvent>> + Send>>>;
    async fn subscribe_all(&self, task_id: &str)
        -> Result<Pin<Box<dyn Stream<Item = Result<UpdateEvent>> + Send>>>;
    async fn broadcast_status(&self, task_id: &str, update: TaskStatusUpdateEvent) -> Result<()>;
    async fn broadcast_artifact(&self, task_id: &str, update: TaskArtifactUpdateEvent) -> Result<()>;
}
```

### AgentResolver

```rust
#[async_trait]
pub trait AgentResolver: Send + Sync {
    async fn fetch_card(&self, url: &str) -> Result<AgentCard>;
    async fn register(&self, card: AgentCard, trust_level: TrustLevel) -> Result<()>;
    async fn unregister(&self, agent_id: &str) -> Result<()>;
    async fn list_agents(&self) -> Result<Vec<RegisteredAgent>>;
    async fn resolve_by_id(&self, agent_id: &str) -> Result<Option<RegisteredAgent>>;
    /// LLM smart routing: match best agent from natural language description
    async fn resolve_by_intent(&self, intent: &str) -> Result<Option<RegisteredAgent>>;
}

pub struct RegisteredAgent {
    pub card: AgentCard,
    pub trust_level: TrustLevel,
    pub base_url: String,
    pub last_seen: DateTime<Utc>,
    pub health: AgentHealth,        // Healthy, Degraded, Unreachable
}
```

### A2AAuthenticator

```rust
#[async_trait]
pub trait A2AAuthenticator: Send + Sync {
    async fn authenticate(&self, context: &A2AAuthContext) -> Result<A2AAuthPrincipal>;
    async fn authorize(&self, principal: &A2AAuthPrincipal, action: &A2AAction) -> Result<bool>;
    fn supported_schemes(&self) -> Vec<SecurityScheme>;
}

pub struct A2AAuthPrincipal {
    pub agent_id: Option<String>,
    pub trust_level: TrustLevel,
    pub permissions: Vec<String>,   // Allowed tools/skills
}

pub enum A2AAction {
    SendMessage,
    GetTask,
    CancelTask,
    ListTasks,
    Subscribe,
}
```

---

## Adapter Layer

### Server

#### Routes

```rust
pub fn a2a_routes(state: A2AServerState) -> Router {
    Router::new()
        .route("/.well-known/agent-card.json", get(get_agent_card))
        .route("/a2a", post(handle_jsonrpc))
        .route("/a2a/stream", post(handle_jsonrpc_stream))
        .with_state(state)
}

pub struct A2AServerState {
    pub task_manager: Arc<dyn A2ATaskManager>,
    pub message_handler: Arc<dyn A2AMessageHandler>,
    pub streaming: Arc<dyn A2AStreamingHandler>,
    pub authenticator: Arc<dyn A2AAuthenticator>,
    pub card: AgentCard,
}
```

#### Request Processor (JSON-RPC Method Routing)

```rust
pub struct A2ARequestProcessor { /* holds A2AServerState */ }

impl A2ARequestProcessor {
    pub async fn process(&self, request: JsonRpcRequest, auth: A2AAuthPrincipal) -> JsonRpcResponse {
        match request.method.as_str() {
            "message/send"      => self.handle_message_send(request, auth).await,
            "tasks/get"         => self.handle_tasks_get(request, auth).await,
            "tasks/cancel"      => self.handle_tasks_cancel(request, auth).await,
            "tasks/list"        => self.handle_tasks_list(request, auth).await,
            "tasks/pushNotificationConfig/set"    => ...,
            "tasks/pushNotificationConfig/get"    => ...,
            "tasks/pushNotificationConfig/list"   => ...,
            "tasks/pushNotificationConfig/delete" => ...,
            _ => JsonRpcResponse::error(METHOD_NOT_FOUND, "Unknown method"),
        }
    }
}
```

#### AgentLoopBridge (Core Bridge)

```rust
/// The most critical adapter: bridges A2A messages to Aleph's Agent Loop
pub struct AgentLoopBridge {
    pub agent_registry: Arc<AgentRegistry>,
    pub app_context: Arc<AppContext>,
}

#[async_trait]
impl A2AMessageHandler for AgentLoopBridge {
    async fn handle_message(&self, task_id: &str, message: A2AMessage, session_id: Option<&str>) -> Result<A2ATask> {
        // 1. A2AMessage → Aleph internal Message format conversion
        // 2. Create/reuse SessionKey::Task
        // 3. Trigger Agent Loop via AgentInstance
        // 4. Wait for completion, collect results
        // 5. Aleph result → A2ATask format conversion
    }

    async fn handle_message_stream(&self, task_id: &str, message: A2AMessage, session_id: Option<&str>)
        -> Result<Pin<Box<dyn Stream<Item = Result<UpdateEvent>> + Send>>>
    {
        // 1. Same conversion
        // 2. Trigger Agent Loop, attach broadcast listener
        // 3. Agent Loop callback events → UpdateEvent stream
    }
}
```

#### StreamHub (Broadcast-based Streaming)

```rust
pub struct StreamHub {
    channels: RwLock<HashMap<String, broadcast::Sender<UpdateEvent>>>,
    channel_capacity: usize,  // Default 256
}

// Key behaviors:
// - get_or_create_sender: lazy channel creation per task
// - Lagged subscribers: log warning, skip events (no panic)
// - Orphan tasks: ignore SendError when no subscribers
// - remove_channel: cleanup after task completion
```

### Client

```rust
pub struct A2AClient {
    http: reqwest::Client,
    base_url: String,
    auth_token: Option<String>,
    timeout: Duration,
}

impl A2AClient {
    pub async fn fetch_agent_card(&self) -> Result<AgentCard>;
    pub async fn send_message(&self, task_id: &str, message: A2AMessage) -> Result<A2ATask>;
    pub async fn send_message_stream(&self, task_id: &str, message: A2AMessage)
        -> Result<Pin<Box<dyn Stream<Item = Result<UpdateEvent>> + Send>>>;
    pub async fn get_task(&self, task_id: &str) -> Result<A2ATask>;
    pub async fn cancel_task(&self, task_id: &str) -> Result<A2ATask>;
    pub async fn list_tasks(&self, params: ListTasksParams) -> Result<ListTasksResult>;
}

pub struct A2AClientPool {
    clients: RwLock<HashMap<String, Arc<A2AClient>>>,
}
// Manages per-agent client instances with health checking
```

### Auth (Tiered Trust)

```rust
pub struct TieredAuthenticator {
    local_bypass: bool,
    token_store: Arc<dyn TokenStore>,
    oauth2_config: Option<OAuth2Config>,
}

// Authentication cascade:
// 1. localhost + local_bypass → TrustLevel::Local (full permissions)
// 2. Valid Bearer Token → TrustLevel::Trusted (configured permissions)
// 3. Valid OAuth2 → TrustLevel::Public (restricted permissions)
// 4. No credentials → Err(Unauthorized)
```

---

## Domain Services

### CardRegistry

```rust
pub struct CardRegistry {
    resolver: Arc<dyn AgentResolver>,
    config: A2AConfig,
    cache_ttl: Duration,
}

// Responsibilities:
// - load_from_config(): Load static agents from aleph.toml at startup
// - discover(url): Runtime dynamic registration
// - refresh_loop(): Background Agent Card refresh + health check
```

### SmartRouter

```rust
pub struct SmartRouter {
    resolver: Arc<dyn AgentResolver>,
    thinker: Arc<Thinker>,              // Reuses Aleph's LLM interaction layer
}

// Three-tier routing:
// 1. Exact name match: "Please use「交易助手」" → direct hit
// 2. Exact skill match: skill ID lookup
// 3. LLM semantic match: construct prompt with Agent Card summaries,
//    LLM returns {agent_id, confidence, reason}

pub struct RoutingDecision {
    pub agent: RegisteredAgent,
    pub confidence: f64,
    pub method: RoutingMethod,          // ExactName | ExactSkill | LlmSemantic
    pub reason: Option<String>,
}
```

### CardBuilder

```rust
pub struct CardBuilder;

// Auto-generates Aleph's own AgentCard from:
// - aleph.toml [a2a.server] config
// - ToolRegistry public tools → AgentSkill mappings
// - SecuritySchemes from auth config
```

### NotificationService

```rust
pub struct NotificationService {
    configs: RwLock<HashMap<String, PushNotificationConfig>>,
    http_client: reqwest::Client,
}

// CRUD for push notification configs
// Sends webhook POST on task status/artifact updates
```

---

## SubAgent Integration

### A2ASubAgent

```rust
pub struct A2ASubAgent {
    smart_router: Arc<SmartRouter>,
    client_pool: Arc<A2AClientPool>,
    card_registry: Arc<CardRegistry>,
}

#[async_trait]
impl SubAgent for A2ASubAgent {
    fn id(&self) -> &str { "a2a" }

    async fn can_handle(&self, request: &SubAgentRequest) -> bool {
        // SmartRouter quick check for matching remote agent
    }

    async fn execute(&self, request: SubAgentRequest) -> Result<SubAgentResult> {
        // 1. SmartRouter.route(prompt) → RoutingDecision
        // 2. A2AClientPool.get_or_create(agent)
        // 3. Build A2AMessage from request
        // 4. A2AClient.send_message(task_id, message)
        // 5. Convert A2ATask → SubAgentResult
    }
}
```

### End-to-End Flow

```
User: "请使用交易助手分析当前黄金的走势"
  │
  ▼
Main Agent Loop (Observe → Think)
  │  LLM identifies external delegation needed
  ▼
SubAgentDispatcher.dispatch(request)
  │  A2ASubAgent.can_handle() → true
  ▼
A2ASubAgent.execute(request)
  ├─ SmartRouter.route("请使用交易助手分析黄金走势")
  │    └─ try_exact_match → hit "交易助手" (confidence: 1.0)
  ├─ A2AClientPool.get_or_create(trading-agent)
  ├─ A2AClient.send_message("task-uuid", message)
  │    └─ POST http://trading-agent:8080/a2a
  └─ Return SubAgentResult { summary: "黄金当前..." }
  │
  ▼
Main Agent Loop (Think → Act)
  │  LLM composes response based on SubAgentResult
  ▼
User receives: "根据交易助手的分析，当前黄金走势..."
```

---

## Built-in Tools

Five AlephTool implementations for explicit A2A control:

| Tool | Purpose | Args |
|------|---------|------|
| `a2a_discover` | Discover and register a remote agent by URL | `{ url }` |
| `a2a_list_agents` | List all registered remote agents | `{ filter? }` |
| `a2a_send_message` | Send message to a remote agent | `{ agent_id, message, wait }` |
| `a2a_get_task` | Query remote task status | `{ agent_id, task_id }` |
| `a2a_cancel_task` | Cancel a remote task | `{ agent_id, task_id }` |

### Implicit vs Explicit Paths

```
Path A (Implicit — SubAgent):
  LLM natural language → SubAgentDispatcher → A2ASubAgent → SmartRouter → A2AClient
  Use: "请使用交易助手分析黄金走势" (LLM doesn't need A2A protocol details)

Path B (Explicit — Tools):
  LLM tool call → a2a_send_message { agent_id, message } → A2AClient
  Use: LLM needs fine control (specify agent_id, async, check status, cancel)

Both paths share: CardRegistry, A2AClientPool, A2AClient
```

---

## Configuration

```toml
# aleph.toml

[a2a]
enabled = true

# Server configuration
[a2a.server]
enabled = true
bind = "0.0.0.0:8080"
card_name = "Aleph"
card_description = "Personal AI assistant"
card_version = "1.0.0"

[a2a.server.security]
local_bypass = true
tokens = ["sk-aleph-xxx"]
# oauth2_issuer = "https://..."

[[a2a.server.skills]]
id = "general_chat"
name = "General Conversation"
description = "Natural language conversation and reasoning"

# Client configuration — pre-registered remote agents
[[a2a.agents]]
name = "交易助手"
url = "http://trading-agent:8080"
trust_level = "trusted"
token = "sk-trading-xxx"

[[a2a.agents]]
name = "代码审查"
url = "http://localhost:3000"
```

---

## Error Handling

```rust
#[derive(Debug, thiserror::Error)]
pub enum A2AError {
    // JSON-RPC standard errors
    ParseError(String),              // -32700
    InvalidRequest(String),          // -32600
    MethodNotFound(String),          // -32601
    InvalidParams(String),           // -32602
    InternalError(String),           // -32603

    // A2A business errors
    TaskNotFound(String),            // -32001
    TaskNotCancelable(TaskState),    // -32002
    PushNotSupported,                // -32003
    UnsupportedContentType,          // -32004

    // Security errors
    Unauthorized,                    // 401
    Forbidden,                       // 403

    // Client-side errors
    AgentUnreachable(String),
    NoMatchingAgent,
    Timeout(Duration),
}
```

---

## Server Mounting

```rust
// A2A routes merge alongside existing Gateway
let app = Router::new()
    .merge(gateway_routes())          // Existing WebSocket Gateway
    .merge(a2a_routes(a2a_state))     // A2A HTTP endpoints
    .layer(/* shared middleware */);
```

---

## Testing Strategy

### L1: Unit Tests

- **domain/**: Serialization roundtrips, JSON Schema compatibility with A2A spec
- **service/**: SmartRouter exact matching, TrustLevel inference from address
- **error**: JSON-RPC error code mapping correctness

### L2: Integration Tests

- **Server + Client roundtrip**: Start A2A Server (mock bridge), Client sends message, verify Task state
- **SSE streaming roundtrip**: Server broadcasts events, Client receives UpdateEvent stream
- **Tiered auth**: localhost → 200, valid token → 200 (restricted), no creds → 401

### L3: Mock Helpers

Each port trait has a Mock implementation for isolated testing:
- `MockTaskManager`, `MockMessageHandler`, `MockStreamingHandler`, `MockAgentResolver`

---

## Design Principles Alignment

| Principle | How |
|-----------|-----|
| R1 (Brain-Limb Separation) | A2A is a "nerve" (protocol), core logic stays in port traits |
| R4 (I/O-Only Interfaces) | Server routes are pure I/O, delegate to port traits |
| P1 (Low Coupling) | 5 independent port traits, adapter swappable |
| P2 (High Cohesion) | All A2A logic in `core/src/a2a/`, ~25 focused files |
| P3 (Extensibility) | New transport (gRPC) = new adapter, no port changes |
| P4 (Dependency Inversion) | Core defines traits, adapters implement |
| P5 (Least Knowledge) | A2ASubAgent only knows SmartRouter + ClientPool interfaces |
| P6 (Simplicity) | ~25 files, each <300 lines, no over-abstraction |
| P7 (Defensive Design) | Tiered auth, Lagged stream handling, SendError silencing |
