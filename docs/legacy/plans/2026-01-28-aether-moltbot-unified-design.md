# Aether → Rust Moltbot: Unified Architecture & Implementation Design

**Date:** 2026-01-28
**Status:** Design Phase
**Strategy:** Hybrid refactoring (aggressive + incremental)
**Timeline:** 11-15 weeks across 4 phases

**GitHub Reference:**
- Moltbot: https://github.com/moltbot/moltbot
- Local Path: `/Users/zouguojun/Workspace/moltbot`

---

## Executive Summary

This document outlines a comprehensive architectural redesign of Aether, inspired by [Moltbot](https://github.com/moltbot/moltbot)'s proven design patterns. The redesign adopts Moltbot's WebSocket Gateway control plane while extending its capabilities with Aether's unique features.

### Core Objectives

1. **Gateway-Centric Architecture** - Single WebSocket control plane at `ws://127.0.0.1:18789`
2. **Multi-Agent Support** - Parallel execution with workspace isolation
3. **Real-Time Streaming** - Event-driven UI updates for reasoning and tool execution
4. **Simplified Dispatcher** - Reduce 16 modules to 3 layers
5. **Extended Capabilities** - Multi-channel integration, sandbox isolation, local tools

### Refactoring Strategy

- **Aggressive:** WebSocket Gateway (replace UniFFI)
- **Incremental:** Agent Loop enhancements with event emission
- **Targeted:** Dispatcher simplification (remove redundancy)
- **Expansive:** Multi-channel connectors, sandbox, local tools

### Current Pain Points Addressed

| Pain Point | Current State | Moltbot-Inspired Solution |
|------------|---------------|---------------------------|
| **Communication** | UniFFI blocking calls | WebSocket streaming |
| **Architecture** | 16 dispatcher modules | 3-layer simplification |
| **Multi-Agent** | Single agent bottleneck | Workspace isolation + routing |
| **Session Isolation** | Flat topic_id | Hierarchical SessionKey |
| **Real-Time UX** | No progress feedback | Event streaming |
| **Multi-Client** | Single macOS app | WebSocket multi-client support |

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Core Components](#2-core-components)
3. [Technical Deep Dive](#3-technical-deep-dive)
4. [Extended Features](#4-extended-features)
5. [Implementation Phases](#5-implementation-phases)
6. [Risk Mitigation](#6-risk-mitigation)
7. [Success Criteria](#7-success-criteria)
8. [Appendices](#8-appendices)

---

## 1. Architecture Overview

### 1.1 Target Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                    Aether Gateway (Rust)                             │
│                 WebSocket Server :18789                              │
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │                  AgentRouter                                    │ │
│  │  Route requests to agents (peer/channel/task matching)         │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                              ↓                                       │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │                SessionManager                                   │ │
│  │  Hierarchical SessionKey resolution (Main/PerPeer/Task)        │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                              ↓                                       │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │              ExecutionEngine                                    │ │
│  │  ┌─────────────────┐        ┌─────────────────┐                │ │
│  │  │ AgentInstance   │        │ AgentInstance   │                │ │
│  │  │   (main)        │        │   (work)        │                │ │
│  │  │ • Workspace     │        │ • Workspace     │                │ │
│  │  │ • Sessions DB   │        │ • Sessions DB   │                │ │
│  │  │ • Agent Loop    │        │ • Agent Loop    │                │ │
│  │  └─────────────────┘        └─────────────────┘                │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                              ↓                                       │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │                   Event Bus                                     │ │
│  │  Pub/Sub streaming events to connected clients                 │ │
│  └────────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────┘
           │                    │                    │
           ▼                    ▼                    ▼
    ┌──────────────┐     ┌──────────────┐    ┌──────────────┐
    │  macOS App   │     │  Tauri UI    │    │  CLI Client  │
    │  (Swift/WS)  │     │  (React/WS)  │    │  (Rust/WS)   │
    └──────────────┘     └──────────────┘    └──────────────┘

┌─────────────────────────────────────────────────────────────────────┐
│                    Extended Components (Below Gateway)               │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │               Channel Connectors (Rust)                         │ │
│  │  • Telegram Bot     • Discord Bot      • Slack Bot              │ │
│  │  • WhatsApp Web     • Signal           • iMessage              │ │
│  │  • WebChat (HTTP)   • MS Teams         • Matrix                │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │               Sandbox Manager (Docker)                          │ │
│  │  • Main Session: full tool access                              │ │
│  │  • Non-Main Session: Docker container isolation                │ │
│  │  • Permission escalation: /elevated on|off                     │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │               Local Tools (Rust)                                │ │
│  │  • Chrome CDP Controller (browser automation)                  │ │
│  │  • Cron Scheduler (task scheduling)                            │ │
│  │  • Webhook Listener (event-driven workflows)                   │ │
│  │  • File operations, System integration                         │ │
│  └────────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────┘
```

### 1.2 Architectural Principles

1. **Local-First:** Gateway binds to 127.0.0.1 by default, optional remote via Tailscale
2. **Single Control Plane:** All state owned by Gateway, clients are stateless
3. **Agent Isolation:** Each agent has independent workspace, session store, config
4. **Streaming by Default:** All operations emit events for real-time feedback
5. **Protocol Standardization:** JSON-RPC 2.0 with binary frame support
6. **Modular Extensions:** Channel connectors, sandbox, tools as plugin-style modules

---

## 2. Core Components

### 2.1 WebSocket Gateway

**Responsibilities:**
- Accept WebSocket connections from multiple clients
- Authenticate via App-Token (local) or Device-Token (remote)
- Route requests to appropriate agents
- Broadcast events to subscribed clients
- Manage device pairing for remote connections

**Technology Stack:**
- `tokio-tungstenite` - Async WebSocket server
- `serde_json` / `rmp-serde` - JSON and MessagePack serialization
- `dashmap` - Concurrent HashMap for client connections
- `jsonrpc-core` - JSON-RPC 2.0 protocol

**Protocol: JSON-RPC 2.0**

```rust
// Request
{
  "jsonrpc": "2.0",
  "id": "req-123",
  "method": "agent.run",
  "params": {
    "input": "Hello, Aether",
    "session_key": "agent:main:main"
  }
}

// Response (accepted)
{
  "jsonrpc": "2.0",
  "id": "req-123",
  "result": {
    "run_id": "run-456",
    "accepted_at": "2026-01-28T10:30:00Z"
  }
}

// Event (unidirectional streaming)
{
  "jsonrpc": "2.0",
  "method": "stream.reasoning",
  "params": {
    "run_id": "run-456",
    "seq": 1,
    "content": "I need to analyze this request...",
    "is_complete": false
  }
}
```

**Binary Frame Support:**
```rust
// Frame structure: [4-byte header][payload]
pub struct BinaryFrameHeader {
    frame_type: u8,     // 0x01=MessagePack, 0x02=Raw binary
    compression: u8,    // 0x00=None, 0x01=Zstd
    reserved: u16,
}

// Use cases:
// - Large responses (>1KB): 20-30% size reduction
// - File uploads/downloads
// - Image data transfer
```

**Core RPC Methods:**
```rust
// Agent execution
agent.run { input, session_key, stream: true }
agent.wait { run_id }
agent.cancel { run_id }

// Session management
sessions.list { agent_id? }
sessions.history { session_key, limit? }
sessions.reset { session_key }
sessions.send { session_key, message }

// Multi-agent coordination
agents.list {}
agents.status { agent_id }

// Channel routing (extended feature)
channels.status {}
channels.send { channel, recipient, message }

// Sandbox control (extended feature)
sandbox.create { session_id, mode }  // "main" | "docker"
sandbox.elevate { session_id, enabled }

// System
system.health {}
system.version {}
```

### 2.2 Multi-Agent Router

**Routing Strategy (priority order):**
1. **Peer Match** - Exact DM/group/channel ID
2. **Channel Match** - Input source (GUI window, CLI, hotkey)
3. **Task Match** - Task type (cron, webhook)
4. **Fallback** - Default agent ("main")

**Agent Instance Isolation:**
```
~/.aether/
├── agents/
│   ├── main/
│   │   ├── workspace/          # Independent working directory
│   │   ├── sessions.db         # SQLite session store
│   │   └── config.toml         # Agent-specific config
│   └── work/
│       ├── workspace/
│       ├── sessions.db
│       └── config.toml
└── gateway/
    ├── gateway.db              # Global gateway state
    └── devices.json            # Paired devices
```

**Configuration Example:**
```toml
# ~/.aether/config.toml

[gateway]
host = "127.0.0.1"
port = 18789
max_connections = 100

[agents.main]
workspace = "~/aether-main"
model = "claude-sonnet-4-5"
fallback_models = ["claude-opus-4-5", "gpt-4-turbo"]

[agents.work]
workspace = "~/aether-work"
model = "claude-opus-4-5"

[bindings]
"gui:window1" = "main"
"cli:*" = "work"
"hotkey:global" = "main"

# Extended features
[channels.telegram]
enabled = true
token = "${TELEGRAM_BOT_TOKEN}"

[channels.discord]
enabled = true
token = "${DISCORD_BOT_TOKEN}"

[sandbox]
enabled = true
docker_image = "aether-sandbox:latest"
memory_limit_mb = 512

[tools.chrome]
enabled = true
executable_path = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
```

### 2.3 Hierarchical Session Management

**Session Key Types:**
```rust
pub enum SessionKey {
    // Main session (cross-device shared)
    Main {
        agent_id: String,
        main_key: String  // Default: "main"
    },

    // Per-peer isolation (different GUI windows, chat conversations)
    PerPeer {
        agent_id: String,
        peer_id: String  // Window ID, chat ID, channel ID
    },

    // Task isolation (cron jobs, webhooks, scheduled tasks)
    Task {
        agent_id: String,
        task_type: String,  // "cron", "webhook", "scheduled"
        task_id: String
    },

    // Ephemeral (single-turn, no persistence)
    Ephemeral {
        agent_id: String,
        ephemeral_id: String  // UUID
    },
}

// Example keys:
// "agent:main:main"
// "agent:work:peer:window-abc"
// "agent:main:peer:telegram:123456789"
// "agent:main:cron:daily-summary"
// "agent:main:ephemeral:550e8400-e29b-41d4-a716-446655440000"
```

**Session Lifecycle:**
```rust
pub struct Session {
    pub id: String,
    pub key: SessionKey,
    pub mode: SessionMode,
    pub channel: String,
    pub user_id: String,
    pub history: Vec<ChatMessage>,
    pub metadata: SessionMetadata,
    pub created_at: i64,
    pub last_active: i64,
    pub auto_reset_at: Option<i64>,  // Daily reset timestamp
}

pub enum SessionMode {
    Main { elevated: bool },
    NonMain { container_id: Option<String> },
    Ephemeral,
}
```

**Lifecycle Management:**
- **Daily Reset:** 4:00 AM local time (configurable)
- **Idle Reset:** N minutes after last interaction (optional)
- **Manual Reset:** `/new`, `/reset` commands
- **Smart Compaction:** Auto-compress when context exceeds limit (preserve recent + important)
- **Expiration:** Delete ephemeral sessions after 24h, archived sessions after 30 days

**Persistence:**
- SQLite storage (`~/.aether/agents/{agent_id}/sessions.db`)
- Auto-compression (keep last 50 messages per session)
- Cross-device sync via Gateway (Main sessions only)

### 2.4 Streaming Architecture

**Event Types:**
```rust
pub enum StreamEvent {
    // Agent lifecycle
    RunAccepted {
        run_id: String,
        session_key: String,
    },

    // Reasoning process (thinking)
    Reasoning {
        run_id: String,
        seq: u64,
        content: String,
        is_complete: bool,
    },

    // Tool execution lifecycle
    ToolStart {
        run_id: String,
        seq: u64,
        tool_name: String,
        tool_id: String,
        params: Value,
    },
    ToolUpdate {
        run_id: String,
        seq: u64,
        tool_id: String,
        progress: String,  // "Reading file... 50%"
    },
    ToolEnd {
        run_id: String,
        seq: u64,
        tool_id: String,
        result: ToolResult,
        duration_ms: u64,
    },

    // Response streaming (chunked text)
    ResponseChunk {
        run_id: String,
        seq: u64,
        content: String,
        chunk_index: u32,
        is_final: bool,
    },

    // Run completion
    RunComplete {
        run_id: String,
        seq: u64,
        summary: RunSummary,
        total_duration_ms: u64,
    },
    RunError {
        run_id: String,
        seq: u64,
        error: String,
    },

    // User interaction
    AskUser {
        run_id: String,
        seq: u64,
        question: String,
        options: Vec<String>,
    },
}
```

**Block Chunking Strategy (from Moltbot):**
- **Min chars:** 200 (buffer before sending)
- **Max chars:** 2000 (hard limit, force send)
- **Break preference:** paragraph → newline → sentence → whitespace
- **Code fence rule:** Never split inside ``` blocks; close and reopen if forced
- **Coalescing:** Wait 500ms idle before flush (merge small chunks)

**Sequence Numbers:**
All events include a monotonically increasing `seq` field to guarantee ordering, even with concurrent tool execution.

### 2.5 Dispatcher Simplification

**Current Architecture (16 modules) → Target (3 layers):**

| Old Module | New Home | Rationale |
|------------|----------|-----------|
| `planner` | `AgentRouter` | Simple rule-based routing |
| `scheduler` | Remove (per-session serialization) | Gateway manages session queues |
| `executor` | `ExecutionEngine` | Merge with agent_loop |
| `model_router` | `agent_loop` internal | Loop decides model failover |
| `skill_router` | `agent_loop` internal | Loop loads skills on-demand |
| `tool_router` | `ToolRegistry` | Simplified discovery only |
| `cowork_types` | Remove | DAG replaced by streaming |
| `engine.rs` | `ExecutionEngine` | Merge + simplify |
| `sub_agents` | Keep & enhance | Multi-agent coordination |
| `monitor` | `EventBus` | Event-driven monitoring |
| Other 6 modules | Remove or merge | Redundant/unused code |

**New 3-Layer Architecture:**
```rust
pub struct Gateway {
    // Layer 1: "Who handles this?"
    router: AgentRouter,

    // Layer 2: "Where to execute?"
    session_manager: SessionManager,

    // Layer 3: "How to execute?"
    execution_engine: ExecutionEngine,
}

// Simplified request flow:
// Request → AgentRouter → SessionManager → ExecutionEngine → Agent Loop → Tools
```

**Code Reduction Target:** 30%+ fewer lines vs current dispatcher

---

## 3. Technical Deep Dive

### 3.1 Security: Token Auth & Device Pairing

**App-Token (Local Connections):**
```rust
// Generated on first launch, stored in macOS Keychain
pub struct TokenManager {
    pub fn ensure_app_token(&self) -> Result<String> {
        if let Some(token) = self.read_from_keychain()? {
            return Ok(token);
        }

        let token = self.generate_256bit_token();
        self.store_to_keychain(&token)?;
        Ok(token)
    }

    fn generate_256bit_token(&self) -> String {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        hex::encode(bytes)
    }
}
```

**Device-Token (Remote Connections):**
```rust
// JWT with claims
pub struct DeviceTokenClaims {
    device_id: String,      // Device fingerprint
    scopes: Vec<Scope>,     // operator.read, operator.write, etc.
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,  // 365 days
}

pub enum Scope {
    OperatorRead,   // View agent status, sessions
    OperatorWrite,  // Send messages, control agents
    Admin,          // Manage devices, settings
}
```

**Pairing Flow:**
1. Remote device connects without token
2. Gateway generates 6-digit approval code
3. Code displayed on device, broadcast to operator clients
4. Operator approves via UI (`aether devices approve <code>`)
5. Gateway issues Device-Token (JWT)
6. Device stores token for future connections

**Security Implementation:**
```rust
pub struct SecurityManager {
    app_token: String,
    device_tokens: Arc<RwLock<HashMap<String, DeviceToken>>>,
    pending_pairs: Arc<RwLock<HashMap<String, PendingPair>>>,
}

impl SecurityManager {
    pub fn authenticate(&self, token: &str) -> Result<AuthContext> {
        // Try app token
        if token == self.app_token {
            return Ok(AuthContext::Local);
        }

        // Try device token
        if let Some(device) = self.verify_device_token(token)? {
            return Ok(AuthContext::Remote { device });
        }

        Err(anyhow!("Invalid token"))
    }
}
```

### 3.2 Protocol: JSON-RPC 2.0 + Binary Frames

**Content Negotiation:**
```rust
// Client declares capabilities in connect request
pub struct ClientCapabilities {
    encodings: Vec<Encoding>,      // ["json", "msgpack"]
    compressions: Vec<String>,     // ["zstd", "gzip"]
    binary_frames: bool,
    max_message_size: usize,       // Client buffer limit
}

// Gateway selects best option
pub struct ConnectResponse {
    selected_encoding: Encoding,
    selected_compression: Option<String>,
    protocol_version: u32,
}
```

**Protocol Versioning:**
```rust
pub const PROTOCOL_VERSION_MIN: u32 = 1;
pub const PROTOCOL_VERSION_MAX: u32 = 2;

pub fn negotiate_version(client_min: u32, client_max: u32) -> Result<u32> {
    let server_min = PROTOCOL_VERSION_MIN;
    let server_max = PROTOCOL_VERSION_MAX;

    // Find intersection
    let compatible_min = client_min.max(server_min);
    let compatible_max = client_max.min(server_max);

    if compatible_min > compatible_max {
        return Err(anyhow!("No compatible protocol version"));
    }

    // Return highest compatible version
    Ok(compatible_max)
}
```

### 3.3 Rate Limiting & Backpressure

**Per-Client Rate Limiting:**
```rust
use governor::{Quota, RateLimiter};

// 100 requests/second per client
let limiter = RateLimiter::keyed(Quota::per_second(100));

if limiter.check_key(&client_id).is_err() {
    return JsonRpcError::rate_limit_exceeded();
}
```

**Event Stream Backpressure:**
```rust
// If client falls behind, apply backpressure
pub struct EventBuffer {
    buffer: VecDeque<StreamEvent>,
    max_size: usize,  // e.g., 1000 events
}

impl EventBuffer {
    pub async fn send(&mut self, event: StreamEvent) -> Result<()> {
        // Block if buffer full (client too slow)
        while self.buffer.len() >= self.max_size {
            warn!("Client lagging, applying backpressure");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        self.buffer.push_back(event);
        Ok(())
    }
}
```

### 3.4 Agent Loop Integration

**Event Emitter Trait:**
```rust
#[async_trait]
pub trait EventEmitter: Send + Sync {
    async fn emit(&self, event: StreamEvent) -> Result<()>;
    async fn emit_reasoning(&self, run_id: &str, content: &str, complete: bool);
    async fn emit_tool_start(&self, run_id: &str, tool: &str, params: Value);
    async fn emit_tool_end(&self, run_id: &str, tool_id: &str, result: ToolResult);
    async fn emit_response_chunk(&self, run_id: &str, content: &str, final_chunk: bool);
}
```

**Modified Agent Loop:**
```rust
pub async fn execute_with_emitter(
    &self,
    input: &str,
    session_key: &SessionKey,
    emitter: Arc<dyn EventEmitter>,
) -> Result<String> {
    let run_id = Uuid::new_v4().to_string();

    loop {
        // Observe
        let context = self.observe(session_key).await?;

        // Think
        emitter.emit_reasoning(&run_id, "Analyzing request...", false).await?;
        let decision = self.think(&context).await?;

        // Act
        if let Some(tool_call) = decision.tool_call {
            emitter.emit_tool_start(&run_id, &tool_call.tool, tool_call.params.clone()).await?;
            let result = self.execute_tool(&tool_call).await?;
            emitter.emit_tool_end(&run_id, &tool_call.id, result.clone()).await?;
        }

        // Feedback
        if decision.complete {
            emitter.emit_response_chunk(&run_id, &decision.response, true).await?;
            return Ok(decision.response);
        }
    }
}
```

### 3.5 Health Check & Diagnostics

**Health Endpoint:**
```rust
pub async fn handle_health(&self) -> JsonRpcResult<HealthStatus> {
    Ok(HealthStatus {
        status: "healthy",
        uptime_seconds: self.start_time.elapsed().as_secs(),
        connected_clients: self.clients.len(),
        active_agents: self.agents.len(),
        active_runs: self.active_runs.len(),
        protocol_version: PROTOCOL_VERSION_MAX,
        memory_mb: self.memory_usage_mb(),
    })
}
```

**Diagnostics CLI:**
```bash
$ aether doctor

🔍 Aether Diagnostics

✓ Gateway process: Running (PID 12345)
✓ WebSocket connection: OK (ws://127.0.0.1:18789)
✓ App token: Valid
✓ Port 18789: Available

📊 Detailed Stats:
  - Connected clients: 3 (2 macOS, 1 CLI)
  - Active agents: 2 (main, work)
  - Active sessions: 5
  - Memory usage: 142 MB
  - CPU: 2.3%
  - Uptime: 2d 14h 32m

🔧 Agent Status:
  main: active, 3 sessions, workspace: ~/aether-main
  work: active, 2 sessions, workspace: ~/aether-work
```

---

## 4. Extended Features

### 4.1 Multi-Channel Connectors

**Responsibilities:**
- Connect to external messaging platforms
- Bidirectional message routing
- Unified message format translation
- Channel-specific features (reactions, threads, etc.)

**Supported Platforms:**

| Platform | Implementation | Priority | Tech Stack |
|----------|---------------|----------|------------|
| **WebChat** | HTTP Server (axum) | P0 | Built-in |
| **Telegram** | Bot API | P0 | `teloxide` |
| **Discord** | Bot API | P0 | `serenity` |
| **Slack** | Bolt SDK | P1 | `slack-morphism` |
| **WhatsApp** | Browser automation | P1 | Chrome CDP |
| **Signal** | Signal CLI | P1 | subprocess |
| **iMessage** | AppleScript | P1 | macOS native |
| **MS Teams** | Bot Framework | P2 | Extension |
| **Matrix** | Matrix SDK | P2 | Extension |

**Unified Message Format:**
```rust
pub struct UnifiedMessage {
    pub id: String,
    pub channel: String,
    pub sender: Sender,
    pub content: MessageContent,
    pub timestamp: i64,
    pub metadata: HashMap<String, Value>,
}

pub enum MessageContent {
    Text(String),
    Image { url: String, caption: Option<String> },
    File { url: String, filename: String },
    Audio { url: String, duration: Option<u64> },
    Video { url: String, duration: Option<u64> },
    Sticker { sticker_id: String },
}

pub struct Sender {
    pub id: String,
    pub name: String,
    pub avatar_url: Option<String>,
}
```

**Connector Trait:**
```rust
#[async_trait]
pub trait ChannelConnector: Send + Sync {
    fn name(&self) -> &str;
    async fn start(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn send(&self, recipient: &str, message: &UnifiedMessage) -> Result<()>;
    async fn receive(&self) -> Result<UnifiedMessage>;
}
```

**Message Flow:**
```
External Message (Telegram/Discord/etc.)
    │
    ▼
Channel Connector (parse to UnifiedMessage)
    │
    ▼
Gateway (route to Agent via SessionKey)
    │
    ▼
Agent Loop (process message)
    │
    ▼
Gateway (broadcast response events)
    │
    ▼
Channel Connector (translate and send to original channel)
```

### 4.2 Sandbox Manager (Docker Isolation)

**Purpose:** Session-level permission control with Docker isolation for untrusted contexts.

**Modes:**
```rust
pub enum SandboxMode {
    // Main session: full tool access, no isolation
    Main { elevated: bool },

    // Non-main session: Docker container isolated
    NonMain {
        container_id: String,
        resource_limits: ResourceLimits,
    },
}

pub struct ResourceLimits {
    memory_mb: u64,     // Default: 512MB
    cpu_quota: u64,     // Default: 50% CPU
    network: NetworkMode,
    filesystem: FilesystemMode,
}

pub enum NetworkMode {
    None,               // No network access
    Restricted,         // Whitelist-based
    Full,               // Full internet access
}
```

**Docker Container Configuration:**
```dockerfile
# Base sandbox image
FROM rust:1.85-slim
RUN apt-get update && apt-get install -y \
    curl git python3 nodejs \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /workspace
USER sandbox  # Non-root user
```

**Container Management:**
```rust
use bollard::Docker;
use bollard::container::{Config, CreateContainerOptions};

pub async fn create_sandbox_container(session_id: &str) -> Result<String> {
    let docker = Docker::connect_with_local_defaults()?;

    let config = Config {
        image: Some("aether-sandbox:latest"),
        network_disabled: Some(false),
        host_config: Some(HostConfig {
            memory: Some(512 * 1024 * 1024),  // 512MB
            cpu_quota: Some(50000),  // 50% CPU
            readonly_rootfs: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    };

    let container = docker
        .create_container(
            Some(CreateContainerOptions { name: session_id }),
            config,
        )
        .await?;

    docker.start_container::<String>(&container.id, None).await?;

    Ok(container.id)
}
```

**Permission Matrix:**

| Tool Category | Main Session | Non-Main Session | Elevated |
|--------------|-------------|------------------|----------|
| File Read | ✅ | ✅ (limited paths) | ✅ |
| File Write | ✅ | ❌ | ✅ |
| Shell Execution | ✅ | ❌ | ✅ |
| Browser Control | ✅ | ❌ | ✅ |
| Network Access | ✅ | ✅ (restricted) | ✅ |
| System Info | ✅ | ✅ | ✅ |

### 4.3 Local Tools Integration

**Chrome CDP Controller:**
```rust
use chromiumoxide::Browser;

pub struct ChromeController {
    browser: Arc<Browser>,
}

impl ChromeController {
    pub async fn new() -> Result<Self> {
        let (browser, mut handler) = Browser::launch(
            BrowserConfig::builder()
                .with_head()
                .build()?
        ).await?;

        tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                // Handle browser events
            }
        });

        Ok(Self { browser: Arc::new(browser) })
    }

    pub async fn navigate(&self, url: &str) -> Result<String> {
        let page = self.browser.new_page("about:blank").await?;
        page.goto(url).await?;
        let content = page.content().await?;
        Ok(content)
    }

    pub async fn screenshot(&self, url: &str) -> Result<Vec<u8>> {
        let page = self.browser.new_page(url).await?;
        page.wait_for_navigation().await?;
        let bytes = page.screenshot().await?;
        Ok(bytes)
    }
}
```

**Cron Scheduler:**
```rust
use tokio_cron_scheduler::{JobScheduler, Job};

pub struct CronManager {
    scheduler: Arc<JobScheduler>,
}

impl CronManager {
    pub async fn schedule(&self, cron_expr: &str, agent_id: &str, message: &str) -> Result<Uuid> {
        let job = Job::new_async(cron_expr, {
            let agent_id = agent_id.to_string();
            let message = message.to_string();
            move |_uuid, _l| {
                Box::pin(async move {
                    // Send message to agent
                    gateway_client
                        .send_message(&agent_id, &message)
                        .await
                        .ok();
                })
            }
        })?;

        let job_id = self.scheduler.add(job).await?;
        Ok(job_id)
    }
}
```

**Webhook Listener:**
```rust
use axum::{Router, Json, extract::Path};

pub struct WebhookManager {
    endpoints: Arc<RwLock<HashMap<String, WebhookConfig>>>,
}

impl WebhookManager {
    pub async fn create_endpoint(&self, name: &str, config: WebhookConfig) -> Result<String> {
        let webhook_id = Uuid::new_v4().to_string();
        let url = format!("http://localhost:18790/webhooks/{}", webhook_id);

        self.endpoints.write().await.insert(webhook_id.clone(), config);

        Ok(url)
    }

    pub fn router(&self) -> Router {
        Router::new()
            .route("/webhooks/:id", post(handle_webhook))
    }
}

async fn handle_webhook(
    Path(id): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    // Forward to agent via Gateway
    gateway_client
        .send_message(&id, &format!("Webhook received: {:?}", payload))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({"status": "received"})))
}
```

**Tool Registry:**
```rust
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    categories: HashMap<String, Vec<String>>,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> ToolSchema;
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput>;
}

// Tool registration
impl ToolRegistry {
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.insert(name.clone(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn list_by_category(&self, category: &str) -> Vec<Arc<dyn Tool>> {
        self.categories
            .get(category)
            .map(|names| {
                names.iter()
                    .filter_map(|n| self.tools.get(n).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }
}
```

---

## 5. Implementation Phases

### Phase 1: Gateway Foundation (2-3 weeks)

**Objective:** Establish WebSocket Gateway without disrupting existing functionality.

**Deliverables:**
```
core/src/gateway/
├── server.rs          // WebSocket server (tokio-tungstenite)
├── protocol.rs        // JSON-RPC 2.0 frame definitions
├── event_bus.rs       // Pub/sub event system
├── security/
│   ├── token.rs       // Token generation/validation (Keychain)
│   └── pairing.rs     // Device pairing logic
└── handlers/
    ├── health.rs      // Health check endpoint
    ├── echo.rs        // Test echo handler
    └── version.rs     // Protocol version negotiation
```

**Key Tasks:**
- [ ] Implement WebSocket server on port 18789
- [ ] JSON-RPC 2.0 protocol parser
- [ ] App-Token generation and Keychain storage (macOS)
- [ ] Event bus (tokio broadcast channel)
- [ ] Health check endpoint
- [ ] CLI command: `aether gateway [--daemon]`
- [ ] Integration test: connect + echo

**Verification:**
- [ ] `aether gateway` starts successfully
- [ ] `wscat -c ws://127.0.0.1:18789` connects and handshakes
- [ ] Health check responds correctly
- [ ] Existing UniFFI interfaces still work (no regression)

**Rollback:** Phase 1 is purely additive, no changes to existing code. Can be disabled with feature flag.

---

### Phase 2: Agent Loop Integration (2-3 weeks)

**Objective:** Enable agent_loop to emit streaming events via Gateway.

**Deliverables:**
```
core/src/gateway/
├── event_emitter.rs   // EventEmitter trait
├── router.rs          // Simple AgentRouter (single agent)
└── handlers/
    ├── agent.rs       // agent.run, agent.wait handlers
    └── session.rs     // session.list, session.reset handlers

core/src/components/
└── agent_loop.rs      // Add execute_with_emitter() method
```

**Key Tasks:**
- [ ] Define `EventEmitter` trait
- [ ] Modify `agent_loop.rs` to accept emitter
- [ ] Implement streaming events:
  - `stream.reasoning` (thinking process)
  - `tool.start`, `tool.update`, `tool.end`
  - `response.chunk` (chunked text)
- [ ] Gateway handler: `agent.run`
- [ ] Gateway handler: `sessions.list`, `sessions.reset`
- [ ] Simple routing (single "main" agent)
- [ ] Event sequencing (monotonic seq numbers)

**Verification:**
- [ ] WebSocket client can call `agent.run`
- [ ] Receive streaming events in real-time
- [ ] Events are ordered correctly (seq numbers)
- [ ] Existing UniFFI path still functional (backward compat)

**Rollback:** Compile-time feature flag `--features gateway-mode`

---

### Phase 3: Swift Client Migration (3-4 weeks)

**Objective:** Migrate macOS UI from UniFFI to WebSocket.

**Deliverables:**
```swift
// platforms/macos/Aether/Sources/Gateway/
├── GatewayClient.swift        // WebSocket client (URLSession)
├── ProtocolModels.swift       // JSON-RPC types
├── EventStream.swift          // Event handling (AsyncStream)
└── TokenManager.swift         // App-Token from Keychain

// platforms/macos/Aether/Sources/MultiTurn/
├── UnifiedConversationViewModel.swift  // Subscribe to events
└── Views/
    ├── ReasoningPartView.swift         // Display streaming reasoning
    ├── ToolExecutionView.swift         // Show tool progress
    └── MessageBubble.swift             // Streaming text append
```

**App Startup Flow:**
1. Check if Gateway process is running (`Process.runningProcesses`)
2. If not, launch: `Process().run("aether", args: ["gateway", "--daemon"])`
3. Wait for port 18789 to be available (max 5s, exponential backoff)
4. Connect WebSocket client
5. Authenticate with App-Token from Keychain
6. Initialize UI

**Key Tasks:**
- [ ] Swift `GatewayClient` implementation
- [ ] JSON-RPC request/response models
- [ ] Event stream handling (`AsyncStream<StreamEvent>`)
- [ ] ViewModel subscribes to event stream
- [ ] UI components render streaming events
- [ ] Gateway auto-launch on app startup
- [ ] Auto-reconnect with exponential backoff
- [ ] Feature flag: `LEGACY_MODE` vs `GATEWAY_MODE`

**Verification:**
- [ ] macOS app launches Gateway automatically
- [ ] Real-time display of agent reasoning
- [ ] Tool call progress indicators work
- [ ] Long responses stream smoothly (no jank)
- [ ] Reconnects automatically on Gateway restart

**Rollback:** Keep UniFFI code as `#if LEGACY_MODE`, controlled by build flag.

---

### Phase 4: Multi-Agent & Dispatcher Cleanup (4-5 weeks)

**Objective:** Simplify dispatcher, enable multi-agent routing, add extended features.

**Deliverables:**
```
core/src/gateway/
├── router.rs              // Multi-agent routing (peer/channel/task)
├── agent_instance.rs      // Isolated agent instances
└── session.rs             // Hierarchical SessionKey

core/src/dispatcher/       // REMOVE or deprecate
// Logic migrated to:
// - AgentRouter (routing decisions)
// - SessionManager (session resolution)
// - ExecutionEngine (execution orchestration)

core/src/channels/         // NEW: Multi-channel connectors
├── mod.rs
├── telegram.rs
├── discord.rs
└── slack.rs

core/src/sandbox/          // NEW: Docker isolation
├── mod.rs
└── docker.rs

core/src/tools/            // ENHANCED: Local tools
├── chrome_cdp.rs
├── cron.rs
└── webhook.rs

~/.aether/config.toml      // Multi-agent + extended features config
```

**Key Tasks:**
- [ ] Multi-agent routing logic (peer/channel/task matching)
- [ ] `AgentInstance` isolation (workspace, sessions.db, config)
- [ ] Hierarchical `SessionKey` (Main/PerPeer/Task/Ephemeral)
- [ ] Config file parsing (agents + bindings + extended features)
- [ ] **Remove/deprecate old dispatcher modules** (16 → 3 layers)
- [ ] Migrate logic to Router/SessionManager/ExecutionEngine
- [ ] Channel connectors: Telegram, Discord, Slack (P0)
- [ ] Sandbox manager: Docker container lifecycle
- [ ] Local tools: Chrome CDP, Cron, Webhook
- [ ] Multi-agent integration tests
- [ ] Performance benchmarks

**Configuration Example:**
```toml
[agents.main]
workspace = "~/aether-main"
model = "claude-sonnet-4-5"

[agents.work]
workspace = "~/aether-work"
model = "claude-opus-4-5"

[bindings]
"gui:window1" = "main"
"cli:*" = "work"
"telegram:*" = "main"

[channels.telegram]
enabled = true
token = "${TELEGRAM_BOT_TOKEN}"

[sandbox]
enabled = true
docker_image = "aether-sandbox:latest"

[tools.chrome]
enabled = true
```

**Verification:**
- [ ] Two agents run simultaneously (main + work)
- [ ] Different GUI windows route to different agents
- [ ] Sessions fully isolated (separate workspace)
- [ ] Telegram bot messages route to correct agent
- [ ] Docker sandbox creates/destroys containers
- [ ] Chrome CDP opens browser and takes screenshot
- [ ] Cron schedules and executes task
- [ ] Webhook endpoint receives POST request
- [ ] Codebase reduced by 30%+ LOC (removed dispatcher redundancy)
- [ ] Performance: single-agent latency <= current implementation
- [ ] Performance: multi-agent overhead < 5%

**Rollback:** NOT RECOMMENDED (major refactor). Requires extensive testing before release.

---

### Migration Timeline

| Phase | Duration | Cumulative | Key Milestone |
|-------|----------|------------|---------------|
| Phase 1 | 2-3 weeks | 3 weeks | Gateway runs independently |
| Phase 2 | 2-3 weeks | 6 weeks | WebSocket agent.run works |
| Phase 3 | 3-4 weeks | 10 weeks | macOS UI on WebSocket |
| Phase 4 | 4-5 weeks | 15 weeks | Multi-agent + extended features |

**Testing Buffer:** +1 week between phases for QA and bug fixes
**Total Estimate:** 11-15 weeks (conservative: 15 weeks)

---

## 6. Risk Mitigation

### Risk 1: WebSocket Connection Stability

**Risk:** Gateway crash renders UI unusable; network issues disconnect clients.

**Mitigation:**

1. **Gateway as Daemon:**
```bash
# Install launchd service (macOS)
# KeepAlive=true ensures auto-restart on crash
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "...">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.aether.gateway</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/aether</string>
        <string>gateway</string>
        <string>--daemon</string>
    </array>
    <key>KeepAlive</key>
    <true/>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
```

2. **Client Auto-Reconnect:**
```swift
class GatewayClient {
    var reconnectAttempts = 0
    let maxAttempts = 10

    func handleDisconnection() {
        guard reconnectAttempts < maxAttempts else {
            showError("Gateway unreachable. Please restart.")
            return
        }

        reconnectAttempts += 1
        let delay = min(pow(2.0, Double(reconnectAttempts)), 30.0)

        Timer.scheduledTimer(withTimeInterval: delay, repeats: false) { _ in
            self.connect()
        }
    }
}
```

3. **Health Monitoring:**
- UI polls health endpoint every 30s
- Display warning if Gateway unreachable
- Provide "Restart Gateway" button

### Risk 2: Event Ordering Issues

**Risk:** Concurrent tool execution causes out-of-order events.

**Mitigation:**
```rust
// Add sequence numbers to events
pub struct StreamEvent {
    seq: u64,  // Monotonically increasing per run
    run_id: String,
    event: EventType,
}

// Client-side reordering buffer
class EventBuffer {
    var buffer: [UInt64: StreamEvent] = [:]
    var nextExpectedSeq: UInt64 = 0

    func receive(_ event: StreamEvent) {
        buffer[event.seq] = event

        while let event = buffer.removeValue(forKey: nextExpectedSeq) {
            emit(event)
            nextExpectedSeq += 1
        }
    }
}
```

### Risk 3: Session Isolation Failure

**Risk:** Agent workspaces overlap, session leakage.

**Mitigation:**
```rust
// Strict directory isolation validation
impl AgentInstance {
    fn new(agent_id: &str) -> Result<Self> {
        let agents_dir = dirs::home_dir()
            .ok_or_else(|| anyhow!("No home directory"))?
            .join(".aether/agents");

        let agent_dir = agents_dir.join(agent_id);

        // Ensure isolation
        if !agent_dir.exists() {
            fs::create_dir_all(&agent_dir)?;
        }

        // Validate no path traversal
        if !agent_dir.starts_with(&agents_dir) {
            return Err(anyhow!("Invalid agent directory"));
        }

        // Set restrictive permissions (macOS/Linux)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o700);
            fs::set_permissions(&agent_dir, perms)?;
        }

        Ok(Self {
            agent_id: agent_id.to_string(),
            workspace_dir: agent_dir.join("workspace"),
            session_store: SessionStore::new(agent_dir.join("sessions.db"))?,
            config: load_config(agent_dir.join("config.toml"))?,
        })
    }
}
```

### Risk 4: Performance Regression

**Risk:** WebSocket overhead > UniFFI direct calls.

**Mitigation:**

1. **Benchmarking Requirements:**
```rust
#[bench]
fn bench_uniffi_call() { /* baseline: ~50µs */ }

#[bench]
fn bench_websocket_call() {
    // Must be < 1.5x uniffi baseline (~75µs)
}

#[bench]
fn bench_event_broadcast() {
    // 100 clients, must be < 1ms per event
}
```

2. **Optimization Techniques:**
- Use `simd-json` for 2-3x faster JSON parsing
- Event batching/coalescing (reduce syscalls)
- Binary frames (MessagePack) for large payloads (20-30% size reduction)
- Zero-copy deserialization where possible
- Connection pooling for channel connectors

3. **CI Performance Gates:**
- Automated benchmarks on every PR
- Fail build if >10% regression
- Benchmark results published in PR comments

### Risk 5: Docker Overhead

**Risk:** Container creation latency, resource consumption.

**Mitigation:**
- Pre-warm container pool (keep 2-3 ready)
- Fast container images (<100MB)
- Reuse containers across sessions (clean workspace between uses)
- Fallback to non-sandboxed mode if Docker unavailable

### Risk 6: Channel Connector Failures

**Risk:** External API rate limits, downtime, breaking changes.

**Mitigation:**
- Rate limiting per channel (respect API limits)
- Exponential backoff on errors
- Circuit breaker pattern (disable connector after N failures)
- Graceful degradation (queue messages, retry later)
- Version lock on external SDKs

---

## 7. Success Criteria

### 7.1 Functional Requirements

- [ ] **Multi-Client Support:** 3+ simultaneous WebSocket connections
- [ ] **Streaming UX:** Real-time reasoning/tool progress display
- [ ] **Multi-Agent:** 2+ agents with isolated workspaces
- [ ] **Session Isolation:** No context leakage between agents
- [ ] **Auto-Reconnect:** Client recovers from Gateway restart in <5s
- [ ] **Device Pairing:** Remote connection pairing flow works end-to-end
- [ ] **Channel Connectors:** Telegram + Discord + Slack working
- [ ] **Sandbox Isolation:** Docker containers create/destroy successfully
- [ ] **Local Tools:** Chrome CDP, Cron, Webhook functional

### 7.2 Performance Requirements

- [ ] **Latency:** WebSocket request-response < 1.5x UniFFI baseline
- [ ] **Event Throughput:** 100 events/sec broadcast to 10 clients
- [ ] **Memory:** Gateway RAM usage < 200MB idle, < 500MB under load
- [ ] **CPU:** Gateway CPU < 5% idle, < 20% under load
- [ ] **Container Overhead:** Docker sandbox <100ms startup latency

### 7.3 Code Quality Requirements

- [ ] **LOC Reduction:** 30% fewer lines vs current dispatcher
- [ ] **Test Coverage:** 80% coverage on gateway/ module
- [ ] **Documentation:** GATEWAY_PROTOCOL.md, MIGRATION_GUIDE.md
- [ ] **No Regressions:** All existing tests pass

### 7.4 User Experience Requirements

- [ ] **Startup Time:** macOS app launch < 2s (including Gateway start)
- [ ] **Error Messages:** Clear diagnostics on connection failure
- [ ] **Diagnostics:** `aether doctor` command for troubleshooting
- [ ] **Streaming Smoothness:** No UI jank during long responses

---

## 8. Appendices

### Appendix A: Moltbot Reference Architecture

**Key Learnings from Moltbot:**

| Aspect | Moltbot Approach | Aether Adaptation |
|--------|------------------|-------------------|
| **Control Plane** | Single Gateway WebSocket | Direct port |
| **Local-First** | 127.0.0.1 bind, optional Tailscale | Same |
| **Multi-Agent** | Workspace + routing bindings | Simplified config |
| **Session Management** | Main/PerPeer/PerChannelPeer keys | Extended hierarchy |
| **Streaming** | Block streaming (200-2000 chars) | Same + code fence rules |
| **Authentication** | Gateway token + device pairing | Same |
| **Persistence** | Agent-level SQLite stores | Same |
| **Daemon Management** | launchd/systemd services | launchd (macOS) |

**Moltbot Links:**
- **GitHub:** https://github.com/moltbot/moltbot
- **Local Path:** `/Users/zouguojun/Workspace/moltbot`
- **Documentation:** https://docs.molt.bot
- **Protocol:** WebSocket at `ws://127.0.0.1:18789`

### Appendix B: Technology Stack

**Rust Crates:**

| Category | Crates | Purpose |
|----------|--------|---------|
| **WebSocket** | `tokio-tungstenite` | Async WebSocket server |
| **JSON-RPC** | `jsonrpc-core` | Protocol implementation |
| **Serialization** | `serde_json`, `rmp-serde` | JSON + MessagePack |
| **Docker** | `bollard` | Docker API client |
| **Browser** | `chromiumoxide` | Chrome DevTools Protocol |
| **Scheduler** | `tokio-cron-scheduler` | Cron job scheduling |
| **HTTP** | `axum` | Webhook listener |
| **Telegram** | `teloxide` | Telegram Bot API |
| **Discord** | `serenity` | Discord Bot API |
| **Slack** | `slack-morphism` | Slack SDK |
| **Security** | `jsonwebtoken`, `rand` | JWT, token generation |
| **Rate Limiting** | `governor` | Per-client rate limits |
| **Compression** | `zstd` | Binary frame compression |

**Swift Dependencies:**
- `URLSession` - WebSocket client (native)
- `Combine` - Reactive event streams
- `Security.framework` - Keychain access

### Appendix C: File Structure

```
aether/
├── core/
│   └── src/
│       ├── gateway/                  # NEW: WebSocket Gateway
│       │   ├── server.rs
│       │   ├── protocol.rs
│       │   ├── event_bus.rs
│       │   ├── router.rs
│       │   ├── session.rs
│       │   ├── agent_instance.rs
│       │   └── security/
│       ├── channels/                 # NEW: Multi-channel
│       │   ├── telegram.rs
│       │   ├── discord.rs
│       │   └── slack.rs
│       ├── sandbox/                  # NEW: Docker isolation
│       │   └── docker.rs
│       ├── tools/                    # ENHANCED
│       │   ├── chrome_cdp.rs
│       │   ├── cron.rs
│       │   └── webhook.rs
│       ├── components/
│       │   └── agent_loop.rs        # MODIFIED: event emission
│       └── dispatcher/               # DEPRECATED/REMOVED
├── platforms/
│   └── macos/
│       └── Aether/Sources/
│           ├── Gateway/             # NEW: WebSocket client
│           │   ├── GatewayClient.swift
│           │   ├── ProtocolModels.swift
│           │   └── EventStream.swift
│           └── MultiTurn/
│               └── Views/           # MODIFIED: streaming UI
└── docs/
    ├── GATEWAY_PROTOCOL.md          # NEW
    ├── MIGRATION_GUIDE.md           # NEW
    └── ARCHITECTURE.md              # UPDATED
```

### Appendix D: Configuration Schema

```toml
# ~/.aether/config.toml

[gateway]
host = "127.0.0.1"
port = 18789
max_connections = 100
protocol_version = 2

[agents.main]
workspace = "~/aether-main"
model = "claude-sonnet-4-5"
fallback_models = ["claude-opus-4-5", "gpt-4-turbo"]
max_loops = 20

[agents.work]
workspace = "~/aether-work"
model = "claude-opus-4-5"

[bindings]
"gui:window1" = "main"
"cli:*" = "work"
"hotkey:global" = "main"
"telegram:*" = "main"

[channels.telegram]
enabled = true
token = "${TELEGRAM_BOT_TOKEN}"
route_to_agent = "main"

[channels.discord]
enabled = true
token = "${DISCORD_BOT_TOKEN}"

[channels.slack]
enabled = false

[sandbox]
enabled = true
docker_image = "aether-sandbox:latest"
memory_limit_mb = 512
cpu_quota_percent = 50
network_mode = "restricted"

[tools.chrome]
enabled = true
executable_path = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

[tools.cron]
enabled = true

[tools.webhook]
enabled = true
port = 18790
```

### Appendix E: Glossary

| Term | Definition |
|------|------------|
| **Gateway** | WebSocket control plane coordinating all components |
| **Agent Instance** | Isolated agent execution environment (workspace + sessions) |
| **SessionKey** | Hierarchical identifier (Main/PerPeer/Task/Ephemeral) |
| **Channel Connector** | Integration module for external messaging platforms |
| **Sandbox** | Docker-isolated execution environment for untrusted contexts |
| **Event Bus** | Pub/sub system for real-time event streaming |
| **RPC** | Remote Procedure Call (JSON-RPC 2.0) |
| **CDP** | Chrome DevTools Protocol |
| **UniFFI** | Uniform FFI (legacy communication layer) |

---

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-01-28 | 1.0 | Unified design combining architecture redesign + implementation plan |

---

**Document Status:** Ready for Review
**Next Steps:** Technical validation → Phase 1 kickoff
**Approvers:** Architecture team, Platform leads

**End of Document**
