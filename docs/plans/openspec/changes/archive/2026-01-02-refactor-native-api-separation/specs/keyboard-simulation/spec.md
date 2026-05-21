# keyboard-simulation Spec Delta (refactor-native-api-separation)

## ADDED Requirements

### Requirement: Native Keyboard Simulation
The system SHALL provide keyboard event simulation using native platform APIs (CGEvent on macOS) for simulating shortcuts and typewriter effects.

#### Scenario: Simulate Cut (Cmd+X)
- **WHEN** Swift calls `KeyboardSimulator.simulateCut()`
- **THEN** creates CGEvent for Meta (Command) key press
- **AND** creates CGEvent for 'x' key press
- **AND** posts both events to HID event tap
- **AND** creates CGEvent for Meta key release
- **AND** completes within < 10ms

#### Scenario: Simulate Copy (Cmd+C)
- **WHEN** Swift calls `KeyboardSimulator.simulateCopy()`
- **THEN** creates CGEvent sequence for Command+C
- **AND** posts events to system event stream
- **AND** triggers clipboard update in active application

#### Scenario: Simulate Paste (Cmd+V)
- **WHEN** Swift calls `KeyboardSimulator.simulatePaste()`
- **THEN** creates CGEvent sequence for Command+V
- **AND** pastes clipboard content into active application
- **AND** preserves focus on active application

### Requirement: Typewriter Effect
The system SHALL support character-by-character typing with configurable speed for AI response output.

#### Scenario: Type text with default speed
- **WHEN** Swift calls `typeText(_:)` with a string
- **THEN** iterates through each character
- **AND** creates CGEvent with Unicode string for each character
- **AND** posts KeyDown and KeyUp events
- **AND** waits 20ms between characters (default 50 chars/sec)
- **AND** supports all Unicode characters including emoji

#### Scenario: Type text with custom speed
- **WHEN** Swift calls `typeText(_:speed: 100)` (100 chars/sec)
- **THEN** calculates delay as 1000ms / 100 = 10ms
- **AND** waits 10ms between characters
- **AND** completes faster than default speed

#### Scenario: Cancel typewriter effect
- **WHEN** user presses Esc during typewriter animation
- **THEN** Swift cancels the async task via CancellationToken
- **AND** typing stops immediately
- **AND** partially typed text remains in the application
- **AND** Halo overlay hides

### Requirement: Special Character Handling
The system SHALL correctly simulate special characters and control sequences.

#### Scenario: Type newline character
- **WHEN** typewriter encounters '\n' character
- **THEN** creates CGEvent for Return key
- **AND** moves cursor to new line in target application

#### Scenario: Type Tab character
- **WHEN** typewriter encounters '\t' character
- **THEN** creates CGEvent for Tab key
- **AND** inserts tab spacing in target application

#### Scenario: Type emoji
- **WHEN** typewriter encounters emoji (e.g., 😊)
- **THEN** creates CGEvent with Unicode string
- **AND** emoji appears correctly in target application
- **AND** no encoding issues

### Requirement: Keyboard Simulation Ownership
The Swift layer SHALL own all keyboard simulation logic, with Rust core having no knowledge of keyboard events.

#### Scenario: Swift handles output simulation
- **WHEN** AI response is received from Rust
- **THEN** Swift calls `KeyboardSimulator.typeText(response)`
- **AND** Rust is NOT involved in keyboard simulation
- **AND** Rust only provides the response string

#### Scenario: No UniFFI keyboard methods
- **WHEN** defining AlephCore interface
- **THEN** NO keyboard simulation methods in UniFFI
- **AND** Rust core does NOT depend on enigo crate
- **AND** reduces binary size by ~300KB

### Requirement: Application Compatibility
The system SHALL work reliably across common macOS applications with minimal compatibility issues.

#### Scenario: Compatible with major editors
- **WHEN** simulating keyboard events
- **THEN** works correctly in VSCode, Xcode, Sublime Text
- **AND** respects application-specific keyboard shortcuts
- **AND** no interference with editor features

#### Scenario: Compatible with browsers
- **WHEN** simulating keyboard events
- **THEN** works correctly in Safari, Chrome, Firefox
- **AND** respects browser keyboard shortcuts
- **AND** no interference with web page input

#### Scenario: Handle incompatible applications
- **WHEN** encountering an application that blocks CGEvent (e.g., password managers)
- **THEN** logs a warning message
- **AND** documents the incompatibility in `docs/COMPATIBILITY.md`
- **AND** provides guidance for users (optional compatibility mode in future)

### Requirement: Error Handling and Resilience
The system SHALL gracefully handle keyboard simulation failures without crashing.

#### Scenario: Handle CGEvent creation failure
- **WHEN** `CGEvent.keyboardEvent()` returns nil
- **THEN** logs an error message
- **AND** returns an error to caller
- **AND** does NOT crash the application
- **AND** caller can show user-friendly error UI

#### Scenario: Handle Accessibility permission denied
- **WHEN** Accessibility permission is not granted
- **THEN** CGEvent posting fails silently (macOS behavior)
- **AND** logs a warning message
- **AND** prompts user to check permissions

---

**Related specs**: `macos-client`, `uniffi-bridge`, `core-library`
**Relationship**: This is a NEW capability introduced in `macos-client` to replace Rust's enigo-based keyboard simulation.
