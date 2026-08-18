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
│                    (Orchestrator → Harness → Think → Act)                   │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                        Orchestrator                                  │   │
│  │  Resolves AgentDef + FlowSpec, builds HarnessDeps, dispatches       │   │
│  └───────────────────────────────┬──────────────────────────────────────┘   │
│                                  │ HarnessRunner::run                       │
│  ┌───────────────────────────────▼──────────────────────────────────────┐   │
│  │                         AgentHarness                                 │   │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐   │   │
│  │  │ Context │→ │  Think  │→ │   Act   │→ │Stop-Hook│→ │ Compact │   │   │
│  │  │ (Budget)│  │(Thinker)│  │ (Tools) │  │  Check  │  │ (Budget)│   │   │
│  │  └─────────┘  └─────────┘  └─────────┘  └─────────┘  └─────────┘   │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐                      │
│  │  ToolService │  │    Guards    │  │   Overflow   │                      │
│  │  (Tool Exec) │  │  (Safety)    │  │  Detector    │                      │
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
│  │  Memory (SQLite+vec0)│  │ Resilience     │  │  Config Store   │          │
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
                    │   gateway   │ ← Entry point (chat ingress, protocol adapters)
                    └──────┬──────┘
                           │ FlowRequest
          ┌────────────────┼────────────────┐
          │                │                │
          ▼                ▼                ▼
    ┌───────────┐    ┌───────────┐    ┌───────────┐
    │interfaces │    │  routing  │    │orchestrator│
    └───────────┘    └───────────┘    └─────┬─────┘
                                            │ HarnessRunner::run
                           ┌────────────────┼────────────────┐
                           │                │                │
                            ▼                ▼                ▼
                      ┌───────────┐    ┌───────────┐    ┌───────────┐
                      │  harness  │    │  memory   │    │   exec    │
                      └─────┬─────┘    └───────────┘    └───────────┘
                            │
           ┌────────────────┼────────────────┐
           │                │                │
           ▼                ▼                ▼
     ┌───────────┐    ┌───────────┐    ┌───────────┐
     │  thinker  │    │tool_service│   │  engine   │
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
│   • Construct FlowRequest { agent_id, input, ... }  │
└─────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────┐
│ Orchestrator                                        │
│   • Resolve AgentDef + FlowSpec                     │
│   • Build HarnessDeps (SessionService, ToolService, │
│     Sandbox, AiProvider)                            │
│   • Dispatch to HarnessRunner::run                  │
└─────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────┐
│ AgentHarness: Think → Act loop                      │
│   1. Context: Apply budget, preflight prep          │
│   2. Think: Call Thinker (LLM) for decision         │
│   3. Act: Execute tools via ToolService             │
│   4. Stop-hook: Evaluate completion / guards        │
│   5. Compact: If overflow, compact history          │
└─────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────┐
│ FlowOutcome → Gateway renders response              │
│   • Stream events via TraceSink / EventBus          │
│   • Final response as JSON-RPC result               │
└─────────────────────────────────────────────────────┘
```

### Tool Execution Flow

```
Thinker Decision (tool_use)
    │ validate non-empty, response-unique call_id
    │ persist AssistantMessage before side effects
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

