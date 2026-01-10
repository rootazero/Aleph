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

#### Selection-Based Flow ("Transmutation")

1. User selects text/image in ANY app, presses global hotkey (default: ` key, customizable)
2. Aether simulates Cut (Cmd+X) - content "disappears" for physical feedback
3. Beautiful "Halo" appears at cursor location (native transparent overlay)
4. Backend routes request to appropriate AI (OpenAI/Claude/Gemini/Local LLM)
5. Halo dissolves, result is pasted back (Cmd+V) or typed character-by-character

#### Unified Input Flow (refactor-unified-halo-window)

The new unified input system provides a Raycast-style interface:

1. User places cursor in any input field, presses hotkey (default: `Cmd+Opt+/`)
2. **Focus Detection**: FocusDetector checks if cursor is in a valid input field
   - If focused: Show unified input Halo at caret position
   - If not focused: Show toast warning "请先点击输入框"
   - If Accessibility denied: Fall back to mouse position
3. **Unified Halo Window** appears with:
   - Input field for conversation or command entry
   - SubPanel below for command completion, selectors, or CLI output
4. User types:
   - Start with `/` → SubPanel shows command completion list
   - Plain text → Multi-turn conversation mode
5. On submit:
   - Command: Route to AI with command-specific system prompt
   - Conversation: Continue multi-turn dialogue with context
6. **Output Routing**:
   - If target app available: Paste/type result to app
   - If no target: Display result in SubPanel CLI mode

**Key Components:**
- `UnifiedInputCoordinator.swift` - Main coordinator for the flow
- `FocusDetector.swift` - Accessibility API integration for focus detection
- `SubPanelState.swift` - State management for SubPanel modes
- `UnifiedInputView.swift` - SwiftUI view for unified input

**Migration Note:** The old `CommandModeCoordinator` is deprecated and will be removed in Phase 8.

### Multimodal Content Data Order

When processing multimodal content (text + images/attachments), data is assembled in this order:

```
Final Data = Window Text + Clipboard Text Context + Clipboard Attachments + Window Attachments
```

**Key Rules:**
- **Clipboard**: Contains ONE type only (either text OR attachments like images/videos/PDFs)
- **Window**: May contain BOTH text AND attachments (e.g., Notes app with embedded images)
- **Text always comes first**: This ensures command prefixes (like `/en`) are at the beginning for routing

**Data Sources:**
| Source | Content | When Captured |
|--------|---------|---------------|
| Window Text | Text from active window | After Cut/Copy operation |
| Clipboard Text Context | Recent clipboard text (within 10s) | Via ClipboardMonitor |
| Clipboard Attachments | Images/files user copied | BEFORE Cut/Copy (preserved) |
| Window Attachments | Embedded media in window | After Cut/Copy operation |

**Example Scenarios:**

| Window | Clipboard | Final Result |
|--------|-----------|--------------|
| "Summarize:" + ImageW | ImageC | "Summarize:" + ImageC + ImageW |
| "Summarize:" + ImageW | "Context text" | "Summarize:" + "Context text" + ImageW |
| "Translate this" | ImageC | "Translate this" + ImageC |
| "Hello" | "World" | "Hello" + "World" |

**Implementation:** See `AppDelegate.swift` → `processWithInputMode(useCutMode:)` for the merging logic.

---

## Technical Stack

### The New Architecture: "Rust Core + UniFFI + Native UI"

**NO WEBVIEWS. NO TAURI. NO ELECTRON.**

This is a library-based architecture optimized for performance and native integration:

1. **Rust Core (Library)**: Headless service compiled as `cdylib` + `staticlib`
   - Global hotkeys: `rdev`
   - Clipboard: `arboard` (text & images)
   - Keyboard simulation: `enigo`
   - Async runtime: `tokio`
   - HTTP client: `reqwest`
   - **UniFFI**: Generates Swift/Kotlin/C# bindings automatically
   - **Memory Module**:
     - Vector DB: `rusqlite` + `sqlite-vec`
     - Embedding inference: `fastembed` (ONNX-based, bundled models)
     - Embedding model: `bge-small-zh-v1.5` (Chinese-optimized, 512-dim)

2. **Native UIs (Platform-Specific)**:
   - **macOS**: Swift + SwiftUI (NSWindow for Halo overlay, NSStatusBar for menu)
   - **Windows** (Future): C# + WinUI 3
   - **Linux** (Future): Rust + GTK4

3. **Communication Pattern**: Rust → UniFFI → Swift (callback-based)
   - Rust core exposes API via UniFFI traits
   - Swift implements `AetherEventHandler` protocol
   - Bidirectional callbacks for UI state updates

---

## Project Structure

**Note**: This project uses [XcodeGen](https://github.com/yonaskolb/XcodeGen) to manage the Xcode project. The `.xcodeproj` file is generated from `project.yml` and should not be edited directly.

```
aether/
├── project.yml                    # XcodeGen configuration (source of truth)
├── Aether.xcodeproj/              # Generated by XcodeGen (not in Git)
│
├── Aether/                        # macOS application source directory
│   ├── Sources/                   # Swift source files
│   │   ├── AetherApp.swift           # App entry point (@main, WindowGroup)
│   │   ├── AppDelegate.swift         # Menu bar lifecycle + Rust core integration
│   │   ├── HaloWindow.swift          # Transparent overlay (NSWindow)
│   │   ├── HaloView.swift            # SwiftUI Halo animation
│   │   ├── EventHandler.swift        # Implements AetherEventHandler
│   │   ├── Components/
│   │   │   └── Window/            # macOS 26 window design components
│   │   └── Generated/
│   │       └── aether.swift          # UniFFI-generated bindings
│   ├── Frameworks/
│   │   └── libaethecore.dylib     # Rust library (embedded in app)
│   └── core/                      # Rust core library (cdylib + staticlib)
│       ├── Cargo.toml             # crate-type = ["cdylib", "staticlib"]
│       ├── src/
│       │   ├── lib.rs             # UniFFI exports and public API
│       │   ├── aether.udl         # UniFFI interface definition
│       │   ├── core.rs            # AetherCore struct
│       │   ├── event_handler.rs   # AetherEventHandler trait (callbacks)
│       │   ├── router/            # Smart routing logic
│       │   ├── clipboard/         # Clipboard abstraction (arboard)
│       │   ├── input/             # Input simulation (enigo)
│       │   ├── providers/         # AI provider clients
│       │   ├── config/            # TOML config parsing
│       │   ├── memory/            # Memory module (Local RAG)
│       │   └── dispatcher/        # Dispatcher Layer (multi-layer routing)
│       └── uniffi.toml            # UniFFI configuration
│
├── docs/                          # Documentation
└── config.toml                    # User config (~/.config/aether/config.toml)
```

---

## Build Commands

**Note**: This project uses XcodeGen to manage the Xcode project. See [docs/XCODEGEN_README.md](./docs/XCODEGEN_README.md) for detailed workflow instructions.

### Building Rust Core with UniFFI

```bash
cd Aether/core/

# Development build
cargo build

# Release build for current platform
cargo build --release

# Generate UniFFI bindings for Swift
cargo run --bin uniffi-bindgen generate src/aether.udl \
  --language swift \
  --out-dir ../Sources/Generated/

# Copy library to Frameworks directory
cp target/release/libaethecore.dylib ../Frameworks/
```

### Building macOS Client with XcodeGen

```bash
# Generate Xcode project from project.yml
xcodegen generate

# Open in Xcode
open Aether.xcodeproj

# Or build from command line
xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Release build
```

### Testing

```bash
# Rust core tests
cd Aether/core/
cargo test                              # All tests
cargo test router                       # Module-specific tests

# macOS client tests
xcodebuild test -project Aether.xcodeproj -scheme Aether
```

---

## Architecture: Rust Core + UniFFI Bindings

### Core Design Pattern

Aether uses a **library-based architecture** with callback-driven UI updates:

1. **Rust Core (Headless Service)**
   - Compiled as dynamic library (`cdylib`)
   - Exposes API through UniFFI interface definition (`.udl` file)
   - Defines callback trait for UI state updates
   - No direct UI rendering - only business logic

2. **UniFFI Bridge**
   - Automatically generates Swift/Kotlin/C# bindings from `.udl` file
   - Handles type conversions (Rust ↔ Swift)
   - Manages memory safety across FFI boundary
   - Supports callbacks from Rust → Swift

3. **Native UI Layer**
   - Implements `AetherEventHandler` protocol
   - Renders native overlay window (NSWindow on macOS)
   - Manages menu bar icon and settings UI

### UniFFI Interface Definition (aether.udl)

```idl
namespace aether {
  AetherCore init(AetherEventHandler handler);
};

enum ProcessingState {
  "Idle",
  "Listening",
  "Processing",
  "Success",
  "Error"
};

interface AetherCore {
  constructor(AetherEventHandler handler);
  void start_listening();
  void stop_listening();
  void process_clipboard();
  Config get_config();
  void update_config(Config config);
};

callback interface AetherEventHandler {
  void on_state_changed(ProcessingState state);
  void on_halo_show(HaloPosition position, string? provider_color);
  void on_halo_hide();
  void on_error(string message);
};
```

### Event Flow Example

```
User presses Cmd+~
    ↓
Rust: rdev detects hotkey
    ↓
Rust: Simulates Cmd+C, reads clipboard
    ↓
Rust → Swift: handler.on_state_changed(.listening)
Rust → Swift: handler.on_halo_show(pos, color)
    ↓
Swift: Renders Halo at mouse position
    ↓
Rust: Routes to AI provider (async)
    ↓
Rust: Receives AI response
    ↓
Rust: Simulates Cmd+V (paste result)
    ↓
Rust → Swift: handler.on_halo_hide()
```

---

## Permission Authorization Architecture

### Overview

Aether's permission system uses a **three-layer protection architecture** to eliminate crashes and restart loops:

1. **Swift UI Layer** - Passive monitoring + waterfall guidance
2. **Rust Core Layer** - Panic protection + permission pre-check
3. **System Integration** - Deep links + macOS native prompts

### Key Components

#### PermissionManager (Swift)
- **Location**: `Aether/Sources/Utils/PermissionManager.swift`
- **Role**: Passive permission monitoring without automatic restart logic
- **Key Features**:
  - Timer-based polling (1-second interval)
  - Updates `@Published` properties for UI binding
  - Uses `IOHIDManager` for accurate Input Monitoring detection
  - **NEVER calls** `exit()` or `NSApp.terminate()`

#### PermissionGateView (Swift)
- **Location**: `Aether/Sources/Components/PermissionGateView.swift`
- **Role**: Waterfall flow permission guidance
- **Design**:
  - Step 1: Accessibility permission
  - Step 2: Input Monitoring permission (enabled only after Step 1)
  - "进入 Aether" button shown when both permissions granted
  - User manually clicks button to restart (not automatic)

#### PermissionChecker (Swift)
- **Location**: `Aether/Sources/Utils/PermissionChecker.swift`
- **Key Methods**:
  - `hasAccessibilityPermission()` - Direct `AXIsProcessTrusted()` call
  - `hasInputMonitoringViaHID()` - Uses `IOHIDManager` for accurate detection
  - `openSystemSettings(for:)` - Deep links to specific permission panes

#### AetherCore Permission Pre-check (Rust)
- **Location**: `Aether/core/src/core.rs`
- **Key Features**:
  - `has_input_monitoring_permission` field (set by Swift via UniFFI)
  - `set_input_monitoring_permission(granted: bool)` - UniFFI method
  - `start_listening()` checks permission before calling `rdev::listen()`
  - Returns `AetherError::PermissionDenied` if permission missing

#### rdev Panic Protection (Rust)
- **Location**: `Aether/core/src/hotkey/rdev_listener.rs`
- **Mechanism**: `std::panic::catch_unwind()` wraps `rdev::listen()`
- **Behavior**: Converts panic to error log instead of crashing app

### Permission Flow

**Startup (No Permissions)**:
```
App Launch
    ↓
AppDelegate.applicationDidFinishLaunching()
    ↓
PermissionChecker.hasAllRequiredPermissions() → false
    ↓
Show PermissionGateView (Step 1: Accessibility)
    ↓
PermissionManager.startMonitoring() (polls every 1s)
    ↓
User clicks "Open System Settings"
    ↓
User grants Accessibility → PermissionManager detects
    ↓
UI auto-progresses to Step 2 (Input Monitoring)
    ↓
User grants Input Monitoring → PermissionManager detects
    ↓
"进入 Aether" button appears
    ↓
User clicks button → App restarts
    ↓
App relaunches with permissions → Initializes AetherCore
```

**Runtime Permission Check** (Rust Layer):
```
Swift: core.start_listening()
    ↓
Rust: Check has_input_monitoring_permission
    ↓ (if false)
Rust: Return Err(PermissionDenied)
Rust: event_handler.on_error("Permission required")
    ↓
Swift: Show error alert
App remains functional (degraded mode)
```

### Design Principles

1. **Passive Monitoring, No Auto-Restart**
   - PermissionManager only updates UI state
   - macOS Accessibility permission is real-time effective (no restart needed)
   - User controls restart timing via "进入 Aether" button

2. **Rust Core Panic Protection**
   - `catch_unwind()` prevents `rdev::listen()` panic from crashing app
   - Permission pre-check prevents `rdev::listen()` call without permission

3. **Accurate Detection**
   - `IOHIDManager` provides more accurate Input Monitoring status
   - Directly attempts to open keyboard device stream

4. **User Experience First**
   - Waterfall flow guides users step-by-step
   - Deep links open exact System Settings panes
   - Clear error messages with actionable suggestions

### Critical Behaviors

**✅ DO**:
- Use `PermissionManager` for passive monitoring
- Let users control restart timing
- Check permissions in `AppDelegate` before initializing Core
- Use `IOHIDManager` for Input Monitoring detection

**❌ DO NOT**:
- Call `exit()` or `NSApp.terminate()` when permissions change
- Assume Accessibility permission requires app restart
- Rely on `IOHIDRequestAccess()` alone (use `IOHIDManager`)
- Skip permission pre-check in Rust Core

---

## Configuration Schema

Configuration stored in `~/.config/aether/config.toml`:

```toml
[general]
theme = "cyberpunk"                # cyberpunk | zen | jarvis
default_provider = "openai"

[shortcuts]
summon = "Command+Grave"           # Cmd + ~
cancel = "Escape"

[behavior]
input_mode = "cut"                 # cut | copy
output_mode = "typewriter"         # typewriter | instant
typing_speed = 50                  # chars per second

[memory]
enabled = true                     # Enable/disable memory module
embedding_model = "bge-small-zh-v1.5"
max_context_items = 5
retention_days = 90
vector_db = "sqlite-vec"
similarity_threshold = 0.7

[search]
enabled = true                     # Enable/disable search capability
default_provider = "tavily"        # Default search provider
fallback_providers = ["searxng"]   # Fallback providers if default fails
max_results = 5                    # Maximum search results
timeout_seconds = 10               # Search timeout

[search.backends.tavily]
provider_type = "tavily"
api_key = "tvly-..."

[search.backends.searxng]
provider_type = "searxng"
base_url = "http://localhost:8888"

[search.backends.google]
provider_type = "google"
api_key = "AIzaSy..."
engine_id = "012345..."            # Custom Search Engine ID

[dispatcher]
enabled = true                     # Enable Dispatcher Layer
l3_enabled = true                  # Enable L3 AI inference routing
l3_timeout_ms = 5000               # L3 inference timeout (ms)
confirmation_enabled = true        # Enable tool confirmation UI
confirmation_threshold = 0.7       # Confidence below this triggers confirmation
confirmation_timeout_ms = 30000    # Auto-cancel after timeout (ms)

[providers.openai]
api_key = "sk-..."
model = "gpt-4o"
base_url = "https://api.openai.com/v1"
color = "#10a37f"

[providers.claude]
api_key = "sk-ant-..."
model = "claude-3-5-sonnet-20241022"
color = "#d97757"

[[rules]]
regex = "^/translate"
provider = "openai"
system_prompt = "You are a translator."
capabilities = ["memory"]          # Enable Memory capability for context
intent_type = "translation"        # Custom intent classification
context_format = "markdown"        # Context format (markdown | xml | json)

[[rules]]
regex = "^/search"
provider = "openai"
system_prompt = "You are a search assistant. Answer based on search results."
capabilities = ["search"]          # Enable Search capability
intent_type = "web_search"
context_format = "markdown"

[[rules]]
regex = "^/research"
provider = "claude"
system_prompt = "You are a research analyst. Use memory and search to provide comprehensive answers."
capabilities = ["memory", "search"]  # Enable both Memory and Search
intent_type = "research"
context_format = "markdown"

[[rules]]
regex = "^/draw"
provider = "openai"
system_prompt = "You are DALL-E. Generate images."

[[rules]]
regex = ".*"                       # Catch-all
provider = "openai"
capabilities = ["memory"]          # Enable memory for all requests
```

---

## Key Design Constraints

### Modularity Requirements

Use trait-based abstractions for all core components to support swapping:
- `ClipboardManager` trait for clipboard implementations
- `InputSimulator` trait for keyboard/mouse simulation
- `AiProvider` trait for AI backends
- `Router` trait for routing strategies
- `MemoryStore` trait for vector database implementations
- `EmbeddingModel` trait for embedding inference engines
- `SearchProvider` trait for search backend implementations

### Memory Module Requirements

**Architecture:**
- All memory operations run in Rust Core (no Swift involvement)
- Vector database runs embedded within the process (no external services)
- Embedding inference runs locally (no cloud API calls)

**Context Capture:**
- Use macOS Accessibility API to query active application bundle ID and window title
- Capture context at the moment of hotkey press
- Store context anchors with each memory entry

**Privacy Guarantees:**
- Raw memory data never leaves the device
- Only retrieved context snippets are sent to cloud LLMs
- User can view/delete all stored memories via Settings UI
- Implement retention policies (auto-delete after N days)

**Performance:**
- Embedding inference must complete within 100ms
- Vector search must complete within 50ms
- Use lazy loading for embedding model (load on first use)

### Search Module Requirements

**Architecture:**
- All search operations run in Rust Core (no Swift involvement)
- Support multiple search providers via trait abstraction
- Implement provider fallback mechanism for reliability
- PII scrubbing before sending queries to external services

**Supported Providers:**
- **Tavily**: AI-optimized search with automatic summarization
- **SearXNG**: Self-hosted privacy-focused meta-search
- **Google CSE**: Comprehensive search with Custom Search Engine
- **Bing**: Cost-effective search API
- **Brave**: Privacy-focused search
- **Exa.ai**: AI-native semantic search

**Fallback Mechanism:**
- Primary provider → Fallback providers → Error
- Configurable fallback chain via TOML
- Automatic retry with backoff

**Privacy & Security:**
- PII scrubbing integrated with global PII settings
- Only scrubbed queries sent to external APIs
- Configurable timeout protection (default: 10s)
- No search history stored externally

**Performance:**
- Search requests must complete within timeout (configurable)
- Non-blocking async execution
- Graceful degradation on failure (continue without results)

### Critical UI Behavior

**macOS Halo Window Requirements:**
- NSWindow with `styleMask: .borderless`
- `level: .floating` (above all apps)
- `backgroundColor: .clear`, `isOpaque: false`
- `ignoresMouseEvents: true` (click-through)
- Never call `makeKeyAndOrderFront()` to avoid focus theft

**Focus Protection:**
- Halo window MUST NEVER steal focus from active application
- No window activation during Cut/Paste cycle
- Use `orderFrontRegardless()` instead of `makeKeyAndOrderFront()`

### Multi-Model Orchestration & Structured Context Protocol

Aether uses a **Structured Context Protocol** for intelligent request processing:

**Core Architecture**:
- **AgentPayload**: Type-safe data structure replaces string concatenation
- **Dynamic Capabilities**: Memory (implemented), Search (implemented), MCP tools (future)
- **Intent Classification**: BuiltinSearch, Custom, Skills, GeneralChat
- **Context Assembly**: Markdown/XML/JSON formatting for LLM consumption

**Processing Flow**:
```
User Input → Router → PayloadBuilder → CapabilityExecutor → PromptAssembler → Provider
                ↓           ↓                  ↓                    ↓
          RoutingDecision  Payload      Memory/Search/MCP    Formatted Context
```

**Key Features**:
- **Smart Routing**: Config-based rules (regex matching + intent inference)
- **Transparent Memory**: Local RAG injects relevant conversation history
- **Capability Execution**: Memory (implemented), Search (reserved), MCP (reserved)
- **Format Flexibility**: Markdown (MVP), XML/JSON (reserved)
- **Providers**: OpenAI, Anthropic, Google, Local (Ollama)
- **Fallback**: Auto-retry with default model on timeout/error

**Detailed Architecture**: See [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) for complete technical documentation.

### Routing Rules & Prompt Assembly

Aether uses a layered routing system with rules that can be combined:

**Rule Types:**

1. **Builtin Commands** (one-to-one capability binding, not user-customizable):
   - `/search` → Search capability (web search results injected)
   - `/mcp` → MCP capability (future)
   - `/skill` → Skill capability (future)

2. **User-Defined Slash Commands** (in config.toml):
   - `/zh`, `/en`, `/draw`, etc.
   - Only have `system_prompt`, no special capabilities
   - Only ONE slash command can match per request

3. **Keyword Rules** (in config.toml):
   - Match by regex patterns in user input
   - Multiple keyword rules can match simultaneously
   - Can be combined with slash commands

**Prompt Assembly Order:**
```
final_system_prompt = slash_command_prompt + keyword_rule1_prompt + keyword_rule2_prompt + ...
                      (separated by \n\n)
```

**Memory Behavior:**
- Memory is available in EVERY conversation (regardless of command used)
- Memory provides context for accuracy and continuity
- Memory should NOT directly interfere with the response

**System Prompt Mode (for APIs that ignore system role):**

Some APIs (like certain OpenAI-compatible endpoints) ignore the `system` role message. For these providers, configure `system_prompt_mode = "prepend"` in config.toml:

```toml
[providers.my_provider]
provider_type = "openai"
system_prompt_mode = "prepend"  # Prepend system prompt to user message
```

**Prepend Mode Logic:**
```
Normal Mode:
  system_message = "You are a helpful AI assistant." + memory_context + search_results
  user_message = user_input

Prepend Mode (with rule prompt):
  system_message = (none, or ignored by API)
  user_message = [指令] rule_prompts + memory_context + search_results
                 ---
                 [用户输入] user_input

Prepend Mode (without rule prompt):
  system_message = (none, or ignored by API)
  user_message = user_input (with context prepended if available)
```

**Key Implementation Points:**
- `rule_system_prompt`: Combined prompts from matched slash command + keyword rules
- `context_only`: Memory + Search results (WITHOUT "You are a helpful AI assistant.")
- `assembled_system_prompt`: Base prompt + Memory + Search results (full version)

**Code Locations:**
- Rule matching: `Aether/core/src/router/mod.rs` → `RoutingMatch::assemble_prompt()`
- Prompt assembly: `Aether/core/src/payload/assembler.rs` → `PromptAssembler`
- System prompt mode handling: `Aether/core/src/core.rs` (search for `provider_uses_prepend`)
- API request formatting: `Aether/core/src/providers/openai.rs` (search for `system_prompt_mode`)

### Dispatcher Layer (Aether Cortex)

The Dispatcher Layer provides intelligent tool routing with confidence-based confirmation:

**Architecture:**
```
User Input
     ↓
┌─────────────────────┐
│   Dispatcher Layer  │
│                     │
│  ┌───────────────┐  │
│  │ ToolRegistry  │  │  ← Aggregates Native/MCP/Skills/Custom
│  └───────┬───────┘  │
│          ↓          │
│  ┌───────────────┐  │
│  │ Multi-Layer   │  │
│  │ Router        │  │  ← L1 → L2 → L3 cascade
│  └───────┬───────┘  │
│          ↓          │
│  ┌───────────────┐  │
│  │ Confirmation  │  │  ← If confidence < threshold
│  └───────────────┘  │
└──────────┼──────────┘
           ↓
   Execution Layer
```

**Multi-Layer Routing:**

| Layer | Method | Latency | Confidence | Use Case |
|-------|--------|---------|------------|----------|
| L1 | Regex pattern match | <10ms | 1.0 | Explicit slash commands (`/search`, `/translate`) |
| L2 | Semantic keyword match | 200-500ms | 0.7 | Natural language with keywords ("search for...", "translate this") |
| L3 | LLM inference | >1s | 0.5-0.9 | Ambiguous intent, pronoun resolution, complex queries |
| Default | Fallback | 0ms | 0.0 | General chat when no tool matches |

**Routing Cascade:**
- L1 tries first → if match (confidence ≥ 0.9), execute
- L2 tries if L1 fails → if match (confidence ≥ 0.7), execute
- L3 tries if L2 fails or confidence too low → AI decides tool + params
- Default provider if all layers fail

**Tool Sources:**

| Source | Description | Example |
|--------|-------------|---------|
| `Builtin` | System builtin commands | `/search`, `/mcp`, `/skill`, `/video`, `/chat` |
| `Native` | Built-in capabilities | Search, Video transcript |
| `Mcp` | MCP server tools | `github:git_status`, `filesystem:read_file` |
| `Skill` | Claude Agent Skills | `refine-text`, `code-review` |
| `Custom` | User-defined rules | `[[rules]]` in config.toml |

**Single Source of Truth (BUILTIN_COMMANDS):**

The 5 builtin commands are defined in `dispatcher/builtin_defs.rs` and serve as the single source of truth for:
- Tool metadata (UI display, command completion, L3 router awareness)
- Routing rules (system_prompt, capabilities, regex patterns)

```rust
// dispatcher/builtin_defs.rs
pub const BUILTIN_COMMANDS: &[BuiltinCommandDef] = &[
    BuiltinCommandDef { name: "search", ... },
    BuiltinCommandDef { name: "mcp", ... },
    BuiltinCommandDef { name: "skill", ... },
    BuiltinCommandDef { name: "video", ... },
    BuiltinCommandDef { name: "chat", ... },
];

// Used by:
// - ToolRegistry.register_builtin_tools() - for tool metadata
// - get_builtin_routing_rules() - for routing config
// - Config module - for default rules
```

**Event System (Tool Changes):**

When tools change (MCP connect/disconnect, skill install/uninstall), the event system notifies Swift UI:

```
Rust: refresh_tool_registry()
    ↓
Rust: event_handler.on_tools_changed(tool_count)
    ↓
Swift: EventHandler.onToolsChanged() posts .toolsDidChange notification
    ↓
Swift: CommandCompletionManager auto-refreshes command list
```

**Confirmation Flow:**
- Tools with `confidence < confirmation_threshold` trigger user confirmation
- Halo shows tool preview (name, icon, parameters)
- User can Execute, Edit parameters, or Cancel
- Cancel falls back to GeneralChat

**Configuration:**
```toml
[dispatcher]
enabled = true                    # Enable Dispatcher Layer
l3_enabled = true                 # Enable L3 AI inference
l3_timeout_ms = 5000              # L3 inference timeout
confirmation_enabled = true       # Enable confirmation UI
confirmation_threshold = 0.7      # Confidence below this triggers confirmation
confirmation_timeout_ms = 30000   # Auto-cancel after timeout
```

**Code Locations:**
- Dispatcher module: `Aether/core/src/dispatcher/`
- Builtin definitions: `dispatcher/builtin_defs.rs` (SINGLE SOURCE OF TRUTH)
- Tool Registry: `dispatcher/registry.rs`
- L3 Router: `dispatcher/l3_router.rs`
- Prompt Builder: `dispatcher/prompt_builder.rs`
- Confirmation: `dispatcher/confirmation.rs`
- Integration: `dispatcher/integration.rs`
- Swift event handler: `Sources/EventHandler.swift`
- Swift notifications: `Sources/Notifications.swift`
- Command completion: `Sources/Utils/CommandCompletionManager.swift`

**UniFFI Interface:**
```swift
// List all available tools
let tools = core.listTools()

// Filter by source type
let mcpTools = core.listToolsBySource(sourceType: .mcp)

// Search tools by query
let matches = core.searchTools(query: "git")

// Refresh registry (after MCP server changes)
try core.refreshTools()
```

### Privacy & Security

- **PII Scrubbing**: Regex-based removal of phone/email before API calls
- **Local-First**: All config stored locally
- **No Telemetry**: Zero tracking, no analytics
- **API Key Storage**: Use macOS Keychain (via `Security` framework in Swift)
- **Memory Privacy**: All memory data stored locally, only augmented prompts sent to cloud

---

## Anti-Patterns to Avoid

- DO NOT use webviews (violates native-first principle)
- DO NOT create permanent GUI windows (violates "Ghost" philosophy)
- DO NOT require manual app switching
- DO NOT hardcode AI providers (must be config-driven)
- DO NOT ignore permissions errors (especially Accessibility)
- DO NOT block main thread during API calls (use tokio async)
- DO NOT put business logic in Swift (belongs in Rust core only)
- DO NOT manually write FFI bindings (use UniFFI)

---

## Testing Strategy

- **Unit tests**: Rust core logic (router, providers, config)
- **Integration tests**: Clipboard operations, keyboard simulation
- **Mock providers**: Use fake AI responses for deterministic tests
- **UI tests**: XCTest for SwiftUI components (manual testing preferred for overlay)
- **Manual testing**: Hotkeys, permissions, focus behavior across different apps

See [docs/TESTING_GUIDE.md](./docs/TESTING_GUIDE.md) for detailed testing procedures.

---

## Critical Success Factors

1. **Zero Focus Loss**: Halo must never interfere with active window
2. **Sub-100ms Latency**: From hotkey press to Halo appearance
3. **Reliable Clipboard**: Handle all content types (text, images, rich text)
4. **Robust Permissions**: Clear UX for granting Accessibility access
5. **Memory Safety**: No crashes at FFI boundary
6. **Smooth Animations**: 60fps Halo transitions

---

## Additional Documentation

**Core Architecture:**
- [Architecture Guide](./docs/ARCHITECTURE.md) - Structured Context Protocol, request flow, and core components

**Detailed Guides:**
- [Development Phases](./docs/DEVELOPMENT_PHASES.md) - Project roadmap and phase completion status
- [macOS 26 Window Design](./docs/MACOS26_WINDOW_DESIGN.md) - Modern window design architecture
- [Platform-Specific Notes](./docs/PLATFORM_NOTES.md) - macOS/Windows/Linux setup and permissions
- [Debugging Guide](./docs/DEBUGGING_GUIDE.md) - Rust and Swift debugging techniques
- [Localization Guide](./docs/LOCALIZATION.md) - i18n implementation and translation workflow
- [XcodeGen Workflow](./docs/XCODEGEN_README.md) - Project generation and management

**Testing & Quality:**
- [Testing Guide](./docs/TESTING_GUIDE.md) - Automated and manual testing strategies
- [Performance Guide](./docs/PERFORMANCE_GUIDE.md) - Performance optimization techniques
- [Manual Testing Checklist](./docs/manual-testing-checklist.md) - Comprehensive test scenarios

**Design & UI:**
- [UI Design Guide](./docs/ui-design-guide.md) - Design system and component guidelines
- [Component Index](./docs/ComponentsIndex.md) - SwiftUI component catalog

---

## Skills

使用 skills：~/.claude/skills/build-macos-apps 中的 macOS 开发规范和技能。

---

## Environment

- Python path: `~/.python3/bin/python`
- Activate python: `source ~/.python3/bin/activate`
- Install package: `cd ~/.python3 && uv pip install <package>`
- Xcode generation: `xcodegen generate`
- Syntax validation: `~/.python3/bin/python verify_swift_syntax.py <file.swift>`
- Script files: `Scripts/` directory
- Documentation: `docs/` directory

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
