# core-library Specification

## Purpose
TBD - created by archiving change add-rust-core-foundation. Update Purpose after archive.
## Requirements
### Requirement: Library Initialization
The system SHALL provide a library-based Rust core that can be compiled as a dynamic library (cdylib) and static library (staticlib) for consumption by native UI clients, targeting Rust 1.92+ for modern standard library features.

#### Scenario: Build dynamic library for macOS with Rust 1.92
- **WHEN** developer runs `cargo build --release --target aarch64-apple-darwin`
- **THEN** the build uses Rust 1.92 or higher as specified in Cargo.toml
- **AND** the build produces `libaethecore.dylib` in target/release/
- **AND** the library exports C-compatible symbols for FFI
- **AND** the library uses standard library features (OnceLock, native async traits) instead of external crates

#### Scenario: Build static library
- **WHEN** developer runs `cargo build --release --lib`
- **THEN** the build produces `libaethecore.a` for static linking
- **AND** the library contains all required dependencies
- **AND** the library does NOT include removed dependencies (once_cell, async-trait)

#### Scenario: Verify minimum Rust version requirement
- **WHEN** developer attempts to build with Rust < 1.92
- **THEN** cargo SHALL fail with error: "package requires `rustc 1.92` or newer"
- **AND** the error message directs the user to update Rust toolchain

### Requirement: Core Lifecycle Management
The system SHALL provide an `AetherCore` struct that manages the lifecycle of all core components with robust permission handling and error recovery.

#### Scenario: Start listening for system events (MODIFIED)
- **WHEN** client calls `core.start_listening()`
- **THEN** the system performs permission pre-check (new step)
- **AND** if permission is NOT granted, returns `Err(AetherError::PermissionDenied)` immediately
- **AND** if permission IS granted, wraps `rdev::listen()` in `catch_unwind()` (new protection)
- **AND** the hotkey listener spawns a background thread
- **AND** begins monitoring for global hotkey events
- **AND** returns success if no errors occur

#### Scenario: Handle initialization error (MODIFIED)
- **WHEN** core initialization fails (e.g., hotkey listener cannot start or panics)
- **THEN** the constructor catches any panics via `catch_unwind()`
- **AND** returns an `AetherError` with descriptive message
- **AND** provides actionable guidance (e.g., "Input Monitoring permission required")
- **AND** does NOT crash the application

### Requirement: Thread-Safe Callback Invocation
The system SHALL support thread-safe callback invocation from Rust to client code via the event handler trait.

#### Scenario: Invoke callback from background thread
- **WHEN** hotkey is detected on rdev background thread
- **THEN** the core invokes `event_handler.on_hotkey_detected(content)`
- **AND** the callback executes safely across thread boundary
- **AND** uses Arc<dyn EventHandler> for shared ownership

### Requirement: Error Handling
The system SHALL provide comprehensive error handling and logging for all permission-related failures, enabling effective troubleshooting.

#### Scenario: Hotkey listener error (MODIFIED)
- **WHEN** hotkey listener fails to start (e.g., permissions denied or rdev panic)
- **THEN** catches panic via `catch_unwind()` if rdev panics
- **AND** returns `AetherError::HotkeyError` or `AetherError::PermissionDenied` with descriptive message
- **AND** error can be propagated through Result<T, E>
- **AND** logs detailed error information for debugging

#### Scenario: No panics in library code (ENHANCED)
- **WHEN** any error occurs during operation, including rdev panics
- **THEN** the library catches panics from external dependencies via `catch_unwind()`
- **AND** returns a Result type instead of crashing
- **AND** never allows panics to propagate to client code
- **AND** client can handle errors gracefully

#### Scenario: Log permission check failures (NEW)
- **WHEN** permission pre-check fails
- **THEN** the system logs at WARN level:
  - "Input Monitoring permission not granted. Cannot start hotkey listener."
- **AND** logs the current permission status for debugging
- **AND** returns structured error to Swift layer

#### Scenario: Log rdev panic details (NEW)
- **WHEN** `rdev::listen()` panics
- **THEN** the system logs at ERROR level:
  - "rdev listener panicked: [panic message]"
  - "This usually means Input Monitoring permission is not granted."
  - "Please grant permission in: System Settings > Privacy & Security > Input Monitoring"
- **AND** extracts panic payload (String or &str)
- **AND** logs full error context for debugging

### Requirement: Async Runtime Support
The system SHALL initialize a tokio async runtime to support non-blocking operations for AI API calls, utilizing native async trait implementations without external macro dependencies.

#### Scenario: Runtime initialization
- **WHEN** AetherCore is initialized
- **THEN** tokio runtime is created successfully
- **AND** supports multi-threaded execution
- **AND** allows spawning async tasks
- **AND** uses native async fn in traits (no async-trait macro required)

