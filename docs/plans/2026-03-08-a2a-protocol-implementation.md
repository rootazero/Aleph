# A2A Protocol Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add full A2A v0.3 bidirectional protocol support to Aleph as an independent bounded context.

**Architecture:** DDD bounded context at `core/src/a2a/` with domain types, port traits, adapters (server/client/auth), services (registry/router/notification), and SubAgent integration. Server mounts as parallel axum routes; client integrates via SubAgentDispatcher.

**Tech Stack:** Rust, Tokio, axum (routes + SSE), reqwest (HTTP client), tokio::sync::broadcast (streaming), serde/schemars (serialization), thiserror (errors)

**Design Doc:** `docs/plans/2026-03-08-a2a-protocol-design.md`

---

## Phase 1: Domain Layer (Pure Types)

### Task 1: Core Domain Types

**Files:**
- Create: `core/src/a2a/mod.rs`
- Create: `core/src/a2a/domain/mod.rs`
- Create: `core/src/a2a/domain/agent_card.rs`
- Create: `core/src/a2a/domain/task.rs`
- Create: `core/src/a2a/domain/message.rs`
- Create: `core/src/a2a/domain/events.rs`
- Create: `core/src/a2a/domain/security.rs`
- Create: `core/src/a2a/domain/error.rs`
- Modify: `core/src/lib.rs` (add `pub mod a2a;`)

**Step 1: Create module structure**

Create `core/src/a2a/mod.rs`:
```rust
pub mod domain;
```

Create `core/src/a2a/domain/mod.rs`:
```rust
pub mod agent_card;
pub mod error;
pub mod events;
pub mod message;
pub mod security;
pub mod task;

// Re-exports
pub use agent_card::*;
pub use error::*;
pub use events::*;
pub use message::*;
pub use security::*;
pub use task::*;
```

**Step 2: Implement `security.rs`**

```rust
use serde::{Deserialize, Serialize};

/// Trust level for remote agents — determines auth requirements and permissions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustLevel {
    /// localhost — no auth required, full permissions
    Local,
    /// LAN / paired — token required, configured permissions
    Trusted,
    /// Internet — OAuth2/mTLS required, restricted permissions
    Public,
}

impl TrustLevel {
    /// Infer trust level from a socket address
    pub fn infer_from_addr(addr: &std::net::SocketAddr) -> Self {
        let ip = addr.ip();
        if ip.is_loopback() {
            TrustLevel::Local
        } else if is_private_ip(&ip) {
            TrustLevel::Trusted
        } else {
            TrustLevel::Public
        }
    }

    /// Infer trust level from a URL
    pub fn infer_from_url(url: &str) -> Self {
        if let Ok(parsed) = url::Url::parse(url) {
            match parsed.host_str() {
                Some("localhost") | Some("127.0.0.1") | Some("::1") => TrustLevel::Local,
                Some(host) if is_private_hostname(host) => TrustLevel::Trusted,
                _ => TrustLevel::Public,
            }
        } else {
            TrustLevel::Public
        }
    }
}

/// Security scheme for A2A authentication (A2A spec compliant)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SecurityScheme {
    ApiKey {
        location: ApiKeyLocation,
        name: String,
    },
    Http {
        scheme: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        bearer_format: Option<String>,
    },
    OAuth2 {
        flows: serde_json::Value, // OAuth2 flows config
    },
    OpenIdConnect {
        connect_url: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyLocation {
    Header,
    Query,
    Cookie,
}

/// Credentials extracted from an incoming request
#[derive(Debug, Clone)]
pub enum Credentials {
    BearerToken(String),
    ApiKey(String),
    OAuth2Token(String),
    None,
}

fn is_private_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        std::net::IpAddr::V6(v6) => v6.is_loopback(), // Simplified
    }
}

fn is_private_hostname(host: &str) -> bool {
    host.ends_with(".local") || host.ends_with(".lan")
        || host.starts_with("192.168.") || host.starts_with("10.")
}
```

