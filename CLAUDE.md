<!-- OPENSPEC:START -->
# OpenSpec Instructions

These instructions are for AI assistants working in this project.

Always open `@/openspec/AGENTS.md` when the request:
- Mentions planning or proposals (words like proposal, spec, change, plan)
- Introduces new capabilities, breaking changes, architecture shifts, or big performance/security work
- Sounds ambiguous and you need the authoritative spec before coding

Use `@/openspec/AGENTS.md` to learn:
- How to create and apply change proposals
- Spec format and conventions
- Project structure and guidelines

Keep this managed block so 'openspec update' can refresh the instructions.

<!-- OPENSPEC:END -->

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Aether** is a system-level AI middleware for macOS (primary), Windows, and Linux. It acts as an invisible "ether" connecting user intent with AI models through a frictionless, native interface with zero webview dependencies.

**Current Status**: Phase 8 Completed (Runtime Manager) | Phase 9 Planned (Production Hardening)

### Core Philosophy: "Ghost" Aesthetic

- **Invisible First**: No dock icon, no permanent window. Only background process + menu bar/system tray
- **De-GUI**: Ephemeral UI that appears at cursor, then dissolves
- **Frictionless**: Brings AI intelligence directly to the cursor without context switching
- **Native-First**: 100% native code - Rust core with platform-specific UI (Swift, C#, GTK)

### User Interaction Flows

**Selection-Based Flow ("Transmutation")**:
1. User selects text/image in ANY app, presses global hotkey (default: ` key)
2. Aether simulates Cut (Cmd+X) - content "disappears" for physical feedback
3. "Halo" appears at cursor location (native transparent overlay)
4. Backend routes request to appropriate AI provider via rig-core
5. Halo dissolves, result is pasted back or typed character-by-character

**Unified Input Flow**: Raycast-style interface with focus detection, command completion, and multi-turn conversation. See `UnifiedInputCoordinator.swift` and `UnifiedConversationWindow.swift`.

---

## Technical Stack

### Architecture: "Rust Core + rig-core + UniFFI + Native UI"

**NO WEBVIEWS. NO TAURI. NO ELECTRON.**

1. **Rust Core (Library)**: Headless service compiled as `cdylib` + `staticlib`
   - **rig-core 0.28**: AI agent framework for provider abstraction
   - **rig-sqlite 0.1.31**: Conversation persistence
   - **UniFFI**: Generates Swift/Kotlin/C# bindings automatically
   - Async runtime (`tokio`), HTTP client (`reqwest`)
   - **Memory Module**: `rusqlite` + `sqlite-vec` + `fastembed` (bge-small-zh-v1.5)
   - **Note**: Hotkey, clipboard, input simulation migrated to Swift layer

2. **Native UIs (Platform-Specific)**:
   - **macOS**: Swift + SwiftUI with NSApplicationMain() entry point
   - **Settings**: NSPanel (keyboard support without Dock activation)
   - **Halo**: NSWindow (transparent overlay, click-through)
   - **Windows** (Future): C# + WinUI 3
   - **Linux** (Future): Rust + GTK4

3. **Communication Pattern**: Rust → UniFFI → Swift (callback-based via `CallbackBridge`)

See [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) for complete technical documentation.

---

## Project Structure

**Note**: This project uses [XcodeGen](https://github.com/yonaskolb/XcodeGen). The `.xcodeproj` is generated from `project.yml`.

```
aether/
├── project.yml                    # XcodeGen configuration (source of truth)
├── Aether/
│   ├── Sources/                   # Swift source files
│   │   ├── main.swift             # App entry point (NSApplicationMain)
│   │   ├── AppDelegate.swift      # Menu bar lifecycle + Rust core integration
│   │   ├── Atoms/                 # Atomic design: basic UI elements
│   │   ├── Molecules/             # Atomic design: composed components
│   │   ├── Organisms/             # Atomic design: complex UI sections
│   │   ├── Window/                # Window controllers (Halo, Settings, Conversation)
│   │   ├── Settings/              # Settings tabs (10+ views)
│   │   ├── Coordinators/          # Input/Output/MultiTurn/PermissionCoordinator
│   │   └── Generated/aether.swift # UniFFI-generated bindings
│   ├── Frameworks/
│   │   └── libaethecore.dylib     # Rust library (embedded in app)
│   └── core/                      # Rust core library
│       ├── Cargo.toml
│       ├── src/
│       │   ├── lib.rs             # UniFFI exports and public API
│       │   ├── aether.udl         # UniFFI interface definition
│       │   ├── ffi/               # 9 FFI sub-modules
│       │   ├── agent/             # Agent execution engine
│       │   ├── agents/            # Specialized agents
│       │   ├── capability/        # Capability definitions
│       │   ├── components/        # 8 core components
│       │   ├── config/            # 10 config type modules + policies
│       │   ├── conversation/      # Conversation management
│       │   ├── cowork/            # DAG task orchestration
│       │   ├── dispatcher/        # Multi-layer routing
│       │   ├── event/             # Event system
│       │   ├── generation/        # 10+ media generation providers
│       │   ├── intent/            # 3-layer intent detection
│       │   ├── mcp/               # MCP integration (stdio transport)
│       │   ├── memory/            # Dual-layer memory system
│       │   ├── payload/           # Request payload building
│       │   ├── providers/         # AI provider implementations
│       │   ├── rig_tools/         # rig-core tool definitions
│       │   ├── router/            # Smart routing logic
│       │   ├── runtimes/          # Runtime managers (uv, fnm, yt-dlp)
│       │   ├── search/            # 6 search providers
│       │   ├── services/          # Background services
│       │   ├── skills/            # Skill system
│       │   ├── video/             # Video processing
│       │   ├── vision/            # OCR + image understanding
│       │   └── clarification/     # Phantom Flow (user clarification)
│       └── uniffi.toml
├── docs/                          # Documentation
└── config.toml                    # User config (~/.config/aether/config.toml)
```

### Rust Core Module Count: 44 Modules

| Category | Modules |
|----------|---------|
| **FFI** | 9 sub-modules |
| **Agent** | agent/, agents/, components/ (8 modules) |
| **Config** | 10 type modules + policies |
| **AI** | generation/ (10+ providers), providers/, rig_tools/ |
| **Memory** | Dual-layer (Raw + Facts), compression |
| **Routing** | dispatcher/, intent/ (3 layers), router/ |
| **Tools** | mcp/, skills/, search/ (6 providers), video/, vision/ |
| **Runtime** | runtimes/ (uv, fnm, yt-dlp) |
| **Infra** | services/, event/, conversation/, cowork/, payload/ |

---

## Build Commands

### Building Rust Core

```bash
cd Aether/core/

# Development build
cargo build

# Release build
cargo build --release

# Generate UniFFI bindings
cargo run --bin uniffi-bindgen generate src/aether.udl \
  --language swift --out-dir ../Sources/Generated/

# Copy library
cp target/release/libaethecore.dylib ../Frameworks/
```

### Building macOS Client

```bash
xcodegen generate                  # Generate Xcode project
open Aether.xcodeproj              # Open in Xcode
# Or:
xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Release build
```

### Testing

```bash
cd Aether/core/
cargo test                         # All tests
cargo test router                  # Module-specific tests
```

---

## Key Architecture Components

### 1. Event-Driven Agentic Loop (8 Components)

The core AI execution engine using rig-core 0.28:

| Component | Location | Purpose |
|-----------|----------|---------|
| **IntentAnalyzer** | `intent/` | 3-layer intent detection (L1 Regex, L2 Semantic, L3 LLM) |
| **TaskPlanner** | `agent/` | Multi-step task planning with DAG execution |
| **ToolExecutor** | `components/tool_executor.rs` | Unified tool dispatch system |
| **LoopController** | `components/loop_controller.rs` | Agentic loop state management |
| **SessionRecorder** | `components/session_recorder.rs` | Conversation history tracking |
| **SessionCompactor** | `components/session_compactor.rs` | Memory compression |
| **SubAgentHandler** | `components/subagent_handler.rs` | Sub-agent orchestration |
| **CallbackBridge** | `components/callback_bridge.rs` | Rust-Swift communication |

### 2. rig-core AI Agent Integration

```rust
// AI providers via rig-core
rig_core::providers::openai::Client
rig_core::providers::anthropic::Client
rig_core::providers::gemini::Client

// Conversation persistence
rig_sqlite::SqliteStore  // Conversation history
```

### 3. Runtime Managers (Phase 8)

Automatic runtime environment management:

| Runtime | Purpose | Auto-Install |
|---------|---------|--------------|
| **UvRuntime** | Python environment (uv) | Yes |
| **FnmRuntime** | Node.js environment (fnm) | Yes |
| **YtDlpRuntime** | Video download (yt-dlp) | Yes |

```rust
// Common interface
pub trait RuntimeManager: Send + Sync {
    async fn is_installed(&self) -> bool;
    async fn install(&self) -> Result<()>;
    async fn check_updates(&self) -> Result<Option<String>>;
    async fn update(&self) -> Result<()>;
}
```

### 4. Dual-Layer Memory System

- **Layer 1 (Raw)**: Complete conversation history with full context
- **Layer 2 (Facts)**: AI-extracted facts and insights for efficient retrieval
- **SessionCompactor**: Background compression of old conversations

### 5. Cowork DAG Orchestration + Model Router

```
User Request → TaskPlanner → DAG Graph → ModelRouter → Parallel Execution
                                              ↓
                              Route each task to optimal model
                              (claude-opus for reasoning,
                               claude-haiku for quick tasks)
```

See [docs/COWORK.md](./docs/COWORK.md) for details.

### 6. Media Generation (10+ Providers)

| Provider | Type | Models |
|----------|------|--------|
| Replicate | Image/Video | Flux, SDXL |
| Recraft | Image | V3 |
| Ideogram | Image | V2 |
| Kimi | Image | Visions |
| OpenAI | Image | DALL-E 3 |
| Gemini | Image | Imagen |
| ... | ... | ... |

### 7. MCP Integration

Model Context Protocol for external tool integration:
- Transport: stdio (subprocess)
- Configuration: `[mcp.servers]` in config.toml

### 8. Skills System

Automatic skill matching based on input patterns:
- Skill definitions in `skills/`
- Pattern-based activation
- Multi-turn conversation support

### 9. Vision Capability

- **OCR**: Text extraction from images
- **Image Understanding**: Visual content analysis via AI providers

### 10. Phantom Flow (Clarification System)

When user intent is ambiguous, Aether can ask clarifying questions before proceeding.

---

## Settings UI Tabs (10+)

| Tab | Purpose |
|-----|---------|
| **General** | Theme, version, updates |
| **Providers** | AI provider configuration |
| **Routing** | Rule editor with drag-to-reorder |
| **Shortcuts** | Hotkey recorder |
| **Behavior** | Input/output modes |
| **Memory** | View/delete history, retention policies |
| **MCP** | MCP server configuration |
| **Skills** | Skill management |
| **Cowork** | Task orchestration settings |
| **Policies** | System behavior fine-tuning |
| **Runtimes** | Runtime version management |

---

## Key Design Constraints

### Modularity Requirements

Use trait-based abstractions for all core components:
- `AiProvider`, `Router`, `MemoryStore`
- `EmbeddingModel`, `SearchProvider`
- `RuntimeManager`

### Critical UI Behavior

**macOS Application Entry**:
- Use `main.swift` + `NSApplicationMain()` (macOS 26 bug workaround)
- Do NOT use SwiftUI `@main App` lifecycle on macOS 26+

**macOS Settings Window**:
- Use `NSPanel` (not NSWindow) for keyboard support without Dock activation
- Configure: `styleMask: [.titled, .closable, .resizable, .nonactivatingPanel]`

**macOS Halo Window**:
- `NSWindow` with `styleMask: .borderless`, `level: .floating`
- `backgroundColor: .clear`, `isOpaque: false`
- `ignoresMouseEvents: true` (click-through)
- **NEVER** call `makeKeyAndOrderFront()` to avoid focus theft

### Privacy & Security

- **PII Scrubbing**: Regex-based removal before API calls
- **Local-First**: All config and memory stored locally
- **No Telemetry**: Zero tracking, no analytics
- **API Key Storage**: macOS Keychain via Security framework

---

## Anti-Patterns to Avoid

### Architecture
- DO NOT use webviews (violates native-first principle)
- DO NOT create permanent GUI windows (violates "Ghost" philosophy)
- DO NOT hardcode AI providers (must be config-driven)
- DO NOT bypass RigAgentManager for AI calls
- DO NOT manually manage runtime installations (use RuntimeRegistry)

### macOS Specific
- DO NOT use SwiftUI App lifecycle on macOS 26+ (use main.swift + NSApplicationMain)
- DO NOT create Settings as NSWindow (use NSPanel)
- DO NOT call `makeKeyAndOrderFront()` on Halo window

### Concurrency
- DO NOT block main thread during API calls (use tokio async)
- DO NOT put business logic in Swift (belongs in Rust core only)

### FFI
- DO NOT manually write FFI bindings (use UniFFI)
- DO NOT ignore FFI boundary safety (use proper error handling)

### Permissions
- DO NOT ignore permissions errors (especially Accessibility)
- DO NOT skip permission pre-check in Rust core

---

## Critical Success Factors

1. **Zero Focus Loss**: Halo must never interfere with active window
2. **Sub-100ms Latency**: From hotkey press to Halo appearance
3. **Reliable Clipboard**: Handle all content types (text, images, rich text)
4. **Robust Permissions**: Clear UX for granting Accessibility access
5. **Memory Safety**: No crashes at FFI boundary
6. **Smooth Animations**: 60fps Halo transitions
7. **Auto Runtime**: Runtimes install/update without user intervention

---

## Documentation Index

### Core Architecture
- [Architecture Guide](./docs/ARCHITECTURE.md) - Structured Context Protocol, request flow
- [Dispatcher Layer](./docs/DISPATCHER.md) - Multi-layer routing, L3 Agent
- [Cowork Task Orchestration](./docs/COWORK.md) - DAG-based multi-task execution
- [Configuration Schema](./docs/CONFIGURATION.md) - config.toml reference
- [Permissions](./docs/PERMISSIONS.md) - Permission authorization architecture

### Development Guides
- [Development Phases](./docs/DEVELOPMENT_PHASES.md) - Project roadmap (Phase 1-8 complete)
- [Platform Notes](./docs/PLATFORM_NOTES.md) - macOS/Windows/Linux setup
- [Debugging Guide](./docs/DEBUGGING_GUIDE.md) - Rust and Swift debugging
- [Localization Guide](./docs/LOCALIZATION.md) - i18n implementation
- [XcodeGen Workflow](./docs/XCODEGEN_README.md) - Project generation

### Testing & Quality
- [Testing Guide](./docs/TESTING_GUIDE.md) - Automated testing strategies
- [Manual Testing Checklist](./docs/manual-testing-checklist.md) - Test scenarios

### Design & UI
- [UI Design Guide](./docs/ui-design-guide.md) - Design system
- [Component Index](./docs/ComponentsIndex.md) - SwiftUI component catalog
- [macOS 26 Window Design](./docs/MACOS26_WINDOW_DESIGN.md) - Modern window architecture

---

## HaloState Machine (21 States)

```swift
enum HaloState {
    case idle, hidden, appearing, listening, thinking, processing
    case streaming, success, error, disappearing
    case multiTurnActive, multiTurnThinking, multiTurnStreaming
    case toolExecuting, toolSuccess, toolError
    case clarificationNeeded, clarificationReceived
    case agentPlanning, agentExecuting, agentComplete
}
```

---

## Skills

Use skills from: `~/.claude/skills/build-macos-apps`

---

## Environment

- Python path: `~/.python3/bin/python`
- Activate python: `source ~/.python3/bin/activate`
- Install package: `cd ~/.python3 && uv pip install <package>`
- Xcode generation: `xcodegen generate`
- Syntax validation: `~/.python3/bin/python verify_swift_syntax.py <file.swift>`
- Script files: `Scripts/` directory

---

## git commit

After completing a task or fixing an issue, use `git add` and `git commit` to submit this modification use English.

---

## memory prompt

When the token is low to 10% of the limit, summarize this session at the end of the session to generate a "memory prompt" that can be directly copied and used, so that the next session can be inherited.

---

## language

- Reply language in Chinese
- Program comments in English
