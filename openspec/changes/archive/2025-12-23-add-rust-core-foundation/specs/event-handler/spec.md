## ADDED Requirements

### Requirement: Event Handler Trait Definition
The system SHALL define an `AlephEventHandler` trait that specifies callback methods for Rust-to-client communication.

#### Scenario: Define callback trait
- **WHEN** defining the event handler interface
- **THEN** trait includes methods: on_state_changed, on_hotkey_detected, on_error
- **AND** all methods accept parameters (state, message, etc.)
- **AND** trait is object-safe (can be used as `dyn AlephEventHandler`)

#### Scenario: Implement event handler in client
- **WHEN** Swift client implements AlephEventHandler protocol
- **THEN** all callback methods must be implemented
- **AND** Swift can provide custom logic for each callback
- **AND** implementation is type-checked at compile time

### Requirement: State Change Notifications
The system SHALL notify clients of processing state changes via the `on_state_changed` callback.

#### Scenario: Notify state change
- **WHEN** Rust core changes state from Idle to Listening
- **THEN** `event_handler.on_state_changed(ProcessingState::Listening)` is called
- **AND** client receives the new state
- **AND** client can update UI accordingly

#### Scenario: Processing state enum
- **WHEN** defining processing states
- **THEN** enum includes: Idle, Listening, Processing, Success, Error
- **AND** states represent the current operation phase
- **AND** state transitions are well-defined

### Requirement: Hotkey Detection Notifications
The system SHALL notify clients when a hotkey is detected via the `on_hotkey_detected` callback.

#### Scenario: Notify hotkey detected
- **WHEN** user presses Cmd+~ and clipboard has text "hello"
- **THEN** `event_handler.on_hotkey_detected("hello")` is called
- **AND** clipboard content is passed as parameter
- **AND** client receives the text immediately

#### Scenario: Include clipboard content
- **WHEN** hotkey is detected
- **THEN** callback includes current clipboard text content
- **AND** content is read synchronously before callback
- **AND** empty string is passed if clipboard is empty

### Requirement: Error Notifications
The system SHALL notify clients of errors via the `on_error` callback.

#### Scenario: Notify error
- **WHEN** clipboard read fails with "Clipboard access denied"
- **THEN** `event_handler.on_error("Clipboard access denied")` is called
- **AND** client receives descriptive error message
- **AND** client can display error to user

#### Scenario: Error message format
- **WHEN** error occurs
- **THEN** error message is human-readable
- **AND** message indicates the failure reason
- **AND** message includes context (e.g., "Hotkey listener failed to start")

### Requirement: Thread-Safe Callback Invocation
The system SHALL ensure callbacks can be invoked safely from any thread.

#### Scenario: Callback from background thread
- **WHEN** hotkey is detected on rdev's background thread
- **THEN** callback is invoked from that thread
- **AND** Swift client handles thread safety (DispatchQueue.main.async)
- **AND** no data races occur

#### Scenario: Arc-based handler storage
- **WHEN** AlephCore stores event handler
- **THEN** handler is wrapped in Arc<dyn AlephEventHandler>
- **AND** Arc is Send + Sync
- **AND** handler can be safely shared across threads

### Requirement: Mock Event Handler for Testing
The system SHALL provide a mock implementation of AlephEventHandler for testing purposes.

#### Scenario: Mock handler records calls
- **WHEN** creating a mock event handler
- **THEN** it stores all callback invocations in a vector
- **AND** tests can assert expected callbacks occurred
- **AND** tests can verify callback parameters

#### Scenario: Test callback invocation
- **WHEN** testing AlephCore logic
- **THEN** mock handler is injected into core
- **AND** tests trigger actions (e.g., hotkey detection)
- **AND** tests verify correct callbacks were made
