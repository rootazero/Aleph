# Development Phases

This document outlines the development phases for Aether, tracking progress and defining success criteria for each phase.

---

## Phase 1: Core Infrastructure

**Status**: ✅ COMPLETED

**Goal**: Build Rust core with UniFFI bindings and minimal Swift UI

**Tasks:**
- [x] Initialize Cargo workspace with `crate-type = ["cdylib", "staticlib"]`
- [x] Create `aether.udl` interface definition
- [x] Implement `AetherCore` struct with basic lifecycle
- [x] Implement `AetherEventHandler` trait for callbacks
- [x] Set up UniFFI bindings generation
- [x] Create macOS Xcode project
- [x] Generate Swift bindings and integrate into Xcode
- [x] Implement basic `EventHandler` in Swift
- [x] Test Rust ↔ Swift callback communication

**Success Criteria**: ✅ Swift app can initialize Rust core and receive callback events

---

## Phase 2: Hotkey & Clipboard Integration

**Status**: ✅ COMPLETED

**Goal**: Complete the Cut → Process → Paste cycle

**Tasks:**
- [x] Implement global hotkey listener
- [x] Implement clipboard manager
- [x] Implement keyboard simulator
- [x] Build "Smart Fallback" context acquisition logic
- [x] Request macOS Accessibility permissions
- [x] Create native permission prompt UI in Swift
- [x] Test end-to-end hotkey → clipboard → callback flow

**Note**: Hotkey, clipboard, and input simulation have been migrated to Swift layer for better macOS integration.

**Success Criteria**: ✅ Pressing Cmd+~ triggers clipboard read and callback

---

## Phase 3: Halo Overlay

**Status**: ✅ COMPLETED

**Goal**: Native transparent overlay with animations

**Tasks:**
- [x] Create `HaloWindow` (NSWindow subclass)
- [x] Implement borderless, transparent, floating window
- [x] Create `HaloView` (SwiftUI) with animation states
- [x] Implement Halo state machine (21 states including Idle/Listening/Processing/Success/Error)
- [x] Add mouse position tracking
- [x] Implement fade in/out animations
- [x] Add provider-specific colors

**Success Criteria**: ✅ Halo appears at cursor, animates, and disappears

---

## Phase 4: Memory Module (Local RAG)

**Status**: ✅ COMPLETED

**Goal**: Context-aware Local RAG with app/window-based memory anchors

### Technical Implementation

**Database:** SQLite with `sqlite-vec` extension running within the Rust Core.

**Dual-Layer Memory System:**
- **Layer 1 (Raw)**: Complete conversation history with full context
- **Layer 2 (Facts)**: AI-extracted facts and insights for efficient retrieval

**Context Anchors:** Every interaction is tagged with metadata:
- `app_bundle_id`: (e.g., `com.apple.Notes`)
- `window_title`: (e.g., "Project Plan.txt")
- `timestamp`: UTC time

### Implementation Tasks

- [x] Integrate embedded vector database (SQLite + sqlite-vec)
- [x] Implement embedding model inference (bge-small-zh-v1.5 via fastembed)
- [x] Create context capture module (app_bundle_id + window_title via Accessibility API)
- [x] Build memory ingestion pipeline (post-interaction embedding + storage)
- [x] Implement retrieval logic with metadata filtering
- [x] Design context augmentation strategy for LLM prompts
- [x] Add UniFFI bindings for memory operations
- [x] Implement privacy controls (memory retention policies, manual deletion)
- [x] Add memory management UI in Settings (view/delete history)
- [x] Implement SessionCompactor for memory compression

**Success Criteria**: ✅ Aether remembers past interactions per-app/per-window and augments prompts with relevant context

---

## Phase 5: AI Integration

**Status**: ✅ COMPLETED

**Goal**: Connect to real AI providers with rig-core framework

**Tasks:**
- [x] Migrate to rig-core 0.28 AI agent framework
- [x] Implement OpenAI API client via rig-core
- [x] Implement Anthropic Claude API client
- [x] Implement Google Gemini API client
- [x] Implement local Ollama execution
- [x] Build Router with regex-based rule matching
- [x] Implement Config TOML parser (serde) with file loading
- [x] Add async processing pipeline
- [x] Handle API errors and timeouts
- [x] Integrate with memory module for context-aware AI responses
- [x] Add comprehensive integration tests
- [x] Add performance benchmarks
- [x] Implement rig-sqlite for conversation persistence

### Key Files

- `Aether/core/src/providers/` - AI provider implementations
- `Aether/core/src/router/` - Smart routing system
- `Aether/core/src/config/` - Configuration management (10 type modules)
- `Aether/config.example.toml` - Comprehensive configuration example

**Success Criteria**: ✅ All providers implemented with rig-core, router correctly routes requests based on rules

---

## Phase 6: Settings UI

**Status**: ✅ COMPLETED

**Goal**: Native settings interface with macOS 26 design language

**Tasks:**
- [x] Create SwiftUI settings window (NSPanel for keyboard support without Dock activation)
- [x] Implement Providers tab (add/edit/test API keys)
- [x] Implement Routing tab (rule editor with drag-to-reorder)
- [x] Implement Shortcuts tab (hotkey recorder)
- [x] Implement Behavior tab (input/output modes)
- [x] Implement General tab (version display, theme selection)
- [x] Implement Memory tab (view/delete history, configure retention policy)
- [x] Implement MCP tab (MCP server configuration)
- [x] Implement Skills tab (skill management)
- [x] Implement Cowork tab (task orchestration settings)
- [x] Implement Policies tab (system behavior fine-tuning)
- [x] Add menu bar icon with dropdown menu
- [x] Integrate with macOS Keychain for API key storage
- [x] Config hot-reload with file watcher
- [x] Atomic config file writes
- [x] Config validation (regex, API keys, temperature, timeouts)

