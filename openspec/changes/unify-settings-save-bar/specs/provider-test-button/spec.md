# provider-test-button Specification

## Purpose

Define the behavior and appearance of per-provider test connection buttons displayed on provider cards in the left sidebar. These buttons allow users to test API connectivity with unsaved configuration values before committing changes.

## ADDED Requirements

### Requirement: Test Button Visual Design and Positioning

The test button SHALL be displayed as an icon-only button on each provider card in the provider list.

#### Scenario: Button appearance on provider card
- **GIVEN** a provider card is displayed in the left sidebar (preset or custom)
- **WHEN** the card renders
- **THEN** the test button SHALL:
  - Use SF Symbol: `network` (or `antenna.radiowaves.left.and.right`)
  - Icon size: 16×16 points
  - Hit area: 24×24 points (for touch accessibility)
  - Color: `DesignTokens.Colors.textSecondary` (gray) in idle state
  - Color: `DesignTokens.Colors.accentBlue` (blue) on hover
  - Position: Immediately left of the Active toggle switch
  - Vertical alignment: Centered with provider name row

#### Scenario: Button layout in provider card
- **GIVEN** a provider card is rendered
- **WHEN** the card displays provider information
- **THEN** the layout SHALL be:
  ```
  [Icon] [Provider Name]            [Test Button] [Active Toggle]
         [Provider Type]
         [Test Result (if present)]
  ```
- **AND** the test button SHALL have:
  - Leading padding: `DesignTokens.Spacing.md` (12pt) from provider name
  - Trailing padding: `DesignTokens.Spacing.sm` (8pt) to Active toggle

#### Scenario: Tooltip on hover
- **GIVEN** the user hovers over the test button
- **WHEN** the mouse enters the button area
- **THEN** a tooltip SHALL appear displaying:
  - Text: "Test connection"
  - Delay: 500ms after hover start
  - Position: Above button, centered

---

### Requirement: Test Button States and Interactions

The test button SHALL provide clear visual feedback for idle, loading, success, and error states.

#### Scenario: Idle state (no test in progress)
- **GIVEN** no connection test is running for this provider
- **WHEN** the test button renders
- **THEN** the button SHALL:
  - Display `network` icon (static, no animation)
  - Color: `DesignTokens.Colors.textSecondary` (gray)
  - Be interactive (`.disabled(false)`)
  - Cursor: pointer on hover

#### Scenario: Loading state (test in progress)
- **GIVEN** the user has clicked the test button
- **WHEN** the connection test is in progress (`isTesting == true`)
- **THEN** the button SHALL:
  - Replace icon with small spinner (ProgressView)
  - Spinner size: 14×14 points
  - Spinner color: `DesignTokens.Colors.accentBlue` (blue)
  - Be non-interactive (`.disabled(true)`)
  - Cursor: default (not pointer)

#### Scenario: Click test button
- **GIVEN** a provider card is displayed
- **AND** the test button is in idle state
- **WHEN** the user clicks the test button
- **THEN** the system SHALL:
  1. Set button to loading state (show spinner)
  2. Build test config from current form values (working copy, not saved state)
  3. Call `AetherCore.testProviderConnectionWithConfig(providerName, workingConfig)`
  4. Wait for async response
  5. Display test result inline below the card
  6. Return button to idle state

---

### Requirement: Test Configuration with Unsaved Values

The test SHALL use the current working copy (form values) instead of saved configuration.

#### Scenario: Test unconfigured preset provider
- **GIVEN** a preset provider (e.g., OpenAI) has never been configured
- **AND** the user has entered API key and model in the form (not saved yet)
- **WHEN** the user clicks the test button
- **THEN** the test SHALL:
  - Use values from form fields (working copy):
    - `api_key`: Current value in API Key field
    - `model`: Current value in Model field
    - `base_url`: Current value in Base URL field (or preset default)
    - Other parameters: Current form values or preset defaults
  - Build a temporary `ProviderConfig` struct
  - Call `AetherCore.testProviderConnectionWithConfig()` (does NOT persist to disk)
  - Display result inline below the card

#### Scenario: Test configured provider with unsaved changes
- **GIVEN** a provider is already configured and saved
- **AND** the user has modified the API key in the form (not saved yet)
- **WHEN** the user clicks the test button
- **THEN** the test SHALL:
  - Use the NEW (unsaved) API key from the form
  - Use other values from working copy (which may also be modified)
  - NOT use the saved config from disk
  - Call test API with working copy values
  - Display result inline below the card

#### Scenario: Test with invalid form values
- **GIVEN** the form has validation errors (e.g., empty API key for OpenAI)
- **WHEN** the user clicks the test button
- **THEN** the test button SHALL:
  - Be disabled (`.disabled(true)`) if form is invalid
  - NOT trigger a connection test
  - Show tooltip: "Complete required fields to test"

---

### Requirement: Test Result Display