`call_id` is the correlation key across the provider response, session log,
approval identity, in-flight cancellation, tool result, trace, and UI projection.
The harness rejects empty or duplicate IDs before persisting the assistant event;
it never repairs IDs heuristically. The process-wide in-flight registry is installed
before its Gateway RPC handlers, independently of the optional result store. MCP
restart failures emit `ServerCrashed`, allowing the existing bridge to remove stale
handlers before a later turn snapshots the tool surface.

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
| **orchestrator** | `src/orchestrator/` | Resolve FlowSpec, build HarnessDeps, dispatch |
| **harness** | `src/harness/` | Think→Act loop, stop-hooks, context budget, compaction |
| **agents** | `src/agents/` | Agent runtime, subagent spawning, team coordination |
| **thinker** | `src/thinker/` | LLM interaction, prompt building, streaming |
| **dispatcher** | `src/dispatcher/` | Task orchestration, tool filtering |
| **engine** | `src/engine/` | Tool execution engine |
| **providers** | `src/providers/` | AI provider implementations |
| **tools** | `src/tools/` | AlephTool trait, tool server |
| **builtin_tools** | `src/builtin_tools/` | Built-in tool implementations |
| **memory** | `src/memory/` | Facts DB, hybrid retrieval (SQLite + sqlite-vec + markdown notes + wikilink graph) |
| **extension** | `src/extension/` | Plugin system (WASM, Node.js) |
| **exec** | `src/exec/` | Shell execution, approval system, OS-native sandboxing (`OsSandboxDriver`) |
| **sandbox** | `src/sandbox/` | `Sandbox` trait + `WorkspaceSandbox` — per-session workspace, capability ledger, `ApprovalGate`-backed escalation ([SANDBOX.md](./SANDBOX.md)) |
| **mcp** | `src/mcp/` | MCP client implementation |
| **routing** | `src/routing/` | Session key resolution |
| **config** | `src/config/` | Configuration management |
| **runtimes** | `src/runtimes/` | Capability ledger — probe, bootstrap, persist external tool status |
| **session** | `src/session/` | Append-only session event log + in-process actor (see below) |
| **a2a** | `src/a2a/` | A2A protocol adapter |
| **acp** | `src/acp/` | ACP protocol implementation |
| **approval** | `src/approval/` | Approval system |
| **arena** | `src/arena/` | Arena functionality |
| **browser** | `src/browser/` | Browser automation |
| **capability** | `src/capability/` | Capability system |
| **clawhub** | `src/clawhub/` | ClawHub integration |
| **cluster** | `src/cluster/` | Single-center node federation — reverse RPC, node registry, `node_invoke`/`node_file`, approval routing ([CLUSTER.md](./CLUSTER.md)) |
| **components** | `src/components/` | Shared components |
| **compressor** | `src/compressor/` | Context compression |
| **core** | `src/core/` | Core types and primitives |
| **discovery** | `src/discovery/` | Service discovery |
| **event** | `src/event/` | Event system |
| **generation** | `src/generation/` | Media generation |
| **group_chat** | `src/group_chat/` | Group chat management |
| **intent** | `src/intent/` | Intent recognition |
| **logging** | `src/logging/` | Logging infrastructure |
| **loop_graph** | `src/loop_graph/` | Loop-governance topology (who watches/audits/anchors whom) — scaffolding only, adjudication stays in LLM turns ([GRAPH_LAYER.md](./GRAPH_LAYER.md)) |
| **markdown** | `src/markdown/` | Markdown processing |
| **media** | `src/media/` | Media processing |
| **metrics** | `src/metrics/` | Metrics collection |
| **permission** | `src/permission/` | Permission system |
| **pii** | `src/pii/` | PII detection and handling |
| **prompt** | `src/prompt/` | Prompt management |
| **resilience** | `src/resilience/` | State management (SQLite) |
| **search** | `src/search/` | Search providers |
| **secrets** | `src/secrets/` | Secret management |
| **security** | `src/security/` | Security utilities |
| **skill** | `src/skill/` | Skill system |
| **supervisor** | `src/supervisor/` | Execution supervision |
| **tasks** | `src/tasks/` | Task management |
| **teams** | `src/teams/` | Team coordination |
| **tool_output** | `src/tool_output/` | Tool output handling |
| **utils** | `src/utils/` | Utilities |
| **vision** | `src/vision/` | Vision processing |
| **wizard** | `src/wizard/` | Wizard flows |

---

## Agent execution

### Session Service

Agent execution's authoritative session state lives in
[`SessionService`](./SESSION_SERVICE.md) (`src/session/`). The
`InProcessActorSessionService` spawns one tokio task per session and
persists each event synchronously to the `session_events` table.
`AgentHarness` reads and writes history exclusively through
`SessionService`. Gateway `session.*` RPC methods continue to use the
legacy `SessionManager`; every `SessionManager` append is mirrored into
`SessionService` via a dual-write shim (`src/session/shim.rs`) until a
future phase migrates Gateway RPC directly.

---

## Design Patterns

Aleph employs several key design patterns to ensure code quality, type safety, and maintainability:

### Context Pattern

