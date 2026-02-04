## ADDED Requirements

### Requirement: Library Initialization
The system SHALL provide a library-based Rust core that can be compiled as a dynamic library (cdylib) and static library (staticlib) for consumption by native UI clients.

#### Scenario: Build dynamic library for macOS
- **WHEN** developer runs `cargo build --release --target aarch64-apple-darwin`
- **THEN** the build produces `libalephcore.dylib` in target/release/
- **AND** the library exports C-compatible symbols for FFI

#### Scenario: Build static library
- **WHEN** developer runs `cargo build --release --lib`
- **THEN** the build produces `libalephcore.a` for static linking
- **AND** the library contains all required dependencies

### Requirement: Core Lifecycle Management
The system SHALL provide an `AlephCore` struct that manages the lifecycle of all core components (hotkey listener, clipboard manager, event handler).

#### Scenario: Initialize core with event handler
- **WHEN** client calls `AlephCore::new(event_handler)`
- **THEN** the core initializes successfully
- **AND** stores a reference to the event handler for callbacks
- **AND** initializes tokio async runtime

#### Scenario: Start listening for system events
- **WHEN** client calls `core.start_listening()`
- **THEN** the hotkey listener spawns a background thread
- **AND** begins monitoring for global hotkey events
- **AND** returns success if no errors occur

#### Scenario: Stop listening
- **WHEN** client calls `core.stop_listening()`
- **THEN** the hotkey listener thread terminates gracefully
- **AND** releases all system resources
- **AND** no further hotkey events are detected

#### Scenario: Handle initialization error
- **WHEN** core initialization fails (e.g., hotkey listener cannot start)
- **THEN** the constructor returns an AlephError
- **AND** provides a descriptive error message

### Requirement: Thread-Safe Callback Invocation
The system SHALL support thread-safe callback invocation from Rust to client code via the event handler trait.

#### Scenario: Invoke callback from background thread
- **WHEN** hotkey is detected on rdev background thread
- **THEN** the core invokes `event_handler.on_hotkey_detected(content)`
- **AND** the callback executes safely across thread boundary
- **AND** uses Arc<dyn EventHandler> for shared ownership

### Requirement: Error Handling
The system SHALL define a custom error type (`AlephError`) that represents all possible error conditions in the core library.

#### Scenario: Hotkey listener error
- **WHEN** hotkey listener fails to start (e.g., permissions denied)
- **THEN** returns `AlephError::HotkeyError` with descriptive message
- **AND** error can be propagated through Result<T, E>

#### Scenario: Clipboard operation error
- **WHEN** clipboard read fails (e.g., unsupported content type)
- **THEN** returns `AlephError::ClipboardError` with details
- **AND** error message indicates the failure reason

#### Scenario: No panics in library code
- **WHEN** any error occurs during operation
- **THEN** the library returns a Result type
- **AND** never panics or crashes
- **AND** client can handle errors gracefully

### Requirement: Async Runtime Support
The system SHALL initialize a tokio async runtime to support non-blocking operations for future async features (AI API calls).

#### Scenario: Runtime initialization
- **WHEN** AlephCore is initialized
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
- **AND** pass the new implementation to AlephCore
- **AND** no changes to core logic are required

#### Scenario: Mock components in tests
- **WHEN** writing unit tests for AlephCore
- **THEN** developer creates mock implementations of traits
- **AND** injects mocks into AlephCore constructor
- **AND** tests core logic in isolation
