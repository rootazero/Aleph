# permission-gating Specification

## Purpose
TBD - created by archiving change redesign-permission-authorization. Update Purpose after archive.
## Requirements
### Requirement: Mandatory Permission Gate at Launch
The system SHALL enforce a blocking permission setup flow on app launch that prevents access to any features until both Accessibility and Input Monitoring permissions are granted, **without automatically restarting the application**.

#### Scenario: First launch with no permissions
- **WHEN** app launches and neither Accessibility nor Input Monitoring is granted
- **THEN** the system displays PermissionGateView as the only visible window
- **AND** settings window is not accessible via menu bar
- **AND** core features (hotkeys, clipboard) remain disabled
- **AND** the app does NOT attempt to restart automatically

#### Scenario: Partial permissions granted
- **WHEN** app launches with only Accessibility permission granted
- **THEN** the system displays PermissionGateView showing Input Monitoring as pending
- **AND** Accessibility permission is shown as granted/completed
- **AND** settings window remains blocked

#### Scenario: All permissions granted
- **WHEN** app launches with both Accessibility and Input Monitoring granted
- **THEN** the system skips PermissionGateView entirely
- **AND** proceeds directly to normal operation (menu bar + background service)
- **AND** settings window is accessible via menu bar

#### Scenario: User cannot dismiss permission gate
- **WHEN** permission gate is displayed with missing permissions
- **THEN** the system provides no "Skip" or "Cancel" button
- **AND** clicking outside the window does not dismiss it
- **AND** Escape key does not close the window
- **AND** only way forward is granting permissions

### Requirement: Real-Time Permission Status Monitoring (Passive)
The system SHALL monitor permission status in real-time and automatically progress through the permission gate when permissions are granted, **without triggering application restart**.

#### Scenario: Accessibility permission granted in System Settings (No Auto-Restart)
- **WHEN** user grants Accessibility permission in System Settings
- **THEN** the system detects the change within 1 second
- **AND** automatically updates UI to show Accessibility as granted
- **AND** automatically proceeds to Input Monitoring step
- **AND** the app does NOT restart automatically
- **AND** the app does NOT call `exit()`, `NSApp.terminate()`, or any restart mechanism

#### Scenario: Input Monitoring permission granted in System Settings (User-Controlled Restart)
- **WHEN** user grants Input Monitoring permission in System Settings
- **THEN** the system detects the change within 1 second
- **AND** automatically updates UI to show Input Monitoring as granted
- **AND** displays "进入 Aether" button for user to manually restart
- **AND** the app does NOT restart automatically until user clicks the button
- **AND** macOS system may show its own "app needs restart" prompt (do not interfere)

#### Scenario: Permission monitoring stops after gate dismissal
- **WHEN** both permissions are granted and gate is dismissed
- **THEN** the system stops polling permission status
- **AND** releases monitoring timer resources

### Requirement: Sequential Permission Setup Flow (Waterfall Design)
The system SHALL guide users through a two-step sequential permission setup process, ensuring Step 2 is only enabled after Step 1 completion.

#### Scenario: Step 1 - Accessibility Permission
- **WHEN** permission gate is displayed
- **THEN** the system shows Accessibility as the active step
- **AND** displays "Open System Settings" button for Accessibility (always enabled)
- **AND** Input Monitoring step is shown as inactive/disabled

#### Scenario: Step 2 - Input Monitoring Permission (Dependent on Step 1)
- **WHEN** Accessibility permission is granted
- **THEN** the system automatically activates Input Monitoring step
- **AND** displays "Open System Settings" button for Input Monitoring (now enabled)
- **AND** shows Accessibility step with checkmark/completed state

#### Scenario: Deep link to System Settings
- **WHEN** user clicks "Open System Settings" for any permission
- **THEN** the system opens macOS System Settings
- **AND** navigates directly to the relevant privacy pane:
  - Accessibility: `x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility`
  - Input Monitoring: `x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent`
- **AND** keeps permission gate window visible on top

### Requirement: Visual Permission Status Indicators
The system SHALL provide clear visual feedback for permission status throughout the gating flow.

#### Scenario: Pending permission state
- **WHEN** permission is not yet granted
- **THEN** the system displays permission step with pending indicator (e.g., empty circle or orange dot)
- **AND** shows "Open System Settings" button enabled
- **AND** uses muted colors to indicate inactive state

#### Scenario: Granted permission state
- **WHEN** permission is granted
- **THEN** the system displays checkmark icon or success indicator (green)
- **AND** changes color to green/success color
- **AND** disables "Open System Settings" button for that permission

#### Scenario: Progress indicator
- **WHEN** permission gate is active
- **THEN** the system shows "Step 1 of 2" or "Step 2 of 2" indicator
- **AND** updates automatically as permissions are granted

### Requirement: Input Monitoring Permission Detection (IOHIDManager)
The system SHALL detect macOS Input Monitoring permission status using IOHIDManager API for accurate detection at app launch and during runtime.

#### Scenario: Check Input Monitoring permission via IOHIDManager
- **WHEN** app queries Input Monitoring permission status
- **THEN** the system attempts to open keyboard device via `IOHIDManagerOpen()`
- **AND** returns true if `IOHIDManagerOpen()` succeeds (kIOReturnSuccess)
- **AND** returns false if `IOHIDManagerOpen()` fails (kIOReturnNotPermitted)
- **AND** closes the HID manager immediately after detection to release resources