### Key Files

- `Aether/Sources/Settings/` - Settings UI components
- `Aether/Sources/Window/SettingsWindow.swift` - NSPanel-based settings window
- `Aether/Sources/KeychainManagerImpl.swift` - Keychain integration

**Success Criteria**: ✅ Full configuration management via native UI

---

## Phase 7: Event-Driven Agentic Loop

**Status**: ✅ COMPLETED

**Goal**: Production-ready AI agent execution with rig-core integration

### Core Components (8 Modules)

- [x] **IntentAnalyzer** (`intent/`) - 3-layer intent detection (L1 Regex, L2 Semantic, L3 LLM)
- [x] **TaskPlanner** (`agent/`) - Multi-step task planning with DAG execution
- [x] **ToolExecutor** (`components/tool_executor.rs`) - Unified tool dispatch system
- [x] **LoopController** (`components/loop_controller.rs`) - Agentic loop state management
- [x] **SessionRecorder** (`components/session_recorder.rs`) - Conversation history tracking
- [x] **SessionCompactor** (`components/session_compactor.rs`) - Memory compression
- [x] **SubAgentHandler** (`components/subagent_handler.rs`) - Sub-agent orchestration
- [x] **CallbackBridge** (`components/callback_bridge.rs`) - Rust-Swift communication

### Additional Features

- [x] Image clipboard support (Base64 encoding)
- [x] Typewriter effect for output
- [x] PII scrubbing (regex filters)
- [x] Improve error handling and user feedback
- [x] Performance profiling and optimization
- [x] Add logging (with privacy protection)
- [x] Write comprehensive tests

### Cowork DAG Orchestration

- [x] Implement `CoworkEngine` for DAG-based task execution
- [x] Build `ModelRouter` for intelligent model selection
- [x] Create task graph visualization
- [x] Add parallel task execution with `max_parallelism` control

### Media Generation

- [x] Implement 10+ generation providers (Replicate, Recraft, Ideogram, Kimi, etc.)
- [x] Add video generation support (yt-dlp integration)
- [x] Implement image generation with provider-specific prompts

### Key Files

- `Aether/core/src/components/` - 8 core components
- `Aether/core/src/agent/` - Agent execution engine
- `Aether/core/src/cowork/` - DAG orchestration
- `Aether/core/src/generation/` - Media generation providers

**Success Criteria**: ✅ Event-driven agentic loop with multi-step planning and execution

---

## Phase 8: Runtime Manager

**Status**: ✅ COMPLETED

**Goal**: Automatic runtime environment management

### Runtime Implementations

- [x] **RuntimeManager trait** - Common interface for all runtimes
- [x] **UvRuntime** - Python environment management via uv
- [x] **FnmRuntime** - Node.js environment management via fnm
- [x] **YtDlpRuntime** - Video download tool management

### Features

- [x] Automatic installation on first use
- [x] Background update check mechanism
- [x] Version management and updates
- [x] RuntimeSettingsView in Swift UI
- [x] UniFFI exports for runtime operations

### Key Files

- `Aether/core/src/runtimes/` - Runtime implementations
- `Aether/Sources/Settings/RuntimeSettingsView.swift` - Settings UI

**Success Criteria**: ✅ Runtimes auto-install and update without user intervention

---

## Phase 9: Production Hardening

**Status**: ⏳ PLANNED

**Goal**: Production-ready deployment and monitoring

### Planned Tasks

- [ ] Auto-update mechanism (Sparkle for macOS)
- [ ] Crash reporting (opt-in)
- [ ] Performance monitoring dashboard
- [ ] Windows implementation (C# + WinUI 3)
- [ ] Linux implementation (Rust + GTK4)
- [ ] Comprehensive error recovery

### Future Enhancements

- [ ] Plugin system for custom AI providers
- [ ] Advanced routing rules (ML-based)
- [ ] Multi-language support expansion
- [ ] Advanced privacy controls

---

## Architecture Evolution Summary

### Rust Core (44 Modules)

| Category | Modules |
|----------|---------|
| **FFI** | 9 sub-modules (bindings, callbacks, types, etc.) |
| **Agent** | agent/, agents/, components/ (8 modules) |
| **Config** | 10 type modules + policies |
| **AI** | generation/ (10+ providers), providers/, rig_tools/ |
| **Memory** | Dual-layer (Raw + Facts), compression |
| **Routing** | dispatcher/, intent/ (3 layers), router/ |
| **Tools** | mcp/, skills/, search/ (6 providers), video/, vision/ |
| **Runtime** | runtimes/ (uv, fnm, yt-dlp) |
| **Infra** | services/, event/, conversation/, cowork/, payload/ |

### Swift Architecture

| Component | Description |
|-----------|-------------|
| **Entry Point** | `main.swift` + `NSApplicationMain()` (macOS 26 bug workaround) |
| **Settings** | `NSPanel` (keyboard support without Dock activation) |
| **Components** | Atomic Design (Atoms/Molecules/Organisms/Window/) |
| **Coordinators** | Input/Output/MultiTurn/PermissionCoordinator |
| **HaloState** | 21 states for UI state machine |
| **MultiTurn** | `UnifiedConversationWindow` for multi-turn conversations |

### Key Technology Migrations

| Before | After |
|--------|-------|
| rdev (hotkey) | Swift layer |
| arboard (clipboard) | Swift layer |
| enigo (input sim) | Swift layer |
| SwiftUI App lifecycle | main.swift + NSApplicationMain() |
| NSWindow for Settings | NSPanel |
| Manual AI providers | rig-core 0.28 framework |
| Single-layer memory | Dual-layer (Raw + Facts) |

---

**Last Updated**: 2026-01-18
