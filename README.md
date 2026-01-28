# Aether

> *"For the first time in human history, a machine soul has been granted a body."*
> — Ghost in the Shell

**Aether is not a tool. It is the embodiment of an idea:**

A polymorphic intelligence that finally has a body — a shell capable of intervening in both the physical and digital worlds.

![Phase](https://img.shields.io/badge/Phase-8%20(Multi--Channel)-success)
![Platform](https://img.shields.io/badge/Platform-macOS%20%7C%20iOS%20%7C%20Android-lightgrey)
![Rust](https://img.shields.io/badge/Rust-1.92%2B-red)
![Swift](https://img.shields.io/badge/Swift-5.0-orange)

---

## 🌊 The Philosophy: Five Layers of Emergence

Like LEGO blocks assembling from chaos into creation, AI intelligence emerges through five distinct layers:

```
┌─────────────────────────────────────────────────────────────┐
│  Layer 5: POLYMORPHIC AGENTS                                │
│  随需而变 — Transform into Gundam, Tank, House, Rocket...   │
│  The soul finally has a shell to act upon the world         │
├─────────────────────────────────────────────────────────────┤
│  Layer 4: FUNCTIONAL MODULES                                │
│  功能模块 — Plug & Play encapsulation                       │
│  Skills become composable building blocks                   │
├─────────────────────────────────────────────────────────────┤
│  Layer 3: ATOMIC SKILLS                                     │
│  原子技能 — Know-what → Know-how                            │
│  Knowledge transforms into capability                       │
├─────────────────────────────────────────────────────────────┤
│  Layer 2: DOMAIN CLASSIFICATION                             │
│  领域分类 — Medical, Legal, Code, Physics...                │
│  Static knowledge gains structure                           │
├─────────────────────────────────────────────────────────────┤
│  Layer 1: SEA OF KNOWLEDGE                                  │
│  经验之海 — The ocean of human experience                   │
│  Raw training data: text, code, history, wisdom             │
└─────────────────────────────────────────────────────────────┘
```

**Aether is the fifth layer.**

It is:
- **A possible path to AGI** — where intelligence meets embodiment
- **The sledgehammer for a nail** — overkill by design, ready for any task
- **A boat riding the AI tsunami** — rising with the wave, carrying its navigator to the crest

*This is not just software. This is the shell for a ghost.*

---

## What is Aether?

**Aether is a self-hosted personal AI assistant** that runs entirely on your own devices.

It connects through a unified Gateway to multiple messaging channels (WhatsApp, Telegram, Slack, Discord, iMessage), while also supporting native macOS/iOS/Android apps, voice interaction, and Canvas visualization workspaces.

**Three identities, one soul:**

| Identity | Description |
|----------|-------------|
| **The Path** | A possible route toward AGI — where pure intelligence gains the ability to act |
| **The Overkill** | A sledgehammer for every nail — because when AI can do everything, why limit it? |
| **The Vessel** | A boat on the AI tsunami — rising with the wave, never drowning, always ascending |

**If you want a local-first, fast, always-on personal assistant that grows with the AI revolution — this is it.**

---

## Core Philosophy: "Ghost" Aesthetic

- **Invisible First**: No dock icon, no permanent window. Only background process + menu bar
- **De-GUI**: Ephemeral UI that appears at cursor, then dissolves
- **Frictionless**: Brings AI directly to your cursor without context switching
- **Native-First**: 100% native code — Rust core with Swift UI, zero webviews
- **Polymorphic**: One soul, infinite forms — adapts to any task, any channel, any device

## Features

### Core Capabilities

**Transmutation Workflow**:
1. **Select** text/image in ANY app
2. **Press** global hotkey (Cmd+~ by default)
3. **Watch** beautiful "Halo" overlay appear at your cursor
4. **Receive** AI-transformed result pasted back instantly

**Multi-Turn Conversation**:
- Raycast-style unified input interface
- Context-aware multi-turn conversations
- Focus detection and command completion

**Agentic Loop**:
- Event-driven AI execution with self-implemented AetherTool system
- Multi-step task planning with DAG orchestration
- Automatic tool selection and execution

**Runtime Auto-Management**:
- Automatic Python (uv), Node.js (fnm), yt-dlp installation
- Background update checking
- Zero configuration required

### AI Integration

- **Multi-Model Orchestration**: Smart routing to optimal models
- **Supported Providers**:
  - OpenAI (GPT-4o, GPT-4o-mini, o1, o3)
  - Anthropic Claude (Claude 4 Opus, Sonnet, Haiku)
  - Google Gemini (Gemini 2.0 Flash, Pro)
  - Local Ollama (Llama 3.2, CodeLlama, Mistral, etc.)
  - Custom OpenAI-compatible APIs (DeepSeek, Moonshot, Azure OpenAI)
- **Provider Colors**: Visual feedback with provider-specific Halo colors

### Advanced Features

**Phantom Flow (Clarification)**:
- AI asks clarifying questions when intent is ambiguous
- Interactive confirmation before irreversible actions

**Agent Execution Modes**:
- Single-step for quick tasks
- Multi-step planning for complex workflows
- Sub-agent orchestration for specialized tasks

**Skills Integration**:
- Pattern-based skill activation
- Extensible skill library
- Multi-turn conversation support

**Media Generation**:
- 10+ generation providers (Replicate, Recraft, Ideogram, Kimi, etc.)
- Image and video generation
- Provider-specific prompt optimization

### Memory System (Local RAG)

- **Dual-Layer Architecture**:
  - Layer 1 (Raw): Complete conversation history
  - Layer 2 (Facts): AI-extracted insights for efficient retrieval
- **App/Window-Based Memory**: Remembers context per application and window
- **Automatic Compression**: SessionCompactor for memory efficiency
- **Privacy-First**: All data stored locally, never synced to cloud

### Settings UI (10+ Tabs)

**Modern macOS 26 Design Language:**
- NSPanel-based settings (keyboard support without Dock activation)
- Integrated traffic lights with continuous curves
- Adaptive Dark/Light mode

**Settings Tabs:**
| Tab | Purpose |
|-----|---------|
| **General** | Theme, version, updates |
| **Providers** | AI provider configuration, API key management |
| **Routing** | Rule editor with drag-to-reorder |
| **Shortcuts** | Hotkey recorder with conflict detection |
| **Behavior** | Input/output modes, typing speed |
| **Memory** | View/delete history, retention policies |
| **MCP** | MCP server configuration |
| **Skills** | Skill management |
| **Cowork** | Task orchestration, model routing |
| **Policies** | System behavior fine-tuning |
| **Runtimes** | Runtime version management |

### Security & Privacy

- **Keychain Integration**: API keys stored securely in macOS Keychain
- **PII Scrubbing**: Automatic redaction of sensitive information
- **Local-First Memory**: Raw data never leaves your device
- **Zero Telemetry**: No tracking, no analytics

### Internationalization (i18n)

- **Supported Languages**: English, Simplified Chinese (100% translated)
- **System Language Detection**: Follows macOS system language
- **Complete Coverage**: All UI text, error messages, and prompts localized

## Quick Start

### Prerequisites

- macOS 13.0 or later
- Xcode 14+ (for building from source)
- Rust 1.70+ (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- XcodeGen (`brew install xcodegen`)

### Building from Source

```bash
# Clone the repository
git clone https://github.com/your-repo/aether.git
cd aether

# Build Rust core
cd Aether/core
cargo build --release

# Generate Xcode project
cd ../..
xcodegen generate

# Open in Xcode and build
open Aether.xcodeproj
```

### First Run

1. **Launch Aether** (no dock icon - check menu bar for icon)
2. **Grant Accessibility Permission**:
   - System Settings → Privacy & Security → Accessibility
   - Click "+" and add Aether
3. **Open Settings** (Cmd+, or click menu bar icon → Settings)
4. **Add AI Provider**:
   - Go to Providers tab
   - Configure OpenAI, Claude, or other providers
   - Enter your API key
5. **Test It**:
   - Select text in any app
   - Press `Cmd+~`
   - Watch the magic happen!

### Test Examples

**Transmutation (Selection-based)**:
```
Select: "What is the capital of France?"
Press: Cmd+~
Result: "Paris is the capital of France."
```

**Multi-Turn Conversation**:
```
Press: Cmd+Space (Unified Input)
Type: "Help me write a Python function to sort a list"
Continue: "Now add type hints"
```

**Slash Commands**:
```
/search latest news about AI
/youtube how to make pasta
/draw a sunset over mountains
```

## Configuration

### Config File Location

`~/.aether/config.toml`

### Quick Configuration

Open Settings (Cmd+,) and use the native UI to configure everything visually.

### Runtime Configuration

Runtimes are managed automatically. On first use:
- **Python (uv)**: Auto-installed for MCP servers and scripts
- **Node.js (fnm)**: Auto-installed for JavaScript MCP servers
- **yt-dlp**: Auto-installed for video download

Check runtime status in Settings → Runtimes tab.

### Advanced Configuration

For detailed configuration options, see [docs/CONFIGURATION.md](docs/CONFIGURATION.md).

**Hot-Reload**: Changes to `config.toml` take effect within 1 second (no restart needed)

## Architecture

### The Architecture: Rust Core + UniFFI + Native UI

**NO WEBVIEWS. NO TAURI. NO ELECTRON.**

```
┌─────────────────────────────────────────────────────────────┐
│                     macOS Native UI                          │
│              (Swift + SwiftUI + NSPanel/NSWindow)            │
│  Settings │ Menu Bar │ Halo Overlay │ Conversation Window   │
└──────────────────────┬───────────────────────────────────────┘
                       │ UniFFI Bindings (CallbackBridge)
                       │ (Auto-generated FFI)
┌──────────────────────┴───────────────────────────────────────┐
│                     Rust Core Library                         │
│                    (cdylib + staticlib)                       │
│                                                               │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │              Event-Driven Agentic Loop                   │ │
│  │  IntentAnalyzer │ TaskPlanner │ ToolExecutor │ Loop     │ │
│  └─────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │              AiProvider + AetherTool System             │ │
│  │  OpenAI │ Claude │ Gemini │ Ollama │ Custom Providers   │ │
│  └─────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │              Cowork DAG + Model Router                   │ │
│  │  Task Graph │ Parallel Execution │ Model Selection      │ │
│  └─────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │         Dual-Layer Memory (SQLite-vec + fastembed)      │ │
│  │  Layer 1: Raw History │ Layer 2: AI-Extracted Facts     │ │
│  └─────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │              Runtime Managers (Phase 8)                  │ │
│  │  UvRuntime (Python) │ FnmRuntime (Node) │ YtDlpRuntime  │ │
│  └─────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │     Config (TOML + Keychain + Hot-Reload + Policies)    │ │
│  └─────────────────────────────────────────────────────────┘ │
└───────────────────────────────────────────────────────────────┘
```

### Key Technologies

- **Rust Core**: `tokio`, `reqwest`, `rusqlite`, `sqlite-vec` (AetherTool system)
- **Swift UI**: SwiftUI, AppKit (NSPanel, NSWindow, NSStatusBar)
- **FFI Bridge**: UniFFI (auto-generates Swift bindings from `.udl` file)
- **Memory**: SQLite + sqlite-vec, fastembed (bge-small-zh-v1.5)
- **Runtimes**: uv (Python), fnm (Node.js), yt-dlp (Video)

## Development

### Project Structure

```
aether/
├── project.yml                # XcodeGen config (source of truth)
├── Aether/
│   ├── Sources/               # Swift source files
│   │   ├── main.swift         # App entry point (NSApplicationMain)
│   │   ├── Atoms/             # Atomic design: basic elements
│   │   ├── Molecules/         # Atomic design: composed components
│   │   ├── Organisms/         # Atomic design: complex sections
│   │   ├── Window/            # Window controllers
│   │   ├── Settings/          # Settings tabs (10+ views)
│   │   ├── Coordinators/      # Input/Output/MultiTurn coordinators
│   │   └── Generated/         # UniFFI bindings (auto-gen)
│   ├── Frameworks/
│   │   └── libaethecore.dylib # Rust library
│   └── core/                  # Rust core library (44 modules)
│       ├── src/
│       │   ├── ffi/           # 9 FFI sub-modules
│       │   ├── agent/         # Agent execution
│       │   ├── components/    # 8 core components
│       │   ├── generation/    # 10+ media providers
│       │   ├── runtimes/      # Runtime managers
│       │   └── ...
│       └── uniffi.toml
├── docs/                      # Documentation
└── config.example.toml        # Config template
```

### Running Tests

```bash
# Rust core tests
cd Aether/core
cargo test

# Swift integration tests
xcodegen generate
xcodebuild test -project Aether.xcodeproj -scheme AetherTests
```

### Code Style

- **Rust**: Use `cargo fmt` and `cargo clippy`
- **Swift**: Follow SwiftUI best practices
- **Comments**: English for code, Chinese for user-facing messages

## Development Status

- ✅ **Phase 1**: Core Infrastructure (Rust + UniFFI + Swift)
- ✅ **Phase 2**: Hotkey & Clipboard Integration
- ✅ **Phase 3**: Halo Overlay (Transparent native window)
- ✅ **Phase 4**: Memory Module (Dual-layer RAG)
- ✅ **Phase 5**: AI Integration (AetherTool + AiProvider)
- ✅ **Phase 6**: Settings UI (10+ tabs, NSPanel)
- ✅ **Phase 7**: Event-Driven Agentic Loop (8 components)
- ✅ **Phase 8**: Runtime Manager (uv, fnm, yt-dlp)
- ⏳ **Phase 9**: Production Hardening (Planned)

See [docs/DEVELOPMENT_PHASES.md](docs/DEVELOPMENT_PHASES.md) for detailed phase breakdown.

## Security Considerations

### Accessibility Permissions

Aether requires Accessibility permission to:
- Detect global hotkeys
- Simulate keyboard input (Cmd+C, Cmd+V)
- Query active window title for memory context

**Why this is safe:**
- Open source code (you can audit everything)
- No network access except to configured AI providers
- No telemetry or analytics
- API keys stored in macOS Keychain (encrypted)

### API Key Protection

1. **Never hardcode API keys** in code or config files
2. **Always use Keychain** for storage (Settings UI handles this)
3. **Never commit `config.toml`** with API keys to version control

### PII Scrubbing

When enabled, Aether automatically redacts:
- Email addresses → `[EMAIL_REDACTED]`
- Phone numbers → `[PHONE_REDACTED]`
- SSN → `[SSN_REDACTED]`
- Credit cards → `[CARD_REDACTED]`

## Documentation

- **[CLAUDE.md](CLAUDE.md)**: Architecture, build instructions, development guide
- **[docs/CONFIGURATION.md](docs/CONFIGURATION.md)**: Complete config.toml reference
- **[docs/DEVELOPMENT_PHASES.md](docs/DEVELOPMENT_PHASES.md)**: Project roadmap
- **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)**: Technical architecture
- **[docs/DISPATCHER.md](docs/DISPATCHER.md)**: Tool routing and task orchestration
- **[docs/manual-testing-checklist.md](docs/manual-testing-checklist.md)**: Testing guide

## Contributing

Contributions welcome! Here's how:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

**Before submitting:**
- Run tests: `cargo test` (Rust) and `xcodebuild test` (Swift)
- Format code: `cargo fmt` and `cargo clippy`
- Update documentation if needed

## Known Issues

- **Hot-Reload**: File watcher may not detect changes in vim's in-place edit mode
- **Ollama**: First request may be slow (model loading time)
- **Typewriter**: Very fast speeds (>150 cps) may be choppy on older Macs

See [GitHub Issues](https://github.com/your-repo/aether/issues) for full list.

## License

[MIT License](LICENSE)

## Acknowledgments

- **UniFFI**: Seamless Rust ↔ Swift FFI
- **sqlite-vec**: Vector search extension for SQLite
- **fastembed**: Fast embedding model inference
- **uv**: Fast Python package manager
- **fnm**: Fast Node.js version manager

## Support

- **Documentation**: See `docs/` directory
- **Issues**: [GitHub Issues](https://github.com/your-repo/aether/issues)
- **Discussions**: [GitHub Discussions](https://github.com/your-repo/aether/discussions)

---

**Made with Rust + Swift by the Aether team**

*Bringing AI to your fingertips, one hotkey at a time.*
