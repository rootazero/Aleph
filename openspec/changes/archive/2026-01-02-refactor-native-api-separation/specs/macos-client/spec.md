# macos-client Spec Delta (refactor-native-api-separation)

## MODIFIED Requirements

### Requirement: System Integration Layer
The macOS client SHALL implement all system integration using native Swift and macOS APIs.

**Previous**: macOS client delegated system interactions to Rust core via UniFFI.
**Changed to**: macOS client directly uses native APIs (CGEventTap, NSPasteboard, CGEvent) and only calls Rust for business logic.

#### Scenario: Native hotkey monitoring (MODIFIED)
- **WHEN** app starts
- **THEN** Swift creates `GlobalHotkeyMonitor` instance
- **AND** calls `startMonitoring()` to register CGEventTap
- **AND** does NOT call Rust `core.start_listening()`
- **AND** handles hotkey events entirely in Swift

#### Scenario: Native clipboard operations (MODIFIED)
- **WHEN** hotkey is detected
- **THEN** Swift calls `ClipboardManager.getText()` using NSPasteboard
- **AND** does NOT call Rust `core.get_clipboard_text()`
- **AND** receives text directly without FFI overhead

## REMOVED Requirements

N/A - No requirements are being removed from macos-client. This change adds capabilities.

## ADDED Requirements

### Requirement: ClipboardManager Component
The macOS client SHALL provide a ClipboardManager class for all clipboard operations using NSPasteboard.

#### Scenario: Create ClipboardManager
- **WHEN** initializing clipboard operations
- **THEN** creates `ClipboardManager` instance
- **AND** uses `NSPasteboard.general` for clipboard access
- **AND** requires NO initialization parameters

#### Scenario: Read text with error handling
- **WHEN** calling `ClipboardManager.getText()`
- **THEN** returns `String?` (nil if no text or error)
- **AND** handles clipboard access errors gracefully
- **AND** logs errors for debugging

#### Scenario: Support image operations
- **WHEN** clipboard contains an image
- **THEN** `ClipboardManager.hasImage()` returns `true`
- **AND** `ClipboardManager.getImage()` returns `NSImage?`
- **AND** automatically handles PNG, JPEG, TIFF, GIF formats

#### Scenario: Detect clipboard changes
- **WHEN** another application modifies clipboard
- **THEN** `ClipboardManager.changeCount()` increments
- **AND** can be used to detect external clipboard changes
- **AND** enables smart clipboard polling

### Requirement: KeyboardSimulator Component
The macOS client SHALL provide a KeyboardSimulator class for keyboard event simulation using CGEvent.

#### Scenario: Create KeyboardSimulator
- **WHEN** initializing keyboard simulation
- **THEN** creates `KeyboardSimulator` instance
- **AND** uses CGEvent API for event creation
- **AND** requires Accessibility permission (checked at app level)

#### Scenario: Simulate shortcuts reliably
- **WHEN** calling `simulateCut()`, `simulateCopy()`, or `simulatePaste()`
- **THEN** creates proper CGEvent sequence (modifier press → key press → key release → modifier release)
- **AND** posts events to `.cghidEventTap`
- **AND** events are processed by active application
- **AND** completes within < 10ms

#### Scenario: Typewriter effect with async/await
- **WHEN** calling `typeText(_:speed:cancellationToken:)`
- **THEN** uses Swift async/await for character-by-character typing
- **AND** calculates delay based on speed parameter
- **AND** supports cancellation via CancellationToken
- **AND** updates UI progress via callbacks

#### Scenario: Handle special characters
- **WHEN** typewriter encounters '\n', '\t', or emoji
- **THEN** creates appropriate CGEvent
- **AND** newlines insert line breaks
- **AND** tabs insert tab spacing
- **AND** emoji display correctly

### Requirement: Streamlined Event Handler
The EventHandler SHALL coordinate between native system APIs and Rust core business logic.