**Step 3: Implement `agent_card.rs`**

```rust
use serde::{Deserialize, Serialize};
use super::security::SecurityScheme;

/// A2A Agent Card — metadata describing an agent's capabilities
/// Served at `/.well-known/agent-card.json`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentProvider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<AgentInterface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<AgentSkill>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security: Vec<SecurityScheme>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<AgentExtension>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_input_modes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_output_modes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProvider {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInterface {
    pub url: String,
    pub protocol: TransportProtocol,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransportProtocol {
    JsonRpc,
    Grpc,
    HttpJson,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_types: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_types: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentExtension {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub params: serde_json::Map<String, serde_json::Value>,
}
```

**Step 4: Implement `message.rs`**

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Map;

/// A2A message role
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum A2ARole {
    User,
    Agent,
}

/// A2A message — the communication payload
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2AMessage {
    pub message_id: String,
    pub role: A2ARole,
    pub parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, serde_json::Value>>,
}

/// Content part — text, file, or structured data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Part {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Map<String, serde_json::Value>>,
    },
    File {
        file: FileContent,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Map<String, serde_json::Value>>,
    },
    Data {
        data: Map<String, serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Map<String, serde_json::Value>>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<String>, // Base64
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

/// Artifact — output produced by an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub artifact_id: String,
    pub kind: String,
    pub parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, serde_json::Value>>,
}

impl A2AMessage {
    /// Create a simple text message
    pub fn text(role: A2ARole, text: impl Into<String>) -> Self {
        Self {
            message_id: uuid::Uuid::new_v4().to_string(),
            role,
            parts: vec![Part::Text { text: text.into(), metadata: None }],
            session_id: None,
            timestamp: Some(Utc::now()),
            metadata: None,
        }
    }

    /// Extract all text parts concatenated
    pub fn text_content(&self) -> String {
        self.parts.iter().filter_map(|p| match p {
            Part::Text { text, .. } => Some(text.as_str()),
            _ => None,
        }).collect::<Vec<_>>().join("\n")
    }
}
```

**Step 5: Implement `task.rs`**

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Map;
use super::message::{A2AMessage, Artifact};
use crate::domain::{Entity, AggregateRoot};

/// A2A task state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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

impl TaskState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Canceled | Self::Failed | Self::Rejected)
    }

    pub fn is_cancelable(&self) -> bool {
        matches!(self, Self::Submitted | Self::Working | Self::InputRequired)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<A2AMessage>,
    pub timestamp: DateTime<Utc>,
}

/// A2A Task — the aggregate root of the A2A bounded context
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2ATask {
    pub id: String,
    pub context_id: String,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<A2AMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, serde_json::Value>>,
    pub kind: String, // Always "task"
}

impl Entity for A2ATask {
    type Id = String;
    fn id(&self) -> &Self::Id { &self.id }
}

impl AggregateRoot for A2ATask {}

impl A2ATask {
    pub fn new(id: impl Into<String>, context_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            context_id: context_id.into(),
            status: TaskStatus {
                state: TaskState::Submitted,
                message: None,
                timestamp: Utc::now(),
            },
            artifacts: Vec::new(),
            history: Vec::new(),
            metadata: None,
            kind: "task".to_string(),
        }
    }
}

/// Parameters for listing tasks
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTasksParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_filter: Option<Vec<TaskState>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTasksResult {
    pub tasks: Vec<A2ATask>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}
```

**Step 6: Implement `events.rs`**

