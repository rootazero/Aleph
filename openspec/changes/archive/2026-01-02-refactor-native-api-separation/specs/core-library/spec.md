# core-library Spec Delta (refactor-native-api-separation)

## MODIFIED Requirements

### Requirement: Platform-Agnostic Business Logic
The Rust core library SHALL contain ONLY platform-agnostic business logic, with NO dependencies on platform-specific system APIs.

**Previous**: Rust core included hotkey detection (rdev), clipboard management (arboard), and keyboard simulation (enigo).
**Changed to**: Rust core focuses solely on AI routing, memory system, and provider communication.

#### Scenario: Rust core dependencies (MODIFIED)
- **WHEN** examining `Cargo.toml` dependencies
- **THEN** does NOT include:
  - `rdev` (hotkey detection → moved to Swift)
  - `arboard` (clipboard → moved to Swift)
  - `enigo` (keyboard simulation → moved to Swift)
  - `core-foundation` (macOS-specific, only used by rdev)
  - `core-graphics` (macOS-specific, only used by rdev)
- **AND** only includes platform-agnostic crates:
  - `tokio` (async runtime)
  - `reqwest` (HTTP client)
  - `rusqlite` (embedded database)
  - `serde` (serialization)
  - `uniffi` (FFI bindings)

#### Scenario: Rust core exports (MODIFIED)
- **WHEN** examining public API in `lib.rs`
- **THEN** does NOT export:
  - `HotkeyListener` trait
  - `ClipboardManager` trait
  - `InputSimulator` trait
- **AND** only exports:
  - `AetherCore` (main interface)
  - `AetherEventHandler` callback trait
  - Configuration types
  - Memory types
  - Provider types

### Requirement: Simplified Core Interface
The AetherCore interface SHALL provide high-level business operations, not low-level system API wrappers.

#### Scenario: Core method signatures (MODIFIED)
- **WHEN** calling AetherCore methods
- **THEN** provides `process_input(user_input, context)` (high-level)
- **AND** does NOT provide:
  - `start_listening()` (moved to Swift)
  - `stop_listening()` (moved to Swift)
  - `get_clipboard_text()` (moved to Swift)
  - `read_clipboard_image()` (moved to Swift)
  - `write_clipboard_image()` (moved to Swift)

## REMOVED Requirements

### Requirement: System API Abstraction Layer (REMOVED)
**Reason**: System APIs are now handled by platform-native code (Swift for macOS). Rust core no longer provides system API abstractions.

**Previous scenarios removed**:
- Trait-based clipboard abstraction
- Trait-based hotkey listener abstraction
- Trait-based input simulator abstraction

### Requirement: Cross-Platform System API Support (REMOVED)
**Reason**: Cross-platform support is achieved by implementing multiple frontend layers (Swift, C#, GTK), not by Rust wrapping platform APIs.

**Previous scenarios removed**:
- Platform-specific cfg blocks for system APIs
- Cross-platform key code mapping
- Cross-platform clipboard format handling

## ADDED Requirements

### Requirement: Pure Business Logic Core
The Rust core SHALL implement ONLY business logic: AI routing, memory operations, provider communication, and configuration management.

#### Scenario: AI processing pipeline
- **WHEN** Swift calls `core.process_input(text, context)`
- **THEN** Rust performs:
  1. PII scrubbing (regex-based filtering)
  2. Memory retrieval (vector search, optional)
  3. Provider selection (routing rules)
  4. AI HTTP call (async reqwest)
  5. Memory storage (SQLite + embeddings)
- **AND** does NOT perform any system API calls
- **AND** returns AI response string

#### Scenario: Configuration management
- **WHEN** loading configuration
- **THEN** Rust reads `~/.aether/config.toml`
- **AND** parses TOML using serde
- **AND** validates provider configs
- **AND** does NOT interact with Keychain (handled by Swift via callback)

#### Scenario: Memory system operations
- **WHEN** performing memory operations
- **THEN** Rust manages:
  - Vector database (rusqlite + sqlite-vec)
  - Embedding inference (ONNX Runtime)
  - Semantic search algorithms
- **AND** does NOT capture context (handled by Swift)
- **AND** receives context as function parameter

### Requirement: Minimal FFI Surface
The Rust core SHALL minimize FFI boundary crossings by accepting pre-processed input and returning final output.

#### Scenario: Single FFI call for processing
- **WHEN** user triggers Aether
- **THEN** Swift performs ALL preprocessing:
  - Read clipboard (ClipboardManager)
  - Capture context (ContextCapture)
  - Combine into single input
- **AND** makes ONE FFI call: `core.process_input(input, context)`
- **AND** Rust performs all business logic
- **AND** returns ONE response string
- **AND** Swift performs ALL postprocessing:
  - Keyboard simulation (KeyboardSimulator)
  - UI updates (Halo animation)

#### Scenario: Reduced callback frequency
- **WHEN** processing user input
- **THEN** Rust calls back to Swift only for:
  - State changes (on_state_changed)
  - Errors (on_error)
  - Progress updates (on_progress, optional)
- **AND** does NOT callback for:
  - Hotkey detection (no longer in Rust)
  - Clipboard polling (no longer in Rust)

### Requirement: Binary Size Optimization
The Rust core binary SHALL be optimized for size by removing unnecessary platform-specific dependencies.

#### Scenario: Reduced binary size
- **WHEN** building Rust core in release mode
- **THEN** binary size reduces by >= 2MB
- **AND** removed crates:
  - rdev: ~500KB
  - arboard: ~200KB
  - enigo: ~300KB
  - core-graphics: ~1MB
- **AND** improves app launch time

#### Scenario: Faster compilation
- **WHEN** running `cargo build`
- **THEN** compilation time reduces by >= 20%
- **AND** fewer crates to compile
- **AND** no platform-specific conditional compilation

---

**Related specs**: `uniffi-bridge`, `hotkey-detection`, `clipboard-management`, `keyboard-simulation`
**Relationship**: This change simplifies `core-library` by removing system API responsibilities, which are moved to platform-specific specs.
