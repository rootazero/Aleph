# hotkey-detection Spec Delta (refactor-native-api-separation)

## MODIFIED Requirements

### Requirement: Global Hotkey Detection
The system SHALL detect global hotkey presses (single ` key on macOS) using native platform APIs (CGEventTap on macOS).

**Previous**: Used `rdev` crate in Rust layer for cross-platform hotkey detection.
**Changed to**: Use native `CGEventTap` API in Swift layer for better performance and reliability.

#### Scenario: Detect ` key press (MODIFIED)
- **WHEN** user presses Grave (`) key
- **THEN** the `GlobalHotkeyMonitor` (Swift) detects the key press via CGEventTap
- **AND** returns `nil` to swallow the event (prevents ` character from typing)
- **AND** triggers a Swift callback to EventHandler
- **AND** callback includes timestamp of the event

#### Scenario: Ignore other key combinations (MODIFIED)
- **WHEN** user presses keys other than ` (grave)
- **THEN** the CGEventTap callback returns the event unchanged
- **AND** the event propagates to the application
- **AND** no performance impact from filtering

#### Scenario: Start hotkey monitor (MODIFIED)
- **WHEN** Swift calls `GlobalHotkeyMonitor.startMonitoring()`
- **THEN** a CGEventTap is created for KeyDown events
- **AND** the tap is added to the main run loop
- **AND** the tap is enabled
- **AND** returns `true` if successful, `false` otherwise

#### Scenario: Stop hotkey monitor (MODIFIED)
- **WHEN** Swift calls `GlobalHotkeyMonitor.stopMonitoring()`
- **THEN** the event tap is disabled
- **AND** the run loop source is removed
- **AND** system resources are released

#### Scenario: Handle missing Accessibility permissions (MODIFIED)
- **WHEN** macOS Accessibility permissions are not granted
- **THEN** `CGEvent.tapCreate()` returns `nil`
- **AND** `startMonitoring()` returns `false`
- **AND** logs error message "Accessibility permission not granted"
- **AND** Swift can show permission gate UI

## REMOVED Requirements

### Requirement: Cross-Platform Hotkey Support (REMOVED)
**Reason**: Hotkey detection is now platform-specific (Swift for macOS, C# for Windows, Rust+GTK for Linux). This improves performance and allows using platform-native best practices.

**Previous scenarios removed**:
- Platform-agnostic key codes (no longer using rdev)

### Requirement: Thread-Safe Hotkey Callback (REMOVED)
**Reason**: Hotkey detection now happens in Swift layer. No cross-FFI callback needed for hotkey events. Swift handles callbacks on main thread via DispatchQueue.

**Previous scenarios removed**:
- Callback from rdev thread
- Arc-based shared state

### Requirement: Hotkey Listener Trait (REMOVED)
**Reason**: Hotkey listener is no longer in Rust core. Swift implements `GlobalHotkeyMonitor` as a concrete class.

**Previous scenarios removed**:
- Implement HotkeyListener trait
- Mock hotkey listener in tests

## ADDED Requirements

### Requirement: Native macOS Hotkey Implementation
The system SHALL use CGEventTap API for global hotkey detection on macOS, providing zero-overhead event interception.

#### Scenario: Create event tap with correct parameters
- **WHEN** initializing GlobalHotkeyMonitor
- **THEN** creates a CGEventTap with:
  - **tap**: `.cgSessionEventTap` (current user session)
  - **place**: `.headInsertEventTap` (intercept before app)
  - **options**: `.defaultTap` (can modify/delete events)
  - **eventsOfInterest**: KeyDown and KeyUp events
- **AND** provides callback closure with self reference

#### Scenario: Event tap runs on main run loop
- **WHEN** event tap is started
- **THEN** the tap's run loop source is added to `CFRunLoopGetMain()`
- **AND** uses `.commonModes` for run loop mode
- **AND** ensures tap events are processed even during modal dialogs

#### Scenario: Prevent ` character from typing
- **WHEN** ` key is detected by event tap
- **THEN** callback returns `nil` (swallow event)
- **AND** the ` character does NOT appear in the active application
- **AND** Aether processing begins immediately

#### Scenario: Preserve focus on active application
- **WHEN** hotkey is detected
- **THEN** the active application remains focused
- **AND** Halo overlay appears but does NOT steal keyboard focus
- **AND** user can continue typing in the original app if needed

### Requirement: Hotkey Detection Ownership
The Swift layer SHALL own all hotkey detection logic, with Rust core having no knowledge of hotkey events.

#### Scenario: Swift handles hotkey lifecycle
- **WHEN** app launches
- **THEN** Swift creates and starts GlobalHotkeyMonitor
- **AND** Rust core is NOT involved in hotkey listening
- **AND** Rust core only receives processed input via `process_input()`

#### Scenario: No FFI calls for hotkey events
- **WHEN** hotkey is detected
- **THEN** Swift directly handles the event (read clipboard, capture context)
- **AND** NO callback to Rust until `process_input()` is called
- **AND** reduces FFI overhead by ~50%

---

**Related specs**: `macos-client`, `uniffi-bridge`, `core-library`
**Relationship**: This change enables `macos-client` to implement native hotkey detection, and removes hotkey-related interfaces from `uniffi-bridge`.