#### Scenario: Native async trait implementation
- **WHEN** AI provider implements the AiProvider trait
- **THEN** the implementation uses native `async fn process()` syntax
- **AND** does NOT require `#[async_trait]` attribute
- **AND** the compiler generates correct async trait object code
- **AND** trait objects remain Send + Sync compatible

### Requirement: Modular Trait-Based Architecture
The system SHALL define traits for all core components (HotkeyListener, ClipboardManager, InputSimulator) to enable swappable implementations and testing.

#### Scenario: Swap clipboard implementation
- **WHEN** developer wants to use a different clipboard backend
- **THEN** they implement the ClipboardManager trait
- **AND** pass the new implementation to AetherCore
- **AND** no changes to core logic are required

#### Scenario: Mock components in tests
- **WHEN** writing unit tests for AetherCore
- **THEN** developer creates mock implementations of traits
- **AND** injects mocks into AetherCore constructor
- **AND** tests core logic in isolation

### Requirement: Panic Protection for rdev Hotkey Listener
The Rust core SHALL protect against panics from the rdev library that occur when Input Monitoring permission is not granted, preventing application crashes.

#### Scenario: Catch panic from rdev::listen()
- **WHEN** the hotkey listener calls `rdev::listen()` without Input Monitoring permission
- **THEN** the function wraps the call in `std::panic::catch_unwind()`
- **AND** if `rdev::listen()` panics, the panic is caught and converted to an error
- **AND** returns `Err(HotkeyError::PermissionDenied)` instead of crashing

#### Scenario: Log detailed panic information
- **WHEN** a panic is caught from `rdev::listen()`
- **THEN** the system extracts the panic payload message
- **AND** logs the panic message with ERROR level
- **AND** logs user-friendly guidance: "Input Monitoring permission required. Please grant in System Settings > Privacy & Security > Input Monitoring"
- **AND** returns a descriptive error to Swift layer via UniFFI

#### Scenario: Graceful degradation after panic
- **WHEN** hotkey listener fails to start due to panic
- **THEN** the system does NOT terminate the application
- **AND** AetherCore remains in a valid state (core.is_listening = false)
- **AND** notifies Swift layer via `event_handler.on_error()` callback
- **AND** user can retry after granting permission or use other features

### Requirement: Permission Pre-Check Before Starting Hotkey Listener
The Rust core SHALL verify Input Monitoring permission status before attempting to start the rdev hotkey listener, preventing unnecessary panic attempts.

#### Scenario: Check permission before calling rdev::listen()
- **WHEN** Swift layer calls `core.start_listening()`
- **THEN** the system checks `self.has_input_monitoring_permission` flag
- **AND** if permission is NOT granted, returns `Err(AetherError::PermissionDenied)` immediately
- **AND** does NOT call `rdev::listen()` at all
- **AND** logs warning: "Input Monitoring permission not granted. Cannot start hotkey listener."

#### Scenario: Permission pre-check passes
- **WHEN** `has_input_monitoring_permission` is true
- **THEN** the system proceeds to call `start_rdev_listener()`
- **AND** wraps the call in panic protection (catch_unwind)
- **AND** logs info: "Starting hotkey listener with permission granted"

#### Scenario: Swift updates permission status
- **WHEN** Swift layer detects permission status change
- **THEN** Swift calls `core.set_input_monitoring_permission(true/false)` via UniFFI
- **AND** Rust core updates internal `has_input_monitoring_permission` flag
- **AND** if permission becomes true, Swift can retry `core.start_listening()`

### Requirement: UniFFI Error Propagation for Permission Issues
The Rust core SHALL propagate permission-related errors to Swift layer via UniFFI callbacks and return values, enabling proper UI feedback.

#### Scenario: Return permission error via Result type
- **WHEN** `start_listening()` fails due to missing permission
- **THEN** the function returns `Err(AetherError::PermissionDenied(message))`
- **AND** Swift layer receives the error via UniFFI binding
- **AND** Swift can display error alert or update UI status

#### Scenario: Notify Swift via event handler callback
- **WHEN** permission error occurs
- **THEN** Rust calls `self.event_handler.on_error(error_message)`
- **AND** Swift's `AetherEventHandler` implementation receives the callback
- **AND** Swift can show real-time error notification or update permission gate UI

#### Scenario: Error message includes actionable guidance
- **WHEN** permission error is returned or notified
- **THEN** the error message includes clear guidance:
  - "Input Monitoring permission not granted. Please grant in System Settings > Privacy & Security > Input Monitoring."
- **AND** Swift layer can display this message to user
- **AND** user knows exactly what action to take

### Requirement: Panic Safety in Hotkey Listener Thread
The hotkey listener thread SHALL be designed to be panic-safe, ensuring that panics in the listener do not corrupt shared state or leak resources.

#### Scenario: Panic-safe thread execution
- **WHEN** rdev listener runs in a separate thread
- **THEN** the thread is wrapped in `catch_unwind()` at the entry point
- **AND** if panic occurs, the thread terminates gracefully without poisoning mutexes
- **AND** parent thread can detect listener failure via channel or status check

