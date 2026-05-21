# Specification: Provider Active State

## Purpose
Defines how providers can be enabled/disabled through active state toggles in the UI, with visual indicators on cards and edit panel.

## ADDED Requirements

### Requirement: Active State Visual Indicator
Provider cards and edit panel SHALL display active/inactive state clearly.

#### Scenario: Active provider card indicator
- **GIVEN** a provider is configured with a valid API key
- **WHEN** the provider card is rendered
- **THEN** the card SHALL display:
- A blue filled circle (8pt diameter) indicator
- Positioned in the top-right corner OR next to the provider name
- Color: `#007AFF` or `DesignTokens.Colors.accentBlue`
- **AND** the StatusIndicator component SHALL show "Active" label

#### Scenario: Inactive provider card indicator
- **GIVEN** a provider has no API key OR is explicitly disabled
- **WHEN** the provider card is rendered
- **THEN** the card SHALL display:
- A gray outlined circle (8pt diameter) indicator
- Color: `DesignTokens.Colors.textSecondary` with 0.3 opacity
- **AND** the StatusIndicator component SHALL show "Inactive" label

#### Scenario: Active state badge in edit panel
- **GIVEN** the user views a provider in the edit panel
- **WHEN** the provider is active
- **THEN** the panel SHALL display:
- An "Active" badge (green background, white text)
- Positioned next to the provider name in the header
- **AND** a toggle switch in the ON position

### Requirement: Active State Toggle Control
Users SHALL be able to enable/disable providers via toggle switch.

#### Scenario: Toggle active state in view mode
- **GIVEN** a provider is selected in view mode
- **WHEN** the edit panel header is rendered
- **THEN** a toggle switch SHALL be displayed:
- Positioned to the right of the "Active"/"Inactive" badge
- Bound to the provider's active state
- Enabled (clickable) in view mode
- **AND** toggling the switch SHALL immediately update the active state
- **AND** the change SHALL persist to config without requiring "Save"

#### Scenario: Toggle active state in edit mode
- **GIVEN** a provider is being edited
- **WHEN** the user toggles the active switch
- **THEN** the UI SHALL update:
- Badge text changes between "Active" and "Inactive"
- Badge color changes (green for active, gray for inactive)
- **AND** the change SHALL be included in the save operation
- **AND** canceling the edit SHALL revert the toggle state

### Requirement: Active State Persistence
Active state SHALL be stored in the provider configuration.

#### Scenario: Save active state to config
- **GIVEN** a user has toggled a provider's active state
- **WHEN** the configuration is saved
- **THEN** the active state SHALL be persisted:
- If ProviderConfig has an `enabled: bool` field, use it
- If not, derive from API key presence (has key = active)
- **AND** the state SHALL survive app restarts

#### Scenario: Load active state from config
- **GIVEN** a provider configuration exists on disk
- **WHEN** the Settings UI loads
- **THEN** the provider's active state SHALL be read:
- Check `enabled` field if available
- Otherwise, check if `api_key` field is non-empty
- **AND** the UI SHALL reflect the loaded state

### Requirement: Active State Impact on Routing
Only active providers SHALL be used by the routing system.

#### Scenario: Route to inactive provider
- **GIVEN** a routing rule targets a provider
- **AND** that provider is marked inactive
- **WHEN** a user triggers the hotkey
- **THEN** the router SHALL:
- Skip the inactive provider
- Fall back to the next matching rule OR default provider
- **AND** optionally log a warning: "Provider 'X' is inactive, using fallback"

#### Scenario: All providers inactive
- **GIVEN** all configured providers are marked inactive
- **WHEN** a user triggers the hotkey
- **THEN** the system SHALL:
- Display an error message: "No active providers available"
- NOT attempt to send requests to inactive providers
- **AND** suggest enabling a provider in Settings

## Related Specs
- `ai-routing`: Defines how routing rules interact with provider availability
- `ai-provider-interface`: Defines the ProviderConfig data structure
- `settings-ui-layout`: Defines where active toggle appears in UI