Groups related function parameters into dedicated structs, reducing parameter count and improving API ergonomics.

**Example: `HarnessDeps` / `FlowRequest`**
```rust
// Before: 7 parameters passed directly
legacy_agent_loop.run(request, context, tools, identity, callback, abort_signal, initial_history).await

// After: structured deps + FlowRequest
let deps = HarnessDeps { session, tool_service, sandbox, provider, trace_sink };
let flow = FlowRequest::new(agent_id, input, identity)
    .with_abort_signal(abort_rx);
HarnessRunner::run(flow, deps).await
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

### Desktop Bridge

The desktop bridge provides native OS capabilities to the core through IPC:

```
desktop/
├── shared/                   # DesktopCapability trait + IPC protocol
├── macos/                    # macOS native implementation (AppKit, Vision)
├── linux/                    # Linux native implementation
└── windows/                  # Windows native implementation
```

## Swift Helper Process (macOS)

aleph-server spawns `AlephBridge` (Swift) as a long-lived child process for
native macOS API access. The process-level split of responsibilities:

- **Swift owns**: AVFoundation (camera, audio), Vision (OCR),
  Accessibility (AX tree), SFSpeechRecognizer (speech-to-text)
- **Rust owns**: IOKit (`IOPMAssertion` for the sleep inhibitor), TCC
  framework status checks via `objc2-av-foundation` / `objc2-speech` in
  `permission.rs`, all business logic, vault, and sessions

This satisfies architectural redline R1 (Brain–Limb separation): the Rust
core defines `DesktopCapability` traits; the helper provides physical
implementations over JSON-RPC 2.0 stdio. See
[DESKTOP_BRIDGE.md](DESKTOP_BRIDGE.md) for the full protocol specification,
method reference, error envelope, and debugging procedures.

### Web Chat Interface

```
interfaces/webchat/          # Web-based chat interface (Leptos/WASM panel)
```

### Terminal Interface (TUI)

```
interfaces/tui/              # Full-screen terminal chat (ratatui + crossterm)
```

A thin, remote client: it speaks JSON-RPC to the Gateway over a WebSocket via
`aleph-client` + `aleph-protocol` and **must not depend on alephcore** (enforced
in its `Cargo.toml`). Like the CLI it holds no agent/tools/memory in-process —
it is a pure I/O interface (redline R4): user input → JSON-RPC request, and
`StreamEvent`s → rendered output. See
[FEATURE_LOCATOR.md](FEATURE_LOCATOR.md) §5.13 for the file-level map.

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
│  3. Harness Execution                                            │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  HarnessRunner::run(                                      │  │
│  │      FlowRequest {                                        │  │
│  │          agent_id,                                        │  │
│  │          input,                                           │  │
│  │          identity,  // ← IdentityContext passed here     │  │
│  │          abort_signal,                                    │  │
│  │      },                                                   │  │
│  │      HarnessDeps { session, tool_service, sandbox, ... } │  │
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
| **Orchestrator** | `src/orchestrator/` | Identity injection into FlowRequest |
| **AgentHarness** | `src/harness/` | Identity propagation through Think→Act loop |
| **Engine** | `src/engine/` | Permission enforcement |

### Security Properties

1. **Immutability**: IdentityContext cannot be modified after creation
2. **Frozen Permissions**: Guest scope is frozen at session creation time
3. **Stateless Checks**: PolicyEngine has no mutable state
4. **Certificate of Authority**: Identity is constructed once and passed down
5. **Fail-Safe**: Missing or invalid metadata defaults to Owner (backward compatible)

---

## Prompt System

The prompt system lives entirely in `src/thinker/`. The sole public entry point is `thinker::PromptBuilder`, which wraps a `PromptPipeline`. The old `agent_loop::PromptBuilder` was removed during the Harness migration (Phase 6/7).

### PromptBuilder

```rust
// Standard usage — identity is a single file-based source of truth:
// agent-dir SOUL.md (persona), AGENTS.md (project context), etc. ride in via
// `with_identity_files`; `SoulLayer` / `ProfileLayer` render them.
let builder = PromptBuilder::new(config).with_identity_files(identity_files);

