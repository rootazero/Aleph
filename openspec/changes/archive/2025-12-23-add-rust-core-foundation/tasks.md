# Implementation Tasks

## 1. Project Setup
- [ ] 1.1 Create `core/` directory and initialize Cargo workspace
- [ ] 1.2 Configure `Cargo.toml` with `crate-type = ["cdylib", "staticlib"]`
- [ ] 1.3 Add dependencies: uniffi, rdev, arboard, tokio, serde, thiserror
- [ ] 1.4 Create `uniffi.toml` configuration file
- [ ] 1.5 Set up `.gitignore` for Rust artifacts (target/, Cargo.lock)

## 2. Error Handling Infrastructure
- [ ] 2.1 Create `src/error.rs` with `AlephError` enum
- [ ] 2.2 Implement error variants: HotkeyError, ClipboardError, CallbackError
- [ ] 2.3 Add `thiserror` derive macros for error messages
- [ ] 2.4 Write unit tests for error creation and Display impl

## 3. Event Handler Trait
- [ ] 3.1 Create `src/event_handler.rs` with `AlephEventHandler` trait
- [ ] 3.2 Define trait methods: on_state_changed, on_hotkey_detected, on_error
- [ ] 3.3 Create mock implementation for testing
- [ ] 3.4 Write unit tests for mock handler

## 4. Clipboard Management
- [ ] 4.1 Create `src/clipboard/mod.rs` with `ClipboardManager` trait
- [ ] 4.2 Define trait methods: read_text, write_text
- [ ] 4.3 Create `src/clipboard/arboard_manager.rs` implementing the trait
- [ ] 4.4 Handle arboard errors and convert to AlephError
- [ ] 4.5 Write unit tests: read/write text, error cases
- [ ] 4.6 Add integration test: write then read clipboard content

## 5. Hotkey Detection
- [ ] 5.1 Create `src/hotkey/mod.rs` with `HotkeyListener` trait
- [ ] 5.2 Define trait methods: start_listening, stop_listening
- [ ] 5.3 Create `src/hotkey/rdev_listener.rs` implementing the trait
- [ ] 5.4 Implement hardcoded Cmd+~ detection (Key::Grave + Cmd modifier)
- [ ] 5.5 Handle rdev callback and forward to AlephCore
- [ ] 5.6 Add thread-safe state management for listener lifecycle
- [ ] 5.7 Write unit tests: hotkey pattern matching
- [ ] 5.8 Add manual test instructions (requires Accessibility permissions)

## 6. Core Library Entry Point
- [ ] 6.1 Create `src/core.rs` with `AlephCore` struct
- [ ] 6.2 Implement constructor: `new(event_handler)`
- [ ] 6.3 Implement `start_listening()` → spawns rdev thread
- [ ] 6.4 Implement `stop_listening()` → stops rdev thread
- [ ] 6.5 Implement `get_clipboard_text()` → calls clipboard manager
- [ ] 6.6 Add tokio runtime initialization
- [ ] 6.7 Handle callback invocation (Rust → event_handler)
- [ ] 6.8 Write unit tests with mock dependencies

## 7. UniFFI Interface Definition
- [ ] 7.1 Create `src/aleph.udl` interface file
- [ ] 7.2 Define `ProcessingState` enum (Idle, Listening, Processing, Success, Error)
- [ ] 7.3 Define `AlephCore` interface with methods
- [ ] 7.4 Define `AlephEventHandler` callback interface
- [ ] 7.5 Define `Config` dictionary (stub for future use)
- [ ] 7.6 Add namespace and init function

## 8. Library Exports (lib.rs)
- [ ] 8.1 Create `src/lib.rs` with UniFFI exports
- [ ] 8.2 Add `uniffi::include_scaffolding!` macro
- [ ] 8.3 Re-export public types (AlephCore, ProcessingState, etc.)
- [ ] 8.4 Add module declarations (mod core, mod hotkey, etc.)
- [ ] 8.5 Verify all public APIs are documented

## 9. Configuration Stub
- [ ] 9.1 Create `src/config.rs` with `Config` struct
- [ ] 9.2 Add fields: default_hotkey (String)
- [ ] 9.3 Add Serialize/Deserialize derives for future TOML support
- [ ] 9.4 Implement Default trait with sensible defaults

## 10. Input Simulator Stub
- [ ] 10.1 Create `src/input/mod.rs` with `InputSimulator` trait
- [ ] 10.2 Define trait methods: simulate_cut, simulate_paste (stubs)
- [ ] 10.3 Add TODO comments for Phase 2 implementation
- [ ] 10.4 Create placeholder struct for future enigo integration

## 11. Build and Bindings
- [ ] 11.1 Add build script if needed (uniffi-bindgen integration)
- [ ] 11.2 Test build: `cargo build --release`
- [ ] 11.3 Verify .dylib output in target/release/
- [ ] 11.4 Generate Swift bindings: `cargo run --bin uniffi-bindgen generate src/aleph.udl --language swift`
- [ ] 11.5 Verify Swift bindings compile without errors

## 12. Testing and Validation
- [ ] 12.1 Run all unit tests: `cargo test`
- [ ] 12.2 Run clippy: `cargo clippy --all-targets -- -D warnings`
- [ ] 12.3 Run rustfmt: `cargo fmt --check`
- [ ] 12.4 Test hotkey detection manually (requires Accessibility permission)
- [ ] 12.5 Test clipboard read/write manually
- [ ] 12.6 Verify no panics during operation

## 13. Documentation
- [ ] 13.1 Add module-level documentation to lib.rs
- [ ] 13.2 Document all public APIs with `///` doc comments
- [ ] 13.3 Add usage examples in lib.rs doc comment
- [ ] 13.4 Create README.md in core/ with build instructions
- [ ] 13.5 Document macOS permission requirements

## 14. CI/CD Setup
- [ ] 14.1 Create `.github/workflows/rust.yml`
- [ ] 14.2 Add job: Run `cargo test` on push/PR
- [ ] 14.3 Add job: Run `cargo clippy`
- [ ] 14.4 Add job: Run `cargo fmt --check`
- [ ] 14.5 Test CI pipeline with dummy commit

## Dependencies

**Sequential Dependencies:**
- 2 (Error Handling) must complete before 4, 5, 6
- 3 (Event Handler) must complete before 6
- 4, 5 must complete before 6 (Core Library)
- 7 (UniFFI .udl) must complete before 8 (lib.rs exports)
- All implementation (1-10) must complete before 11 (Build)
- 11 must complete before 12 (Testing)

**Parallelizable Work:**
- 4 (Clipboard) and 5 (Hotkey) can be developed in parallel
- 9 (Config stub) and 10 (Input stub) can be done anytime before 8
- 13 (Documentation) can be done alongside implementation
- 14 (CI/CD) can be set up anytime after 1 (Project Setup)
