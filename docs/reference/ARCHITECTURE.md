# Aleph Architecture

> Complete system architecture overview

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

---

## Core Modules Summary

| Module | Path | Purpose |
|--------|------|---------|
| **gateway** | `core/src/gateway/` | WebSocket server, JSON-RPC routing, interfaces |
| **agent_loop** | `core/src/agent_loop/` | Observe-Think-Act-Feedback cycle |
| **thinker** | `core/src/thinker/` | LLM interaction, prompt building, streaming |
| **dispatcher** | `core/src/dispatcher/` | Task orchestration, tool filtering |
| **executor** | `core/src/executor/` | Tool execution engine |
| **providers** | `core/src/providers/` | AI provider implementations |
| **tools** | `core/src/tools/` | AlephTool trait, tool server |
| **builtin_tools** | `core/src/builtin_tools/` | Built-in tool implementations |
| **memory** | `core/src/memory/` | Facts DB, hybrid retrieval |
| **extension** | `core/src/extension/` | Plugin system (WASM, Node.js) |
| **exec** | `core/src/exec/` | Shell execution, approval system, OS-native sandboxing |
| **skill_evolution** | `core/src/skill_evolution/` | Dynamic skill generation, sandboxed execution |
| **mcp** | `core/src/mcp/` | MCP client implementation |
| **routing** | `core/src/routing/` | Session key resolution |
| **config** | `core/src/config/` | Configuration management |
| **runtimes** | `core/src/runtimes/` | Capability ledger — probe, bootstrap, persist external tool status |

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
| **SessionIdentityMeta** | `core/src/gateway/session_manager.rs` | Persistent identity metadata |
| **PolicyEngine** | `core/src/gateway/security/policy_engine.rs` | Stateless permission checker |
| **InvitationManager** | `core/src/gateway/security/invitation_manager.rs` | Guest invitation lifecycle |
| **SessionManager** | `core/src/gateway/session_manager.rs` | Identity construction |
| **ExecutionEngine** | `core/src/gateway/execution_engine.rs` | Identity injection |
| **AgentLoop** | `core/src/agent_loop/agent_loop.rs` | Identity propagation |
| **Executor** | `core/src/executor/` | Permission enforcement |

### Security Properties

1. **Immutability**: IdentityContext cannot be modified after creation
2. **Frozen Permissions**: Guest scope is frozen at session creation time
3. **Stateless Checks**: PolicyEngine has no mutable state
4. **Certificate of Authority**: Identity is constructed once and passed down
5. **Fail-Safe**: Missing or invalid metadata defaults to Owner (backward compatible)

---

## See Also

- [Agent System](AGENT_SYSTEM.md) - Agent loop internals
- [Gateway](GATEWAY.md) - WebSocket protocol and RPC methods
- [Tool System](TOOL_SYSTEM.md) - Tool development guide
- [Memory System](MEMORY_SYSTEM.md) - RAG and retrieval
- [Extension System](EXTENSION_SYSTEM.md) - Plugin architecture
- [Security](SECURITY.md) - Exec approval and permissions
- [Skill Sandboxing](SKILL_SANDBOXING.md) - OS-native sandboxing for evolved skills
