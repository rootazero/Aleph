## MODIFIED Requirements

### Requirement: Permission Documentation
The project SHALL document all required macOS permissions and their purposes.

#### Scenario: Input Monitoring permission explanation
- **WHEN** a user or developer reviews the README
- **THEN** it SHALL explain why Input Monitoring permission is required (for global hotkey detection via rdev)
- **AND** provide step-by-step instructions for granting the permission

## ADDED Requirements

### Requirement: Mandatory Permission Gate at Launch
The macOS client SHALL enforce a blocking permission setup flow on app launch that prevents access to settings and features until required permissions are granted.

#### Scenario: Launch with missing permissions
- **WHEN** app launches and required permissions (Accessibility or Input Monitoring) are not granted
- **THEN** the app displays PermissionGateView as a blocking modal
- **AND** does not initialize AlephCore or start hotkey listening
- **AND** disables "Settings..." menu bar item

#### Scenario: Launch with all permissions granted
- **WHEN** app launches and both Accessibility and Input Monitoring are granted
- **THEN** the app skips permission gate
- **AND** initializes AlephCore normally
- **AND** enables all menu bar features

### Requirement: Info.plist Permission Descriptions
The macOS client SHALL declare all required permissions with user-friendly descriptions in Info.plist.

#### Scenario: Accessibility usage description
- **WHEN** system prompts user for Accessibility permission
- **THEN** Info.plist SHALL contain NSAppleEventsUsageDescription explaining the need for keyboard simulation

#### Scenario: Input Monitoring usage description
- **WHEN** system prompts user for Input Monitoring permission
- **THEN** Info.plist SHALL contain NSAppleEventsUsageDescription (or equivalent) explaining the need for global hotkey detection
