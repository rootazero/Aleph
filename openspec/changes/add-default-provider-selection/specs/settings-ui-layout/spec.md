# settings-ui-layout Specification Delta

## ADDED Requirements

### Requirement: Default Provider Indicator in ProvidersView
The ProvidersView SHALL display a visual indicator for the currently selected default provider.

#### Scenario: Display "Default" badge on provider card
- **GIVEN** the user is viewing the Providers tab
- **AND** "openai" is set as `general.default_provider` in config
- **WHEN** the provider list renders
- **THEN** the "openai" provider card SHALL display:
  - A "Default" badge near the provider name
  - Badge background color: `DesignTokens.Colors.accentBlue` or similar primary color
  - Badge text color: white
  - Badge font: `.caption` or `.caption2` (small, subtle)
  - Badge position: top-right corner OR next to provider name (consistent with "Active" badge)
- **AND** no other provider card SHALL display the "Default" badge

#### Scenario: Default badge coexists with Active indicator
- **GIVEN** a provider is both enabled (active) and set as default
- **WHEN** the provider card renders
- **THEN** the card SHALL display:
  - "Active" status indicator (existing, green/blue)
  - "Default" badge (new, blue accent)
  - Both indicators should be visually distinct and not overlap
  - Suggested layout: "Active" indicator (left), "Default" badge (right)

#### Scenario: Only enabled providers can be set as default
- **GIVEN** a provider is disabled
- **WHEN** the user attempts to set it as default via the edit panel
- **THEN** the system SHALL:
  - Show an error alert: "Cannot set disabled provider as default. Please enable it first."
  - NOT update `general.default_provider` in config
  - Keep the current default provider unchanged

### Requirement: Default Provider Indicator in ProviderEditPanel
The ProviderEditPanel SHALL show default status and allow setting as default.

#### Scenario: Display default status in provider info card
- **GIVEN** the user selects a provider in the edit panel
- **AND** that provider is the current default
- **WHEN** the provider info card renders
- **THEN** the card SHALL display:
  - A "Default Provider" text label or badge in the header
  - Positioned near the provider name or Active toggle
  - Styled with accent color to indicate special status

#### Scenario: Set as Default button in edit panel
- **GIVEN** the user is editing a provider
- **AND** the provider is enabled
- **AND** the provider is NOT the current default
- **WHEN** the edit panel renders
- **THEN** the panel SHALL display:
  - A "Set as Default" button below the provider info card
  - Button style: secondary or prominent action button
  - Button action: calls `setAsDefault(providerId)` method
- **AND** if the provider is already default
- **THEN** the button SHALL be:
  - Disabled with text "Default Provider" (indicating current state)
  - OR hidden entirely

#### Scenario: Disable "Set as Default" button for inactive providers
- **GIVEN** the user is editing a provider
- **AND** the provider is disabled (Active toggle is OFF)
- **WHEN** the edit panel renders
- **THEN** the "Set as Default" button SHALL be:
  - Disabled (grayed out)
  - Tooltip: "Enable this provider to set as default"
  - NOT clickable

## MODIFIED Requirements

### Requirement: Provider-Specific Configuration Parameters
The edit panel SHALL integrate default provider UI elements without disrupting existing parameter form layout.

#### Scenario: Default provider UI integration
- **GIVEN** the user is viewing the provider edit panel
- **WHEN** the panel renders with provider-specific parameters
- **THEN** the panel SHALL:
  - Display "Set as Default" button above or below the parameter form
  - Maintain consistent spacing using `DesignTokens.Spacing` values
  - NOT overlap with existing parameter fields
  - Ensure Active toggle and Default indicator are visually distinct
  - Preserve all existing parameter field functionality
