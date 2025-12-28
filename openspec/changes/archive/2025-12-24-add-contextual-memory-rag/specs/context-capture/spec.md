# context-capture Specification

## Purpose

The context-capture capability enables Aether to capture the active application and window context (app bundle ID + window title) at the moment of user interaction, providing context anchors for memory storage and retrieval.

## ADDED Requirements

### Requirement: Active Application Detection
The system SHALL capture the bundle ID of the frontmost application on macOS.

#### Scenario: Capture active app bundle ID
- **WHEN** user presses hotkey
- **THEN** Swift code calls `NSWorkspace.shared.frontmostApplication?.bundleIdentifier`
- **AND** returns bundle ID string (e.g., "com.apple.Notes")
- **AND** completes within 5ms

#### Scenario: Handle system app
- **WHEN** Finder is active
- **THEN** returns "com.apple.finder"

#### Scenario: Handle no active app
- **WHEN** no application is frontmost (edge case)
- **THEN** returns "unknown" as fallback

---

### Requirement: Active Window Title Detection
The system SHALL capture the title of the frontmost window using macOS Accessibility API.

#### Scenario: Capture window title with permission
- **GIVEN** Accessibility permission granted
- **WHEN** user presses hotkey in Notes.app with "Project Plan.txt" open
- **THEN** Swift code uses `AXUIElementCopyAttributeValue` with `kAXTitleAttribute`
- **AND** returns "Project Plan.txt"
- **AND** completes within 10ms

#### Scenario: Handle permission denied
- **WHEN** Accessibility permission not granted
- **THEN** window title capture fails gracefully
- **AND** returns empty string ""
- **AND** logs warning: "Accessibility permission required for window title"

#### Scenario: Handle window with no title
- **WHEN** active window has no title attribute (e.g., menu bar app)
- **THEN** returns empty string ""
- **AND** does not throw error

---

### Requirement: Context Anchor Creation
The system SHALL package captured context as a structured data type and send to Rust core.

#### Scenario: Create context anchor
- **GIVEN** bundle_id = "com.apple.Notes"
- **AND** window_title = "Project Plan.txt"
- **WHEN** context is captured
- **THEN** creates `CapturedContext` struct with both fields
- **AND** sends to Rust via `core.setCurrentContext(context)`
- **AND** Rust stores in `Arc<Mutex<Option<CapturedContext>>>`

---

## Cross-References

### Dependencies
- **uniffi-bridge**: UniFFI dictionary for CapturedContext
- **macOS Accessibility API**: System permission required

### Consumers
- **memory-storage**: Uses context anchors to tag memories
- **memory-augmentation**: Uses context to filter retrieval

---

## Acceptance Criteria

- [ ] Can capture bundle ID of active app
- [ ] Can capture window title with permission
- [ ] Handles permission denial gracefully
- [ ] Context passed to Rust correctly via UniFFI
- [ ] Completes within 15ms total (not blocking hotkey)
