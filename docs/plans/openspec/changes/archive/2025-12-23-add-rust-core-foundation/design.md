# Design: Rust Core Foundation

## Context

Aleph needs a platform-agnostic core library that can be consumed by native UI clients (Swift on macOS, C# on Windows, GTK on Linux). The core must:
1. Expose a clean FFI boundary via UniFFI
2. Handle system-level operations (hotkeys, clipboard)
3. Support async operations without blocking
4. Provide extensibility for future features (AI providers, routing)

**Stakeholders:**
- macOS client (Swift) - primary consumer
- Windows/Linux clients (future) - secondary consumers
- Development team - maintainability and testability

**Constraints:**
- Must be library (not binary) - compiled as `cdylib` for dynamic linking
- Must use UniFFI for automatic binding generation (no manual FFI)
- Must work on macOS 13+ initially (cross-platform later)
- Must not panic in library code (all errors via Result<T, E>)

## Goals / Non-Goals

**Goals:**
- ✅ Create working hotkey detection (detect Cmd+~ press)
- ✅ Create working clipboard reading (read text/image content)
- ✅ Define UniFFI interface for callbacks (AlephEventHandler trait)
- ✅ Establish trait-based architecture for modularity
- ✅ Set up async runtime (tokio) for future async operations
- ✅ Prove FFI boundary works (generate Swift bindings successfully)

**Non-Goals:**
- ❌ Implement Swift client (separate proposal)
- ❌ Implement keyboard simulation (Phase 2)
- ❌ Implement AI providers (Phase 4)
- ❌ Implement routing logic (Phase 4)
- ❌ Build configuration UI
- ❌ Windows/Linux platform support (macOS first)

## Decisions

### Decision 1: Use UniFFI instead of manual FFI

**Rationale:**
- UniFFI automatically generates safe Swift/Kotlin/C# bindings from `.udl` interface definition
- Handles memory management and type conversions across FFI boundary
- Supports callbacks (Rust → Swift) which is critical for our event-driven architecture
- Reduces boilerplate and potential memory safety bugs

**Alternatives considered:**
- Manual `extern "C"` FFI: More control, but error-prone and requires manual binding code in Swift
- cbindgen: Only generates C headers, doesn't handle callbacks or Swift-friendly types

**Trade-offs:**
- ✅ Pro: Type-safe, automatic binding generation
- ✅ Pro: Supports callbacks and async patterns
- ⚠️ Con: Less flexible than manual FFI for edge cases
- ⚠️ Con: Adds uniffi dependency and build complexity

### Decision 2: Use rdev for global hotkey detection

**Rationale:**
- Cross-platform library (works on macOS, Windows, Linux)
- Low-level event listening without polling
- Proven in production (used by multiple projects)
- MIT licensed

**Alternatives considered:**
- device_query: Simpler API but polling-based (higher CPU usage)
- Platform-specific APIs (CGEventTap on macOS): More control but not cross-platform

**Trade-offs:**
- ✅ Pro: Cross-platform ready for future Windows/Linux support
- ✅ Pro: Event-driven (efficient)
- ⚠️ Con: Requires Accessibility permissions on macOS

### Decision 3: Use arboard for clipboard management

**Rationale:**
- Cross-platform (macOS, Windows, Linux)
- Supports text, images, and rich text
- Simple, synchronous API
- Well-maintained and widely used

**Alternatives considered:**
- copypasta: Less actively maintained
- Platform-specific APIs (NSPasteboard): Not cross-platform

**Trade-offs:**
- ✅ Pro: Handles multiple content types (text, images)
- ✅ Pro: Cross-platform ready
- ⚠️ Con: Synchronous API (may block on large clipboard content)

### Decision 4: Trait-based architecture for core components

**Rationale:**
- Allows easy mocking in tests (swap real implementations with test doubles)
- Future-proofs for multiple implementations (e.g., different clipboard backends)
- Follows Rust best practices (composition over inheritance)

**Pattern:**
```rust
// Define traits for swappable components
trait HotkeyListener {
    fn start_listening(&self) -> Result<(), AlephError>;
    fn stop_listening(&self) -> Result<(), AlephError>;
}

trait ClipboardManager {
    fn read_text(&self) -> Result<String, AlephError>;
    fn write_text(&self, content: &str) -> Result<(), AlephError>;
}

// AlephCore composes these traits
pub struct AlephCore {
    hotkey_listener: Arc<dyn HotkeyListener>,
    clipboard_manager: Arc<dyn ClipboardManager>,
    event_handler: Arc<dyn AlephEventHandler>,
}
```

**Trade-offs:**
- ✅ Pro: Highly testable (inject mocks)
- ✅ Pro: Extensible (add new implementations easily)
- ⚠️ Con: Slight indirection cost (trait dispatch)

### Decision 5: Use tokio for async runtime

**Rationale:**
- Industry-standard async runtime in Rust ecosystem
- Required for future async AI API calls (reqwest uses tokio)
- Supports multi-threaded execution for concurrent operations
- Excellent ecosystem integration

**Trade-offs:**
- ✅ Pro: Required for future async HTTP calls to AI APIs
- ✅ Pro: Allows non-blocking operations (e.g., clipboard read while processing AI response)
- ⚠️ Con: Adds complexity (need to handle sync/async boundaries)
- ⚠️ Con: Increases binary size (~500KB)

## Architecture

### Module Structure

```
core/
├── Cargo.toml                  # Dependencies and build config
├── build.rs                    # Build script (if needed)
├── uniffi.toml                 # UniFFI configuration
└── src/
    ├── lib.rs                  # UniFFI exports, public API
    ├── aleph.udl              # UniFFI interface definition
    ├── core.rs                 # AlephCore struct
    ├── event_handler.rs        # AlephEventHandler trait
    ├── error.rs                # Custom error types
    ├── config.rs               # Config struct (stub for Phase 1)
    ├── hotkey/
    │   ├── mod.rs              # HotkeyListener trait
    │   └── rdev_listener.rs    # rdev implementation
    ├── clipboard/
    │   ├── mod.rs              # ClipboardManager trait
    │   └── arboard_manager.rs  # arboard implementation
    └── input/
        └── mod.rs              # InputSimulator trait (stub for Phase 2)
```

### Component Interaction

```
┌─────────────────────────────────────────────────────────┐
│  Swift Client (Future Proposal #2)                      │
│  - Implements AlephEventHandler protocol              │
│  - Calls AlephCore methods via UniFFI bindings        │
└─────────────────┬───────────────────────────────────────┘
                  │ UniFFI Bridge
                  ↓
┌─────────────────────────────────────────────────────────┐
│  AlephCore (lib.rs)                                    │
│  - init(event_handler)                                  │
│  - start_listening() → spawns hotkey listener thread   │
│  - stop_listening()                                     │
│  - get_clipboard_content() → reads clipboard           │
└─────────────────┬───────────────────────────────────────┘
                  │
        ┌─────────┴──────────┬──────────────────┐
        ↓                    ↓                   ↓
┌───────────────┐  ┌──────────────────┐  ┌──────────────┐
│ HotkeyListener│  │ ClipboardManager │  │EventHandler  │
│  (rdev impl)  │  │ (arboard impl)   │  │ (callback)   │
└───────────────┘  └──────────────────┘  └──────────────┘
```

### Event Flow (Phase 1)

```
1. Swift calls: core.start_listening()
   ↓
2. Rust: Spawns rdev background thread
   ↓
3. User presses: Cmd + ~
   ↓
4. rdev detects keypress → callback to AlephCore
   ↓
5. AlephCore reads clipboard via arboard
   ↓
6. AlephCore calls: event_handler.on_hotkey_detected(clipboard_content)
   ↓
7. Swift receives callback → updates UI
```

## UniFFI Interface Definition (aleph.udl)

```idl
namespace aleph {
  AlephCore init(AlephEventHandler handler);
};

enum ProcessingState {
  "Idle",
  "Listening",
  "Processing",
  "Success",
  "Error"
};

interface AlephCore {
  constructor(AlephEventHandler handler);
  void start_listening();
  void stop_listening();
  string get_clipboard_text();
};

callback interface AlephEventHandler {
  void on_state_changed(ProcessingState state);
  void on_hotkey_detected(string clipboard_content);
  void on_error(string message);
};

dictionary Config {
  string default_hotkey;
};
```

## Error Handling Strategy

**Custom Error Type:**
```rust
#[derive(Debug, thiserror::Error)]
pub enum AlephError {
    #[error("Hotkey listener error: {0}")]
    HotkeyError(String),

    #[error("Clipboard error: {0}")]
    ClipboardError(String),

    #[error("FFI callback error: {0}")]
    CallbackError(String),
}
```

**Propagation:**
- All public API methods return `Result<T, AlephError>`
- UniFFI converts Rust errors to Swift exceptions
- Never panic in library code (use Result/unwrap_or)

## Testing Strategy

**Unit Tests:**
```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Mock event handler for testing
    struct MockEventHandler {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl AlephEventHandler for MockEventHandler {
        fn on_hotkey_detected(&self, content: String) {
            self.calls.lock().unwrap().push(content);
        }
    }

    #[test]
    fn test_clipboard_read() {
        let clipboard = ArboardManager::new();
        clipboard.write_text("test").unwrap();
        assert_eq!(clipboard.read_text().unwrap(), "test");
    }
}
```

**Integration Tests:**
- Test hotkey detection (requires macOS permissions)
- Test clipboard read/write with different content types
- Test callback invocation from Rust → Mock handler

**Manual Testing:**
- Run `cargo test` to verify unit tests
- Build .dylib and verify Swift bindings generation
- Test with minimal Swift app (Proposal #2)

## Risks / Trade-offs

### Risk 1: macOS Accessibility Permissions
**Issue:** rdev requires Accessibility permissions to detect global hotkeys. If denied, hotkey detection fails silently.

**Mitigation:**
- Document permission requirements clearly
- Add error handling for permission-denied case
- Swift client (Proposal #2) will check permissions on startup

### Risk 2: UniFFI Learning Curve
**Issue:** Team may not be familiar with UniFFI patterns (`.udl` syntax, callback interfaces).

**Mitigation:**
- Provide clear examples in this design doc
- Reference UniFFI documentation in code comments
- Start simple (basic types first, complex patterns later)

### Risk 3: Cross-Platform Hotkey Differences
**Issue:** Cmd+~ works on macOS, but Windows/Linux use different modifier keys.

**Mitigation:**
- Make hotkey configurable (accept key code + modifiers)
- Platform-specific defaults in config
- Future: UI for hotkey customization

### Risk 4: Clipboard Performance with Large Content
**Issue:** arboard is synchronous - reading large images may block thread.

**Mitigation:**
- Use tokio::task::spawn_blocking for clipboard operations
- Add timeout for clipboard reads (5s max)
- Future: Stream large content instead of loading all in memory

## Migration Plan

N/A (initial implementation, no existing code to migrate)

**Rollback Plan:**
- This is foundational infrastructure
- If issues arise, entire proposal can be reverted (delete `core/` directory)
- No user-facing impact until Proposal #2 (Swift client) is implemented

## Open Questions

1. **Hotkey Configuration:** Should we hardcode Cmd+~ for Phase 1, or add a `Config` struct now?
   - **Recommendation:** Hardcode for Phase 1, add config in Phase 4 with TOML parsing

2. **Error Logging:** Should we add logging (tracing/log crate) in Phase 1?
   - **Recommendation:** Add basic `tracing` for debugging during development

3. **Binary Size:** Should we optimize for binary size (use dynamic linking for tokio)?
   - **Recommendation:** No optimization in Phase 1, address in Phase 6 (polish)

4. **CI/CD:** Should we set up GitHub Actions for Rust tests?
   - **Recommendation:** Yes, add basic `cargo test` CI in this proposal

## Success Criteria

✅ **Phase 1 is successful when:**
1. `cargo build --release` produces `libalephcore.dylib`
2. `cargo test` passes all unit tests
3. `uniffi-bindgen generate src/aleph.udl --language swift` generates valid Swift bindings
4. Manual test: Pressing Cmd+~ triggers callback with clipboard content
5. Manual test: Reading clipboard returns correct text
6. No panics or crashes during operation
7. All clippy lints pass (`cargo clippy --all-targets`)