Test results SHALL be displayed inline below the provider card with clear success/error indicators.

#### Scenario: Successful connection test
- **GIVEN** a connection test has completed successfully
- **WHEN** the test result is displayed
- **THEN** the result SHALL appear below the provider card as:
  - Icon: SF Symbol `checkmark.circle.fill` (✓)
  - Icon color: `DesignTokens.Colors.success` (green)
  - Icon size: 12pt
  - Text: "Connected successfully" (or provider-specific message)
  - Text color: `DesignTokens.Colors.success` (green)
  - Font: `DesignTokens.Typography.caption` (small, 11pt)
  - Background: None (transparent)
  - Padding: `DesignTokens.Spacing.sm` (8pt) top, left-aligned with provider icon

#### Scenario: Failed connection test
- **GIVEN** a connection test has failed (network error, invalid credentials, etc.)
- **WHEN** the test result is displayed
- **THEN** the result SHALL appear below the provider card as:
  - Icon: SF Symbol `xmark.circle.fill` (❌)
  - Icon color: `DesignTokens.Colors.error` (red)
  - Icon size: 12pt
  - Text: Error message (e.g., "Invalid API key" or "Connection timeout")
  - Text color: `DesignTokens.Colors.error` (red)
  - Font: `DesignTokens.Typography.caption` (small, 11pt)
  - Text truncation: Max 80 characters, ellipsis at end (`...`)
  - Full error message: Available in tooltip on hover

#### Scenario: Test result persistence
- **GIVEN** a test result is displayed below a provider card
- **WHEN** the result is shown
- **THEN** the result SHALL:
  - Auto-hide after 5 seconds (fade out animation)
  - OR clear immediately when user edits any form field
  - OR clear when user clicks test button again
  - NOT persist across provider selection changes

#### Scenario: Test result for multiple providers
- **GIVEN** the user tests multiple providers in quick succession
- **WHEN** test results are displayed
- **THEN** each provider card SHALL:
  - Display its own independent test result
  - NOT affect other providers' test results
  - Allow concurrent testing (multiple tests can run simultaneously)

---

### Requirement: Test Button Availability

The test button SHALL be available for all provider types with appropriate validation.

#### Scenario: Test button on preset providers
- **GIVEN** a preset provider card is displayed (OpenAI, Claude, Gemini, Ollama)
- **WHEN** the card renders
- **THEN** the test button SHALL be visible and enabled (if form is valid)

#### Scenario: Test button on custom providers
- **GIVEN** a custom provider card is displayed
- **WHEN** the card renders
- **THEN** the test button SHALL be visible and enabled (if form is valid)

#### Scenario: Test button on unconfigured providers
- **GIVEN** a preset provider has never been configured (no saved config exists)
- **WHEN** the card renders
- **THEN** the test button SHALL:
  - Be visible
  - Be disabled (gray, non-interactive) until user enters required fields
  - Show tooltip: "Complete required fields to test"

