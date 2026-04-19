# Aleph Architecture

> Complete system architecture overview

> **Terminology:** See [GLOSSARY.md](./GLOSSARY.md) for canonical Anthropic-aligned definitions of Harness, Sandbox, Session, Tools, Orchestrator, and AcpAdapter.

---

## System Diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           INTERFACE LAYER (I/O)                               │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐      │
│  │ macOS    │  │  Tauri   │  │   CLI    │  │ Telegram │  │ Discord  │      │
│  │  App     │  │   App    │  │          │  │Interface │  │Interface │      │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘      │
│       │             │             │             │             │             │
│       └─────────────┴─────────────┴─────────────┴─────────────┘             │
│                                   │                                          │
│                          WebSocket (JSON-RPC 2.0)                           │
│                          ws://127.0.0.1:18790/ws                             │
└───────────────────────────────────┬─────────────────────────────────────────┘
                                    │
┌───────────────────────────────────┴─────────────────────────────────────────┐
│                              GATEWAY LAYER                                   │
│                         (Control Plane + Routing)                           │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐        │
│  │   Router    │  │  Session    │  │   Event     │  │  Security   │        │
│  │  (JSON-RPC) │  │  Manager    │  │    Bus      │  │  (Auth)     │        │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘        │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐        │
│  │ Interface   │  │   Config    │  │  Webhooks   │  │    Cron     │        │
│  │  Registry   │  │ Hot Reload  │  │             │  │  Scheduler  │        │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘        │
└───────────────────────────────────┬─────────────────────────────────────────┘
                                    │
┌───────────────────────────────────┴─────────────────────────────────────────┐
│                              AGENT LAYER                                     │
│                      (Observe-Think-Act-Feedback Loop)                      │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                         Agent Loop                                   │   │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐   │   │
│  │  │ Observe │→ │  Think  │→ │   Act   │→ │Feedback │→ │ Compress│   │   │
│  │  │ (Input) │  │(Thinker)│  │(Execute)│  │ (Eval)  │  │ (Memory)│   │   │
│  │  └─────────┘  └─────────┘  └─────────┘  └─────────┘  └─────────┘   │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐                      │
│  │  Dispatcher  │  │    Guards    │  │   Overflow   │                      │
│  │ (Orchestrate)│  │  (Safety)    │  │  Detector    │                      │
│  └──────────────┘  └──────────────┘  └──────────────┘                      │
└───────────────────────────────────┬─────────────────────────────────────────┘
                                    │
┌───────────────────────────────────┴─────────────────────────────────────────┐
│                            EXECUTION LAYER                                   │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐        │
│  │   Thinker   │  │  Executor   │  │    Tool     │  │    Exec     │        │
│  │ (LLM Call)  │  │ (Tool Run)  │  │   Server    │  │  (Shell)    │        │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘        │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐        │
│  │  Providers  │  │  Builtin    │  │    MCP      │  │  Extension  │        │
│  │ (AI APIs)   │  │   Tools     │  │   Client    │  │  (Plugins)  │        │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘        │
└───────────────────────────────────┬─────────────────────────────────────────┘
                                    │
┌───────────────────────────────────┴─────────────────────────────────────────┐
│                            STORAGE LAYER                                     │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌──────────────────────┐  ┌────────────────┐  ┌─────────────────┐          │
│  │  Memory (LanceDB)    │  │ Resilience     │  │  Config Store   │          │
│  │  ┌──────┐ ┌───────┐  │  │   (SQLite)     │  │  ┌─────┐┌────┐ │          │
│  │  │Facts │ │ Graph │  │  │  ┌──────────┐  │  │  │TOML ││Keys│ │          │
│  │  │+Vec  │ │ Nodes │  │  │  │  State   │  │  │  │File ││    │ │          │
│  │  │+FTS  │ │ Edges │  │  │  │ Database │  │  │  └─────┘└────┘ │          │
│  │  └──────┘ └───────┘  │  │  └──────────┘  │  └─────────────────┘          │
│  └──────────────────────┘  └────────────────┘                               │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Module Dependencies