#### Scenario: Resource cleanup after panic
- **WHEN** a panic is caught in the listener thread
- **THEN** the system logs the panic before thread termination
- **AND** releases any held resources (channels, mutexes, file handles)
- **AND** sets listener status to "stopped" or "failed"
- **AND** parent thread can restart listener if permission is re-granted

### Requirement: Standard Library Synchronization Primitives
The system SHALL use Rust standard library synchronization primitives (OnceLock, LazyLock) for lazy initialization and thread-safe state management, eliminating external dependencies.

#### Scenario: Lazy initialization with OnceLock
- **WHEN** EmbeddingModel needs lazy initialization of model files
- **THEN** the system uses `std::sync::OnceLock<bool>` for initialization tracking
- **AND** uses `.get_or_try_init()` for thread-safe initialization
- **AND** does NOT use the `once_cell` crate

#### Scenario: Thread-safe lazy static with LazyLock
- **WHEN** the system needs a lazily-initialized global constant
- **THEN** the system uses `std::sync::LazyLock<T>` instead of `lazy_static!` macro
- **AND** initialization code executes exactly once
- **AND** provides thread-safe access without runtime overhead after initialization

### Requirement: Safe Zero-Initialization for FFI
The system SHALL use standard library's `Arc::new_zeroed` and `Box::new_zeroed` for allocating zero-initialized memory in FFI contexts, eliminating manual `MaybeUninit` initialization patterns.

#### Scenario: Allocate zero-initialized Arc for FFI
- **WHEN** the system needs to allocate zero-initialized memory in an Arc to pass to C code
- **THEN** it uses `Arc::new_zeroed()` to create the allocation
- **AND** calls `.assume_init()` after verifying safety invariants
- **AND** does NOT manually construct `MaybeUninit<T>` and perform initialization dance

#### Scenario: Allocate zero-initialized Box for FFI buffers
- **WHEN** the system needs a zero-initialized heap buffer for FFI boundary
- **THEN** it uses `Box::new_zeroed()` for type-safe allocation
- **AND** the API provides ergonomic, safe interface compared to manual `MaybeUninit::zeroed()`
- **AND** reduces boilerplate code for common FFI pattern

#### Scenario: Prevent unsafe initialization anti-patterns
- **WHEN** code review detects manual zero-initialization patterns
- **THEN** reviewer SHALL recommend `Arc::new_zeroed` or `Box::new_zeroed` instead
- **AND** clippy lint (if available) SHALL warn on manual `MaybeUninit::zeroed()` usage
- **AND** code maintains type safety without verbose initialization ceremony

### Requirement: UniFFI 0.28 Compatibility
The system SHALL use UniFFI 0.28 or higher for FFI binding generation, leveraging modern performance optimizations and C string literal support.

#### Scenario: Generate Swift bindings with UniFFI 0.28
- **WHEN** developer runs `cargo run --bin uniffi-bindgen generate src/aether.udl --language swift`
- **THEN** the system uses UniFFI 0.28 binding generator
- **AND** generates Swift bindings compatible with existing UDL schema
- **AND** utilizes C string literals (c"...") for improved performance where applicable

#### Scenario: Maintain backward compatibility
- **WHEN** UniFFI bindings are regenerated with version 0.28
- **THEN** the generated Swift API surface SHALL remain unchanged
- **AND** existing Swift code continues to compile without modifications
- **AND** no breaking changes to `AetherCore`, `AetherEventHandler`, or exposed types

#### Scenario: Verify UniFFI build integration
- **WHEN** cargo build executes
- **THEN** uniffi build dependency version 0.28 is used
- **AND** UDL schema validation passes
- **AND** build script generates updated bindings if needed

### Requirement: Dependency Minimization
The system SHALL minimize external dependencies by using standard library equivalents where available, reducing build time and binary size.

#### Scenario: Remove once_cell dependency
- **WHEN** the system uses lazy initialization
- **THEN** it uses `std::sync::OnceLock` or `std::cell::OnceCell` from the standard library
- **AND** the `once_cell` crate is NOT listed in Cargo.toml dependencies
- **AND** all previous `OnceCell` usage is migrated to stdlib equivalents

#### Scenario: Remove async-trait dependency
- **WHEN** the system defines or implements async traits
- **THEN** it uses native `async fn` in trait definitions
- **AND** the `async-trait` crate is NOT listed in Cargo.toml dependencies
- **AND** all previous `#[async_trait]` attributes are removed

#### Scenario: Measure dependency reduction impact
- **WHEN** the build completes after dependency removal
- **THEN** build time SHALL be 5-10% faster than baseline (measured with cargo --timings)
- **AND** release binary size SHALL be 2-5% smaller than baseline
- **AND** dependency tree depth is reduced (verified via `cargo tree`)

