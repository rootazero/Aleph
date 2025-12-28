# macOS Client Specification Deltas

## MODIFIED Requirements

### Requirement: Config Hot-Reload Integration
The macOS client **SHALL** respond to config change notifications from Rust core and update UI accordingly.

#### Scenario: Subscribe to config change events
- **GIVEN** AppDelegate initializes AetherCore with EventHandler
- **WHEN** Rust config watcher detects external config.toml modification
- **THEN** Rust calls `handler.onConfigChanged(new_config)` callback
- **AND** EventHandler receives callback on background thread
- **AND** dispatches to main queue with `DispatchQueue.main.async`
- **AND** posts `NSNotification.Name("AetherConfigDidChange")` with new config

#### Scenario: Settings window observes config changes
- **GIVEN** Settings window is open displaying providers list
- **WHEN** `AetherConfigDidChange` notification is posted
- **THEN** SettingsView receives notification via `.onReceive(NotificationCenter.publisher)`
- **AND** calls `core.getConfig()` to fetch updated config
- **AND** all tabs (General, Providers, Routing, Shortcuts, Behavior) refresh with new values
- **AND** toast notification appears: "Settings updated from file"

#### Scenario: Handle config reload while editing
- **GIVEN** user is editing a routing rule in RuleEditorView modal
- **AND** external process modifies config.toml
- **WHEN** config change notification arrives
- **THEN** modal displays alert: "Configuration was updated externally. Your changes will be lost. Continue editing?"
- **WHEN** user clicks "Continue"
- **THEN** modal remains open with user's unsaved changes
- **WHEN** user clicks "Reload"
- **THEN** modal closes and list view updates with new config

---

## ADDED Requirements

### Requirement: Settings Menu Integration
The macOS client **SHALL** provide menu bar access to Settings window.

#### Scenario: Open settings from menu
- **WHEN** user clicks "Settings..." in menu bar
- **THEN** AppDelegate.showSettings() is called
- **AND** Settings window opens at size 800x550
- **AND** window is centered on screen
- **AND** General tab is selected by default
- **WHEN** Settings window is already open
- **AND** user clicks "Settings..." again
- **THEN** existing window is brought to front (orderFront)
- **AND** no duplicate window is created

---

### Requirement: Error Handling for Config Operations
The macOS client **SHALL** display user-friendly error messages when config operations fail.

#### Scenario: Display validation error
- **WHEN** user attempts to save routing rule with invalid regex
- **AND** Rust returns `ConfigError::InvalidRegex`
- **THEN** Swift catches error
- **AND** displays alert with title "Invalid Configuration"
- **AND** message: "Regex pattern '[invalid' is invalid: Unclosed character class"

#### Scenario: Display Keychain access error
- **WHEN** user attempts to save API key
- **AND** Keychain access is denied (e.g., app not authorized)
- **THEN** Swift catches Keychain error
- **AND** displays alert with title "Keychain Access Denied"
- **AND** message: "Aether needs permission to store API keys securely. Please allow access in System Settings."
- **AND** "Open System Settings" button to open Security & Privacy pane

---

### Requirement: Config Operation Loading States
The macOS client **SHALL** provide visual feedback during async config operations.

#### Scenario: Show loading spinner during provider test
- **WHEN** user clicks "Test Connection" button for OpenAI
- **THEN** button text changes to "Testing..."
- **AND** loading spinner appears on button
- **AND** button is disabled during test
- **WHEN** test completes (success or failure)
- **THEN** spinner disappears
- **AND** button re-enables
- **AND** result toast notification appears

#### Scenario: Show progress during config save
- **WHEN** user clicks "Save" in RuleEditorView
- **AND** Rust validation and write operation is in progress
- **THEN** "Save" button shows loading spinner
- **AND** button is disabled
- **WHEN** save completes
- **THEN** modal closes
- **AND** routing list updates with new rule