```
                    ┌─────────────┐
                    │   gateway   │ ← Entry point (feature-gated)
                    └──────┬──────┘
                           │
          ┌────────────────┼────────────────┐
          │                │                │
          ▼                ▼                ▼
    ┌───────────┐    ┌───────────┐    ┌───────────┐
    │interfaces │    │  routing  │    │ handlers  │
    └───────────┘    └───────────┘    └─────┬─────┘
                                            │
                           ┌────────────────┼────────────────┐
                           │                │                │
                           ▼                ▼                ▼
                     ┌───────────┐    ┌───────────┐    ┌───────────┐
                     │agent_loop │    │  memory   │    │   exec    │
                     └─────┬─────┘    └───────────┘    └───────────┘
                           │
          ┌────────────────┼────────────────┐
          │                │                │
          ▼                ▼                ▼
    ┌───────────┐    ┌───────────┐    ┌───────────┐
    │  thinker  │    │ dispatcher│    │  executor │
    └─────┬─────┘    └───────────┘    └─────┬─────┘
          │                                  │
          ▼                                  ▼
    ┌───────────┐                      ┌───────────┐
    │ providers │                      │   tools   │
    └───────────┘                      └─────┬─────┘
                                             │
                           ┌─────────────────┼─────────────────┐
                           │                 │                 │
                           ▼                 ▼                 ▼
                     ┌───────────┐     ┌───────────┐     ┌───────────┐
                     │  builtin  │     │    mcp    │     │ extension │
                     │   tools   │     │  client   │     │ (plugins) │
                     └───────────┘     └───────────┘     └───────────┘
```

---

## Data Flow

### Request Processing

```
Client Request (JSON-RPC)
    │
    ▼
┌─────────────────────────────────────────────────────┐
│ Gateway: InboundRouter                              │
│   • Parse JSON-RPC message                          │
│   • Route to appropriate handler                    │
│   • Authentication check (if enabled)              │
└─────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────┐
│ Handler: agent.run                                  │
│   • Resolve session key                             │
│   • Load session history                            │
│   • Create AgentLoop instance                       │
└─────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────┐
│ AgentLoop: Observe-Think-Act-Feedback               │
│   1. Observe: Build context from history + input    │
│   2. Think: Call Thinker (LLM) for decision         │
│   3. Act: Execute tools via Executor                │
│   4. Feedback: Evaluate result, update state        │
│   5. Compress: If overflow, compact history         │
└─────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────┐
│ Response (Streaming)                                │
│   • Stream events via EventBus                      │
│   • Final response as JSON-RPC result               │
└─────────────────────────────────────────────────────┘
```

### Tool Execution Flow

```
Thinker Decision (tool_use)
    │
    ▼
┌─────────────────────────────────────────────────────┐
│ Dispatcher                                          │
│   • Analyze tool request                            │
│   • Check permissions (ToolFilter)                  │
│   • Risk evaluation                                 │
│   • Confirmation flow (if needed)                   │
└─────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────┐
│ Executor                                            │
│   • Lookup tool (Builtin / MCP / Extension)         │
│   • Deserialize arguments                           │
│   • Execute with timeout                            │
│   • Capture output                                  │
└─────────────────────────────────────────────────────┘
    │
    ├─── Builtin Tool (AlephTool trait)
    │       • Direct Rust execution
    │
    ├─── MCP Tool (Model Context Protocol)
    │       • JSON-RPC to external process
    │
    └─── Extension Tool (WASM / Node.js)
            • Plugin runtime execution
```

Exec-class tools (`code_exec`, `bash_exec`) route through an additional
**Sandbox layer** (`src/sandbox/`) between the tool and process execution.
The sandbox owns per-session workspace provisioning
(`~/.aleph/workspaces/{session_id}/`), capability enforcement, and OS-level
seatbelt isolation via `OsSandboxDriver`. Tool-level permissions
(`SmartFilter`) gate *whether* a call runs; the sandbox's capability check
gates *what the subprocess can do* once it is allowed to run. See
[SANDBOX.md](./SANDBOX.md) for the six-step execute pipeline and testing
pattern.

---

## Core Modules Summary

