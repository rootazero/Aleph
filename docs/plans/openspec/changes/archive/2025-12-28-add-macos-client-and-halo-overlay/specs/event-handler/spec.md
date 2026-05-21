# Specification Delta: Event Handler

## MODIFIED Requirements

### Requirement: Swift Implementation of Event Handler

The macOS client **SHALL** provide a concrete Swift implementation of the AlephEventHandler protocol.

**Why:** Bridge Rust callbacks to Swift UI updates.

**Changes from previous version:**
- Previously only specified the trait definition
- Now requires actual Swift implementation with thread safety

**Acceptance criteria:**
- EventHandler class in Swift conforms to AlephEventHandler
- Implements all 3 callback methods:
  - onStateChanged(state: ProcessingState)
  - onHotkeyDetected(clipboardContent: String)
  - onError(message: String)
- All callbacks use DispatchQueue.main.async for UI updates
- Weak references to prevent retain cycles
- Error logging for debugging

#### Scenario: Swift implementation receives callbacks

**Given** EventHandler is initialized and passed to AlephCore
**When** Rust core detects hotkey
**Then** onHotkeyDetected() is called from background thread
**And** callback dispatches to main queue
**And** HaloWindow.show(at:) is called on main thread
**And** no thread safety issues occur

---

### Requirement: Callback to UI Component Integration

The event handler **SHALL** trigger appropriate UI updates in response to Rust callbacks.

**Why:** Visual feedback based on Rust core state changes.

**Changes from previous version:**
- Previously callbacks were abstract
- Now specifies concrete UI actions

**Acceptance criteria:**
- onStateChanged() updates menu bar icon state
- onHotkeyDetected() shows Halo at cursor location
- onError() displays error alert or logs to console
- UI updates are batched on main thread
- No UI blocking during callbacks

#### Scenario: State change triggers menu bar update

**Given** app is running with menu bar icon
**When** onStateChanged(.listening) is called
**Then** DispatchQueue.main.async executes
**And** menu bar icon changes to "listening" indicator
**And** icon update is visible within 50ms

---

### Requirement: Error Recovery in Swift

The event handler **SHALL** handle Rust callback errors gracefully without crashing the app.

**Why:** Rust errors shouldn't bring down the entire UI.

**Changes from previous version:**
- Previously only specified error notifications
- Now requires error recovery and fallback behavior

**Acceptance criteria:**
- Callbacks wrapped in do-catch blocks
- Errors logged to console with context
- User-friendly error messages in UI
- App remains functional after errors
- Retry logic for transient errors

#### Scenario: Rust callback throws exception

**Given** EventHandler is active
**When** Rust core throws AlephError.ClipboardError
**Then** Swift catches the exception
**And** logs error to console: "Clipboard error: ..."
**And** shows alert to user: "Failed to read clipboard"
**And** app continues running normally
