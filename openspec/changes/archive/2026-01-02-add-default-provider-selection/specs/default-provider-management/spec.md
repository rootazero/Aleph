# default-provider-management Specification

## Purpose
Manage the default provider selection for Aleph's AI routing system. The default provider is used when no routing rule matches the user's input. This spec ensures users can visually identify, select, and switch the default provider through both Settings UI and menu bar.

## ADDED Requirements

### Requirement: Default Provider Selection
The system SHALL allow users to designate one enabled provider as the default.

#### Scenario: Set provider as default from edit panel
- **GIVEN** the user is editing a provider in the edit panel
- **AND** the provider is enabled
- **WHEN** the user clicks "Set as Default" button in the edit panel
- **THEN** the system SHALL:
  - Update `general.default_provider` in config.toml to the selected provider name
  - Save the config file atomically
  - Update the UI to show "Default" badge on the selected provider card in the provider list
  - Remove "Default" badge from the previous default provider (if any)
  - Change the button to "Default Provider" (disabled state) to indicate success

#### Scenario: Set provider as default from menu bar
- **GIVEN** the user clicks the menu bar icon
- **AND** the enabled providers menu section is displayed
- **WHEN** the user clicks on a provider name in the menu
- **THEN** the system SHALL:
  - Update `general.default_provider` to the clicked provider
  - Save config atomically
  - Rebuild the menu to show checkmark (✓) next to the new default
  - Remove checkmark from the previous default

#### Scenario: Prevent setting disabled provider as default
- **GIVEN** a provider is disabled (active state is OFF)
- **WHEN** the user attempts to set it as default (via any method)
- **THEN** the system SHALL:
  - Show an error message: "Cannot set disabled provider as default. Please enable it first."
  - NOT update the config
  - Keep the current default provider unchanged

### Requirement: Default Provider Validation
The system SHALL validate that the default provider exists and is enabled.

#### Scenario: Validate default provider on app startup
- **GIVEN** the app is launching
- **AND** config.toml has `general.default_provider = "openai"`
- **WHEN** the config is loaded and validated
- **THEN** the system SHALL:
  - Check if "openai" exists in `providers` section
  - Check if "openai" has `enabled = true`
  - If both checks pass, use "openai" as default
  - If any check fails, log a warning and clear `general.default_provider`

#### Scenario: Validate default provider on config reload
- **GIVEN** the config file is modified externally
- **AND** the app detects the change via file watcher
- **WHEN** the config is reloaded
- **THEN** the system SHALL re-validate the default provider as in startup scenario
- **AND** update the UI to reflect any changes

#### Scenario: Handle default provider deleted
- **GIVEN** the current default provider is "claude"
- **WHEN** the user deletes the "claude" provider from Settings
- **THEN** the system SHALL:
  - Clear `general.default_provider` from config
  - Show a warning toast: "Default provider 'claude' was deleted. Please select a new default."
  - Use the first enabled provider as fallback for routing
  - Update the menu bar to remove checkmark from any provider

#### Scenario: Handle default provider disabled
- **GIVEN** the current default provider is "openai"
- **WHEN** the user disables "openai" by toggling the Active switch
- **THEN** the system SHALL:
  - Clear `general.default_provider` from config
  - Show a warning toast: "Default provider 'openai' was disabled. Please select a new default."
  - Use the first enabled provider as fallback for routing
  - Update the UI to remove "Default" badge
  - Update menu bar to remove checkmark

### Requirement: Default Provider Visual Indicator
The system SHALL clearly indicate which provider is currently set as default.

#### Scenario: Display default badge in ProvidersView
- **GIVEN** the user is viewing the Providers tab
- **AND** "openai" is set as the default provider
- **WHEN** the provider list renders
- **THEN** the system SHALL:
  - Display a "Default" badge on the "openai" provider card
  - Use blue accent color (`DesignTokens.Colors.accentBlue`)
  - Position the badge near the provider name or in a consistent location
  - NOT display the badge on any other provider

#### Scenario: Display default status in edit panel
- **GIVEN** the user selects a provider in the edit panel
- **AND** that provider is the current default
- **WHEN** the edit panel renders
- **THEN** the system SHALL:
  - Show "Default Provider" text or indicator in the provider info card
  - Disable the "Set as Default" button (already default)
  - OR change button text to "Default Provider" with disabled state

#### Scenario: Display checkmark in menu bar
- **GIVEN** the user opens the menu bar menu
- **AND** "claude" is set as the default provider
- **WHEN** the enabled providers menu section renders
- **THEN** the system SHALL:
  - Display a checkmark (✓) before "claude" menu item
  - NOT display checkmark before any other provider menu item
  - Use standard macOS menu item state styling

### Requirement: Menu Bar Enabled Providers Display
The menu bar SHALL display only enabled providers in a dedicated section.

#### Scenario: Show only enabled providers in menu
- **GIVEN** the config has 5 providers: 3 enabled, 2 disabled
- **WHEN** the user opens the menu bar menu
- **THEN** the menu SHALL:
  - Display a section with the 3 enabled providers
  - NOT display the 2 disabled providers
  - Show providers in alphabetical order (or config order)