| Module | Path | Purpose |
|--------|------|---------|
| **gateway** | `src/gateway/` | WebSocket server, JSON-RPC routing, interfaces |
| **agent_loop** | `src/agent_loop/` | Observe-Think-Act-Feedback cycle |
| **thinker** | `src/thinker/` | LLM interaction, prompt building, streaming |
| **dispatcher** | `src/dispatcher/` | Task orchestration, tool filtering |
| **executor** | `src/executor/` | Tool execution engine |
| **providers** | `src/providers/` | AI provider implementations |
| **tools** | `src/tools/` | AlephTool trait, tool server |
| **builtin_tools** | `src/builtin_tools/` | Built-in tool implementations |
| **memory** | `src/memory/` | Facts DB, hybrid retrieval |
| **extension** | `src/extension/` | Plugin system (WASM, Node.js) |
| **exec** | `src/exec/` | Shell execution, approval system, OS-native sandboxing (`OsSandboxDriver`) |
| **sandbox** | `src/sandbox/` | `Sandbox` trait + `WorkspaceSandbox` — per-session workspace, capability ledger, `ApprovalGate`-backed escalation ([SANDBOX.md](./SANDBOX.md)) |
| **mcp** | `src/mcp/` | MCP client implementation |
| **routing** | `src/routing/` | Session key resolution |
| **config** | `src/config/` | Configuration management |
| **runtimes** | `src/runtimes/` | Capability ledger — probe, bootstrap, persist external tool status |
| **session** | `src/session/` | Append-only session event log + in-process actor (see below) |

---

## Agent execution

### Session Service

Agent execution's authoritative session state lives in
[`SessionService`](./SESSION_SERVICE.md) (`src/session/`). The Phase 1
implementation `InProcessActorSessionService` spawns one tokio task per
session and persists each event synchronously to the `session_events`
table. Gateway `session.*` RPC methods continue to use the legacy
`SessionManager`; every `SessionManager` append is mirrored into
`SessionService` via a dual-write shim (`src/session/shim.rs`) until
Phase 6 migrates Gateway RPC directly. `agent_loop` is already
decoupled from `SessionManager` and will adopt `SessionService` in
Phase 4.

---

## Design Patterns

Aleph employs several key design patterns to ensure code quality, type safety, and maintainability:

### Context Pattern

Groups related function parameters into dedicated structs, reducing parameter count and improving API ergonomics.

**Example: `RunContext`**
```rust
// Before: 7 parameters
agent_loop.run(request, context, tools, identity, callback, abort_signal, initial_history).await

// After: 2 parameters + Context
let run_context = RunContext::new(request, context, tools, identity)
    .with_abort_signal(abort_rx)
    .with_initial_history(history);
agent_loop.run(run_context, callback).await
```

**Benefits:**
- Extensibility: Add parameters without breaking changes
- Readability: Clear parameter grouping
- Type Safety: Compile-time validation
- Ergonomics: Builder pattern for optional parameters

### Newtype Pattern

Wraps primitive types in distinct structs for type safety and semantic clarity.

**Example: ID Types**
```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExperimentId(String);

impl ExperimentId {
    pub fn new(id: impl Into<String>) -> Self { Self(id.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl Deref for ExperimentId {
    type Target = str;
    fn deref(&self) -> &Self::Target { &self.0 }
}
```

**Newtype Catalog:**
- **IDs**: `ExperimentId`, `VariantId`, `ContextId`, `TaskId`, `SubscriptionId`
- **Collections**: `Ruleset` (permission rules)
- **Values**: `Answer` (question responses)

**Benefits:**
- Type Safety: Prevents mixing different ID types
- Self-Documentation: Clear semantic meaning
- Encapsulation: Controlled access to inner value
- Extension Points: Add methods without modifying primitives

### FromStr Trait Pattern

Provides consistent parsing interface across the codebase.

**Example:**
```rust
impl FromStr for TaskStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            _ => Err(format!("Invalid TaskStatus: {}", s)),
        }
    }
}

// Usage
let status: TaskStatus = "pending".parse()?;
```

**Implemented for:** `FactType`, `HookKind`, `DeviceType`, `TaskStatus`, `RuntimeKind`, and 10+ other types.

### Complete Documentation

For detailed information on design patterns, implementation guidelines, and migration guides, see:
- **[DESIGN_PATTERNS.md](DESIGN_PATTERNS.md)** - Complete design patterns reference

---

## Providers

### Protocol Adapter Architecture

Aleph uses a layered protocol adapter system supporting multiple AI provider protocols:

**Layer 1: Built-in Protocols** (Compiled Rust)
- `OpenAiProtocol` - OpenAI-compatible APIs
- `AnthropicProtocol` - Claude/Anthropic APIs
- `GeminiProtocol` - Google Gemini APIs
- `OllamaProvider` - Local Ollama (native implementation)