#### Scenario: Handle hotkey event
- **WHEN** GlobalHotkeyMonitor detects ` key
- **THEN** EventHandler callback is invoked
- **AND** calls `ClipboardManager.getText()` to get user input
- **AND** calls `ContextCapture.getCurrentContext()` to get app info
- **AND** shows Halo overlay at cursor position
- **AND** calls `core.process_input(text, context)` via UniFFI
- **AND** waits for AI response asynchronously

#### Scenario: Handle AI response
- **WHEN** Rust returns AI response via `process_input()`
- **THEN** EventHandler receives response string
- **AND** updates Halo to show checkmark
- **AND** calls `KeyboardSimulator.typeText(response)`
- **AND** monitors for Esc key to cancel typing
- **AND** hides Halo when complete

#### Scenario: Handle errors gracefully
- **WHEN** Rust throws `AetherException`
- **THEN** EventHandler catches exception
- **AND** updates Halo to show error state
- **AND** displays user-friendly error message
- **AND** logs detailed error for debugging

### Requirement: AppDelegate Integration
The AppDelegate SHALL orchestrate initialization of all native components and Rust core.

#### Scenario: Initialize native components first
- **WHEN** app launches
- **THEN** AppDelegate checks permissions via PermissionChecker
- **AND** creates GlobalHotkeyMonitor if permissions granted
- **AND** creates ClipboardManager instance
- **AND** creates KeyboardSimulator instance
- **AND** creates ContextCapture instance
- **AND** THEN initializes AetherCore (Rust)
- **AND** AetherCore does NOT depend on system API access

#### Scenario: Handle permission errors
- **WHEN** Accessibility permission is not granted
- **THEN** shows PermissionGateView
- **AND** does NOT start GlobalHotkeyMonitor
- **AND** does NOT initialize AetherCore
- **AND** waits for user to grant permissions

#### Scenario: Clean shutdown
- **WHEN** app terminates
- **THEN** calls `GlobalHotkeyMonitor.stopMonitoring()`
- **AND** releases all native resources
- **AND** Rust core shuts down gracefully

### Requirement: Performance Optimization
The macOS client SHALL optimize performance by minimizing FFI calls and using native APIs directly.

#### Scenario: Measure hotkey response latency
- **WHEN** user presses ` key
- **THEN** time from key press to Halo appearance is < 100ms (p95)
- **AND** includes:
  - CGEventTap detection: ~5ms
  - Clipboard read: ~5ms
  - Context capture: ~5ms
  - Halo rendering: ~10ms
  - FFI call overhead: ~5ms
- **AND** total latency reduced by ~30% compared to old architecture

#### Scenario: Minimize memory allocation
- **WHEN** performing native operations
- **THEN** reuses ClipboardManager, KeyboardSimulator instances
- **AND** does NOT create new instances per operation
- **AND** reduces memory allocation overhead

### Requirement: Testing Support
The macOS client SHALL provide testable components with dependency injection.

#### Scenario: Unit test ClipboardManager
- **WHEN** writing unit tests
- **THEN** can test `ClipboardManager` independently
- **AND** can mock NSPasteboard (via protocol)
- **AND** tests clipboard read/write logic

#### Scenario: Unit test KeyboardSimulator
- **WHEN** writing unit tests
- **THEN** can test `KeyboardSimulator` independently
- **AND** can verify CGEvent creation (without posting)
- **AND** tests keyboard simulation logic

#### Scenario: Integration test E2E flow
- **WHEN** writing integration tests
- **THEN** can simulate hotkey → clipboard → AI → typewriter flow
- **AND** uses mock AetherCore (no real AI calls)
- **AND** verifies complete workflow

---

**Related specs**: `hotkey-detection`, `clipboard-management`, `keyboard-simulation`, `uniffi-bridge`, `core-library`
**Relationship**: This change makes `macos-client` the owner of all system interactions, implementing capabilities defined in `hotkey-detection`, `clipboard-management`, and `keyboard-simulation`, while consuming simplified `uniffi-bridge` to access `core-library` business logic.