// Production main-loop entry point — two-part split (cacheable stable prefix +
// per-request dynamic suffix) on `AssemblyPath::Cached`. This is what the
// harness bridge calls.
let parts = builder.build_system_prompt_cached_with_mode(&tools, mode);

// Sub-agent inline prompt — one flat string, no cache split, on
// `AssemblyPath::Basic`. This is what `subagent_spawner` calls.
let prompt = builder.build_system_prompt(&tools);
```

> **Two entry points, two assembly paths, and nothing else.** The legacy
> `SoulManifest`→prompt builders (`build_system_prompt_with_soul` /
> `build_for_agent` / `build_for_agent_basic` / `build_with_budget` /
> `build_system_prompt_with_mode` / `build_system_prompt_with_full_context` /
> `build_system_prompt_with_context`) and the fork-snapshot cluster
> (`capture_snapshot` / `execute_stable_only` / `build_from_snapshot`) were all
> removed as dead entry points. Identity flows exclusively from the agent-dir
> files threaded via `with_identity_files`; the `SoulManifest` struct survives
> only as the parser behind the `identity.get` RPC.

### Layers

**The registration block in `PromptPipeline::default_layers()` is the only
authoritative list** — layer set, priorities, and assembly order all live there,
and each layer self-declares its `priority()`, `stability()`, `paths()` and
`supports_mode()`. This document deliberately does **not** reproduce that list.

`stability()` has **no default body** (2026-08-04) — a new layer that says nothing
about whether its bytes change per turn does not compile. It used to default to
`Stable`, which meant omission silently placed the layer in the provider-cached
prefix; `ToolRuntimeStateLayer` rode that default until a person reading the code
noticed a 30-second health probe was re-keying whole sessions. The question is
asked at the one moment its author knows the answer. See FEATURE_LOCATOR §2.19 ⑥.

A hand-maintained priority table used to live here, and by 2026-07-26 it named
eight layers that no longer existed (`HydratedToolsLayer`, `HeartbeatLayer`,
`ResponseFormatLayer`, `SkillModeLayer`, `InboundContextLayer`,
`MemoryAugmentationLayer`, and more), three assembly paths that had been
deleted, and a `TokenBudget` mechanism that had been replaced. The same table
was removed from `prompt_pipeline.rs` years earlier for the same reason. A
duplicated list of a thing that changes is a list that will be wrong.

To see the current set, with sizes, run:

```bash
aleph-server prompt-size --path cached --paradigm background
```

It prints every contributing layer (bytes / chars / tokens / zone / priority)
plus, by name, every layer that stayed silent and why that is possible.

**Invariants** (each locked by a test, not by prose):

| Invariant | Guard |
|---|---|
| Priorities are unique and ascending | `default_layers_have_unique_priorities`, `test_default_layers_sorted` |
| Every Stable layer precedes every Dynamic one (this boundary *is* the prompt-cache breakpoint) | `stable_layers_come_before_dynamic` |
| Stable + Dynamic concatenate back to the full assembly | `stable_plus_dynamic_reconstructs_full` |
| Every layer can actually speak in production, or is declared conditionally-silent with a reason | `prompt_contract::reachable_layers` |
| The fixed scaffold does not grow | `prompt_contract::scaffold_bytes_ratchet` |
| No sentence is emitted by two layers | `prompt_contract::no_sentence_is_stated_twice` |

### Assembly Paths

Exactly one variant per entry point — a path no caller requests is a trap,
because a layer listing only that path renders nowhere and the omission is
invisible. Three such phantoms (`Context`, `Hydration`, `Soul`) have been
removed for precisely that reason.

| Path | Entry point |
|------|-------------|
| `Basic` | `build_system_prompt` — sub-agent inline prompt, one flat string |
| `Cached` | `build_system_prompt_cached_with_mode` — main loop, stable/dynamic split |

### Prompt Modes

`[execution] prompt_mode` picks the verbosity tier; each layer opts out via
`supports_mode()`.

| Mode | Behavior |
|------|----------|
| `Full` (default) | Every layer participates |
| `Compact` | Sheds the heavy guidance layers (see `compact_mode_excludes_heavy_layers`) |
| `Minimal` | Persona / curated memory / language only (see `minimal_mode_only_core_layers`) |

### Stable-Prefix Reuse

The prompt is partitioned by each layer's `LayerStability` into a cacheable
**Stable** prefix (persona, security, skills …) and a per-request **Dynamic**
suffix (session / memory / runtime context).
`build_system_prompt_cached_with_mode()` returns
`[SystemPromptPart { cache: true, .. }, SystemPromptPart { cache: false, .. }]`,
so the provider (e.g. Anthropic) caches the prefix; the breakpoint sits exactly
at the Stable→Dynamic boundary.

A fresh `PromptBuilder` (and pipeline) is constructed per build, so there is no
in-pipeline mutable cache to invalidate. Layer renders are deterministic
functions of their inputs, so rebuilding stays byte-stable when nothing changed
— which is what keeps the provider-side prefix cache warm. (An earlier
name-keyed `execute_cached()` and a session-level stable-prompt LRU were both
removed: keyed by layer name with no input fingerprint, they could only ever
serve stale sections.)

**Corollary that governs what may be added:** anything whose bytes change per
run must NOT enter the system prompt, or it re-keys the whole conversation
prefix. Per-run content travels as a transient trailing message instead
(`HarnessDeps::recall_context`). This is why `RuntimeContextLayer` coarsens its
timestamp to the hour and why per-query memory recall left the prompt entirely.

### AgentRoleLayer

Replaces the old `prompt_sections::resolve()` function. When
`LayerInput.agent_def` is set, this layer injects:
- Role header from `AgentDef.role`
- Protocol blocks declared in `AgentDef.prompt_sections` (e.g.
  `explore_constraints`, `coder_guidelines`, `researcher_protocol`,
  `verify_protocol`, `plan_protocol`)

### What belongs in the system prompt

Two filters, both enforced by tests rather than by review discipline
(see [HARNESS_PHILOSOPHY.md §8](HARNESS_PHILOSOPHY.md)):

1. **Runtime fact, not instruction.** Time, cwd, active goal, security posture,
   identity files — things the model cannot know. Not "how to think".
2. **No single tool owns it.** If one tool owns the sentence, it belongs in that
   tool's `DESCRIPTION`, which ships with its schema and only reaches requests
   that can actually call it. The system prompt carries only what no tool can
   state: cross-tool routing, runtime facts, safety boundaries.

### TokenBudget

`TokenBudget` caps the assembled prompt in characters (`max_total_chars`,
window-scaled via `window_char_budget`). Enforcement is `fit_dynamic_suffix`:
the **stable prefix is a protected floor** and is never trimmed, so the cache
breakpoint stays valid; only the per-request dynamic suffix is head/tail
truncated, with a model-visible notice appended. A no-op for prompts under
budget.

This is an overflow backstop, not a leanness measure — the fixed scaffold is
two orders of magnitude below the cap, so only
`prompt_contract::SCAFFOLD_CEILING_BYTES` can detect prompt bloat. See
FEATURE_LOCATOR §1.2 for the distinction.

---

## See Also

- [Agent System](AGENT_SYSTEM.md) - Agent loop internals
- [Gateway](GATEWAY.md) - WebSocket protocol and RPC methods
- [Cluster](CLUSTER.md) - Single-center node federation (center/node, reverse RPC, `node_invoke`/`node_file`)
- [Tool System](TOOL_SYSTEM.md) - Tool development guide
- [Memory System](MEMORY_SYSTEM.md) - RAG and retrieval
- [Extension System](EXTENSION_SYSTEM.md) - Plugin architecture
- [Security](SECURITY.md) - Exec approval and permissions
- [Sandbox](SANDBOX.md) - `Sandbox` trait, `WorkspaceSandbox`, capability pipeline
- [Skill Sandboxing](SKILL_SANDBOXING.md) - OS-native sandboxing for evolved skills
- [Skill Model Taxonomy](SKILL_MODEL_TAXONOMY.md) - Four-layer skill type map (Phase 1 of unification 2026-05-20)
- [Markdown Skill Authoring](MARKDOWN_SKILL_AUTHORING.md) - SKILL.md frontmatter + sandbox modes + host-mode contract
