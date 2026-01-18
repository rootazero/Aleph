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

**Note**: This is a Monorepo with platform-specific directories. macOS uses [XcodeGen](https://github.com/yonaskolb/XcodeGen).

```
aether/
├── Cargo.toml                     # Workspace root configuration
├── VERSION                        # Single version source of truth
│
├── core/                          # 🦀 Shared Rust Core Library
│   ├── Cargo.toml                 # Feature flags: uniffi (macOS), cabi (Windows)
│   ├── src/
│   │   ├── lib.rs                 # UniFFI/C ABI exports and public API
│   │   ├── aether.udl             # UniFFI interface definition
│   │   ├── ffi_cabi.rs            # Windows C ABI exports (csbindgen)
│   │   ├── ffi/                   # 9 FFI sub-modules
│   │   ├── agent/                 # Agent execution engine
│   │   ├── agents/                # Specialized agents
│   │   ├── capability/            # Capability definitions
│   │   ├── components/            # 8 core components
│   │   ├── config/                # 10 config type modules + policies
│   │   ├── conversation/          # Conversation management
│   │   ├── cowork/                # DAG task orchestration
│   │   ├── dispatcher/            # Multi-layer routing
│   │   ├── event/                 # Event system
│   │   ├── generation/            # 10+ media generation providers
│   │   ├── intent/                # 3-layer intent detection
│   │   ├── mcp/                   # MCP integration (stdio transport)
│   │   ├── memory/                # Dual-layer memory system
│   │   ├── payload/               # Request payload building
│   │   ├── providers/             # AI provider implementations
│   │   ├── rig_tools/             # rig-core tool definitions
│   │   ├── router/                # Smart routing logic
│   │   ├── runtimes/              # Runtime managers (uv, fnm, yt-dlp)
│   │   ├── search/                # 6 search providers
│   │   ├── services/              # Background services
│   │   ├── skills/                # Skill system
│   │   ├── video/                 # Video processing
│   │   ├── vision/                # OCR + image understanding
│   │   └── clarification/         # Phantom Flow (user clarification)
│   └── uniffi.toml
│
├── platforms/                     # 📱 Platform-Specific Code
│   ├── macos/                     # 🍎 macOS Application
│   │   ├── project.yml            # XcodeGen configuration
│   │   ├── Aether/
│   │   │   ├── Sources/           # Swift source files
│   │   │   ├── Frameworks/        # libaethecore.dylib
│   │   │   └── Resources/         # Assets, localization
│   │   ├── AetherTests/
│   │   └── AetherUITests/
│   │
│   └── windows/                   # 🪟 Windows Application
│       ├── Aether.sln             # Visual Studio solution
│       ├── Aether/
│       │   ├── Aether.csproj      # .NET 8.0 WinUI 3 project
│       │   ├── App.xaml           # Application entry
│       │   ├── Interop/           # csbindgen P/Invoke bindings
│       │   └── libs/              # aethecore.dll
│       └── Aether.Tests/
│
├── shared/                        # 📦 Cross-Platform Resources
│   ├── config/                    # Default configuration templates
│   ├── locales/                   # Master localization files
│   └── docs/                      # Shared documentation
│
├── scripts/                       # 🔧 Build Scripts
│   ├── build-core.sh              # Build Rust core (multi-target)
│   ├── build-macos.sh             # macOS full build
│   ├── build-windows.ps1          # Windows full build
│   └── generate-bindings.sh       # FFI binding generation
│
├── docs/                          # Documentation
└── .github/workflows/             # CI/CD pipelines
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

### Detailed Directory Structure

```
aether/
├── .github/
│   └── workflows/
│       ├── rust-core.yml              # Rust CI (test, lint, build)
│       ├── macos-app.yml              # macOS app build
│       └── windows-app.yml            # Windows app build
│
├── core/                              # 🦀 Rust Core Library
│   ├── Cargo.toml                     # [features] uniffi, cabi
│   ├── build.rs                       # UniFFI build script
│   ├── uniffi.toml                    # UniFFI configuration
│   ├── benches/                       # Performance benchmarks
│   ├── bindings/                      # Pre-generated bindings (reference)
│   ├── examples/                      # Usage examples
│   ├── tests/                         # Integration tests
│   └── src/
│       ├── lib.rs                     # Public API, UniFFI scaffolding
│       ├── aether.udl                 # UniFFI interface definition
│       ├── ffi_cabi.rs                # Windows C ABI exports
│       ├── error.rs                   # Error types
│       ├── initialization.rs          # First-time setup
│       ├── event_handler.rs           # Event callback traits
│       ├── cowork_ffi.rs              # Cowork FFI bindings
│       ├── title_generator.rs         # Conversation title generation
│       ├── uniffi_core.rs             # UniFFI core re-exports
│       │
│       ├── agent/                     # Agent execution engine
│       │   ├── mod.rs, config.rs, manager.rs, types.rs, conversation.rs
│       │
│       ├── agents/                    # Sub-agent system (Phase 4)
│       │   ├── mod.rs, registry.rs, task_tool.rs, types.rs
│       │   └── prompts/               # Agent system prompts
│       │
│       ├── capability/                # Capability definitions
│       │   ├── mod.rs, declaration.rs, executor.rs, vision.rs
│       │
│       ├── clarification/             # Phantom Flow interaction
│       │   ├── mod.rs, types.rs
│       │
│       ├── clipboard/                 # Image types (for AI providers)
│       │   └── mod.rs
│       │
│       ├── command/                   # Command completion system
│       │   ├── mod.rs, types.rs, registry.rs, suggestions.rs
│       │
│       ├── components/                # 8 Core agentic loop components
│       │   ├── mod.rs
│       │   ├── callback_bridge.rs     # Rust-Swift communication
│       │   ├── intent_analyzer.rs     # Intent detection
│       │   ├── loop_controller.rs     # Agentic loop state
│       │   ├── session_compactor.rs   # Memory compression
│       │   ├── session_recorder.rs    # Conversation history
│       │   ├── subagent_handler.rs    # Sub-agent orchestration
│       │   ├── task_planner.rs        # Multi-step planning
│       │   └── tool_executor.rs       # Unified tool dispatch
│       │
│       ├── config/                    # Configuration types
│       │   ├── mod.rs, types.rs, policies.rs, provider_entry.rs
│       │
│       ├── conversation/              # Multi-turn conversation
│       │   ├── mod.rs, manager.rs, session.rs, turn.rs
│       │
│       ├── core/                      # Internal core types
│       │   ├── mod.rs, types.rs, memory_types.rs
│       │
│       ├── cowork/                    # DAG task orchestration
│       │   ├── mod.rs, executor.rs, graph.rs, model_router.rs
│       │   ├── file_operations.rs, code_executor.rs
│       │
│       ├── dispatcher/                # Multi-layer routing
│       │   ├── mod.rs, config.rs, integration.rs
│       │   ├── tool_registry.rs, tool_types.rs
│       │
│       ├── event/                     # Event-driven architecture
│       │   ├── mod.rs, bus.rs, types.rs, handlers.rs
│       │
│       ├── ffi/                       # 9 FFI sub-modules
│       │   ├── mod.rs, core.rs, config_ffi.rs
│       │   ├── memory_ffi.rs, conversation_ffi.rs
│       │   ├── dispatcher_ffi.rs, mcp_ffi.rs
│       │   ├── search_ffi.rs, skills_ffi.rs
│       │
│       ├── generation/                # Media generation providers
│       │   ├── mod.rs, types.rs, registry.rs, mock.rs
│       │   └── providers/             # 10+ generation backends
│       │
│       ├── intent/                    # 3-layer intent detection
│       │   ├── mod.rs, classifier.rs, aggregator.rs
│       │   ├── calibrator.rs, cache.rs, types.rs
│       │   ├── context.rs, defaults.rs, presets.rs
│       │   └── rollback.rs
│       │
│       ├── logging/                   # Structured logging
│       │   ├── mod.rs, file_logging.rs, pii_layer.rs
│       │
│       ├── mcp/                       # Model Context Protocol
│       │   ├── mod.rs, types.rs, service.rs, manager.rs
│       │
│       ├── memory/                    # Dual-layer memory system
│       │   ├── mod.rs, database.rs, context.rs
│       │   ├── embedding.rs, facts.rs, retrieval.rs
│       │
│       ├── metrics/                   # Performance metrics
│       │   └── mod.rs
│       │
│       ├── payload/                   # Structured context protocol
│       │   ├── mod.rs, builder.rs, types.rs
│       │
│       ├── providers/                 # AI provider implementations
│       │   ├── mod.rs, rig_providers.rs
│       │
│       ├── rig_tools/                 # rig-core tool definitions
│       │   ├── mod.rs, registry.rs
│       │   ├── builtin_tools.rs, mcp_tools.rs, skill_tools.rs
│       │   └── generation/            # Generation tool wrappers
│       │
│       ├── runtimes/                  # Runtime managers
│       │   ├── mod.rs, registry.rs, traits.rs
│       │   ├── uv.rs, fnm.rs, ytdlp.rs
│       │
│       ├── search/                    # 6 search providers
│       │   ├── mod.rs, registry.rs, types.rs
│       │
│       ├── services/                  # Background services
│       │   ├── mod.rs, file_ops.rs, git_ops.rs, system_info.rs
│       │
│       ├── skills/                    # Skill system
│       │   ├── mod.rs, types.rs, registry.rs, installer.rs
│       │
│       ├── suggestion/                # AI response parsing
│       │   └── mod.rs
│       │
│       ├── utils/                     # Utilities
│       │   ├── mod.rs, pii.rs, text.rs
│       │
│       ├── video/                     # Video processing
│       │   ├── mod.rs, transcript.rs, youtube.rs
│       │
│       └── vision/                    # Vision capability
│           ├── mod.rs, service.rs, types.rs
│
├── platforms/
│   ├── macos/                         # 🍎 macOS Application
│   │   ├── project.yml                # XcodeGen configuration
│   │   ├── Aether/
│   │   │   ├── Info.plist
│   │   │   ├── Aether.entitlements
│   │   │   ├── config.example.toml
│   │   │   ├── Assets.xcassets/       # App icons, colors
│   │   │   ├── Frameworks/            # libaethecore.dylib
│   │   │   ├── Generated/             # Reference bindings
│   │   │   ├── Resources/
│   │   │   │   ├── en.lproj/          # English localization
│   │   │   │   ├── zh-Hans.lproj/     # Chinese localization
│   │   │   │   ├── skills/            # Built-in skills
│   │   │   │   └── ProviderIcons/     # AI provider icons
│   │   │   └── Sources/
│   │   │       ├── main.swift         # NSApplicationMain entry
│   │   │       ├── AppDelegate.swift  # Menu bar lifecycle
│   │   │       ├── AetherBridgingHeader.h
│   │   │       ├── Generated/         # UniFFI Swift bindings
│   │   │       │   ├── aether.swift
│   │   │       │   ├── aetherFFI.h
│   │   │       │   └── aetherFFI.modulemap
│   │   │       ├── Components/
│   │   │       │   ├── Atoms/         # Basic UI elements
│   │   │       │   ├── Molecules/     # Composed components
│   │   │       │   ├── Organisms/     # Complex UI sections
│   │   │       │   └── Window/        # Window controllers
│   │   │       ├── Controllers/       # View controllers
│   │   │       ├── Coordinator/       # Input/Output/MultiTurn
│   │   │       ├── DesignSystem/      # Theme, colors, fonts
│   │   │       ├── DI/                # Dependency injection
│   │   │       ├── Extensions/        # Swift extensions
│   │   │       ├── Handlers/          # Event handlers
│   │   │       ├── Managers/          # State managers
│   │   │       ├── Models/            # Data models
│   │   │       ├── MultiTurn/         # Multi-turn conversation UI
│   │   │       ├── Protocols/         # Swift protocols
│   │   │       ├── Services/          # Swift services
│   │   │       ├── Store/             # State store
│   │   │       ├── Utils/             # Utilities
│   │   │       └── Vision/            # Screen capture UI
│   │   ├── AetherTests/               # Unit tests
│   │   └── AetherUITests/             # UI tests
│   │
│   └── windows/                       # 🪟 Windows Application
│       ├── Aether.sln                 # Visual Studio solution
│       ├── Aether/
│       │   ├── Aether.csproj          # .NET 8.0 WinUI 3
│       │   ├── App.xaml               # Application entry
│       │   ├── App.xaml.cs
│       │   ├── MainWindow.xaml        # Main window (placeholder)
│       │   ├── MainWindow.xaml.cs
│       │   ├── app.manifest           # Windows manifest
│       │   ├── Interop/
│       │   │   └── NativeMethods.g.cs # csbindgen P/Invoke
│       │   └── libs/                  # aethecore.dll
│       └── Aether.Tests/              # Unit tests
│
├── shared/                            # 📦 Cross-Platform Resources
│   ├── config/
│   │   └── default-config.toml        # Default configuration
│   ├── locales/                       # Master locale files (future)
│   └── docs/                          # Shared documentation (future)
│
├── scripts/                           # 🔧 Build Scripts
│   ├── build-core.sh                  # Build Rust (macos/windows/all)
│   ├── build-macos.sh                 # macOS full build
│   ├── build-windows.ps1              # Windows full build
│   └── generate-bindings.sh           # FFI binding generation
│
├── Scripts/                           # Legacy scripts (macOS)
│   ├── gen_bindings.sh
│   └── monitor_startup.sh
│
├── docs/                              # Documentation
│   ├── ARCHITECTURE.md
│   ├── CONFIGURATION.md
│   ├── DISPATCHER.md
│   ├── COWORK.md
│   └── ...
│
├── openspec/                          # OpenSpec change management
│   ├── AGENTS.md
│   ├── project.md
│   ├── specs/                         # Specifications
│   └── changes/                       # Change proposals
│
├── CLAUDE.md                          # This file
├── README.md
├── Cargo.toml                         # Workspace root
├── Cargo.lock
└── VERSION                            # 0.1.0
```

---

## Build Commands

### Building Rust Core

```bash
# From repository root
cd core/

# Development build (default: UniFFI for macOS)
cargo build

# Release build
cargo build --release

# Build for Windows (C ABI)
cargo build --release --no-default-features --features cabi

# Generate UniFFI bindings (macOS)
cargo run --bin uniffi-bindgen generate \
  --library target/release/libaethecore.dylib \
  --language swift \
  --out-dir ../platforms/macos/Aether/Sources/Generated/
```

### Building macOS Client

```bash
cd platforms/macos/
xcodegen generate                  # Generate Xcode project
open Aether.xcodeproj              # Open in Xcode
# Or:
xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Release build
```

### Building Windows Client (on Windows)

```powershell
cd platforms/windows/
dotnet build -c Release
```

### Using Build Scripts

```bash
# Build core for all platforms
./scripts/build-core.sh all

# Build macOS app
./scripts/build-macos.sh release

# Build Windows app (on Windows)
.\scripts\build-windows.ps1 -Config Release
```

### Testing

```bash
cd core/
cargo test                         # All tests
cargo test router                  # Module-specific tests
cargo test --workspace             # All workspace tests
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
