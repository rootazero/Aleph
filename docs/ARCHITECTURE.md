# Aether Architecture

> Complete system architecture overview

---

## System Diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              CLIENT LAYER                                    │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐      │
│  │ macOS    │  │  Tauri   │  │   CLI    │  │ Telegram │  │ Discord  │      │
│  │  App     │  │   App    │  │          │  │   Bot    │  │   Bot    │      │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘      │
│       │             │             │             │             │             │
│       └─────────────┴─────────────┴─────────────┴─────────────┘             │
│                                   │                                          │
│                          WebSocket (JSON-RPC 2.0)                           │
│                          ws://127.0.0.1:18789                               │
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
│  │  Channel    │  │   Config    │  │  Webhooks   │  │    Cron     │        │
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
│  ┌─────────────────────────┐  ┌─────────────────────────┐                   │
│  │      Memory System      │  │     Config Store        │                   │
│  │  ┌───────┐  ┌────────┐  │  │  ┌───────┐  ┌────────┐  │                   │
│  │  │ Facts │  │ Vector │  │  │  │ TOML  │  │Keychain│  │                   │
│  │  │  DB   │  │  Index │  │  │  │ File  │  │ (Keys) │  │                   │
│  │  └───────┘  └────────┘  │  │  └───────┘  └────────┘  │                   │
│  └─────────────────────────┘  └─────────────────────────┘                   │
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
    │ channels  │    │  routing  │    │ handlers  │
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
    ├─── Builtin Tool (AetherTool trait)
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
| **gateway** | `core/src/gateway/` | WebSocket server, JSON-RPC routing, channels |
| **agent_loop** | `core/src/agent_loop/` | Observe-Think-Act-Feedback cycle |
| **thinker** | `core/src/thinker/` | LLM interaction, prompt building, streaming |
| **dispatcher** | `core/src/dispatcher/` | Task orchestration, tool filtering |
| **executor** | `core/src/executor/` | Tool execution engine |
| **providers** | `core/src/providers/` | AI provider implementations |
| **tools** | `core/src/tools/` | AetherTool trait, tool server |
| **builtin_tools** | `core/src/builtin_tools/` | Built-in tool implementations |
| **memory** | `core/src/memory/` | Facts DB, hybrid retrieval |
| **extension** | `core/src/extension/` | Plugin system (WASM, Node.js) |
| **exec** | `core/src/exec/` | Shell execution, approval system |
| **mcp** | `core/src/mcp/` | MCP client implementation |
| **routing** | `core/src/routing/` | Session key resolution |
| **config** | `core/src/config/` | Configuration management |
| **runtimes** | `core/src/runtimes/` | Runtime managers (uv, fnm, yt-dlp) |

---

## Feature Flags

```toml
[features]
default = ["gateway"]

# Core features
gateway = ["tokio-tungstenite", "axum", ...]
cli = ["inquire"]

# Channels (require gateway)
telegram = ["teloxide", "gateway"]
discord = ["serenity", "gateway"]
all-channels = ["telegram", "discord"]

# Optional features
cron = ["cron", "gateway"]
browser = ["chromiumoxide", "gateway"]

# Plugin runtimes
plugin-wasm = ["extism"]
plugin-nodejs = []
plugin-all = ["plugin-wasm", "plugin-nodejs"]
```

---

## Platform Architecture

### macOS App

```
platforms/macos/
├── Aether/
│   ├── Sources/
│   │   ├── App/              # App lifecycle
│   │   ├── Gateway/          # WebSocket client
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
platforms/tauri/
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

## See Also

- [Agent System](AGENT_SYSTEM.md) - Agent loop internals
- [Gateway](GATEWAY.md) - WebSocket protocol and RPC methods
- [Tool System](TOOL_SYSTEM.md) - Tool development guide
- [Memory System](MEMORY_SYSTEM.md) - RAG and retrieval
- [Extension System](EXTENSION_SYSTEM.md) - Plugin architecture
- [Security](SECURITY.md) - Exec approval and permissions
