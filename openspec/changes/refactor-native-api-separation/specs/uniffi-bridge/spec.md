# uniffi-bridge Spec Delta (refactor-native-api-separation)

## MODIFIED Requirements

### Requirement: Minimal FFI Interface
The UniFFI bridge SHALL expose ONLY high-level business operations, not low-level system API wrappers.

**Previous**: UniFFI exposed hotkey control, clipboard access, and keyboard simulation methods.
**Changed to**: UniFFI exposes only AI processing, memory management, and configuration methods.

#### Scenario: Removed interface methods (MODIFIED)
- **WHEN** examining `aether.udl` interface definition
- **THEN** the following methods are REMOVED:
  - `void start_listening()`
  - `void stop_listening()`
  - `boolean is_listening()`
  - `string get_clipboard_text()`
  - `boolean has_clipboard_image()`
  - `ImageData? read_clipboard_image()`
  - `void write_clipboard_image(ImageData image)`
- **AND** `ImageData` dictionary is REMOVED
- **AND** `ImageFormat` enum is REMOVED

#### Scenario: Simplified callback interface (MODIFIED)
- **WHEN** examining `AetherEventHandler` callback interface
- **THEN** the following callback is REMOVED:
  - `void on_hotkey_detected(string clipboard_content)`
- **AND** remaining callbacks:
  - `void on_state_changed(ProcessingState state)`
  - `void on_error(string message, string? suggestion)`
  - `void on_response_chunk(string text)`
  - `void on_ai_processing_started(string provider, string color)`

### Requirement: High-Level Processing API
The UniFFI bridge SHALL provide a single high-level method for processing user input.

#### Scenario: Added process_input method (ADDED)
- **WHEN** Swift calls `core.process_input(user_input, context)`
- **THEN** Rust receives:
  - `user_input`: Pre-processed string from clipboard
  - `context`: Captured application context (bundle ID, window title)
- **AND** Rust performs complete AI pipeline
- **AND** returns AI response as `String`
- **AND** throws `AetherException` on errors

## REMOVED Requirements

### Requirement: System API Exposure (REMOVED)
**Reason**: System APIs are no longer accessible from Rust. Swift handles all system interactions.

**Previous scenarios removed**:
- Expose hotkey listener control
- Expose clipboard read/write operations
- Expose image clipboard operations
- Expose keyboard simulation methods

### Requirement: Platform Callback Patterns (REMOVED)
**Reason**: Hotkey detection no longer triggers callbacks from Rust to Swift. Swift directly handles hotkey events.

**Previous scenarios removed**:
- Hotkey callback from Rust thread
- Clipboard content callback on hotkey

## ADDED Requirements

### Requirement: Streamlined Interface
The UniFFI bridge SHALL minimize FFI calls by batching operations into high-level methods.

#### Scenario: Single call for AI processing
- **WHEN** user triggers Aether
- **THEN** Swift makes ONE FFI call: `process_input()`
- **AND** Rust performs all business logic internally
- **AND** Swift receives ONE response
- **AND** reduces FFI overhead by ~70%

#### Scenario: Reduced type complexity
- **WHEN** examining UniFFI type definitions
- **THEN** removes binary data types (ImageData with byte arrays)
- **AND** focuses on simple types (String, Int, Bool)
- **AND** improves code generation speed
- **AND** reduces generated Swift bindings size

### Requirement: Error Propagation Clarity
The UniFFI bridge SHALL clearly propagate errors from Rust to Swift with actionable information.

#### Scenario: Typed error responses
- **WHEN** Rust encounters an error during processing
- **THEN** throws `AetherException` with error message
- **AND** Swift catches exception
- **AND** displays user-friendly error message
- **AND** suggests recovery actions (e.g., "Check network connection")

#### Scenario: Network error handling
- **WHEN** AI provider HTTP call fails
- **THEN** Rust returns error with type `ErrorType::Network`
- **AND** Swift shows "Network error" UI
- **AND** offers "Retry" button

### Requirement: Callback-Based State Updates
The UniFFI bridge SHALL use callbacks for asynchronous state updates during long-running operations.

#### Scenario: Progress callbacks during AI processing
- **WHEN** Rust processes input (memory retrieval, AI call, storage)
- **THEN** calls back to Swift with state updates:
  - `on_state_changed(RetrievingMemory)`
  - `on_ai_processing_started("openai", "#10a37f")`
  - `on_response_chunk("partial text...")` (if streaming)
- **AND** Swift updates Halo UI accordingly

#### Scenario: Non-blocking AI calls
- **WHEN** Swift calls `process_input()`
- **THEN** Rust uses async tokio runtime
- **AND** does NOT block Swift main thread
- **AND** returns response when complete
- **AND** Swift can show loading indicators via callbacks

---

**Related specs**: `core-library`, `macos-client`
**Relationship**: This change simplifies `uniffi-bridge` to expose only business logic from `core-library`, while `macos-client` handles all system interactions.
