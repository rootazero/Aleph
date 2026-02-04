# Aleph Core

Rust library providing core functionality for the Aleph AI middleware system. This is a shared library (cdylib/staticlib) that exposes a clean FFI boundary via Mozilla UniFFI for native client integration.

## Overview

Aleph Core is a headless service library that provides:
- Global hotkey detection (Cmd+~ on macOS, Ctrl+~ on Windows/Linux)
- Cross-platform clipboard management (text and images)
- Event callback system for Rust → Native UI communication
- Trait-based architecture for swappable implementations
- UniFFI-generated bindings for Swift, Kotlin, and C#

## Architecture

```
┌─────────────────────────────────────┐
│   Native UI (Swift/Kotlin/C#)      │
│   - Settings interface              │
│   - Menu bar/System tray            │
└──────────────┬──────────────────────┘
               │ UniFFI FFI
               ▼
┌─────────────────────────────────────┐
│       Aleph Core (Rust)            │
│  ┌─────────────────────────────┐   │
│  │     AlephCore              │   │
│  │  - Orchestrates components  │   │
│  └──────────┬──────────────────┘   │
│             │                       │
│  ┌──────────┼──────────────────┐   │
│  ▼          ▼          ▼       ▼   │
│ Hotkey  Clipboard  EventHandler    │
│ (rdev)  (arboard)  (callbacks)     │
└─────────────────────────────────────┘
```

## Building

### Prerequisites

- Rust 1.70+ (install via [rustup](https://rustup.rs/))
- macOS: Xcode Command Line Tools
- Linux: `libx11-dev`, `libxtst-dev`, `libxcb1-dev`

### Build Commands

```bash
# Development build
cargo build

# Release build (optimized)
cargo build --release

# Run tests
cargo test

# Run lints
cargo clippy
cargo fmt --check
```

### Build Outputs

After `cargo build --release`, you'll find:

- **macOS**: `target/release/libalephcore.dylib` (shared library)
- **Windows**: `target/release/alephcore.dll`
- **Linux**: `target/release/libalephcore.so`
- **Static library**: `target/release/libalephcore.a` (all platforms)

## Generating Language Bindings

UniFFI automatically generates language bindings from the `src/aether.udl` interface definition.

### Swift (macOS)

```bash
# Install uniffi-bindgen
cargo install uniffi-bindgen

# Generate Swift bindings
uniffi-bindgen generate src/aether.udl --language swift --out-dir ../bindings/swift
```

This creates:
- `aether.swift` - Swift interface code
- `aetherFFI.h` - C header for FFI bridge
- `aetherFFI.modulemap` - Module map for Swift import

### Kotlin (Android/JVM)

```bash
uniffi-bindgen generate src/aether.udl --language kotlin --out-dir ../bindings/kotlin
```

### C# (.NET)

```bash
# Note: C# support requires additional setup
uniffi-bindgen generate src/aether.udl --language csharp --out-dir ../bindings/csharp
```

## Usage Example (Rust)

```rust
use alephcore::{AetherCore, AlephEventHandler, ProcessingState};

// Implement the callback trait
struct MyHandler;
impl AlephEventHandler for MyHandler {
    fn on_state_changed(&self, state: ProcessingState) {
        println!("State changed: {:?}", state);
    }

    fn on_hotkey_detected(&self, clipboard_content: String) {
        println!("Hotkey detected! Clipboard: {}", clipboard_content);
    }

    fn on_error(&self, message: String) {
        eprintln!("Error: {}", message);
    }
}

// Create and use the core
let handler = Box::new(MyHandler);
let core = AlephCore::new(handler).unwrap();

// Start listening for Cmd+~
core.start_listening().unwrap();

// ... application runs ...

// Stop listening
core.stop_listening().unwrap();
```

## Usage Example (Swift)

```swift
import aether

// Implement the callback protocol
class SwiftEventHandler: AlephEventHandler {
    func onStateChanged(state: ProcessingState) {
        print("State: \(state)")
    }

    func onHotkeyDetected(clipboardContent: String) {
        print("Hotkey! Content: \(clipboardContent)")
    }

    func onError(message: String) {
        print("Error: \(message)")
    }
}

// Create and use
let handler = SwiftEventHandler()
let core = try AlephCore(handler: handler)

// Start listening
try core.startListening()

// Later...
try core.stopListening()
```

## macOS Permissions

On macOS, the application using this library **must** request Accessibility permissions:

1. System Settings → Privacy & Security → Accessibility
2. Add your application to the allowed list
3. Toggle the switch to enable

**In code (Swift):**

```swift
import ApplicationServices

func checkAccessibilityPermission() -> Bool {
    let trusted = AXIsProcessTrusted()
    if !trusted {
        // Prompt user to grant permissions
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue(): true] as CFDictionary
        AXIsProcessTrustedWithOptions(options)
    }
    return trusted
}
```

Without these permissions, hotkey detection will not work.

## Testing

```bash
# Run all tests
cargo test

# Run specific module tests
cargo test clipboard
cargo test hotkey

# Run tests with output
cargo test -- --nocapture
```

**Note**: Some clipboard tests may fail in headless/CI environments as arboard requires a GUI event loop. This is expected behavior.

## Project Structure

```
core/
├── src/
│   ├── aether.udl           # UniFFI interface definition
│   ├── lib.rs               # Library entry point
│   ├── core.rs              # AlephCore orchestrator
│   ├── error.rs             # Error types
│   ├── event_handler.rs     # Callback trait
│   ├── clipboard/           # Clipboard management
│   │   ├── mod.rs
│   │   └── arboard_manager.rs
│   ├── hotkey/              # Hotkey detection
│   │   ├── mod.rs
│   │   └── rdev_listener.rs
│   ├── input/               # Input simulation (Phase 2)
│   │   └── mod.rs
│   └── config.rs            # Configuration (Phase 3)
├── Cargo.toml
├── build.rs                 # UniFFI scaffolding generator
└── README.md
```

## Dependencies

- **uniffi** (0.25.3) - FFI binding generation
- **rdev** (0.5) - Cross-platform keyboard/mouse event listening
- **arboard** (3.3) - Cross-platform clipboard access
- **tokio** (1.35) - Async runtime for future AI API calls
- **serde** (1.0) - Serialization (for config files)
- **thiserror** (1.0) - Error handling

## Phase 1 Scope

This implementation provides:
- Working hotkey detection (Cmd+~ hardcoded)
- Working clipboard reading (text only)
- UniFFI interface for Swift/Kotlin/C# bindings
- Callback-based event system
- Trait-based architecture for testability

## Future Phases

- **Phase 2**: Keyboard simulation (Cmd+X, Cmd+V) using enigo
- **Phase 3**: Configuration system with TOML file support
- **Phase 4**: AI provider clients (OpenAI, Claude, Gemini, Ollama)
- **Phase 5**: Smart routing and multi-model orchestration

## Contributing

This is part of the Aleph project. See the main [CLAUDE.md](../CLAUDE.md) for full architecture documentation.

## License

[To be determined]
