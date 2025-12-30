# core-library Specification

## Purpose
TBD - created by archiving change add-rust-core-foundation. Update Purpose after archive.
## Requirements
### Requirement: Library Initialization
The system SHALL provide a library-based Rust core that can be compiled as a dynamic library (cdylib) and static library (staticlib) for consumption by native UI clients.

#### Scenario: Build dynamic library for macOS
- **WHEN** developer runs `cargo build --release --target aarch64-apple-darwin`
- **THEN** the build produces `libaethecore.dylib` in target/release/
- **AND** the library exports C-compatible symbols for FFI

#### Scenario: Build static library
- **WHEN** developer runs `cargo build --release --lib`
- **THEN** the build produces `libaethecore.a` for static linking
- **AND** the library contains all required dependencies

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
The system SHALL initialize a tokio async runtime to support non-blocking operations for future async features (AI API calls).

#### Scenario: Runtime initialization
- **WHEN** AetherCore is initialized
- **THEN** tokio runtime is created successfully
- **AND** supports multi-threaded execution
- **AND** allows spawning async tasks

#### Scenario: Blocking operation handling
- **WHEN** a synchronous operation (e.g., clipboard read) is called
- **THEN** the operation runs on tokio blocking pool
- **AND** does not block the async runtime
- **AND** returns result via async/await

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

