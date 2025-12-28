# settings-ui-layout Spec Delta

## ADDED Requirements

### Requirement: Active Toggle Integration in Provider Information Card
The Active toggle SHALL be integrated into the provider information card row for a more compact and contextual layout.

#### Scenario: Toggle position in provider card
- **GIVEN** a provider is selected (preset or custom)
- **WHEN** the provider information card renders
- **THEN** the card SHALL display an HStack containing:
  - Provider icon (left)
  - Provider name and type label (center-left)
  - Active toggle switch (right-aligned with `Spacer()`)
- **AND** the toggle SHALL use the native macOS Toggle style (green when active, gray when inactive)
- **AND** no text label or help text SHALL be displayed with the toggle

#### Scenario: Toggle state visual feedback
- **GIVEN** the Active toggle is rendered
- **WHEN** the toggle is in the ON state
- **THEN** the toggle SHALL display with green accent color (macOS default)
- **AND** when the toggle is in the OFF state
- **THEN** the toggle SHALL display with gray color (macOS default)

### Requirement: Custom Provider Support
The system SHALL support user-defined custom providers that are OpenAI-compatible.

#### Scenario: Custom provider preset availability
- **GIVEN** the user is viewing the provider list
- **WHEN** the provider list renders
- **THEN** a "Custom (OpenAI-compatible)" preset option SHALL be available in the list
- **AND** the custom preset SHALL have a distinct icon (e.g., "puzzlepiece" or "wrench.and.screwdriver")
- **AND** the custom preset SHALL have a neutral color (e.g., gray)

#### Scenario: Multiple custom provider instances
- **GIVEN** the user has selected the "Custom" preset
- **WHEN** the user saves a custom provider configuration
- **THEN** the system SHALL allow creating multiple independent custom provider instances
- **AND** each instance SHALL have a unique provider name (user-defined)
- **AND** each instance SHALL be listed separately in the configured providers list

#### Scenario: Custom provider required fields
- **GIVEN** the user is configuring a custom provider
- **WHEN** the form renders
- **THEN** the following fields SHALL be visible and editable:
  - Provider Name (required, text field, user-defined identifier)
  - Theme Color (required, color picker, used for Halo overlay)
  - Base URL (required, text field, OpenAI-compatible API endpoint)
  - API Key (required, secure field)
  - Model (required, text field)
  - Standard generation parameters (optional)
- **AND** the provider information card SHALL NOT be displayed (since it's custom)

### Requirement: Conditional Field Visibility Based on Provider Type
The edit panel SHALL conditionally show/hide fields based on whether the provider is a preset or custom.

#### Scenario: Preset provider field visibility
- **GIVEN** a preset provider is selected (e.g., OpenAI, Anthropic, Gemini)
- **WHEN** the edit form renders
- **THEN** the following fields SHALL be visible:
  - Provider information card (with integrated Active toggle)
  - API Key (except for Ollama)
  - Model
  - Base URL (optional)
  - Generation parameters (provider-specific)
- **AND** the following fields SHALL be hidden:
  - Provider Name
  - Theme Color

#### Scenario: Custom provider field visibility
- **GIVEN** a custom provider is being added or edited
- **WHEN** the edit form renders
- **THEN** the following fields SHALL be visible:
  - Active toggle (standalone, at top of form)
  - Provider Name (editable)
  - Theme Color (editable)
  - Base URL (required, not optional)
  - API Key
  - Model
  - Generation parameters (OpenAI-compatible)
- **AND** the provider information card SHALL NOT be displayed

### Requirement: Provider Information Display Card
The provider information card SHALL include an integrated Active toggle for compact layout.

#### Scenario: Provider information card layout with toggle
- **GIVEN** a preset provider is selected
- **WHEN** the edit panel renders the provider information card
- **THEN** the card SHALL display in the following structure:
  ```
  [Icon] [Provider Name]            [Toggle]
         [Provider Type]
  [Description text spanning full width below]
  ```
- **AND** the top row SHALL be an HStack with:
  - Circular icon (48x48) on the left
  - VStack with provider name (title font) and type (caption font) in the center
  - `Spacer()` to push toggle to the right
  - Active toggle switch (right-aligned, `labelsHidden()`)
- **AND** the description text SHALL appear below the top row with `fixedSize(horizontal: false, vertical: true)`

#### Scenario: Provider information card only for presets
- **GIVEN** the edit panel is rendering
- **WHEN** a preset provider is selected
- **THEN** the provider information card SHALL be displayed
- **AND** when a custom provider is being configured
- **THEN** the provider information card SHALL NOT be displayed

### Requirement: Form Field Ordering
The form fields SHALL follow a logical order based on provider type.

#### Scenario: Preset provider field order
- **GIVEN** a preset provider is selected
- **WHEN** the form renders
- **THEN** fields SHALL appear in this order:
  1. Provider information card (with integrated toggle)
  2. API Key (if applicable)
  3. Model
  4. Base URL (optional)
  5. Generation Parameters (collapsible)

#### Scenario: Custom provider field order
- **GIVEN** a custom provider is being configured
- **WHEN** the form renders
- **THEN** fields SHALL appear in this order:
  1. Active toggle (standalone, top of form)
  2. Provider Name (editable)
  3. Theme Color (editable)
  4. API Key
  5. Model
  6. Base URL (required)
  7. Generation Parameters (collapsible)
