## ADDED Requirements

### Requirement: Mandatory Permission Gate at Launch
The system SHALL enforce a blocking permission setup flow on app launch that prevents access to any features until both Accessibility and Input Monitoring permissions are granted.

#### Scenario: First launch with no permissions
- **WHEN** app launches and neither Accessibility nor Input Monitoring is granted
- **THEN** the system displays PermissionGateView as the only visible window
- **AND** settings window is not accessible via menu bar
- **AND** core features (hotkeys, clipboard) remain disabled

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

### Requirement: Real-Time Permission Status Monitoring
The system SHALL monitor permission status in real-time and automatically progress through the permission gate when permissions are granted.

#### Scenario: Accessibility permission granted in System Settings
- **WHEN** user grants Accessibility permission in System Settings
- **THEN** the system detects the change within 1 second
- **AND** automatically updates UI to show Accessibility as granted
- **AND** automatically proceeds to Input Monitoring step

#### Scenario: Input Monitoring permission granted in System Settings
- **WHEN** user grants Input Monitoring permission in System Settings
- **THEN** the system detects the change within 1 second
- **AND** automatically updates UI to show Input Monitoring as granted
- **AND** automatically dismisses PermissionGateView
- **AND** initializes core features (hotkey listener)

#### Scenario: Permission monitoring stops after gate dismissal
- **WHEN** both permissions are granted and gate is dismissed
- **THEN** the system stops polling permission status
- **AND** releases monitoring timer resources

### Requirement: Sequential Permission Setup Flow
The system SHALL guide users through a two-step sequential permission setup process.

#### Scenario: Step 1 - Accessibility Permission
- **WHEN** permission gate is displayed
- **THEN** the system shows Accessibility as the active step
- **AND** displays "Open System Settings" button for Accessibility
- **AND** Input Monitoring step is shown as inactive/disabled

#### Scenario: Step 2 - Input Monitoring Permission
- **WHEN** Accessibility permission is granted
- **THEN** the system automatically activates Input Monitoring step
- **AND** displays "Open System Settings" button for Input Monitoring
- **AND** shows Accessibility step with checkmark/completed state

#### Scenario: Deep link to System Settings
- **WHEN** user clicks "Open System Settings" for any permission
- **THEN** the system opens macOS System Settings
- **AND** navigates directly to the relevant privacy pane (Privacy_Accessibility or Privacy_ListenEvent)
- **AND** keeps permission gate window visible on top

### Requirement: Visual Permission Status Indicators
The system SHALL provide clear visual feedback for permission status throughout the gating flow.

#### Scenario: Pending permission state
- **WHEN** permission is not yet granted
- **THEN** the system displays permission step with pending indicator (e.g., empty circle)
- **AND** shows "Open System Settings" button enabled
- **AND** uses muted colors to indicate inactive state

#### Scenario: Granted permission state
- **WHEN** permission is granted
- **THEN** the system displays checkmark icon or success indicator
- **AND** changes color to green/success color
- **AND** disables "Open System Settings" button for that permission

#### Scenario: Progress indicator
- **WHEN** permission gate is active
- **THEN** the system shows "Step 1 of 2" or "Step 2 of 2" indicator
- **AND** updates automatically as permissions are granted

### Requirement: Input Monitoring Permission Detection
The system SHALL detect macOS Input Monitoring permission status at app launch and during runtime.

#### Scenario: Check Input Monitoring permission on launch
- **WHEN** app launches
- **THEN** the system queries Input Monitoring permission status via IOHIDRequestAccess API
- **AND** returns true if permission is granted
- **AND** returns false if permission is denied or not determined

#### Scenario: Input Monitoring permission required for hotkeys
- **WHEN** Input Monitoring permission is not granted
- **THEN** the system cannot detect global hotkeys
- **AND** blocks initialization of rdev hotkey listener

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