```rust
use serde::{Deserialize, Serialize};
use serde_json::Map;
use super::message::Artifact;
use super::task::TaskStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusUpdateEvent {
    pub task_id: String,
    pub context_id: String,
    pub kind: String, // "status-update"
    pub status: TaskStatus,
    #[serde(rename = "final")]
    pub is_final: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskArtifactUpdateEvent {
    pub task_id: String,
    pub context_id: String,
    pub kind: String, // "artifact-update"
    pub artifact: Artifact,
    #[serde(default)]
    pub append: bool,
    #[serde(default)]
    pub last_chunk: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum UpdateEvent {
    StatusUpdate(TaskStatusUpdateEvent),
    ArtifactUpdate(TaskArtifactUpdateEvent),
}
```

**Step 7: Implement `error.rs`**

```rust
use std::time::Duration;
use super::task::TaskState;

/// A2A error type — aligned with JSON-RPC error codes
#[derive(Debug, thiserror::Error)]
pub enum A2AError {
    // JSON-RPC standard errors
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Method not found: {0}")]
    MethodNotFound(String),
    #[error("Invalid params: {0}")]
    InvalidParams(String),
    #[error("Internal error: {0}")]
    InternalError(String),

    // A2A business errors
    #[error("Task not found: {0}")]
    TaskNotFound(String),
    #[error("Task not cancelable in state: {0:?}")]
    TaskNotCancelable(TaskState),
    #[error("Push notification not supported")]
    PushNotSupported,
    #[error("Unsupported content type")]
    UnsupportedContentType,

    // Security errors
    #[error("Unauthorized")]
    Unauthorized,
    #[error("Forbidden: insufficient trust level")]
    Forbidden,

    // Client-side errors
    #[error("Agent unreachable: {0}")]
    AgentUnreachable(String),
    #[error("No matching agent for intent")]
    NoMatchingAgent,
    #[error("Timeout after {0:?}")]
    Timeout(Duration),
}

impl A2AError {
    pub fn error_code(&self) -> i64 {
        match self {
            Self::ParseError(_) => -32700,
            Self::InvalidRequest(_) => -32600,
            Self::MethodNotFound(_) => -32601,
            Self::InvalidParams(_) => -32602,
            Self::InternalError(_) => -32603,
            Self::TaskNotFound(_) => -32001,
            Self::TaskNotCancelable(_) => -32002,
            Self::PushNotSupported => -32003,
            Self::UnsupportedContentType => -32004,
            Self::Unauthorized => -32000,
            Self::Forbidden => -32000,
            _ => -32603,
        }
    }

    pub fn to_jsonrpc_error(&self) -> serde_json::Value {
        serde_json::json!({
            "code": self.error_code(),
            "message": self.to_string(),
        })
    }
}
```

**Step 8: Register module in lib.rs**

Add `pub mod a2a;` in `core/src/lib.rs` after the existing module declarations.

**Step 9: Verify compilation**

Run: `cargo check -p alephcore`
Expected: SUCCESS

**Step 10: Write domain unit tests**

Create tests in each domain file (inline `#[cfg(test)]` modules) covering:
- Serialization roundtrips for all types
- `TaskState::is_terminal()` / `is_cancelable()` logic
- `TrustLevel::infer_from_addr()` / `infer_from_url()`
- `A2AMessage::text()` constructor and `text_content()` extraction
- `A2ATask::new()` defaults
- `A2AError::error_code()` mapping

Run: `cargo test -p alephcore --lib a2a::domain`
Expected: All PASS

**Step 11: Commit**

```bash
git add core/src/a2a/ core/src/lib.rs
git commit -m "a2a: add domain layer — types, events, errors"
```

---

## Phase 2: Port Traits

### Task 2: Port Trait Definitions

**Files:**
- Create: `core/src/a2a/port/mod.rs`
- Create: `core/src/a2a/port/task_manager.rs`
- Create: `core/src/a2a/port/message_handler.rs`
- Create: `core/src/a2a/port/streaming.rs`
- Create: `core/src/a2a/port/agent_resolver.rs`
- Create: `core/src/a2a/port/authenticator.rs`
- Modify: `core/src/a2a/mod.rs` (add `pub mod port;`)

**Step 1: Create all 5 port trait files**

