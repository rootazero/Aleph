## ADDED Requirements

### Requirement: Global Hotkey Detection
The system SHALL detect global hotkey presses (Cmd+~ on macOS) across all applications using the rdev crate.

#### Scenario: Detect Cmd+~ press
- **WHEN** user presses Command + Grave (`) keys simultaneously
- **THEN** the hotkey listener detects the key combination
- **AND** triggers a callback to AetherCore
- **AND** callback includes timestamp of the event

#### Scenario: Ignore other key combinations
- **WHEN** user presses keys other than Cmd+~
- **THEN** the hotkey listener ignores the event
- **AND** no callback is triggered
- **AND** no performance impact from filtering

#### Scenario: Start hotkey listener
- **WHEN** client calls `hotkey_listener.start_listening()`
- **THEN** a background thread spawns to monitor keyboard events
- **AND** the listener begins detecting hotkey patterns
- **AND** returns success if initialization succeeds

#### Scenario: Stop hotkey listener
- **WHEN** client calls `hotkey_listener.stop_listening()`
- **THEN** the background thread terminates gracefully
- **AND** no further keyboard events are monitored
- **AND** system resources are released

#### Scenario: Handle missing Accessibility permissions
- **WHEN** macOS Accessibility permissions are not granted
- **THEN** `start_listening()` returns an error
- **AND** error message indicates "Accessibility permissions required"
- **AND** client can prompt user to grant permissions

### Requirement: Cross-Platform Hotkey Support
The system SHALL use rdev for hotkey detection to ensure future cross-platform compatibility (Windows, Linux).

#### Scenario: Platform-agnostic key codes
- **WHEN** detecting hotkeys
- **THEN** uses rdev's platform-agnostic key code enum
- **AND** same Rust code works on macOS, Windows, Linux
- **AND** platform-specific modifiers (Cmd vs Ctrl) are handled internally

### Requirement: Thread-Safe Hotkey Callback
The system SHALL invoke callbacks from the hotkey listener thread to AetherCore safely without data races.

#### Scenario: Callback from rdev thread
- **WHEN** hotkey is detected on rdev's background thread
- **THEN** the listener invokes a callback closure
- **AND** callback has access to shared AetherCore state via Arc
- **AND** no data races occur (verified by Rust's Send/Sync)

### Requirement: Hotkey Listener Trait
The system SHALL define a `HotkeyListener` trait to enable swappable implementations and testing.

#### Scenario: Implement HotkeyListener trait
- **WHEN** creating a new hotkey listener implementation
- **THEN** it must implement `start_listening()` and `stop_listening()`
- **AND** methods return `Result<(), AetherError>`
- **AND** trait supports dependency injection

#### Scenario: Mock hotkey listener in tests
- **WHEN** testing AetherCore logic
- **THEN** a mock HotkeyListener can be injected
- **AND** mock can simulate hotkey events programmatically
- **AND** core logic is tested without rdev dependency
