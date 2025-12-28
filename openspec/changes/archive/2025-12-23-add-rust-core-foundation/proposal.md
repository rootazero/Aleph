# Change: Add Rust Core Foundation with UniFFI

## Why

Aether requires a headless Rust core library that handles all business logic (hotkey detection, clipboard management, AI routing) and exposes APIs to native UIs via UniFFI. This is the foundational infrastructure for the entire project.

Without this core, we cannot:
- Detect global hotkeys across the system
- Read/write clipboard content programmatically
- Define a clean FFI boundary for Swift/C#/GTK clients
- Build the AI routing and provider integration layer

This change establishes the "brain" of Aether as a library-first architecture.

## What Changes

- Create Rust workspace with `core/` crate configured as `cdylib` + `staticlib`
- Implement **working** global hotkey listener using `rdev` crate
- Implement **working** clipboard reader using `arboard` crate
- Define `AetherCore` struct as the main entry point with lifecycle management
- Define `AetherEventHandler` trait for callback-based UI communication
- Create UniFFI interface definition file (`aether.udl`) for FFI boundary
- Set up `tokio` async runtime for non-blocking operations
- Implement basic error handling with custom error types
- Add configuration structure for future TOML config integration
- Create modular trait-based architecture (ClipboardManager, HotkeyListener, InputSimulator traits)

**Deliverables:**
- `core/Cargo.toml` with dependencies (uniffi, rdev, arboard, tokio, serde)
- `core/src/lib.rs` with UniFFI exports
- `core/src/aether.udl` defining the FFI interface
- Working hotkey detection (can detect Cmd+~ press)
- Working clipboard read capability
- Unit tests for core components
- Build script to generate Swift bindings (`uniffi-bindgen`)

**Out of Scope (Future Proposals):**
- macOS Swift client implementation (Proposal #2)
- Full UniFFI Swift integration and Xcode project (Proposal #3)
- Keyboard input simulation (enigo) - Phase 2
- AI provider clients (OpenAI, Claude, etc.) - Phase 4
- Routing logic and rules engine - Phase 4

## Impact

**Affected specs:**
- **NEW**: `core-library` - Rust core library structure and initialization
- **NEW**: `hotkey-detection` - Global hotkey listening capability
- **NEW**: `clipboard-management` - Clipboard read/write operations
- **NEW**: `uniffi-bridge` - FFI interface definition and binding generation
- **NEW**: `event-handler` - Callback trait for UI state updates

**Affected code:**
- Creates new directory: `core/`
- No existing code modified (greenfield project)

**Dependencies:**
- Requires Rust 1.70+ and cargo
- Requires uniffi-bindgen binary for Swift binding generation
- Platform: macOS 13+ for development/testing

**Breaking changes:**
- None (initial implementation)

**Migration:**
- N/A (no existing implementation)
