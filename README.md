# Aleph (ℵ)

> Self-hosted personal AI assistant — one core, many shells.

[![Rust](https://img.shields.io/badge/Rust-1.92%2B-b7410e)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)]()

[中文文档](README_CN.md)

## What is Aleph?

Aleph is a self-hosted personal AI assistant built in Rust. It runs entirely on your own devices, connecting through a unified Gateway to 15+ messaging channels (Telegram, Discord, Slack, WhatsApp, IRC, Matrix, Signal, and more). The Rust core drives an agent loop with multi-provider LLM support, 30+ built-in tools, hybrid memory search, and a plugin system — accessible through native apps, CLI, a web panel, and social bots simultaneously.

## Key Highlights

### Cognitive Memory Architecture

Aleph's memory system goes beyond simple RAG:

- **Note Layer** — Markdown-based memory with Obsidian-compatible `[[wikilink]]` syntax, forming a traversable knowledge graph
- **Hybrid Retrieval** — Vector ANN (sqlite-vec) + full-text search (FTS5) + wikilink graph traversal for multi-hop reasoning
- **Self-Learning** — Automatic skill generation from notes; the system observes patterns in your notes and suggests or auto-generates skills
- **Dream Daemon** — Background compaction and synthesis of memories into higher-level abstractions

### Decoupled Agent Architecture

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│ Orchestrator│────▶│   Harness   │────▶│   Thinker   │
│             │     │  (Think→Act)│     │  (LLM Call) │
└─────────────┘     └──────┬──────┘     └─────────────┘
                           │
           ┌───────────────┼───────────────┐
           ▼               ▼               ▼
    ┌──────────┐    ┌──────────┐    ┌──────────┐
    │  Session │    │  Tools   │    │ Sandbox  │
    │ (History)│    │(Execution│    │(Isolation│
    └──────────┘    └──────────┘    └──────────┘
```

- **Orchestrator** — Resolves AgentDef + FlowSpec, assembles Harness deps, dispatches execution
- **Harness** — Think→Act loop with stop-hooks, context budget, and emergency compaction
- **Sandbox** — Per-session workspace with capability ledger and OS-native isolation (`OsSandboxDriver`)
- **Session Service** — Append-only event log with in-process actor for authoritative state
- **Tool Service** — Unified façade over built-in tools, MCP servers, and extensions with layered middleware (audit → permission → context rules → timeout)

### System Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                        INTERFACE LAYER (I/O)                        │
│  macOS Native App | WASM Panel | CLI | TUI | Telegram | Discord |   │
│  Slack | WhatsApp | IRC | Matrix | Signal | Email | ...            │
├─────────────────────────────┬───────────────────────────────────────┤
│                       GATEWAY LAYER                                 │
│  Router | Session Manager | Event Bus | Channel Registry | Security │
├─────────────────────────────┼───────────────────────────────────────┤
│                        AGENT LAYER                                  │
│  Orchestrator | Harness | Thinker | Dispatcher | Compressor        │
├─────────────────────────────┼───────────────────────────────────────┤
│                      EXECUTION LAYER                                │
│  Providers | Engine | Tool Server | MCP | Extensions | Sandbox     │
├─────────────────────────────┼───────────────────────────────────────┤
│                       STORAGE LAYER                                 │
│  Memory (SQLite+vec0) | State (SQLite) | Config (~/.aleph/)        │
└─────────────────────────────┴───────────────────────────────────────┘
```

See [docs/reference/ARCHITECTURE.md](docs/reference/ARCHITECTURE.md) for the full architecture documentation.

## Features

### Core

- Multi-provider LLM support (Claude, GPT-4, Gemini, DeepSeek, Ollama, Moonshot, Kimi)
- 15+ messaging channel interfaces via unified Gateway
- 30+ built-in tools with JSON Schema auto-generation
- **Cognitive Memory** — Note layer with wikilink knowledge graph, hybrid retrieval (vector + FTS + graph), and background dream daemon
- **Self-Learning** — Automatic skill generation from note patterns
- MCP protocol support for external tool integration
- **Decoupled Agent Loop** — Orchestrator + Harness + Sandbox + Session + Tool Service architecture
- Desktop Bridge for native OS control (OCR, screenshots, input automation, camera, audio)

### Developer Experience

- Hot reload for configuration and protocol changes
- Plugin system (WASM + Node.js)
- `just` build pipeline with one-command workflows
- 58+ Gateway JSON-RPC handlers
- JSON Schema auto-generation via schemars
- Proptest, Loom concurrency, and Cucumber BDD test suites

## Installation

### macOS / Linux

```bash
curl -fsSL https://raw.githubusercontent.com/rootazero/Aleph/main/install.sh | bash
```

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/rootazero/Aleph/main/install.ps1 | iex
```

The installer automatically detects your platform and architecture (x86_64 / ARM64), downloads the latest release binary, installs it to your PATH, and optionally sets up auto-start as a system service.

After installation, run:

```bash
aleph
```

### Build from Source

If you prefer to build from source:

```bash
# Prerequisites: Rust 1.92+, just (cargo install just), npm, wasm-bindgen, Swift (macOS)
git clone https://github.com/rootazero/Aleph.git
cd Aleph
just dev          # Run server in debug mode (rebuilds WASM)
```

### Configuration

Aleph stores configuration and data at `~/.aleph/`:

```
~/.aleph/
├── aleph.toml       # Main configuration
├── logs/            # Server logs
├── skills/          # User-installed skills
├── plugins/         # Extensions
├── protocols/       # Custom protocol definitions (YAML, hot-reload)
└── workspaces/      # Per-session sandbox workspaces
```

Channel configuration example in `aleph.toml`:

```toml
[channels.telegram]
enabled = true
token = "your-bot-token"
```

## Building

| Command | Description |
|---------|-------------|
| `just dev` | Run server in debug mode (rebuilds WASM) |
| `just build` | Full release build (WASM → Swift Bridge → Server) |
| `just build-debug` | Debug build (faster compile, no Swift bridge) |
| `just wasm` | Build WASM Panel UI only |
| `just swift-bridge` | Build Swift helper process (macOS only) |
| `just test` | Run core tests |
| `just test-desktop` | Run desktop crate tests |
| `just test-desktop-macos` | Run macOS desktop tests |
| `just test-desktop-all` | Run all desktop-related tests |
| `just test-proptest` | Run proptest with high coverage |
| `just test-loom` | Run Loom concurrency tests |
| `just test-all` | Run all tests (core + desktop + proptest) |
| `just clippy` | Lint core with clippy |
| `just clippy-desktop` | Lint desktop crate |
| `just clippy-all` | Lint everything |
| `just check` | Quick compilation check |
| `just check-desktop` | Quick check desktop crate |
| `just deps` | Verify build dependencies are installed |
| `just clean` | Clean all build artifacts |

### Feature Flags

Production features are always compiled. Optional flags control experimental or test functionality:

```toml
[features]
default = []
control-plane = []         # Control plane UI (Leptos/WASM dashboard)
loom = ["dep:loom"]        # Concurrency testing with Loom
test-helpers = []          # Integration test utilities
disabled-tests = []        # Disabled test modules awaiting rewrite
telegram-draft-api = []    # Experimental Telegram Draft API support
```

## Project Structure

```
Aleph/
├── src/                         # Rust Core (alephcore crate)
│   ├── bin/aleph-server/        # Server binary entry point
│   │   ├── commands/            # CLI commands (start, daemon)
│   │   └── main.rs              # Binary entry
│   ├── gateway/                 # WebSocket control plane
│   │   ├── handlers/            # 58+ RPC method handlers
│   │   ├── interfaces/          # 15+ channel interfaces
│   │   ├── security/            # Auth, pairing, device management
│   │   └── ...                  # Router, session, events, voice, webhooks
│   ├── orchestrator/            # AgentDef resolution + Harness dispatch
│   ├── harness/                 # Think→Act loop, stop-hooks, context budget
│   ├── thinker/                 # LLM interaction layer (prompt builder, 29 layers)
│   ├── dispatcher/              # Task orchestration (DAG scheduling)
│   ├── engine/                  # Tool execution engine
│   ├── builtin_tools/           # 30+ built-in tools
│   ├── memory/                  # SQLite+sqlite-vec storage (vectors + FTS + wikilink graph)
│   ├── resilience/              # State management (SQLite)
│   ├── extension/               # WASM + Node.js plugin system
│   ├── providers/               # AI provider integrations (multi-protocol)
│   ├── domain/                  # DDD domain model
│   ├── mcp/                     # MCP protocol client
│   ├── sandbox/                 # Sandbox trait + WorkspaceSandbox + OsSandboxDriver
│   ├── exec/                    # Shell execution + security
│   ├── agents/                  # Agent runtime, subagent spawning, team coordination
│   ├── a2a/                     # A2A protocol adapter
│   ├── acp/                     # ACP protocol
│   ├── approval/                # Approval system
│   ├── arena/                   # Arena functionality
│   ├── browser/                 # Browser automation
│   ├── clawhub/                 # ClawHub integration
│   ├── components/              # Shared components
│   ├── compressor/              # Context compression
│   ├── config/                  # Configuration management
│   ├── context/                 # Context management
│   ├── core/                    # Core types and primitives
│   ├── daemon/                  # Background daemon
│   ├── discovery/               # Service discovery
│   ├── event/                   # Event system
│   ├── generation/              # Media generation
│   ├── group_chat/              # Group chat management
│   ├── intent/                  # Intent recognition
│   ├── logging/                 # Logging infrastructure
│   ├── markdown/                # Markdown processing
│   ├── media/                   # Media processing
│   ├── metrics/                 # Metrics collection
│   ├── pii/                     # PII detection/handling
│   ├── process_supervisor/      # Process supervision
│   ├── prompt/                  # Prompt management
│   ├── routing/                 # Session key resolution
│   ├── runtimes/                # Capability ledger
│   ├── scheduler/               # Job scheduling
│   ├── search/                  # Search providers
│   ├── secrets/                 # Secret management
│   ├── security/                # Security utilities
│   ├── session/                 # Session service (append-only event log)
│   ├── skill/                   # Skill system
│   ├── supervisor/              # Execution supervision
│   ├── task_resilience/         # Task resilience
│   ├── tasks/                   # Task management
│   ├── teams/                   # Team coordination
│   ├── tool_output/             # Tool output handling
│   ├── utils/                   # Utilities
│   ├── verification/            # Verification system
│   ├── vision/                  # Vision processing
│   └── wizard/                  # Wizard flows
├── desktop/
│   ├── shared/                  # DesktopCapability trait + IPC protocol
│   ├── macos/                   # macOS native implementation (AppKit, Vision)
│   │   └── bridge/              # Swift helper process (JSON-RPC over stdio)
│   ├── linux/                   # Linux native implementation
│   └── windows/                 # Windows native implementation
├── shared/
│   ├── protocol/                # Shared protocol types (JSON-RPC schemas)
│   ├── logging/                 # Logging infrastructure
│   ├── client/                  # Shared client utilities
│   └── ui_logic/                # Shared UI logic
├── interfaces/
│   ├── cli/                     # CLI client
│   ├── tui/                     # TUI client
│   └── webchat/                 # Web chat / WASM Panel UI
├── plugins/                     # Plugin crates
│   ├── diagnostics/             # Diagnostics plugin
│   ├── diff-viewer/             # Diff viewer plugin
│   ├── llm-task/                # LLM task plugin
│   ├── media-office/            # Media office plugin
│   ├── memory-analytics/        # Memory analytics plugin
│   ├── phone-control/           # Phone control plugin
│   └── voice-call/              # Voice call plugin
├── apps/
│   └── webchat/                 # Web chat application assets
├── docs/
│   ├── reference/               # Architecture & system docs
│   └── superpowers/             # Design specs & run reports
├── justfile                     # Build pipeline
└── Cargo.toml                   # Workspace root
```

## Documentation

| Document | Link |
|----------|------|
| Architecture | [ARCHITECTURE.md](docs/reference/ARCHITECTURE.md) |
| Agent System | [AGENT_SYSTEM.md](docs/reference/AGENT_SYSTEM.md) |
| Gateway Protocol | [GATEWAY.md](docs/reference/GATEWAY.md) |
| Tool System | [TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) |
| Memory System | [MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md) |
| Extension System | [EXTENSION_SYSTEM.md](docs/reference/EXTENSION_SYSTEM.md) |
| Security | [SECURITY.md](docs/reference/SECURITY.md) |
| Sandbox | [SANDBOX.md](docs/reference/SANDBOX.md) |
| Design Patterns | [DESIGN_PATTERNS.md](docs/reference/DESIGN_PATTERNS.md) |
| Code Organization | [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md) |
| Domain Modeling | [DOMAIN_MODELING.md](docs/reference/DOMAIN_MODELING.md) |
| Agent Design Philosophy | [AGENT_DESIGN_PHILOSOPHY.md](docs/reference/AGENT_DESIGN_PHILOSOPHY.md) |
| Server Development | [SERVER_DEVELOPMENT.md](docs/reference/SERVER_DEVELOPMENT.md) |
| Session Service | [SESSION_SERVICE.md](docs/reference/SESSION_SERVICE.md) |
| Multi-Agent System | [MULTI_AGENT_SYSTEM.md](docs/reference/MULTI_AGENT_SYSTEM.md) |
| State Layer | [STATE_LAYER.md](docs/reference/STATE_LAYER.md) |
| Desktop Bridge | [DESKTOP_BRIDGE.md](docs/reference/DESKTOP_BRIDGE.md) |
| Glossary | [GLOSSARY.md](docs/reference/GLOSSARY.md) |

## Contributing

Single-branch development on `main`. Commit format: `<scope>: <description>` (English).

Example: `gateway: add WebSocket server foundation`

Before restarting Aleph in development:
```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
```
Multiple processes → HMAC failure → **vault data loss**.

## License

MIT. See [LICENSE](LICENSE).