#### Scenario: Input Monitoring permission required for hotkeys
- **WHEN** Input Monitoring permission is not granted
- **THEN** the system cannot detect global hotkeys
- **AND** blocks initialization of rdev hotkey listener in Rust core
- **AND** shows permission gate UI instead of initializing AetherCore

### Requirement: Settings Window Access Blocking
The system SHALL prevent access to settings window and all app features until permission requirements are met.

#### Scenario: Block settings menu item
- **WHEN** permission gate is active
- **THEN** the "Settings..." menu bar item is disabled
- **AND** clicking it has no effect
- **AND** keyboard shortcut (Cmd+,) is disabled

#### Scenario: Enable settings after permissions granted
- **WHEN** both permissions are granted and gate is dismissed
- **THEN** the "Settings..." menu bar item becomes enabled
- **AND** clicking opens settings window normally
- **AND** keyboard shortcut (Cmd+,) works

#### Scenario: Block core functionality
- **WHEN** permission gate is active
- **THEN** the system does not initialize AetherCore
- **AND** global hotkeys are not registered
- **AND** clipboard monitoring is disabled
- **AND** menu bar shows "waiting for permissions" state

### Requirement: Passive Permission Monitoring without Auto-Restart
The system SHALL implement a passive permission monitoring mechanism using timer-based polling that updates UI state without triggering application restart.

#### Scenario: PermissionManager initialization
- **WHEN** PermissionManager is initialized
- **THEN** the system captures initial permission status
- **AND** starts a Timer with 1-second interval
- **AND** polls `AXIsProcessTrusted()` and `IOHIDManagerOpen()` every second

#### Scenario: Permission status change detected
- **WHEN** timer callback detects a permission status change
- **THEN** the system updates `@Published` properties (`accessibilityGranted`, `inputMonitoringGranted`)
- **AND** logs the status change
- **AND** does NOT call `exit()`, `NSApp.terminate()`, or any restart method
- **AND** notifies SwiftUI views via Combine publishers

#### Scenario: PermissionManager cleanup
- **WHEN** PermissionManager is deinitialized or stopped
- **THEN** the system invalidates the polling timer
- **AND** releases all timer resources

### Requirement: User-Controlled Application Restart
The system SHALL provide a manual restart mechanism that users can trigger after granting all required permissions, rather than forcing automatic restart.

#### Scenario: Display restart button after all permissions granted
- **WHEN** both Accessibility and Input Monitoring permissions are granted
- **THEN** the system displays "进入 Aether" button in PermissionGateView
- **AND** the button is enabled and clickable
- **AND** no automatic restart occurs

#### Scenario: User clicks restart button
- **WHEN** user clicks "进入 Aether" button
- **THEN** the system launches a new instance of the app via NSWorkspace
- **AND** terminates the current instance after new instance starts
- **AND** the new instance proceeds with normal operation (permissions already granted)

#### Scenario: User ignores restart button
- **WHEN** user does NOT click "进入 Aether" button
- **THEN** the system keeps the button visible
- **AND** does not force restart
- **AND** allows macOS system to show its own restart prompt (if applicable)

### Requirement: IOHIDManager-Based Input Monitoring Detection
The system SHALL use IOHIDManager API to accurately detect Input Monitoring permission status, providing more reliable detection than IOHIDRequestAccess.

#### Scenario: Create HID Manager for keyboard devices
- **WHEN** checking Input Monitoring permission
- **THEN** the system creates an IOHIDManager instance
- **AND** sets device matching criteria for keyboard devices:
  - Usage Page: `kHIDPage_GenericDesktop`
  - Usage: `kHIDUsage_GD_Keyboard`

#### Scenario: Attempt to open HID Manager
- **WHEN** device matching is configured
- **THEN** the system calls `IOHIDManagerOpen()` with `kIOHIDOptionsTypeNone`
- **AND** if result is `kIOReturnSuccess`, permission is granted
- **AND** if result is `kIOReturnNotPermitted`, permission is denied
- **AND** closes the manager via `IOHIDManagerClose()` immediately after check

#### Scenario: Handle HID Manager creation failure
- **WHEN** IOHIDManager creation fails (returns NULL)
- **THEN** the system logs the failure
- **AND** assumes permission is NOT granted (fail-safe behavior)
- **AND** proceeds with permission gate flow

### Requirement: Simplified Initial Permission Check
The system SHALL simplify initial permission check at app launch by removing complex "initialization phase" logic and debounce mechanisms.

#### Scenario: Initial permission check on app launch
- **WHEN** PermissionGateView appears on screen
- **THEN** the system waits 0.3 seconds (fixed delay)
- **AND** checks current permission status via `PermissionChecker`
- **AND** updates UI to reflect current state (no debounce required)

#### Scenario: Auto-skip to Step 2 if Accessibility already granted
- **WHEN** initial check detects Accessibility is granted but Input Monitoring is not
- **THEN** the system automatically sets current step to Input Monitoring
- **AND** displays Step 1 with completed/checkmark state
- **AND** shows Step 2 as active

#### Scenario: Auto-dismiss gate if all permissions already granted
- **WHEN** initial check detects both permissions are granted
- **THEN** the system calls `onAllPermissionsGranted()` callback
- **AND** dismisses PermissionGateView
- **AND** proceeds to normal app operation

