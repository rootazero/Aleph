## ADDED Requirements

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

## MODIFIED Requirements

### Requirement: Hotkey Listener Initialization (Updated)
The core library SHALL initialize the hotkey listener only when Input Monitoring permission is confirmed granted, with robust error handling for permission issues.

#### Scenario: Initialize listener with permission granted (Updated)
- **WHEN** `start_listening()` is called with Input Monitoring permission granted
- **THEN** the system performs permission pre-check (new step)
- **AND** wraps `rdev::listen()` in `catch_unwind()` (new protection)
- **AND** starts the listener thread
- **AND** returns `Ok(())` on success or `Err(HotkeyError)` on failure

#### Scenario: Initialize listener without permission (New)
- **WHEN** `start_listening()` is called without Input Monitoring permission
- **THEN** the system skips calling `rdev::listen()` entirely (new behavior)
- **AND** returns `Err(AetherError::PermissionDenied)` immediately
- **AND** does NOT create any threads or resources
- **AND** notifies Swift layer via `on_error()` callback

### Requirement: Error Handling and Logging (Updated)
The core library SHALL provide comprehensive error handling and logging for all permission-related failures, enabling effective troubleshooting.

#### Scenario: Log permission check failures (New)
- **WHEN** permission pre-check fails
- **THEN** the system logs at WARN level:
  - "Input Monitoring permission not granted. Cannot start hotkey listener."
- **AND** logs the current permission status for debugging
- **AND** returns structured error to Swift layer

#### Scenario: Log rdev panic details (New)
- **WHEN** `rdev::listen()` panics
- **THEN** the system logs at ERROR level:
  - "❌ rdev listener panicked: [panic message]"
  - "This usually means Input Monitoring permission is not granted."
  - "Please grant permission in: System Settings > Privacy & Security > Input Monitoring"
- **AND** extracts panic payload (String or &str)
- **AND** logs full error context for debugging

#### Scenario: Log listener lifecycle events (Enhanced)
- **WHEN** listener starts successfully
- **THEN** the system logs at INFO level: "Hotkey listener started successfully"
- **WHEN** listener stops gracefully
- **THEN** the system logs at INFO level: "rdev listener stopped gracefully"
- **WHEN** listener fails
- **THEN** the system logs at ERROR level with error details

## UniFFI Interface Changes

### New Methods in AetherCore

```rust
// New method: Set Input Monitoring permission status from Swift
pub fn set_input_monitoring_permission(&self, granted: bool) {
    self.has_input_monitoring_permission = granted;
    log::info!("Input Monitoring permission status updated: {}", granted);
}
```

### New Error Types

```rust
// Add PermissionDenied variant to AetherError
pub enum AetherError {
    PermissionDenied(String),
    HotkeyError(HotkeyError),
    // ... other variants
}

// Add PermissionDenied variant to HotkeyError
pub enum HotkeyError {
    ListenFailed(String),
    PermissionDenied(String),  // New variant
    // ... other variants
}
```

### Updated UniFFI Definition (aether.udl)

```idl
interface AetherCore {
    // ... existing methods

    // New method: Update permission status from Swift
    void set_input_monitoring_permission(boolean granted);

    // Updated: start_listening() may return PermissionDenied error
    [Throws=AetherError]
    void start_listening();
};

// Updated error enum
[Error]
enum AetherError {
    "PermissionDenied",  // New variant
    "HotkeyError",
    // ... other variants
};
```

## Testing Requirements

### Unit Tests (Rust)

#### Test: Panic protection catches rdev panic
- **WHEN** `start_rdev_listener()` is called in a test environment that simulates panic
- **THEN** `catch_unwind()` successfully catches the panic
- **AND** function returns `Err(HotkeyError::PermissionDenied)`
- **AND** no process termination occurs

#### Test: Permission pre-check blocks listener start
- **WHEN** `start_listening()` is called with `has_input_monitoring_permission = false`
- **THEN** function returns `Err(AetherError::PermissionDenied)` immediately
- **AND** `rdev::listen()` is never called
- **AND** no threads are created

#### Test: Permission flag updates correctly
- **WHEN** `set_input_monitoring_permission(true)` is called
- **THEN** internal flag is updated to true
- **AND** subsequent `start_listening()` calls proceed (assuming no panic)

### Integration Tests (Swift ↔ Rust)

#### Test: Swift receives permission error via UniFFI
- **WHEN** Swift calls `core.start_listening()` without permission
- **THEN** Swift receives `AetherError.permissionDenied` error
- **AND** error message includes actionable guidance

#### Test: Swift receives error callback
- **WHEN** Rust core encounters permission error
- **THEN** Swift's `AetherEventHandler.on_error()` is called
- **AND** error message is displayed in UI or logged

#### Test: Permission update flow
- **WHEN** Swift detects permission grant
- **THEN** Swift calls `core.set_input_monitoring_permission(true)`
- **AND** Swift retries `core.start_listening()`
- **AND** listener starts successfully (if no panic)

### Manual Testing

- [ ] Start app without Input Monitoring permission → Core returns error, no crash
- [ ] Grant permission mid-session → Swift updates Rust flag, listener starts
- [ ] Simulate rdev panic → catch_unwind prevents crash, error logged
- [ ] Revoke permission while running → Listener fails gracefully on next restart attempt
