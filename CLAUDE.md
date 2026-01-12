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
4. Backend routes request to appropriate AI provider
5. Halo dissolves, result is pasted back or typed character-by-character

**Unified Input Flow**: Raycast-style interface with focus detection, command completion, and multi-turn conversation. See `UnifiedInputCoordinator.swift`.

---

## Technical Stack

### Architecture: "Rust Core + UniFFI + Native UI"

**NO WEBVIEWS. NO TAURI. NO ELECTRON.**

1. **Rust Core (Library)**: Headless service compiled as `cdylib` + `staticlib`
   - Global hotkeys (`rdev`), Clipboard (`arboard`), Input simulation (`enigo`)
   - Async runtime (`tokio`), HTTP client (`reqwest`)
   - **UniFFI**: Generates Swift/Kotlin/C# bindings automatically
   - **Memory Module**: `rusqlite` + `sqlite-vec` + `fastembed` (bge-small-zh-v1.5)

2. **Native UIs (Platform-Specific)**:
   - **macOS**: Swift + SwiftUI (NSWindow for Halo overlay, NSStatusBar for menu)
   - **Windows** (Future): C# + WinUI 3
   - **Linux** (Future): Rust + GTK4

3. **Communication Pattern**: Rust → UniFFI → Swift (callback-based)

See [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) for complete technical documentation.

---

## Project Structure

**Note**: This project uses [XcodeGen](https://github.com/yonaskolb/XcodeGen). The `.xcodeproj` is generated from `project.yml`.

```
aether/
├── project.yml                    # XcodeGen configuration (source of truth)
├── Aether/
│   ├── Sources/                   # Swift source files
│   │   ├── AetherApp.swift        # App entry point
│   │   ├── AppDelegate.swift      # Menu bar lifecycle + Rust core integration
│   │   ├── HaloWindow.swift       # Transparent overlay (NSWindow)
│   │   ├── EventHandler.swift     # Implements AetherEventHandler
│   │   └── Generated/aether.swift # UniFFI-generated bindings
│   ├── Frameworks/
│   │   └── libaethecore.dylib     # Rust library (embedded in app)
│   └── core/                      # Rust core library
│       ├── Cargo.toml
│       ├── src/
│       │   ├── lib.rs             # UniFFI exports and public API
│       │   ├── aether.udl         # UniFFI interface definition
│       │   ├── router/            # Smart routing logic
│       │   ├── providers/         # AI provider clients
│       │   ├── memory/            # Memory module (Local RAG)
│       │   └── dispatcher/        # Dispatcher Layer (multi-layer routing)
│       └── uniffi.toml
├── docs/                          # Documentation
└── config.toml                    # User config (~/.config/aether/config.toml)
```

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

### Structured Context Protocol

Aether uses a **payload-based architecture** for intelligent request processing:

```
User Input → Router → PayloadBuilder → CapabilityExecutor → PromptAssembler → Provider
```

**Key Features**:
- **AgentPayload**: Type-safe data structure replaces string concatenation
- **Dynamic Capabilities**: Memory, Search, MCP tools
- **Intent Classification**: BuiltinSearch, Custom, Skills, GeneralChat

See [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) for details.

### Dispatcher Layer

Multi-layer routing with confidence-based confirmation:
- **L1**: Regex pattern match (<10ms, confidence 1.0)
- **L2**: Semantic keyword match (200-500ms, confidence 0.7)
- **L3**: LLM inference (>1s, confidence 0.5-0.9)

See [docs/DISPATCHER.md](./docs/DISPATCHER.md) for details.

### Permission System

Three-layer protection architecture:
1. **Swift UI Layer** - Passive monitoring + waterfall guidance
2. **Rust Core Layer** - Panic protection + permission pre-check
3. **System Integration** - Deep links + macOS native prompts

See [docs/PERMISSIONS.md](./docs/PERMISSIONS.md) for details.

---

## Key Design Constraints

### Modularity Requirements

Use trait-based abstractions for all core components:
- `ClipboardManager`, `InputSimulator`, `AiProvider`, `Router`
- `MemoryStore`, `EmbeddingModel`, `SearchProvider`

### Critical UI Behavior

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

- DO NOT use webviews (violates native-first principle)
- DO NOT create permanent GUI windows (violates "Ghost" philosophy)
- DO NOT hardcode AI providers (must be config-driven)
- DO NOT ignore permissions errors (especially Accessibility)
- DO NOT block main thread during API calls (use tokio async)
- DO NOT put business logic in Swift (belongs in Rust core only)
- DO NOT manually write FFI bindings (use UniFFI)

---

## Critical Success Factors

1. **Zero Focus Loss**: Halo must never interfere with active window
2. **Sub-100ms Latency**: From hotkey press to Halo appearance
3. **Reliable Clipboard**: Handle all content types (text, images, rich text)
4. **Robust Permissions**: Clear UX for granting Accessibility access
5. **Memory Safety**: No crashes at FFI boundary
6. **Smooth Animations**: 60fps Halo transitions

---

## Documentation Index

### Core Architecture
- [Architecture Guide](./docs/ARCHITECTURE.md) - Structured Context Protocol, request flow
- [Dispatcher Layer](./docs/DISPATCHER.md) - Multi-layer routing, L3 Agent
- [Configuration Schema](./docs/CONFIGURATION.md) - config.toml reference
- [Permissions](./docs/PERMISSIONS.md) - Permission authorization architecture

### Development Guides
- [Development Phases](./docs/DEVELOPMENT_PHASES.md) - Project roadmap
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