Each file defines one async trait as specified in the design doc (see design doc "Port Traits" section). Key signatures:

- `A2ATaskManager`: create/get/update/cancel/list/add_artifact
- `A2AMessageHandler`: handle_message (sync), handle_message_stream (streaming)
- `A2AStreamingHandler`: subscribe_status/artifacts/all, broadcast_status/artifact
- `AgentResolver`: fetch_card, register, unregister, list_agents, resolve_by_id, resolve_by_intent
- `A2AAuthenticator`: authenticate, authorize, supported_schemes

Include `RegisteredAgent`, `AgentHealth`, `A2AAuthContext`, `A2AAuthPrincipal`, `A2AAction` structs/enums.

**Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: SUCCESS

**Step 3: Commit**

```bash
git add core/src/a2a/port/
git commit -m "a2a: add port traits — 5 async capability contracts"
```

---

## Phase 3: Config Integration

### Task 3: A2A Configuration

**Files:**
- Create: `core/src/a2a/config.rs`
- Modify: `core/src/config/mod.rs` or config types (add `a2a` field to `Config`)
- Modify: `core/src/a2a/mod.rs` (add `pub mod config;`)

**Step 1: Define A2AConfig structs**

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct A2AConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub server: A2AServerConfig,
    #[serde(default)]
    pub agents: Vec<A2AAgentEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct A2AServerConfig {
    #[serde(default = "default_server_enabled")]
    pub enabled: bool,
    #[serde(default = "default_bind")]
    pub bind: String,
    pub card_name: Option<String>,
    pub card_description: Option<String>,
    pub card_version: Option<String>,
    #[serde(default)]
    pub security: A2ASecurityConfig,
    #[serde(default)]
    pub skills: Vec<crate::a2a::domain::AgentSkill>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct A2AAgentEntry {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub trust_level: Option<String>, // "local" | "trusted" | "public"
    pub token: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct A2ASecurityConfig {
    #[serde(default = "default_true")]
    pub local_bypass: bool,
    #[serde(default)]
    pub tokens: Vec<String>,
}
```

**Step 2: Add `a2a` field to main Config struct**

Find the `Config` struct in config types and add:
```rust
#[serde(default)]
pub a2a: crate::a2a::config::A2AConfig,
```

**Step 3: Verify compilation + commit**

Run: `cargo check -p alephcore`

```bash
git commit -m "a2a: add configuration types for aleph.toml [a2a] section"
```

---

## Phase 4: Server Adapters

### Task 4: TaskStore (A2ATaskManager impl)

**Files:**
- Create: `core/src/a2a/adapter/mod.rs`
- Create: `core/src/a2a/adapter/server/mod.rs`
- Create: `core/src/a2a/adapter/server/task_store.rs`

In-memory implementation of `A2ATaskManager` using `RwLock<HashMap<String, A2ATask>>`. Include unit tests for CRUD operations and state transitions.

**Commit:** `a2a: add in-memory TaskStore (A2ATaskManager impl)`

### Task 5: StreamHub (A2AStreamingHandler impl)

**Files:**
- Create: `core/src/a2a/adapter/server/stream_hub.rs`

Broadcast-based streaming using `tokio::sync::broadcast`. Handle Lagged errors gracefully. Include tests for multi-subscriber scenarios.

**Commit:** `a2a: add StreamHub (broadcast-based streaming)`

### Task 6: AgentLoopBridge (A2AMessageHandler impl)

**Files:**
- Create: `core/src/a2a/adapter/server/bridge.rs`

Bridge A2A messages to Aleph's Agent Loop:
- Convert `A2AMessage` → internal message format
- Create `SessionKey::Task { agent_id, task_type: "a2a", task_id }`
- Trigger Agent Loop via `AgentInstance`
- Convert results back to `A2ATask`

This is the most complex adapter — depends on `AgentRegistry`, `AppContext` patterns from `core/src/gateway/`.

**Commit:** `a2a: add AgentLoopBridge (A2A ↔ Agent Loop)`

### Task 7: RequestProcessor + Routes

**Files:**
- Create: `core/src/a2a/adapter/server/request_processor.rs`
- Create: `core/src/a2a/adapter/server/routes.rs`

RequestProcessor: JSON-RPC method dispatch (`message/send`, `tasks/get`, `tasks/cancel`, `tasks/list`, push notification config CRUD).

Routes: 3 axum endpoints:
- `GET /.well-known/agent-card.json`
- `POST /a2a` (JSON-RPC)
- `POST /a2a/stream` (JSON-RPC + SSE response)

**Commit:** `a2a: add RequestProcessor and axum routes`

---

## Phase 5: Auth Adapter

### Task 8: TieredAuthenticator

**Files:**
- Create: `core/src/a2a/adapter/auth/mod.rs`
- Create: `core/src/a2a/adapter/auth/tiered.rs`
- Create: `core/src/a2a/adapter/auth/token_store.rs`

Cascade: localhost → Bearer Token → OAuth2 → Reject. Unit tests for each tier.

**Commit:** `a2a: add TieredAuthenticator (tiered trust security)`

---

## Phase 6: Client Adapters

### Task 9: A2AClient (HTTP)

**Files:**
- Create: `core/src/a2a/adapter/client/mod.rs`
- Create: `core/src/a2a/adapter/client/http_client.rs`

reqwest-based HTTP client implementing: `fetch_agent_card`, `send_message`, `get_task`, `cancel_task`, `list_tasks`.

**Commit:** `a2a: add A2AClient (HTTP)`

### Task 10: SSE Stream Parser + ClientPool

**Files:**
- Create: `core/src/a2a/adapter/client/sse_stream.rs`
- Create: `core/src/a2a/adapter/client/pool.rs`

SSE parser: converts `reqwest::Response` byte stream → `Stream<UpdateEvent>`.
ClientPool: `RwLock<HashMap<String, Arc<A2AClient>>>` with `get_or_create` and health check.

**Commit:** `a2a: add SSE parser and client connection pool`

---

## Phase 7: Domain Services

### Task 11: CardRegistry + CardBuilder

**Files:**
- Create: `core/src/a2a/service/mod.rs`
- Create: `core/src/a2a/service/card_registry.rs`
- Create: `core/src/a2a/service/card_builder.rs`

CardRegistry: `load_from_config()`, `discover(url)`, `refresh_loop()`.
CardBuilder: auto-generate Aleph's AgentCard from config + ToolRegistry.

Implement `AgentResolver` trait (in-memory store with `RwLock<Vec<RegisteredAgent>>`).

**Commit:** `a2a: add CardRegistry and CardBuilder services`

### Task 12: SmartRouter

**Files:**
- Create: `core/src/a2a/service/smart_router.rs`

Three-tier routing:
1. Exact name match (extract quoted names, fuzzy match against agent names/aliases)
2. Exact skill ID match
3. LLM semantic match (construct prompt with Agent Card summaries → Thinker)

Return `RoutingDecision { agent, confidence, method, reason }`.

**Commit:** `a2a: add SmartRouter (exact + LLM semantic routing)`

### Task 13: NotificationService

**Files:**
- Create: `core/src/a2a/service/notification.rs`

Push notification config CRUD + webhook POST on status/artifact updates.

**Commit:** `a2a: add NotificationService (push webhooks)`

---

## Phase 8: SubAgent + Tools Integration

### Task 14: A2ASubAgent

**Files:**
- Create: `core/src/a2a/sub_agent.rs`

Implement `SubAgent` trait:
- `id() → "a2a"`
- `capabilities() → [Custom]` (dynamic from registered agents)
- `can_handle()` → SmartRouter quick check
- `execute()` → SmartRouter.route → A2AClient.send_message → SubAgentResult

Register in SubAgentDispatcher during server startup.

**Commit:** `a2a: add A2ASubAgent (SubAgent trait impl)`

### Task 15: Built-in Tools (5 tools)

**Files:**
- Create: `core/src/builtin_tools/a2a_discover.rs`
- Create: `core/src/builtin_tools/a2a_list_agents.rs`
- Create: `core/src/builtin_tools/a2a_send_message.rs`
- Create: `core/src/builtin_tools/a2a_get_task.rs`
- Create: `core/src/builtin_tools/a2a_cancel_task.rs`
- Modify: `core/src/builtin_tools/mod.rs` (register tools)

Each tool follows the `WebFetchTool` pattern:
- `Args` struct with `JsonSchema`
- `Output` struct with `Serialize`
- `AlephTool` impl with `NAME`, `DESCRIPTION`, `call()`

**Commit:** `a2a: add 5 built-in A2A tools`

---

## Phase 9: Server Wiring

### Task 16: Server Startup Integration

**Files:**
- Modify: `core/src/gateway/server.rs` (merge A2A routes in `build_router()`)
- Modify: `core/src/bin/aleph/commands/start/mod.rs` (initialize A2A subsystem)
- Modify: `core/src/bin/aleph/commands/start/builder/handlers.rs` (if needed)

Wire up:
1. Read `A2AConfig` from loaded config
2. Create `TaskStore`, `StreamHub`, `AgentLoopBridge`, `TieredAuthenticator`
3. Build `A2AServerState`
4. Create `CardRegistry`, `SmartRouter`, `A2AClientPool`
5. Create `A2ASubAgent` and register with `SubAgentDispatcher`
6. Register builtin A2A tools with `ToolRegistry`
7. Merge `a2a_routes()` into main router
8. Spawn `CardRegistry::refresh_loop()` background task

**Commit:** `a2a: wire A2A subsystem into server startup`

---

## Phase 10: Integration Tests

### Task 17: End-to-End Tests

**Files:**
- Create: `core/src/a2a/tests/mod.rs` or `core/tests/a2a_integration.rs`

Tests:
1. **Agent Card discovery**: Start server → GET `/.well-known/agent-card.json` → verify card
2. **message/send roundtrip**: POST JSON-RPC → mock bridge returns result → verify A2ATask
3. **SSE streaming**: POST stream → receive 3 events → verify order and types
4. **Tiered auth**: localhost OK, valid token OK, no creds → 401
5. **Client → Server roundtrip**: A2AClient → local A2A Server → AgentLoopBridge (mock)
6. **SmartRouter exact match**: "请使用「交易助手」" → hit
7. **TaskStore CRUD**: create → update → cancel → list

**Commit:** `a2a: add integration tests`

---

## Summary

| Phase | Tasks | Est. Files | Focus |
|-------|-------|-----------|-------|
| 1. Domain | Task 1 | 8 | Pure types |
| 2. Ports | Task 2 | 6 | Trait contracts |
| 3. Config | Task 3 | 2 | aleph.toml [a2a] |
| 4. Server | Tasks 4-7 | 5 | axum + bridge |
| 5. Auth | Task 8 | 3 | Tiered trust |
| 6. Client | Tasks 9-10 | 4 | HTTP + SSE |
| 7. Services | Tasks 11-13 | 4 | Registry + router |
| 8. Integration | Tasks 14-15 | 7 | SubAgent + tools |
| 9. Wiring | Task 16 | 3 (modify) | Startup |
| 10. Tests | Task 17 | 1 | E2E verification |
| **Total** | **17 tasks** | **~43 files** | |

**Dependencies:** Tasks 1→2→3 are sequential. Tasks 4-8 depend on 1-3 but are parallel to each other. Tasks 9-13 depend on 1-3 but are parallel to 4-8. Tasks 14-15 depend on 7+9. Task 16 depends on all. Task 17 depends on 16.