#### Scenario: Show empty state when no providers enabled
- **GIVEN** all providers are disabled or no providers are configured
- **WHEN** the user opens the menu bar menu
- **THEN** the menu SHALL:
  - NOT display the enabled providers section at all
  - OR display a disabled menu item "No active providers"

#### Scenario: Update menu when provider is enabled/disabled
- **GIVEN** the menu bar is initialized
- **WHEN** the user enables a previously disabled provider in Settings
- **THEN** the menu bar SHALL:
  - Rebuild the enabled providers menu section
  - Add the newly enabled provider to the menu
  - Update immediately without requiring app restart

#### Scenario: Update menu when provider is added/deleted
- **GIVEN** the menu bar is initialized
- **WHEN** the user adds a new custom provider and enables it
- **THEN** the menu bar SHALL:
  - Rebuild the enabled providers menu section
  - Include the new provider in the menu
- **AND** when the user deletes an enabled provider
- **THEN** the menu bar SHALL remove it from the menu immediately

### Requirement: Default Provider Config Persistence
Changes to the default provider SHALL be persisted to config.toml atomically.

#### Scenario: Persist default provider selection
- **GIVEN** the user sets "gemini" as the default provider
- **WHEN** the selection is confirmed
- **THEN** the system SHALL:
  - Update `general.default_provider = "gemini"` in the Config struct
  - Call `config.save()` to write to ~/.aleph/config.toml
  - Use atomic write (temp file + rename) to prevent corruption
  - Set file permissions to 600 (owner read/write only)

#### Scenario: Survive app restart
- **GIVEN** the user set "claude" as default and quit the app
- **WHEN** the app is relaunched
- **THEN** the system SHALL:
  - Load config from ~/.aleph/config.toml
  - Read `general.default_provider = "claude"`
  - Validate that "claude" is enabled
  - Display "Default" badge on "claude" in Settings UI
  - Display checkmark (✓) next to "claude" in menu bar

#### Scenario: Handle concurrent config updates
- **GIVEN** the user is rapidly switching default providers
- **WHEN** multiple set_default_provider calls occur in quick succession
- **THEN** the system SHALL:
  - Queue config writes to prevent race conditions
  - OR use locking mechanism to ensure atomicity
  - Ensure the final config reflects the last user action
  - NOT corrupt the config file

### Requirement: Default Provider Routing Integration
The routing system SHALL use the default provider when no rule matches.

#### Scenario: Route to default provider on no match
- **GIVEN** `general.default_provider = "openai"`
- **AND** no routing rule matches the user input "Hello world"
- **WHEN** the router attempts to select a provider
- **THEN** the router SHALL:
  - Return "openai" provider
  - Use no system prompt override (None)
  - Log: "No rule matched, using default provider: openai"

#### Scenario: Fallback when default provider is disabled
- **GIVEN** `general.default_provider = "claude"`
- **AND** "claude" provider has `enabled = false`
- **WHEN** the router is initialized
- **THEN** the router SHALL:
  - Log a warning: "Default provider 'claude' is disabled"
  - Use the first enabled provider from `providers` map as fallback
  - If no providers are enabled, log error and return None

#### Scenario: Fallback when default provider is missing
- **GIVEN** `general.default_provider = "nonexistent"`
- **AND** "nonexistent" does not exist in `providers` section
- **WHEN** the router is initialized
- **THEN** the router SHALL:
  - Log a warning: "Default provider 'nonexistent' not found in config"
  - Use the first enabled provider as fallback
  - Clear `general.default_provider` to avoid repeated warnings

### Requirement: UniFFI API for Default Provider Management
The Rust core SHALL expose methods for getting and setting the default provider.

#### Scenario: Get current default provider
- **GIVEN** the Swift UI needs to display the default provider
- **WHEN** Swift calls `core.getDefaultProvider()`
- **THEN** the method SHALL:
  - Return `Option<String>` with the provider name if set and valid
  - Return `None` if no default provider is set or if it's invalid
  - NOT throw an error

#### Scenario: Set default provider with validation
- **GIVEN** the user selects "gemini" as default from UI
- **WHEN** Swift calls `core.setDefaultProvider("gemini")`
- **THEN** the method SHALL:
  - Validate that "gemini" exists in providers
  - Validate that "gemini" has `enabled = true`
  - If validation passes:
    - Update `config.general.default_provider = Some("gemini")`
    - Save config to disk
    - Return `Ok(())`
  - If validation fails:
    - Return `Err(AlephError::InvalidConfig("Provider 'gemini' is not enabled"))`

#### Scenario: Get list of enabled providers for menu bar
- **GIVEN** the menu bar needs to populate the providers menu
- **WHEN** Swift calls `core.getEnabledProviders()`
- **THEN** the method SHALL:
  - Return `Vec<String>` with names of all providers where `enabled = true`
  - Sort providers alphabetically (or preserve config order)
  - Return empty Vec if no providers are enabled
