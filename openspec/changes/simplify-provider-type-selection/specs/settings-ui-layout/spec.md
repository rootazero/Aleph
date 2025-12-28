# settings-ui-layout Spec Delta

## ADDED Requirements

### Requirement: Provider Information Display Card
When a preset provider is selected, the edit panel SHALL display a read-only information card showing the provider's identity and purpose.

#### Scenario: Provider information card contents
- **GIVEN** a preset provider is selected from the left panel
- **WHEN** the edit panel renders
- **THEN** the top of the edit panel SHALL display a provider information card containing:
  - A circular icon with the provider's brand color as background
  - The provider's icon symbol in white foreground
  - The provider's display name (e.g., "OpenAI", "Anthropic")
  - The provider type label (e.g., "OpenAI", "Claude", "Gemini")
  - The provider's description text
- **AND** all information SHALL be read-only (non-editable)
- **AND** the card SHALL use `DesignTokens.Spacing.md` for internal spacing

#### Scenario: Provider information updates with selection
- **GIVEN** a preset provider is selected
- **WHEN** the user selects a different preset provider from the left panel
- **THEN** the provider information card SHALL update to show the new provider's details
- **AND** the icon, name, type, and description SHALL all reflect the newly selected provider

### Requirement: Provider Name Field Read-Only Behavior
The Provider Name field SHALL be read-only and auto-populated from the selected preset provider.

#### Scenario: Provider name auto-population
- **GIVEN** a preset provider is selected from the left panel
- **WHEN** the edit panel renders
- **THEN** the Provider Name field SHALL be pre-filled with the preset's ID (e.g., "openai", "anthropic")
- **AND** the field SHALL be disabled (read-only, non-editable)
- **AND** a help text SHALL be displayed below the field stating: "This name is used to reference the provider in routing rules"

#### Scenario: Provider name consistency
- **GIVEN** a configured provider is loaded for editing
- **WHEN** the edit panel renders
- **THEN** the Provider Name SHALL match the `ProviderConfigEntry.name` value
- **AND** the field SHALL remain read-only
- **AND** the user SHALL NOT be able to rename existing providers through the UI

## REMOVED Requirements

### Requirement: Provider Type Manual Selection
**Reason**: Provider type is now automatically determined from the selected preset provider in the left panel, eliminating redundant user input and potential configuration errors.

**Migration**: Existing provider configurations are unaffected. The Provider Type field in config.toml remains unchanged. This change only affects the UI interaction, not the data model.

#### Scenario: Provider type dropdown (REMOVED)
- **PREVIOUSLY**: The edit panel displayed a Picker control allowing users to select provider type (OpenAI, Claude, Gemini, Ollama, Custom)
- **NOW**: Provider type is automatically set based on `selectedPreset.providerType` and no picker is shown

## MODIFIED Requirements

### Requirement: Dynamic Parameter Visibility Based on Provider Type
The edit panel SHALL dynamically show/hide generation parameters based on the provider type of the selected preset.

#### Scenario: OpenAI-specific parameters visibility
- **GIVEN** a preset provider with `providerType == "openai"` is selected
- **WHEN** the Generation Parameters section is expanded
- **THEN** the following OpenAI-specific fields SHALL be visible:
  - Frequency Penalty (range: -2.0 to 2.0)
  - Presence Penalty (range: -2.0 to 2.0)
- **AND** Claude/Gemini/Ollama-specific parameters SHALL be hidden

#### Scenario: Claude-specific parameters visibility
- **GIVEN** a preset provider with `providerType == "claude"` is selected
- **WHEN** the Generation Parameters section is expanded
- **THEN** the following Claude-specific fields SHALL be visible:
  - Top-K (optional integer > 0)
  - Stop Sequences (comma-separated strings)
- **AND** OpenAI/Gemini/Ollama-specific parameters SHALL be hidden
- **AND** Temperature range validation SHALL be 0.0-1.0 (Claude-specific)

#### Scenario: Gemini-specific parameters visibility
- **GIVEN** a preset provider with `providerType == "gemini"` is selected
- **WHEN** the Generation Parameters section is expanded
- **THEN** the following Gemini-specific fields SHALL be visible:
  - Top-K (optional integer > 0)
  - Stop Sequences (comma-separated strings)
  - Thinking Level (segmented picker: LOW / HIGH)
  - Media Resolution (segmented picker: LOW / MEDIUM / HIGH)
- **AND** OpenAI/Ollama-specific parameters SHALL be hidden
- **AND** Temperature range validation SHALL be 0.0-2.0

#### Scenario: Ollama-specific parameters visibility
- **GIVEN** a preset provider with `providerType == "ollama"` is selected
- **WHEN** the Generation Parameters section is expanded
- **THEN** the following Ollama-specific fields SHALL be visible:
  - Top-K (recommended value: 40)
  - Stop Sequences (comma-separated strings)
  - Repeat Penalty (must be >= 1.0)
- **AND** the API Key field SHALL be hidden (Ollama does not require API keys)
- **AND** OpenAI/Claude/Gemini-specific parameters SHALL be hidden

#### Scenario: Parameter visibility reactivity
- **GIVEN** the edit panel is displaying parameters for a provider
- **WHEN** the user selects a different preset provider with a different `providerType`
- **THEN** the parameter visibility SHALL update immediately
- **AND** previously visible provider-specific parameters SHALL hide
- **AND** newly relevant provider-specific parameters SHALL show
- **AND** parameter values SHALL be cleared or reset to defaults when switching provider types