**Layer 2: Configurable Protocols** (YAML-based, hot-reload)
- Minimal configuration mode - Extend existing protocols with differences
- Full template mode - Completely custom protocol implementations
- Loaded from `~/.aleph/protocols/` directory
- Changes detected within 600ms (500ms debounce + processing)

**Layer 3: Extension Protocols** (Future)
- WASM/Node.js plugin protocols
- Independent process protocols (MCP/gRPC)

#### Protocol Resolution Flow

```
User config.protocol
    ↓
ProtocolRegistry.get(name)
    ↓
├─> Dynamic protocols (YAML-loaded) ───> ConfigurableProtocol
│   ├─> Minimal mode: base + differences
│   └─> Custom mode: template rendering
├─> Built-in protocols ───> OpenAi/Anthropic/Gemini
└─> Not found ───> Error with available list
```

#### Hot Reload Mechanism

1. `notify-debouncer-full` watches `~/.aleph/protocols/`
2. File change detected (Create/Modify/Delete)
3. YAML parsed into `ProtocolDefinition`
4. `ConfigurableProtocol` created
5. Registry updated atomically
6. New requests use updated protocol

See `docs/PROTOCOL_ADAPTER_USER_GUIDE.md` for user documentation.

---

## Feature Flags

所有生产功能始终编译，无需 feature flags。仅保留测试用 features：

```toml
[features]
default = []
loom = ["dep:loom"]       # 并发测试
test-helpers = []          # 集成测试工具
```

通道在运行时通过 `aleph.toml` 配置启用/禁用。

---

## Platform Architecture

### macOS App

```
apps/macos/
├── Aleph/
│   ├── Sources/
│   │   ├── App/              # App lifecycle
│   │   ├── Gateway/          # WebSocket interface
│   │   ├── Store/            # SwiftUI state
│   │   ├── Services/         # Core services
│   │   ├── Components/       # UI components
│   │   ├── Settings/         # Settings views
│   │   └── MultiTurn/        # Conversation UI
│   └── Resources/
└── project.yml               # XcodeGen config
```

### Tauri App

```
apps/desktop/
├── src/                      # React frontend
│   ├── components/
│   └── App.tsx
├── src-tauri/
│   └── src/
│       ├── commands/         # IPC commands
│       ├── core/             # Core logic
│       └── main.rs
└── package.json
```

---

## Identity Context Flow

### Overview

IdentityContext is an immutable identity snapshot that flows through the entire execution chain, enabling identity-based permission enforcement at the tool execution level.

