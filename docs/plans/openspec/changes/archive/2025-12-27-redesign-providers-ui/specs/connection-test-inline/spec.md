# Specification: Connection Test Inline Display

## Purpose
Defines how provider connection test results are displayed inline as small text below the test button, replacing the current card-based display.

## ADDED Requirements

### Requirement: Inline Test Result Display
Connection test results SHALL appear as small caption text below the "Test Connection" button.

#### Scenario: Successful connection test
- **GIVEN** a user clicks "Test Connection" with valid credentials
- **WHEN** the provider responds successfully
- **THEN** below the button, display:
- Green checkmark icon (SF Symbol: `checkmark.circle.fill`, size: 12pt)
- Text: "Connected successfully" or provider-specific message
- Font: `DesignTokens.Typography.caption`
- Color: Green (`Color.green` or `#28A745`)
- **AND** the result SHALL persist until the next test OR form field is edited

#### Scenario: Failed connection test
- **GIVEN** a user clicks "Test Connection" with invalid credentials OR network issue
- **WHEN** the provider returns an error
- **THEN** below the button, display:
- Red X icon (SF Symbol: `xmark.circle.fill`, size: 12pt)
- Text: Error message (truncated to 80 characters with "..." if longer)
- Font: `DesignTokens.Typography.caption`
- Color: Red (`Color.red` or `#DC3545`)
- **AND** the full error SHALL be available in a tooltip on hover

#### Scenario: Test in progress
- **GIVEN** a user clicks "Test Connection"
- **WHEN** the request is in flight
- **THEN** the button SHALL show:
- Text changes to "Testing..."
- Button is disabled (grayed out)
- Small spinner (ProgressView) appears inside button OR below it
- **AND** previous test result (if any) SHALL be hidden during test

### Requirement: Result Auto-Clear on Form Edit
Test results SHALL be cleared when the user modifies relevant form fields.

#### Scenario: Edit API key after test
- **GIVEN** a connection test has completed (success or failure)
- **AND** the result is displayed below the button
- **WHEN** the user edits the API Key field
- **THEN** the test result SHALL immediately disappear
- **AND** the "Test Connection" button SHALL return to enabled state

#### Scenario: Edit model or base URL after test
- **GIVEN** a connection test result is displayed
- **WHEN** the user changes the Model OR Base URL field
- **THEN** the test result SHALL be cleared
- **AND** the user can test again with new settings

#### Scenario: Toggle provider type after test
- **GIVEN** a test result is displayed
- **WHEN** the user changes the Provider Type (e.g., OpenAI → Claude)
- **THEN** the test result SHALL be cleared
- **AND** the form fields SHALL reset to defaults for new type
- **AND** previous test is no longer relevant

### Requirement: Test Result Accessibility
Test results SHALL be accessible to assistive technologies.

#### Scenario: Screen reader announcement
- **GIVEN** a connection test completes
- **WHEN** the result is displayed
- **THEN** the result text SHALL have:
- `.accessibilityLabel()` with full error message (not truncated)
- `.accessibilityValue()` indicating success or failure
- **AND** VoiceOver SHALL announce: "Connection test succeeded" or "Connection test failed: [reason]"

#### Scenario: Keyboard navigation
- **GIVEN** a user navigates via keyboard
- **WHEN** the "Test Connection" button has focus
- **THEN** pressing Enter or Space SHALL trigger the test
- **AND** the result SHALL be announced by screen readers

### Requirement: Visual Positioning
Test result text SHALL be positioned consistently regardless of result type.

#### Scenario: Result text layout
- **GIVEN** any test result is displayed
- **WHEN** rendering the edit panel
- **THEN** the result text SHALL:
- Appear directly below the "Test Connection" button
- Have 8pt top padding (`DesignTokens.Spacing.sm`)
- Be left-aligned with the button
- NOT push other form elements down (reserved space)
- Wrap to 2 lines maximum if error is long

#### Scenario: Multi-line error messages
- **GIVEN** a connection test returns a 150-character error
- **WHEN** displaying the result
- **THEN** the text SHALL:
- Truncate to 80 characters with "..." suffix
- Display full message in tooltip on hover
- Use `.lineLimit(2)` and `.truncationMode(.tail)`

## Related Specs
- `provider-active-state`: Defines when test button should be available
- `settings-ui-layout`: Defines overall edit panel layout
- `ai-provider-interface`: Defines the `testProviderConnection()` API