#### Scenario: Test button validation for Ollama
- **GIVEN** an Ollama provider card is displayed
- **WHEN** the form is being edited
- **THEN** the test button SHALL be enabled when:
  - Base URL is not empty (default: `http://localhost:11434`)
  - Model name is not empty
  - API key is NOT required (Ollama doesn't use API keys)

#### Scenario: Test button validation for cloud providers (OpenAI, Claude, Gemini)
- **GIVEN** a cloud provider card is displayed
- **WHEN** the form is being edited
- **THEN** the test button SHALL be enabled when:
  - API key is not empty
  - Model name is not empty
  - Base URL is valid (if custom endpoint is used)

---

### Requirement: Test Connection API Integration

The test button SHALL use the existing `AetherCore.testProviderConnectionWithConfig()` API.

#### Scenario: Test API call signature
- **GIVEN** the user clicks the test button
- **WHEN** the test is triggered
- **THEN** the system SHALL call:
  ```swift
  AetherCore.testProviderConnectionWithConfig(
      providerName: String,      // e.g., "openai", "claude", "custom-provider-1"
      providerConfig: ProviderConfig  // Temporary config built from working copy
  ) -> TestConnectionResult
  ```
- **AND** the `providerConfig` parameter SHALL contain:
  - `providerType`: Provider type (e.g., "openai", "claude")
  - `apiKey`: Current value from form (NOT keychain reference)
  - `model`: Current value from form
  - `baseUrl`: Current value from form (or nil for default)
  - `color`: Current value from form (or preset default)
  - `timeoutSeconds`: Current value from form (default: 30)
  - All generation parameters: Current values from form

#### Scenario: Test result structure
- **GIVEN** the test API call completes
- **WHEN** the result is returned
- **THEN** the result SHALL be:
  ```swift
  struct TestConnectionResult {
      let success: Bool
      let message: String  // Success message or error description
  }
  ```
- **AND** if `success == true`:
  - Display green checkmark + success message
- **AND** if `success == false`:
  - Display red X + error message

#### Scenario: Test timeout handling
- **GIVEN** a test is in progress
- **WHEN** the test exceeds the configured timeout (default: 30 seconds)
- **THEN** the test SHALL:
  - Abort the connection attempt
  - Return `TestConnectionResult(success: false, message: "Connection timeout")`
  - Display error result below the card
  - Return test button to idle state

---

### Requirement: Accessibility and Keyboard Support

The test button SHALL be accessible to assistive technologies and keyboard navigation.

#### Scenario: VoiceOver support
- **GIVEN** VoiceOver is enabled
- **WHEN** the user navigates to the test button
- **THEN** VoiceOver SHALL announce:
  - Label: "Test connection to [Provider Name]"
  - Role: "Button"
  - State: "Enabled" or "Disabled"
  - Hint: "Verifies API credentials and endpoint"

#### Scenario: Keyboard navigation
- **GIVEN** the user navigates with Tab key
- **WHEN** focus reaches a provider card
- **THEN** the focus order SHALL be:
  1. Test button (if enabled)
  2. Active toggle switch
- **AND** pressing Space or Enter on focused test button SHALL trigger the test

#### Scenario: VoiceOver test result announcement
- **GIVEN** VoiceOver is enabled
- **AND** a test completes (success or failure)
- **WHEN** the test result is displayed
- **THEN** VoiceOver SHALL announce:
  - Role: "Status indicator"
  - Text: "Connection test succeeded: [message]" OR "Connection test failed: [error]"
  - Priority: Polite (non-interrupting)

---

### Requirement: Performance and Concurrency

The test button SHALL handle concurrent tests and maintain responsiveness.

#### Scenario: Concurrent testing of multiple providers
- **GIVEN** the user clicks test buttons on 3 different providers in quick succession
- **WHEN** all 3 tests are running simultaneously
- **THEN** the system SHALL:
  - Allow all 3 tests to run concurrently (async)
  - Display independent loading states on each card
  - Display independent results on each card when tests complete
  - NOT block the UI thread

#### Scenario: Test while editing form
- **GIVEN** a test is in progress (loading state)
- **WHEN** the user edits a form field
- **THEN** the system SHALL:
  - Allow the test to complete with the old values
  - Clear the test result when the form field changes
  - NOT cancel the in-flight test (let it finish silently)

#### Scenario: Rapid test button clicks (debouncing)
- **GIVEN** the test button is in idle state
- **WHEN** the user rapidly clicks the test button 5 times in 1 second
- **THEN** the system SHALL:
  - Trigger only ONE test (ignore subsequent clicks while test is running)
  - Keep button in loading state until test completes
  - NOT queue multiple tests

---

### Requirement: Visual Feedback and Animation

The test button SHALL provide smooth transitions between states.

#### Scenario: Transition to loading state
- **GIVEN** the user clicks the test button
- **WHEN** the button enters loading state
- **THEN** the transition SHALL:
  - Fade out the `network` icon (100ms duration)
  - Fade in the spinner (100ms duration)
  - Use ease-in-out timing function

#### Scenario: Test result fade-in
- **GIVEN** a test has completed
- **WHEN** the result is displayed below the card
- **THEN** the result SHALL:
  - Fade in from 0% to 100% opacity (150ms duration)
  - Slide down 4pt (subtle motion)

#### Scenario: Test result auto-hide animation
- **GIVEN** a test result has been displayed for 5 seconds
- **WHEN** the auto-hide timer triggers
- **THEN** the result SHALL:
  - Fade out from 100% to 0% opacity (200ms duration)
  - Slide up 4pt (reverse of fade-in)
  - Remove from layout after animation completes

---

### Requirement: Error Handling and Edge Cases

The test button SHALL handle various error scenarios gracefully.

#### Scenario: Network error during test
- **GIVEN** the device has no internet connection
- **WHEN** the user clicks the test button
- **THEN** the test SHALL:
  - Display error result: "❌ Network error: No internet connection"
  - Keep test button enabled for retry

#### Scenario: Invalid credentials
- **GIVEN** the user enters an invalid API key
- **WHEN** the test completes
- **THEN** the test SHALL:
  - Display error result: "❌ Authentication failed: Invalid API key"
  - Keep test button enabled for retry

#### Scenario: Test while save is in progress
- **GIVEN** the user has clicked Save (save operation is in progress)
- **AND** the test button is visible
- **WHEN** the user clicks the test button
- **THEN** the test button SHALL:
  - Be disabled during save operation
  - Return to enabled state after save completes
  - Use saved config (not working copy) for test after save

#### Scenario: Provider deletion while test is running
- **GIVEN** a custom provider test is in progress
- **WHEN** the user deletes the provider (e.g., via context menu)
- **THEN** the test SHALL:
  - Cancel silently (no result displayed)
  - Remove provider card from list
  - Clean up any pending test state