### Flow Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                    Identity Context Flow                         │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  1. Session Creation                                             │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  SessionManager                                           │  │
│  │  ┌────────────────────────────────────────────────────┐  │  │
│  │  │ Owner Session:                                      │  │  │
│  │  │   metadata = SessionIdentityMeta {                  │  │  │
│  │  │     role: Owner,                                    │  │  │
│  │  │     identity_id: \"owner\",                           │  │  │
│  │  │     scope: None                                     │  │  │
│  │  │   }                                                 │  │  │
│  │  │                                                     │  │  │
│  │  │ Guest Session:                                      │  │  │
│  │  │   metadata = SessionIdentityMeta {                  │  │  │
│  │  │     role: Guest,                                    │  │  │
│  │  │     identity_id: \"guest-123\",                       │  │  │
│  │  │     scope: Some(GuestScope {                        │  │  │
│  │  │       allowed_tools: [\"translate\"],                │  │  │
│  │  │       expires_at: Some(1735689600)                  │  │  │
│  │  │     })                                              │  │  │
│  │  │   }                                                 │  │  │
│  │  └────────────────────────────────────────────────────┘  │  │
│  └──────────────────────────────────────────────────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  2. Request Processing                                           │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  ExecutionEngine                                          │  │
│  │  ┌────────────────────────────────────────────────────┐  │  │
│  │  │ let identity = session_manager                      │  │  │
│  │  │     .get_identity_context(&session_key, \"gateway\")  │  │  │
│  │  │     .await?;                                        │  │  │
│  │  │                                                     │  │  │
│  │  │ // IdentityContext {                               │  │  │
│  │  │ //   request_id: \"req-456\",                        │  │  │
│  │  │ //   session_key: \"session-123\",                   │  │  │
│  │  │ //   role: Guest,                                  │  │  │
│  │  │ //   identity_id: \"guest-123\",                     │  │  │
│  │  │ //   scope: Some(GuestScope { ... }),              │  │  │
│  │  │ //   created_at: 1735689000,                       │  │  │
│  │  │ //   source_channel: \"gateway\"                     │  │  │
│  │  │ // }                                               │  │  │
│  │  └────────────────────────────────────────────────────┘  │  │
│  └──────────────────────────────────────────────────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  3. Agent Loop Execution                                         │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  AgentLoop::run(                                          │  │
│  │      request,                                             │  │
│  │      context,                                             │  │
│  │      tools,                                               │  │
│  │      identity,  // ← IdentityContext passed here         │  │
│  │      callback,                                            │  │
│  │      abort_signal,                                        │  │
│  │      initial_history                                      │  │
│  │  )                                                        │  │
│  └──────────────────────────────────────────────────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  4. Tool Execution                                               │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  Executor::execute(&action, &identity)                    │  │
│  │  ┌────────────────────────────────────────────────────┐  │  │
│  │  │ // Normalize tool name                             │  │  │
│  │  │ let normalized = normalize_tool_name(tool_name);   │  │  │
│  │  │                                                     │  │  │
│  │  │ // Check permission                                │  │  │
│  │  │ let result = PolicyEngine::check_tool_permission(  │  │  │
│  │  │     &identity,                                     │  │  │
│  │  │     &normalized                                    │  │  │
│  │  │ );                                                 │  │  │
│  │  │                                                     │  │  │
│  │  │ match result {                                     │  │  │
│  │  │     Allowed => execute_tool(...),                  │  │  │
│  │  │     Denied { reason } => ToolError { error: reason }│  │  │
│  │  │ }                                                  │  │  │
│  │  └────────────────────────────────────────────────────┘  │  │
│  └──────────────────────────────────────────────────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  5. Permission Check                                             │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  PolicyEngine::check_tool_permission                      │  │
│  │  ┌────────────────────────────────────────────────────┐  │  │
│  │  │ match identity.role {                              │  │  │
│  │  │     Role::Owner => Allowed,                        │  │  │
│  │  │                                                     │  │  │
│  │  │     Role::Guest => {                               │  │  │
│  │  │         // Check scope                             │  │  │
│  │  │         if scope.is_none() {                       │  │  │
│  │  │             return Denied { \"no scope\" };          │  │  │
│  │  │         }                                          │  │  │
│  │  │                                                     │  │  │
│  │  │         // Check expiration                        │  │  │
│  │  │         if is_expired() {                          │  │  │
│  │  │             return Denied { \"expired\" };           │  │  │
│  │  │         }                                          │  │  │
│  │  │                                                     │  │  │
│  │  │         // Check tool permission                   │  │  │
│  │  │         if allowed_tools.contains(tool_name) {     │  │  │
│  │  │             Allowed                                │  │  │
│  │  │         } else {                                   │  │  │
│  │  │             Denied { \"not in scope\" }              │  │  │
│  │  │         }                                          │  │  │
│  │  │     }                                              │  │  │
│  │  │                                                     │  │  │
│  │  │     Role::Anonymous => Denied { \"auth required\" }  │  │  │
│  │  │ }                                                  │  │  │
│  │  └────────────────────────────────────────────────────┘  │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Example: Owner Request

```rust
// 1. Owner creates session (default)
let session_key = "owner-session-1";
let metadata = SessionIdentityMeta::owner("gateway");

// 2. Request arrives
let identity = session_manager.get_identity_context(session_key, "gateway").await?;
// identity.role = Role::Owner

// 3. Execute tool
let action = Action::ToolCall {
    tool_name: "shell_exec".to_string(),
    arguments: json!({"command": "ls"}),
};

let result = executor.execute(&action, &identity).await;
// Result: ToolSuccess (Owner bypasses all checks)
```

### Example: Guest Request

```rust
// 1. Guest activates invitation
let invitation = manager.create_invitation(CreateInvitationRequest {
    guest_name: "Alice".to_string(),
    scope: GuestScope {
        allowed_tools: vec!["translate".to_string()],
        expires_at: Some(now + 3600),
        display_name: Some("Alice".to_string()),
    },
})?;

let guest_token = manager.activate_invitation(&invitation.token)?;

// 2. Guest creates session
// SessionManager stores metadata with guest scope

// 3. Request arrives
let identity = session_manager.get_identity_context(session_key, "gateway").await?;
// identity.role = Role::Guest
// identity.scope = Some(GuestScope { allowed_tools: ["translate"], ... })

// 4. Execute allowed tool
let action1 = Action::ToolCall {
    tool_name: "translate".to_string(),
    arguments: json!({"text": "Hello"}),
};

let result1 = executor.execute(&action1, &identity).await;
// Result: ToolSuccess (tool in allowed_tools)

// 5. Execute denied tool
let action2 = Action::ToolCall {
    tool_name: "shell_exec".to_string(),
    arguments: json!({"command": "ls"}),
};

let result2 = executor.execute(&action2, &identity).await;
// Result: ToolError { error: "Tool 'shell_exec' not in guest 'guest-123' scope" }
```

### Key Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| **IdentityContext** | `shared/protocol/src/auth.rs` | Immutable identity snapshot |
| **SessionIdentityMeta** | `src/gateway/session_manager.rs` | Persistent identity metadata |
| **PolicyEngine** | `src/gateway/security/policy_engine.rs` | Stateless permission checker |
| **InvitationManager** | `src/gateway/security/invitation_manager.rs` | Guest invitation lifecycle |
| **SessionManager** | `src/gateway/session_manager.rs` | Identity construction |
| **ExecutionEngine** | `src/gateway/execution_engine.rs` | Identity injection |
| **AgentLoop** | `src/agent_loop/agent_loop.rs` | Identity propagation |
| **Executor** | `src/executor/` | Permission enforcement |

### Security Properties

1. **Immutability**: IdentityContext cannot be modified after creation
2. **Frozen Permissions**: Guest scope is frozen at session creation time
3. **Stateless Checks**: PolicyEngine has no mutable state
4. **Certificate of Authority**: Identity is constructed once and passed down
5. **Fail-Safe**: Missing or invalid metadata defaults to Owner (backward compatible)

---

## Prompt System

The prompt system lives entirely in `src/thinker/`. The sole public entry point is `thinker::PromptBuilder`, which wraps a `PromptPipeline`. The old `agent_loop::PromptBuilder` has been removed.

### PromptBuilder

```rust
// Standard usage
let builder = PromptBuilder::new(config);
let prompt = builder.build_system_prompt(&tools);

// With soul identity
let prompt = builder.build_system_prompt_with_soul(&tools, &soul, profile);

// Sub-agent usage (replaces old prompt_sections::resolve())
let prompt = builder.build_for_agent(&agent_def, &tools, &soul);

// Mode + budget control
let result = builder.build_with_budget(&tools, &soul, profile, PromptMode::Compact, &budget);
```

### 29 Layers (sorted by priority)

**Stable zone** — content rarely changes, eligible for section-level caching (priorities 50–1600):

| Priority | Layer | Notes |
|----------|-------|-------|
| 50 | `SoulLayer` | Identity / personality |
| 55 | `AgentRoleLayer` | Sub-agent role header + protocol blocks (NEW) |
| 75 | `ProfileLayer` | Workspace profile overlay |
| 100 | `RoleLayer` | Base assistant role |
| 300 | `EnvironmentLayer` | OS, date, working directory |
| 400 | `RuntimeCapabilitiesLayer` | Python, Node.js, FFmpeg, etc. |
| 500 | `ToolsLayer` | Tool definitions (text schema) |
| 501 | `HydratedToolsLayer` | Semantic-retrieval tool definitions |
| 550 | `ToolUsageGrammarLayer` | Data-driven tool usage conventions (NEW) |
| 600 | `SecurityLayer` | Safety / security guidelines |
| 700 | `ProtocolTokensLayer` | JSON-RPC protocol tokens |
| 710 | `HeartbeatLayer` | Session keep-alive instructions |
| 800 | `OperationalGuidelinesLayer` | Operational rules |
| 900 | `CitationStandardsLayer` | Citation formatting |
| 1000 | `GenerationModelsLayer` | Available image/video/audio models |
| 1050 | `SkillInstructionsLayer` | Active skill instructions |
| 1100 | `SpecialActionsLayer` | Special action syntax |
| 1200 | `ResponseFormatLayer` | Response structure |
| 1300 | `GuidelinesLayer` | General guidelines |
| 1350 | `ThinkingGuidanceLayer` | Structured reasoning guidance |
| 1400 | `SkillModeLayer` | Strict skill workflow enforcement |
| 1500 | `CustomInstructionsLayer` | User custom instructions |
| 1600 | `LanguageLayer` | Response language |

**Dynamic zone** — per-request, never cached (priorities 1700–1750):

| Priority | Layer | Notes |
|----------|-------|-------|
| 1700 | `InboundContextLayer` | Sender, channel, session metadata |
| 1710 | `VoiceModeLayer` | Voice-specific response instructions |
| 1720 | `RuntimeContextLayer` | Current time, session info |
| 1730 | `IdentityFilesLayer` | SOUL.md, IDENTITY.md workspace files |
| 1740 | `MemoryAugmentationLayer` | Dual-path memory injection (ENHANCED) |
| 1750 | `SessionContextGuideLayer` | Compressed session context guidance |

### Assembly Paths

Each layer declares which paths it participates in; the pipeline filters by path at assembly time:

| Path | Description |
|------|-------------|
| `Basic` | Minimal — config + tool list only |
| `Hydration` | Tools come from semantic retrieval (`HydrationResult`) |
| `Soul` | Soul-enriched — includes identity / personality |
| `Context` | Context-aware — uses `ResolvedContext` |
| `Cached` | Pre-cached stable prefix |

### Prompt Modes

| Mode | Behavior |
|------|----------|
| `Full` (default) | All 29 layers participate |
| `Compact` | Excludes 14 heavy layers (runtime_context, environment, runtime_capabilities, protocol_tokens, heartbeat, operational_guidelines, citation_standards, generation_models, skill_instructions, special_actions, guidelines, thinking_guidance, skill_mode, poe_success_criteria) |
| `Minimal` | Only 5 core layers: soul, tools, hydrated_tools, response_format, language |

### Section-Level Caching

`execute_cached()` caches the output of every `LayerStability::Stable` layer after the first call. Dynamic layers always recompute. ~23 of 29 layers are Stable. Cache management:

```rust
pipeline.invalidate("soul");    // Invalidate one layer by name
pipeline.invalidate_all();      // Clear all cached sections
pipeline.cache_stats();         // CacheStats { hits, misses, entries }
```

### AgentRoleLayer

Replaces the old `prompt_sections::resolve()` function. When `LayerInput.agent_def` is set, this layer injects:
- Role header from `AgentDef.role`
- Protocol blocks declared in `AgentDef.prompt_sections` (e.g. `explore_constraints`, `coder_guidelines`, `researcher_protocol`, `verify_protocol`, `plan_protocol`)

### ToolUsageGrammarLayer

Reads `ToolInfo.usage_hint` fields (`prefer_for`, `prefer_over`) and generates data-driven "use X instead of Y" guidelines for the LLM. No hardcoded conventions — all tool usage rules come from tool definitions.

### MemoryAugmentationLayer (Hybrid Injection)

Supports dual-path memory injection:
1. **Structured index** — reads `.aleph/MEMORY.md`, truncated to 200 lines
2. **Vector retrieval** — top-K semantic search results from LanceDB
3. Includes memory taxonomy guidelines for how to interpret and use memories

### TokenBudget

Default `max_total_chars` is 80,000. When the assembled prompt exceeds the budget, lower-priority layers are dropped. Protected priorities (50, 55, 75, 100, 500, 501, 1200) always survive enforcement. `PromptResult` includes `truncation_stats` listing which sections were removed.

---

## See Also

- [Agent System](AGENT_SYSTEM.md) - Agent loop internals
- [Gateway](GATEWAY.md) - WebSocket protocol and RPC methods
- [Tool System](TOOL_SYSTEM.md) - Tool development guide
- [Memory System](MEMORY_SYSTEM.md) - RAG and retrieval
- [Extension System](EXTENSION_SYSTEM.md) - Plugin architecture
- [Security](SECURITY.md) - Exec approval and permissions
- [Sandbox](SANDBOX.md) - `Sandbox` trait, `WorkspaceSandbox`, capability pipeline
- [Skill Sandboxing](SKILL_SANDBOXING.md) - OS-native sandboxing for evolved skills
