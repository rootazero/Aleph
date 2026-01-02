# provider-active-state Specification Delta

## MODIFIED Requirements

### Requirement: Active State Impact on Routing
Only active providers SHALL be used by the routing system, including for default provider selection.

#### Scenario: Default provider automatically falls back when disabled
- **GIVEN** "openai" is set as the default provider
- **AND** the user toggles "openai" Active switch to OFF
- **WHEN** the config is saved
- **THEN** the system SHALL:
  - Clear `general.default_provider` from config
  - Show a warning toast: "Default provider 'openai' was disabled. Please select a new default."
  - Use the first enabled provider as fallback for routing
  - Update ProvidersView to remove "Default" badge from "openai"
  - Update menu bar to remove checkmark from "openai"

#### Scenario: Warning shown if default provider is disabled
- **GIVEN** the default provider is disabled (edge case, e.g., manual config edit)
- **WHEN** the app launches and loads config
- **THEN** the system SHALL:
  - Validate default provider is enabled
  - Log warning: "Default provider '<name>' is disabled, using fallback"
  - Show a notification or alert to the user: "Your default provider is disabled. Please enable it or select a new default in Settings."
  - Use the first enabled provider as fallback
  - NOT prevent app from starting

#### Scenario: Auto-enable when setting as default
- **GIVEN** a provider is disabled
- **WHEN** the user attempts to set it as default
- **THEN** the system SHALL:
  - Show a confirmation dialog: "This provider is disabled. Do you want to enable it and set as default?"
  - If user confirms:
    - Set `config.enabled = true`
    - Set `general.default_provider = "<provider_name>"`
    - Save config
    - Update UI to show Active indicator and Default badge
  - If user cancels:
    - Do nothing, keep current default unchanged

**Note**: This scenario is OPTIONAL. The simpler approach (as defined in default-provider-management spec) is to prevent setting disabled providers as default with an error message. This scenario offers better UX but adds complexity.

### Requirement: Active State Toggle Control
The Active state toggle SHALL integrate with default provider management to handle state transitions correctly.

#### Scenario: Toggle OFF clears default if applicable
- **GIVEN** "openai" is both enabled and set as default
- **WHEN** the user toggles "openai" Active switch to OFF
- **THEN** the system SHALL:
  - Set `config.enabled = false` for "openai"
  - Clear `general.default_provider` from config
  - Save config to disk
  - Show warning: "Default provider was disabled. Select a new default."
  - Use first enabled provider as routing fallback

#### Scenario: Toggle ON does not auto-set as default
- **GIVEN** a provider is disabled
- **WHEN** the user toggles it to ON (enabled)
- **THEN** the system SHALL:
  - Set `config.enabled = true`
  - Save config to disk
  - Update UI to show Active indicator
  - NOT automatically set it as default provider
  - Keep current default provider unchanged

## ADDED Requirements

### Requirement: Menu Bar Shows Only Active Providers
The menu bar SHALL display only enabled providers in the quick switch menu.

#### Scenario: Filter inactive providers from menu
- **GIVEN** the config has 5 providers: "openai" (enabled), "claude" (disabled), "gemini" (enabled), "ollama" (disabled), "custom1" (enabled)
- **WHEN** the user opens the menu bar menu
- **THEN** the enabled providers section SHALL display:
  - "openai"
  - "gemini"
  - "custom1"
- **AND** SHALL NOT display:
  - "claude"
  - "ollama"

#### Scenario: Update menu when provider active state changes
- **GIVEN** the menu bar is initialized
- **WHEN** the user enables a previously disabled provider (e.g., "claude")
- **THEN** the menu bar SHALL:
  - Rebuild the enabled providers menu section
  - Add "claude" to the menu in alphabetical order (or config order)
  - Update immediately without app restart
- **AND** when the user disables an enabled provider (e.g., "openai")
- **THEN** the menu bar SHALL:
  - Rebuild the menu section
  - Remove "openai" from the menu
  - If "openai" was the default, remove the checkmark

#### Scenario: Empty menu when no providers are active
- **GIVEN** all providers are disabled
- **WHEN** the user opens the menu bar menu
- **THEN** the enabled providers section SHALL:
  - NOT be displayed at all (no separator, no empty section)
  - OR display a single disabled menu item: "No active providers"
  - User should see: About | Settings | Quit (no provider section)
