# Moltbot-Inspired Architecture Redesign for Aleph

**Date:** 2026-01-28
**Status:** Design Phase
**Author:** AI-assisted design session

---

## Executive Summary

This document outlines a comprehensive architectural redesign of Aleph, inspired by [moltbot](https://github.com/moltbot/moltbot)'s proven design patterns. The redesign addresses four critical pain points:

1. **Single Agent Bottleneck** - No multi-agent parallelization or specialization
2. **Dispatcher Complexity** - 16 tightly-coupled sub-modules difficult to maintain
3. **Session Isolation Gaps** - Context contamination between tasks
4. **Communication Layer Chaos** - FFI blocking, no streaming, poor multi-client support

**Approach:** Hybrid refactoring strategy
- **Aggressive:** WebSocket Gateway control plane (replace UniFFI)
- **Incremental:** Agent Loop enhancements, Multi-Agent routing
- **Targeted:** Dispatcher simplification (remove redundancy)

**Timeline:** 11-15 weeks across 4 phases
**Risk Level:** Medium (mitigated by phased rollout with rollback capability)

---

## Table of Contents

1. [Background & Motivation](#1-background--motivation)
2. [Architecture Overview](#2-architecture-overview)
3. [Core Components](#3-core-components)
4. [Technical Improvements](#4-technical-improvements)
5. [Migration Path](#5-migration-path)
6. [Risk Mitigation](#6-risk-mitigation)
7. [Success Criteria](#7-success-criteria)

---

## 1. Background & Motivation

### 1.1 Current Architecture Pain Points

#### Pain Point 1: Single Agent Bottleneck
**Current State:**
- Single `agent_loop` instance processes all requests
- No ability to run specialized agents for different contexts (work vs personal)
- Cannot leverage parallel execution across multiple agents

**Impact:**
- All tasks share same context/memory
- No isolation for different user personas
- Missed parallelization opportunities

#### Pain Point 2: Dispatcher Complexity
**Current State:**
- 16 sub-modules: planner, scheduler, executor, model_router, skill_router, tool_router, etc.
- Unclear boundaries between components
- Difficult to trace request flow through the system

**Impact:**
- High maintenance cost
- Onboarding friction for new contributors
- Redundant logic across modules

#### Pain Point 3: Session Isolation Gaps
**Current State:**
- `topic_id` as flat identifier
- No hierarchical session management
- Different tasks can pollute shared context

**Impact:**
- Context leakage between unrelated tasks
- No lifecycle management (auto-reset, expiration)
- Difficult to implement per-task sandboxing

#### Pain Point 4: Communication Layer Chaos
**Current State:**
- UniFFI synchronous FFI calls from Swift to Rust
- No streaming capability (UI blocks until completion)
- Each platform (macOS/Tauri) implements own communication logic

**Impact:**
- Poor user experience (no progress feedback)
- Duplicate code across platforms
- Cannot support multi-client scenarios (simultaneous connections)

### 1.2 Moltbot's Solutions

Moltbot addresses these exact challenges through:

| Challenge | Moltbot Solution | Aleph Adoption |
|-----------|------------------|-----------------|
| Communication | WebSocket Gateway control plane | ✅ Direct port |
| Multi-Agent | Workspace isolation + routing bindings | ✅ Adapt to Aleph's needs |
| Session Management | Hierarchical SessionKey (Main/PerPeer/Task) | ✅ Extend current model |
| Streaming | Block streaming + tool streaming | ✅ Essential for UX |
| Architecture | Flat (Gateway → Agent → Tools) | ✅ Simplify Dispatcher |

---

## 2. Architecture Overview

### 2.1 Target Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    WebSocket Gateway (Rust)                 │
│  • ws://127.0.0.1:18789 (local-first)                      │
│  • JSON-RPC 2.0 + Binary frames (MessagePack)              │
│  • Token auth + device pairing                             │
│  • Event bus (pub/sub for streaming)                       │
└─────────────────────────────────────────────────────────────┘
           ↓ WS                ↓ WS              ↓ WS
    ┌─────────────┐     ┌─────────────┐    ┌─────────────┐
    │   macOS UI  │     │  Tauri UI   │    │  CLI Client │
    │   (Swift)   │     │  (React)    │    │   (Rust)    │
    └─────────────┘     └─────────────┘    └─────────────┘

        Gateway Internal Architecture
        ┌──────────────────────────────┐
        │      AgentRouter             │ ← Route requests to agents
        ├──────────────────────────────┤
        │    SessionManager            │ ← Resolve sessions
        ├──────────────────────────────┤
        │  ExecutionEngine             │ ← Execute agent loops
        │    ├─ AgentInstance(main)    │
        │    └─ AgentInstance(work)    │
        └──────────────────────────────┘
```

### 2.2 Key Architectural Principles

1. **Local-First:** Gateway binds to 127.0.0.1 by default, optional remote access via Tailscale
2. **Single Control Plane:** All state owned by Gateway, clients are stateless
3. **Agent Isolation:** Each agent has independent workspace, session store, config
4. **Streaming by Default:** All operations emit events for real-time feedback
5. **Protocol Standardization:** JSON-RPC 2.0 for interoperability

---

## 3. Core Components

### 3.1 WebSocket Gateway

**Responsibilities:**
- Accept WebSocket connections from clients
- Authenticate via App-Token (local) or Device-Token (remote)
- Route requests to appropriate agents
- Broadcast events to subscribed clients
- Manage device pairing for remote connections

**Protocol:**
```rust
// JSON-RPC 2.0 Request
{
  "jsonrpc": "2.0",
  "id": "uuid-123",
  "method": "agent.run",
  "params": {
    "input": "Hello, Aleph",
    "session_key": "agent:main:main"
  }
}

// JSON-RPC 2.0 Response
{
  "jsonrpc": "2.0",
  "id": "uuid-123",
  "result": {
    "run_id": "run-456",
    "accepted_at": "2026-01-28T10:30:00Z"
  }
}

// Event Notification (unidirectional)
{
  "jsonrpc": "2.0",
  "method": "stream.reasoning",
  "params": {
    "run_id": "run-456",
    "content": "I need to analyze this request...",
    "is_complete": false
  }
}
```

**Security:**
- **App-Token:** 256-bit random token stored in macOS Keychain
- **Device-Token:** JWT with device_id, scopes, expiration
- **Pairing Flow:** Remote devices receive 6-digit approval code, operator approves via UI

### 3.2 Multi-Agent Router

**Routing Strategy (priority order):**
1. **Peer Match** - Exact DM/group/channel ID
2. **Channel Match** - Input source (GUI window, CLI, hotkey)
3. **Task Match** - Task type (cron, webhook)
4. **Fallback** - Default agent ("main")

**Agent Instance Isolation:**
```
~/.aleph/
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
# ~/.aleph/config.toml

[agents.main]
workspace = "~/aleph-main"
model = "claude-sonnet-4.5"

[agents.work]
workspace = "~/aleph-work"
model = "claude-opus-4.5"

[bindings]
"gui:window1" = "main"
"cli:*" = "work"
"hotkey:global" = "main"
```

### 3.3 Hierarchical Session Management

**Session Key Types:**
```rust
pub enum SessionKey {
    // Main session (cross-device shared)
    Main {
        agent_id: String,
        main_key: String  // Default: "main"
    },

    // Per-peer isolation (different GUI windows)
    PerPeer {
        agent_id: String,
        peer_id: String  // Window ID, chat ID, etc.
    },

    // Task isolation (cron jobs, webhooks)
    Task {
        agent_id: String,
        task_type: String,  // "cron", "webhook", etc.
        task_id: String
    },

    // Ephemeral (single-turn, no persistence)
    Ephemeral {
        agent_id: String
    },
}

// Example keys:
// "agent:main:main"
// "agent:work:peer:window-abc"
// "agent:main:cron:daily-summary"
// "agent:main:ephemeral:uuid"
```

**Lifecycle Management:**
- **Daily Reset:** 4:00 AM local time (configurable)
- **Idle Reset:** N minutes after last interaction (optional)
- **Manual Reset:** `/new`, `/reset` commands
- **Smart Compaction:** Auto-compress when context exceeds limit (instead of reset)

### 3.4 Streaming Architecture

**Event Types:**
```rust
pub enum StreamEvent {
    // Reasoning process (thinking)
    Reasoning {
        run_id: String,
        content: String,
        is_complete: bool
    },

    // Tool execution lifecycle
    ToolStart {
        run_id: String,
        tool_name: String,
        params: Value
    },
    ToolUpdate {
        run_id: String,
        tool_id: String,
        progress: String  // "Reading file... 50%"
    },
    ToolEnd {
        run_id: String,
        tool_id: String,
        result: ToolResult
    },

    // Response streaming
    ResponseChunk {
        run_id: String,
        content: String,
        chunk_index: u32,
        is_final: bool
    },

    // Run lifecycle
    RunComplete {
        run_id: String,
        summary: RunSummary
    },
    RunError {
        run_id: String,
        error: String
    },
}
```

**Block Chunking Strategy (from Moltbot):**
- **Min chars:** 200 (buffer before sending)
- **Max chars:** 2000 (hard limit, force send)
- **Break preference:** paragraph → newline → sentence → whitespace
- **Code fence rule:** Never split inside ``` blocks; close and reopen if forced
- **Coalescing:** Wait 500ms idle before flush (merge small chunks)

### 3.5 Dispatcher Simplification

**Current (16 modules) → Target (3 layers):**

| Old Module | New Home | Rationale |
|------------|----------|-----------|
| `planner` | `AgentRouter` | Simple rule-based routing |
| `scheduler` | Remove (session lane queuing) | Per-session serialization |
| `executor` | `ExecutionEngine` | Merge with agent_loop |
| `model_router` | `agent_loop` internal | Loop decides model |
| `skill_router` | `agent_loop` internal | Loop loads skills |
| `tool_router` | `ToolRegistry` (simplified) | Discovery only |
| `sub_agents` | Keep & enhance | Multi-agent support |

**New 3-Layer Architecture:**
```rust
pub struct Gateway {
    router: AgentRouter,           // "Who handles this?"
    session_manager: SessionManager, // "Where to execute?"
    execution_engine: ExecutionEngine, // "How to execute?"
}
```

---

## 4. Technical Improvements

### 4.1 Security: Token Auth & Device Pairing

**App-Token (Local Connections):**
```rust
// Generated on first launch, stored in Keychain
pub struct TokenManager {
    pub fn ensure_app_token(&self) -> Result<String> {
        // Try read from keychain
        // If not exists: generate 256-bit token, store
        // Return token
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
```

**Pairing Flow:**
1. Remote device connects without token
2. Gateway generates 6-digit approval code
3. Code displayed on device, broadcast to operator clients
4. Operator approves via UI
5. Gateway issues Device-Token (JWT)
6. Device stores token for future connections

### 4.2 Protocol: JSON-RPC 2.0 + Binary Frames

**Text Frame (JSON-RPC 2.0):**
- Standard for request/response
- Human-readable for debugging
- Wide tooling support

**Binary Frame (MessagePack + Compression):**
```rust
// Frame structure:
// [4-byte header][payload]

pub struct BinaryFrameHeader {
    frame_type: u8,     // 0x01=MessagePack, 0x02=Raw binary
    compression: u8,    // 0x00=None, 0x01=Zstd
    reserved: u16,
}

// Use binary for:
// - Large responses (>1KB): 20-30% size reduction
// - File uploads/downloads
// - Image data
```

**Content Negotiation:**
```rust
// Client declares capabilities in connect request
pub struct ClientCapabilities {
    encodings: Vec<Encoding>,      // ["json", "msgpack"]
    compressions: Vec<String>,     // ["zstd"]
    binary_frames: bool,
}

// Gateway selects best option
pub struct ConnectResponse {
    selected_encoding: Encoding,
    selected_compression: Option<String>,
}
```

### 4.3 Protocol Versioning

```rust
pub const PROTOCOL_VERSION_MIN: u32 = 1;
pub const PROTOCOL_VERSION_MAX: u32 = 2;

// Negotiation in connect handshake
pub fn negotiate(client_min: u32, client_max: u32) -> Result<u32> {
    // Find intersection of client and server ranges
    // Return highest compatible version
    // Error if no overlap
}

// Version-specific handlers
impl GatewayServer {
    async fn handle_request(&self, req: JsonRpcRequest, version: u32) {
        match version {
            1 => self.handle_v1(req).await,
            2 => self.handle_v2(req).await,
            _ => error!("Unsupported version"),
        }
    }
}
```

### 4.4 Rate Limiting & Backpressure

**Per-Client Rate Limiting:**
```rust
use governor::{Quota, RateLimiter};

// 100 requests/second per client
let limiter = RateLimiter::keyed(Quota::per_second(100));

if limiter.check_key(&client_id).is_err() {
    return Err("Rate limit exceeded");
}
```

**Event Stream Backpressure:**
```rust
// If client falls behind, slow down event emission
pub struct EventStreamBackpressure {
    buffer_size: AtomicUsize,
    max_buffer: usize,  // e.g., 1000 events
}

// Block if buffer full
if buffer_size > max_buffer {
    warn!("Client {} lagging, applying backpressure", client_id);
    sleep(100ms).await;
}
```

### 4.5 Health Check & Diagnostics

**Health Endpoint:**
```json
// GET /health (or JSON-RPC method "system.health")
{
  "status": "healthy",
  "uptime_seconds": 86400,
  "connected_clients": 3,
  "active_agents": 2,
  "protocol_version": 2
}
```

**Diagnostics CLI:**
```bash
$ aleph doctor

🔍 Aleph Diagnostics

✓ Gateway process: Running
✓ WebSocket connection: OK
✓ App token: Valid
✓ Port 18789: Available

📊 Detailed Stats:
  - Connected clients: 3 (2 macOS, 1 CLI)
  - Active sessions: 5
  - Memory usage: 142 MB
  - CPU: 2.3%
```

---

## 5. Migration Path

### Phase 1: Gateway Foundation (2-3 weeks)

**Objective:** Establish WebSocket Gateway without disrupting existing functionality

**Deliverables:**
```
core/src/gateway/
├── server.rs          // WebSocket server
├── protocol.rs        // JSON-RPC frame definitions
├── event_bus.rs       // Pub/sub event system
├── security/
│   ├── token.rs       // Token generation/validation
│   └── pairing.rs     // Device pairing logic
└── handlers/
    ├── health.rs      // Health check
    └── echo.rs        // Test echo handler
```

**Verification:**
- [ ] `aleph gateway` starts successfully
- [ ] wscat can connect and handshake
- [ ] Health check responds correctly
- [ ] Existing UniFFI interfaces still work (no regression)

**Rollback:** Phase 1 is purely additive, no changes to existing code

---

### Phase 2: Agent Loop Integration (2-3 weeks)

**Objective:** Enable agent_loop to emit streaming events via Gateway

**Deliverables:**
```
core/src/gateway/
├── event_emitter.rs   // Trait for event emission
└── handlers/
    ├── agent.rs       // agent.run, agent.wait handlers
    └── session.rs     // session.list, session.reset handlers

core/src/components/
└── agent_loop.rs      // Add execute_with_emitter() method

core/src/gateway/
└── router.rs          // Simple router (single agent for now)
```

**Verification:**
- [ ] WebSocket client can call `agent.run`
- [ ] Receive streaming events: `stream.reasoning`, `tool.start`, `response.chunk`
- [ ] Existing UniFFI path still functional (backward compat)

**Rollback:** Compile-time feature flag `--features gateway-mode`

---

### Phase 3: Swift Client Migration (3-4 weeks)

**Objective:** Migrate macOS UI from UniFFI to WebSocket

**Deliverables:**
```swift
// platforms/macos/Aleph/Sources/Gateway/
├── GatewayClient.swift        // WebSocket client
├── ProtocolModels.swift       // JSON-RPC types
└── EventStream.swift          // Event handling

// platforms/macos/Aleph/Sources/MultiTurn/
├── UnifiedConversationViewModel.swift  // Subscribe to events
└── Views/
    ├── ReasoningPartView.swift         // Display streaming reasoning
    ├── ToolExecutionView.swift         // Show tool progress
    └── MessageBubble.swift             // Streaming text append
```

**App Startup Flow:**
1. Check if Gateway process is running (`pgrep aleph`)
2. If not, launch: `Process().run("aleph", args: ["gateway", "--daemon"])`
3. Wait for port 18789 to be available (max 5s)
4. Connect WebSocket client
5. Initialize UI

**Verification:**
- [ ] macOS app launches Gateway automatically
- [ ] Real-time display of agent reasoning
- [ ] Tool call progress indicators work
- [ ] Long responses stream smoothly (no jank)

**Rollback:** Keep UniFFI code as `#if LEGACY_MODE`, controlled by build flag

---

### Phase 4: Multi-Agent & Dispatcher Cleanup (4-5 weeks)

**Objective:** Simplify dispatcher, enable multi-agent routing

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

~/.aleph/config.toml      // Multi-agent configuration support
```

**Configuration Example:**
```toml
[agents.main]
workspace = "~/aleph-main"

[agents.work]
workspace = "~/aleph-work"

[bindings]
"gui:window1" = "main"
"cli:*" = "work"
```

**Verification:**
- [ ] Two agents run simultaneously (main + work)
- [ ] Different GUI windows route to different agents
- [ ] Sessions fully isolated (separate workspace)
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
| Phase 4 | 4-5 weeks | 15 weeks | Multi-agent + dispatcher removal |

**Testing Buffer:** +1 week between phases for QA and bug fixes

**Total Estimate:** 11-15 weeks (conservative: 15 weeks)

---

## 6. Risk Mitigation

### Risk 1: WebSocket Connection Stability

**Risk:** Gateway crash renders UI unusable; network issues disconnect clients

**Mitigation:**
1. **Gateway as Daemon:**
   ```bash
   # Install launchd service (macOS)
   # KeepAlive=true ensures auto-restart on crash
   launchctl load ~/Library/LaunchAgents/com.aleph.gateway.plist
   ```

2. **Client Auto-Reconnect:**
   ```swift
   class GatewayClient {
       func handleDisconnection() {
           reconnectAttempts += 1
           let delay = min(pow(2.0, Double(reconnectAttempts)), 30.0)
           Timer.schedule(delay) { self.connect() }
       }
   }
   ```

3. **Health Monitoring:**
   - UI polls health endpoint every 30s
   - Display warning if Gateway unreachable
   - Provide "Restart Gateway" button

### Risk 2: Event Ordering Issues

**Risk:** Concurrent tool execution causes out-of-order events

**Mitigation:**
```rust
// Add sequence numbers to events
pub struct StreamEvent {
    seq: u64,  // Monotonically increasing
    run_id: String,
    event: EventType,
}

// Client-side reordering buffer
class EventBuffer {
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

**Risk:** Agent workspaces overlap, session leakage

**Mitigation:**
```rust
// Strict directory isolation validation
impl AgentInstance {
    fn new(agent_id: &str) -> Result<Self> {
        let agent_dir = dirs::home_dir()
            .unwrap()
            .join(".aleph/agents")
            .join(agent_id);

        // Ensure isolation
        if !agent_dir.exists() {
            fs::create_dir_all(&agent_dir)?;
        }

        // Validate no path traversal
        if !agent_dir.starts_with(dirs::home_dir().unwrap().join(".aleph/agents")) {
            return Err(anyhow!("Invalid agent directory"));
        }

        Ok(Self {
            workspace_dir: agent_dir.join("workspace"),
            session_store: SessionStore::new(agent_dir.join("sessions.db"))?,
            // ...
        })
    }
}
```

### Risk 4: Performance Regression

**Risk:** WebSocket overhead > UniFFI direct calls

**Mitigation:**
1. **Benchmarking Requirements:**
   ```rust
   #[bench]
   fn bench_uniffi_call() { /* baseline */ }

   #[bench]
   fn bench_websocket_call() {
       // Must be < 1.5x uniffi baseline
   }

   #[bench]
   fn bench_event_broadcast() {
       // 100 clients, must be < 1ms per event
   }
   ```

2. **Optimization Techniques:**
   - Use `simd-json` for 2-3x faster JSON parsing
   - Event batching/coalescing (reduce syscalls)
   - Binary frames (MessagePack) for large payloads
   - Zero-copy deserialization where possible

3. **CI Performance Gates:**
   - Automated benchmarks on every PR
   - Fail build if >10% regression

---

## 7. Success Criteria

### Functional Requirements

- [ ] **Multi-Client Support:** 3+ simultaneous WebSocket connections
- [ ] **Streaming UX:** Real-time reasoning/tool progress display
- [ ] **Multi-Agent:** 2+ agents with isolated workspaces
- [ ] **Session Isolation:** No context leakage between agents
- [ ] **Auto-Reconnect:** Client recovers from Gateway restart in <5s
- [ ] **Device Pairing:** Remote connection pairing flow works end-to-end

### Performance Requirements

- [ ] **Latency:** WebSocket request-response < 1.5x UniFFI baseline
- [ ] **Event Throughput:** 100 events/sec broadcast to 10 clients
- [ ] **Memory:** Gateway RAM usage < 200MB idle, < 500MB under load
- [ ] **CPU:** Gateway CPU < 5% idle, < 20% under load

### Code Quality Requirements

- [ ] **LOC Reduction:** 30% fewer lines vs current dispatcher
- [ ] **Test Coverage:** 80% coverage on gateway/ module
- [ ] **Documentation:** GATEWAY.md protocol spec, MIGRATION_GUIDE.md
- [ ] **No Regressions:** All existing tests pass

### User Experience Requirements

- [ ] **Startup Time:** macOS app launch < 2s (including Gateway start)
- [ ] **Error Messages:** Clear diagnostics on connection failure
- [ ] **Diagnostics:** `aleph doctor` command for troubleshooting

---

## Appendix A: Moltbot Reference Architecture

### Key Learnings from Moltbot

| Aspect | Moltbot Approach | Aleph Adaptation |
|--------|------------------|-------------------|
| **Control Plane** | Single Gateway WebSocket | Direct port |
| **Local-First** | 127.0.0.1 bind, optional Tailscale | Same |
| **Multi-Agent** | Workspace + routing bindings | Simplified config |
| **Session Management** | Main/PerPeer/PerChannelPeer keys | Extended hierarchy |
| **Streaming** | Block streaming + Telegram drafts | Block streaming only |
| **Authentication** | Gateway token + device pairing | Same |
| **Persistence** | Agent-level SQLite stores | Same |
| **Daemon Management** | launchd/systemd services | launchd (macOS) |

### Moltbot Links

- **GitHub:** https://github.com/moltbot/moltbot
- **Documentation:** https://docs.molt.bot
- **Protocol Spec:** Gateway WebSocket at ws://127.0.0.1:18789
- **Community:** 71k+ GitHub stars, active development

---

## Appendix B: Implementation Checklist

### Phase 1 Checklist
- [ ] Implement WebSocket server (tokio-tungstenite)
- [ ] Define JSON-RPC 2.0 frame types
- [ ] Implement event bus (pub/sub)
- [ ] Token generation (Keychain storage)
- [ ] Health check handler
- [ ] CLI command: `aleph gateway`
- [ ] Integration test: connect + echo
- [ ] Documentation: GATEWAY.md basics

### Phase 2 Checklist
- [ ] EventEmitter trait
- [ ] Modify agent_loop for event emission
- [ ] Gateway handler: agent.run
- [ ] Gateway handler: session.list, session.reset
- [ ] Simple AgentRouter (single agent)
- [ ] Integration test: streaming events
- [ ] Backward compat test: UniFFI still works

### Phase 3 Checklist
- [ ] Swift GatewayClient implementation
- [ ] JSON-RPC models in Swift
- [ ] ViewModel event subscription
- [ ] ReasoningPartView streaming display
- [ ] ToolExecutionView progress UI
- [ ] MessageBubble streaming append
- [ ] App startup: launch Gateway if needed
- [ ] Auto-reconnect logic
- [ ] Feature flag: LEGACY_MODE vs GATEWAY_MODE
- [ ] UI/UX testing

### Phase 4 Checklist
- [ ] Multi-agent routing logic
- [ ] AgentInstance isolation
- [ ] Hierarchical SessionKey
- [ ] Config file parsing (agents + bindings)
- [ ] Remove/deprecate old dispatcher modules
- [ ] Migrate logic to Router/SessionManager/ExecutionEngine
- [ ] Multi-agent integration tests
- [ ] Performance benchmarks
- [ ] Code cleanup (remove dead code)
- [ ] Documentation updates

### Post-Migration Checklist
- [ ] Update ARCHITECTURE.md
- [ ] Write MIGRATION_GUIDE.md
- [ ] Update BUILD_COMMANDS.md
- [ ] Create GATEWAY_PROTOCOL.md spec
- [ ] Performance report
- [ ] User-facing changelog
- [ ] Blog post / release notes

---

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-01-28 | 1.0 | Initial design document |

---

**End of Document**
