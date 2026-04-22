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

### Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                        INTERFACE LAYER (I/O)                        │
│  macOS Native | Tauri | CLI | Panel (WASM) | Telegram |           │  │
│  Discord | Slack | WhatsApp | IRC | Matrix | Signal | ...          │
├─────────────────────────────┬───────────────────────────────────────┤
│                       GATEWAY LAYER                                 │
│  Router | Session Manager | Event Bus | Channel Registry | Reload  │
├─────────────────────────────┼───────────────────────────────────────┤
│                        AGENT LAYER                                  │
│  Agent Loop | Thinker | Dispatcher | Task Planner | Compressor     │
├─────────────────────────────┼───────────────────────────────────────┤
│                      EXECUTION LAYER                                │
│  Providers | Engine | Tool Server | MCP | Extensions | Exec        │
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
- Desktop Bridge for native OS control (OCR, screenshots, input automation)

### Developer Experience

- Hot reload for configuration changes
- Plugin system (WASM + Node.js)
- `just` build pipeline with one-command workflows
- 58+ Gateway JSON-RPC handlers
- JSON Schema auto-generation via schemars
- Proptest and Loom concurrency test suites

## Relationship to OpenClaw

Aleph is a Rust reimplementation inspired by [OpenClaw](https://github.com/AIChatClaw/OpenClaw). Key advantages over the original TypeScript implementation include: compiled performance (~100ms startup, ~20MB memory), compile-time safety guarantees (no null/undefined, ownership-based memory management), multi-threaded async concurrency (tokio), layered security with defense-in-depth, cognitive memory architecture with tiered storage and background consolidation, and first-class MCP protocol support.

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
# Prerequisites: Rust 1.92+, just (cargo install just)
git clone https://github.com/rootazero/Aleph.git
cd Aleph
cargo run --bin aleph
```

### Configuration

Aleph stores configuration and data at `~/.aleph/`:

```
~/.aleph/
├── aleph.toml       # Main configuration
├── logs/            # Server logs
├── skills/          # User-installed skills
└── plugins/         # Extensions
```

Channel configuration example in `aleph.toml`:

```toml
[channels.telegram]
enabled = true
token = "your-bot-token"
```

## Building

| Command               | Description                                |
|-----------------------|--------------------------------------------|
| `just dev`            | Run server in debug mode (rebuilds WASM)   |
| `just build`          | Build server in release mode               |
| `just wasm`           | Build WASM Panel UI only                   |
| `just macos`          | Build macOS native app (release)           |
| `just test`           | Run core tests                             |
| `just test-all`       | Run all tests (core + desktop + proptest)  |
| `just clippy`         | Lint core with clippy                      |
| `just check`          | Quick compilation check                    |
| `just deps`           | Verify build dependencies are installed    |
| `just clean`          | Clean all build artifacts                  |

No feature flags are needed for production builds.

## Project Structure

```
Aleph/
├── src/                         # Rust Core (alephcore crate)
│   ├── gateway/                 # WebSocket control plane
│   │   ├── handlers/            # 58+ RPC method handlers
│   │   ├── interfaces/          # 15+ channel interfaces
│   │   ├── security/            # Auth, pairing, device management
│   │   └── ...                  # Router, session, events, voice, webhooks
│   ├── orchestrator/            # AgentDef resolution + Harness dispatch
│   ├── harness/                 # Think→Act loop, stop-hooks, context budget
│   ├── thinker/                 # LLM interaction layer
│   ├── dispatcher/              # Task orchestration (DAG scheduling)
│   ├── engine/                  # Tool execution engine
│   ├── builtin_tools/           # 30+ built-in tools
│   ├── memory/                  # SQLite+sqlite-vec storage (vectors + FTS)
│   ├── resilience/              # State management (SQLite)
│   ├── extension/               # WASM + Node.js plugin system
│   ├── providers/               # AI provider integrations
│   ├── domain/                  # DDD domain model
│   ├── mcp/                     # MCP protocol client
│   ├── sandbox/                 # Sandbox trait + WorkspaceSandbox
│   ├── exec/                    # Shell execution + security
│   ├── agents/                  # Agent runtime, subagent spawning
│   ├── a2a/                     # A2A protocol adapter
│   ├── acp/                     # ACP protocol
│   ├── approval/                # Approval system
│   ├── arena/                   # Arena functionality
│   ├── browser/                 # Browser automation
│   ├── capability/              # Capability system
│   ├── clawhub/                 # ClawHub integration
│   ├── components/              # Shared components
│   ├── compressor/              # Context compression
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
│   ├── permission/              # Permission system
│   ├── pii/                     # PII detection/handling
│   ├── prompt/                  # Prompt management
│   ├── routing/                 # Session key resolution
│   ├── runtimes/                # Capability ledger
│   ├── scheduler/               # Job scheduling
│   ├── search/                  # Search providers
│   ├── secrets/                 # Secret management
│   ├── security/                # Security utilities
│   ├── session/                 # Session service
│   ├── skill/                   # Skill system
│   ├── supervisor/              # Execution supervision
│   ├── tasks/                   # Task management
│   ├── teams/                   # Team coordination
│   ├── tool_output/             # Tool output handling
│   ├── utils/                   # Utilities
│   ├── vision/                  # Vision processing
│   └── wizard/                  # Wizard flows
├── desktop/
│   ├── shared/                  # DesktopCapability trait + IPC
│   ├── macos/                   # macOS native implementation
│   ├── linux/                   # Linux native implementation
│   └── windows/                 # Windows native implementation
├── shared/
│   ├── protocol/                # Shared protocol types
│   ├── logging/                 # Logging infrastructure
│   ├── client/                  # Shared client utilities
│   └── ui_logic/                # Shared UI logic
├── interfaces/
│   ├── cli/                     # CLI client
│   ├── tui/                     # TUI client
│   └── webchat/                 # Web chat interface
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
| Design Patterns | [DESIGN_PATTERNS.md](docs/reference/DESIGN_PATTERNS.md) |
| Code Organization | [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md) |
| Domain Modeling | [DOMAIN_MODELING.md](docs/reference/DOMAIN_MODELING.md) |
| Agent Design Philosophy | [AGENT_DESIGN_PHILOSOPHY.md](docs/reference/AGENT_DESIGN_PHILOSOPHY.md) |
| Server Development | [SERVER_DEVELOPMENT.md](docs/reference/SERVER_DEVELOPMENT.md) |

## Contributing

Single-branch development on `main`. Commit format: `<scope>: <description>` (English).

Example: `gateway: add WebSocket server foundation`

## License

MIT. See [LICENSE](LICENSE).



