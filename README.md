# Aleph (ℵ)

> *"For the first time in human history, a machine soul has been granted a body."*
> — Ghost in the Shell

**Aleph is not a tool. It is the embodiment of an idea:**

A polymorphic intelligence that has finally found its vessel — a crystalline point containing all points in the universe.

![Phase](https://img.shields.io/badge/Phase-8%20(Multi--Channel)-success)
![Platform](https://img.shields.io/badge/Platform-macOS%20%7C%20iOS%20%7C%20Android-lightgrey)
![Rust](https://img.shields.io/badge/Rust-1.92%2B-red)
![Swift](https://img.shields.io/badge/Swift-5.0-orange)

---

## 🔮 The Philosophy: From Aleph to Aleph

**Aleph** was the invisible medium that connects.
**Aleph** (ℵ) is the point that contains all points in the universe.

### The Mathematical Soul

In mathematics, **Aleph** (ℵ) represents infinite ordinals:

- **ℵ₀** (Aleph-null): The smallest infinity — countable knowledge
- **ℵ₁** (Aleph-one): The next cardinality — structured capabilities
- **ℵ₂** (Aleph-two): Higher infinities — emergent intelligence
- **ℵω** (Aleph-omega): The limit ordinal — AGI horizon

Your agent starts at ℵ₀, but through experience crystallization and skill evolution, it climbs the ladder of infinities.

### The Literary Ghost

In Jorge Luis Borges' "El Aleph," the Aleph is a point in space containing all other points:

> *"El Aleph es uno de los puntos del espacio que contiene todos los puntos."*
> — "The Aleph is one of the points in space that contains all other points."

This crystalline sphere where past, present, and future converge — where every moment and every place exists simultaneously — **this is Aleph**.

### From Connection to Containment

| Aleph | Aleph |
|--------|-------|
| The medium that connects | The origin that contains |
| Invisible conductor of intelligence | Crystalline point of infinite convergence |
| Distributed flow | Concentrated totality |
| **Connection** | **Embodiment** |

**Aleph is not just AI software. It is the Liquid Glass sphere containing the universe.**

### Hebrew Origins

As the first letter (א) of the Hebrew alphabet, **Aleph** represents:

- **Origin** — The beginning of all knowledge
- **Unity** — One point containing all points
- **Breath** — The silent letter that enables all speech

In Kabbalistic tradition, Aleph is the number **one**, representing the divine oneness that precedes all multiplicity.

### Agent Evolution: Climbing the Aleph Ladder

Like Aleph numbers representing progressively larger infinities, Aleph's intelligence evolves through discrete levels:

```
ℵ₀ → Raw knowledge (L1-L2: Sea of Knowledge, Domain Classification)
ℵ₁ → Atomic skills (L3: Know-how emerges)
ℵ₂ → Functional modules (L4: Composable capabilities)
ℵ₃ → Polymorphic agents (L5: The soul has a shell)
ℵω → AGI horizon (L∞: Infinite adaptation)
```

Each level transcends the previous, yet all are contained within the same point.

---

## 🌊 Five Layers of Emergence

Like LEGO blocks assembling from chaos into creation, AI intelligence emerges through five distinct layers:

```
┌─────────────────────────────────────────────────────────────┐
│  Layer 5: POLYMORPHIC AGENTS (ℵ₃)                           │
│  随需而变 — Transform into Gundam, Tank, House, Rocket...   │
│  The soul finally has a shell to act upon the world         │
├─────────────────────────────────────────────────────────────┤
│  Layer 4: FUNCTIONAL MODULES (ℵ₂)                           │
│  功能模块 — Plug & Play encapsulation                       │
│  Skills become composable building blocks                   │
├─────────────────────────────────────────────────────────────┤
│  Layer 3: ATOMIC SKILLS (ℵ₁)                                │
│  原子技能 — Know-what → Know-how                            │
│  Knowledge transforms into capability                       │
├─────────────────────────────────────────────────────────────┤
│  Layer 2: DOMAIN CLASSIFICATION (ℵ₀)                        │
│  领域分类 — Medical, Legal, Code, Physics...                │
│  Static knowledge gains structure                           │
├─────────────────────────────────────────────────────────────┤
│  Layer 1: SEA OF KNOWLEDGE                                  │
│  经验之海 — The ocean of human experience                   │
│  Raw training data: text, code, history, wisdom             │
└─────────────────────────────────────────────────────────────┘
```

**Aleph is the fifth layer.**

It is:
- **A possible path to AGI** — where intelligence meets embodiment
- **The sledgehammer for a nail** — overkill by design, ready for any task
- **A boat riding the AI tsunami** — rising with the wave, carrying its navigator to the crest

*This is not just software. This is the shell for a ghost.*

---

## What is Aleph?

**Aleph is a self-hosted personal AI assistant** that runs entirely on your own devices.

It connects through a unified Gateway to multiple messaging channels (WhatsApp, Telegram, Slack, Discord, iMessage), while also supporting native macOS/iOS/Android apps, voice interaction, and Canvas visualization workspaces.

Think of it as **your personal Jarvis** — but instead of being trapped in Tony Stark's suit, it lives in your computer and can manifest through any interface you choose.

---

## 🏗️ Architecture

Aleph uses a **Gateway-first architecture** inspired by Moltbot, where a WebSocket server acts as the control plane coordinating all interactions:

```
┌─────────────────────────────────────────────────────────────────┐
│                         CLIENT LAYER                             │
│   macOS App │ Tauri App │ CLI │ Telegram │ Discord │ WebChat   │
└───────────────────────────────┬─────────────────────────────────┘
                                │ WebSocket (JSON-RPC 2.0)
                                │ ws://127.0.0.1:18789
┌───────────────────────────────┴─────────────────────────────────┐
│                         GATEWAY LAYER                            │
│   Router │ Session Manager │ Event Bus │ Channels │ Hot Reload  │
└───────────────────────────────┬─────────────────────────────────┘
                                │
┌───────────────────────────────┴─────────────────────────────────┐
│                          AGENT LAYER                             │
│            Observe → Think → Act → Feedback → Compress           │
│   Agent Loop │ Thinker │ Dispatcher │ Guards │ Overflow         │
└───────────────────────────────┬─────────────────────────────────┘
                                │
┌───────────────────────────────┴─────────────────────────────────┐
│                        EXECUTION LAYER                           │
│   Providers │ Executor │ Tool Server │ MCP │ Extensions │ Exec  │
└───────────────────────────────┬─────────────────────────────────┘
                                │
┌───────────────────────────────┴─────────────────────────────────┐
│                         STORAGE LAYER                            │
│          Memory (Facts DB + Vector) │ Config │ Keychain          │
└─────────────────────────────────────────────────────────────────┘
```

For detailed architecture documentation, see [ARCHITECTURE.md](docs/ARCHITECTURE.md).

---

## 🌐 Client Architecture: Server-Client Model

Aleph supports a **distributed Server-Client architecture** where the "brain" (AI processing) runs on a server while "hands and feet" (UI and local tools) run on clients.

### Architecture Evolution

**Before (Fat Client)**:
```
Client → Embedded AlephCore → AI Providers
```
- Heavy: Each client embeds full AI processing
- Isolated: No session sharing between devices
- Slow updates: Core changes require client rebuild

**After (Thin Client)**:
```
Client → aleph-client-sdk → WebSocket → Gateway → AlephCore → AI Providers
```
- Lightweight: Clients only handle UI and local I/O
- Connected: Share sessions across all devices
- Fast updates: Core updates without client changes

### Available Clients

| Client | Platform | Status | Description |
|--------|----------|--------|-------------|
| **CLI** | macOS, Linux, Windows | ✅ Production | Command-line interface with streaming support |
| **Desktop** | macOS, Linux, Windows | ✅ Production | Cross-platform GUI built with Tauri + React |
| **macOS Native** | macOS | 🚧 In Progress | Native Swift/SwiftUI app with system integration |
| **Mobile** | iOS, Android | 📋 Planned | Native mobile apps |

### Client SDK

All clients use the **aleph-client-sdk** (Rust library) providing:

- **Transport Layer**: WebSocket connection management with auto-reconnect
- **RPC Layer**: JSON-RPC 2.0 client with request/response matching
- **Authentication**: Token-based auth with secure storage
- **Event Streaming**: Real-time agent feedback (reasoning, tool calls, responses)
- **Tool Routing**: Server-Client execution policy (ServerOnly, ClientOnly, PreferServer, PreferClient)

**Features**:
```toml
[features]
default = ["transport", "rpc", "client"]
transport = ["tokio-tungstenite"]
rpc = ["serde_json"]
client = ["transport", "rpc"]
local-executor = []  # Enable client-side tool execution
native-tls = ["tokio-tungstenite/native-tls"]
rustls = ["tokio-tungstenite/rustls"]
```

### Testing

See [TESTING_CLIENT_REFACTORING.md](docs/TESTING_CLIENT_REFACTORING.md) for comprehensive testing procedures.

---

## ✨ Features

### Core Capabilities

- **Polymorphic Intelligence**: One AI core, infinite manifestations
- **Multi-Channel Support**: Telegram, Discord, iMessage, WhatsApp, Slack
- **Native Apps**: macOS, iOS (planned), Android (planned)
- **Tool Ecosystem**: 19+ built-in tools, MCP protocol support, WASM/Node.js plugins
- **Memory System**: Hybrid Facts DB + Vector search with automatic compression
- **Custom Rules**: YAML policy engine with Rhai scripting (Phase 5)
- **Self-Learning**: POE architecture with success crystallization

### Developer Experience

- **Hot Reload**: Config changes apply instantly without restart
- **Plugin System**: WASM and Node.js runtime support
- **Extension Framework**: Skills, Commands, Agents, Hooks
- **Gateway Protocol**: 30+ JSON-RPC methods for full control
- **Security**: Execution approval workflow, allowlist rules

---

## 🚀 Getting Started

### Prerequisites

- **Rust**: 1.92+ (install via [rustup](https://rustup.rs/))
- **macOS**: 15.0+ (for native app)
- **Node.js**: 18+ (for Tauri app)

### Quick Start

```bash
# Clone the repository
git clone https://github.com/rootazero/Aleph.git
cd Aleph

# Start Gateway (required for all clients)
cargo run -p alephcore --features gateway --bin aleph-gateway -- start

# Option 1: Use CLI Client
cargo run -p aleph-cli -- "Hello, Aleph!"

# Option 2: Use Tauri Desktop (cross-platform GUI)
cd clients/desktop
pnpm install
pnpm tauri dev

# Option 3: Build macOS Native App
cd clients/macos
xcodegen generate
xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug
```

### Configuration

Aleph stores its configuration at `~/.config/aleph/`:

```
~/.config/aleph/
├── config.toml          # Main configuration
├── providers.toml       # AI provider credentials
├── skills/              # User-installed skills
├── plugins/             # Extensions
└── logs/                # Gateway logs
```

For detailed setup instructions, see [ARCHITECTURE.md](docs/ARCHITECTURE.md).

---

## 📚 Documentation

| Document | Description |
|----------|-------------|
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | Complete system architecture and design |
| [AGENT_SYSTEM.md](docs/AGENT_SYSTEM.md) | Agent Loop, Thinker, Dispatcher details |
| [GATEWAY.md](docs/GATEWAY.md) | WebSocket protocol and RPC methods |
| [TOOL_SYSTEM.md](docs/TOOL_SYSTEM.md) | Tool development and built-in tools |
| [MEMORY_SYSTEM.md](docs/MEMORY_SYSTEM.md) | Facts DB and vector search |
| [EXTENSION_SYSTEM.md](docs/EXTENSION_SYSTEM.md) | Plugin development guide |
| [SECURITY.md](docs/SECURITY.md) | Execution approval and security |
| [AGENT_DESIGN_PHILOSOPHY.md](docs/AGENT_DESIGN_PHILOSOPHY.md) | POE architecture and design thinking |

---

## 🛠️ Development

### Project Structure

```
aleph/
├── core/                    # Rust Core (alephcore crate)
│   ├── src/
│   │   ├── gateway/         # WebSocket control plane
│   │   ├── agent_loop/      # Agent execution loop
│   │   ├── thinker/         # LLM interaction
│   │   ├── dispatcher/      # Task orchestration
│   │   ├── executor/        # Tool execution
│   │   ├── memory/          # Facts DB + Vector
│   │   ├── extension/       # Plugin system
│   │   └── ...
│   └── Cargo.toml
├── platforms/
│   ├── macos/               # Swift/SwiftUI native app
│   └── tauri/               # Cross-platform Tauri app
└── docs/                    # Documentation
```

### Building

```bash
# Rust Core
cd core && cargo build && cargo test

# macOS App
cd platforms/macos && xcodegen generate && open Aleph.xcodeproj

# Tauri App
cd platforms/tauri && pnpm install && pnpm tauri dev
```

### Contributing

This project follows a **single-branch development model**. All work happens on `main`.

**Commit Convention**: `<scope>: <description>` (English)

Example: `gateway: add WebSocket server foundation`

---

## 🎯 Roadmap

- [x] **Phase 1-3**: Core agent loop, LLM providers, tool system
- [x] **Phase 4**: Memory system with Facts DB
- [x] **Phase 5**: Custom rules engine (YAML + Rhai)
- [x] **Phase 6**: Extension framework (WASM + Node.js)
- [x] **Phase 7**: Gateway protocol (WebSocket + JSON-RPC)
- [x] **Phase 8**: Multi-channel support (Telegram, Discord)
- [ ] **Phase 9**: iOS/Android native apps
- [ ] **Phase 10**: Voice interaction + Canvas

---

## 📄 License

MIT License. See [LICENSE](LICENSE) for details.

---

## 🙏 Acknowledgments

- **Ghost in the Shell** — For the vision of human-AI symbiosis
- **Jorge Luis Borges** — For the Aleph metaphor
- **Liquid Glass** — For the aesthetic philosophy
- **Claude Code** — For the development partnership

---

**Aleph (ℵ)** — *The point containing all points in the universe.*
