# Specification: Action Buttons Layout

## Purpose
Defines the positioning, styling, and behavior of action buttons (Test Connection, Cancel, Save) in the provider edit panel, matching the bottom-right placement from the reference design.

## ADDED Requirements

### Requirement: Button Positioning
Action buttons SHALL be anchored to the bottom-right corner of the edit panel.

#### Scenario: Edit mode button layout
- **GIVEN** a provider is in edit mode
- **WHEN** the edit panel is rendered
- **THEN** the action buttons SHALL be positioned:
- In the bottom-right corner of the panel
- With 16pt margin from right edge (`DesignTokens.Spacing.lg`)
- With 16pt margin from bottom edge
- Arranged horizontally in the order: [Test Connection] [Cancel] [Save]
- **AND** buttons SHALL remain visible when form content scrolls

#### Scenario: View mode button layout
- **GIVEN** a provider is in view mode (not editing)
- **WHEN** the detail panel is rendered
- **THEN** the action buttons SHALL be:
- "Edit Configuration" (primary button)
- "Delete Provider" (danger button)
- Positioned at the bottom of the panel
- Arranged vertically with 8pt spacing
- **AND** buttons SHALL be full-width (stretch to panel width minus padding)

### Requirement: Button Hierarchy
Buttons SHALL follow a clear visual hierarchy based on importance.

#### Scenario: Save button prominence
- **GIVEN** the edit panel is in edit mode
- **WHEN** rendering action buttons
- **THEN** the "Save" button SHALL:
- Use primary style (`ActionButton.primary`)
- Background color: `DesignTokens.Colors.accentBlue`
- Be positioned as the rightmost button
- Have white text color
- Be the default action (Enter key triggers it when form is valid)

#### Scenario: Cancel button secondary style
- **GIVEN** the edit panel is in edit mode
- **WHEN** rendering the "Cancel" button
- **THEN** it SHALL:
- Use secondary style (`ActionButton.secondary`)
- Background color: Transparent or light gray
- Border: 1pt solid `DesignTokens.Colors.border`
- Be positioned between "Test Connection" and "Save"
- Trigger escape/cancel action (Esc key)

#### Scenario: Test Connection button placement
- **GIVEN** the edit panel is in edit mode
- **WHEN** rendering the "Test Connection" button
- **THEN** it SHALL:
- Use secondary style (`ActionButton.secondary`)
- Be positioned to the left of "Cancel" button
- Have adequate spacing (12pt) from Cancel button
- Show inline result below (not to the right)

### Requirement: Button States
Buttons SHALL have distinct visual states for enabled, disabled, and loading.

#### Scenario: Disabled state styling
- **GIVEN** a form field is invalid (e.g., empty API key)
- **WHEN** rendering the "Save" or "Test Connection" button
- **THEN** the button SHALL:
- Opacity: 0.5
- Cursor: not-allowed (if applicable in SwiftUI)
- Be non-interactive (clicks have no effect)
- Background color: Grayed out version of normal color

#### Scenario: Loading state during save
- **GIVEN** the user clicks "Save"
- **WHEN** the save operation is in progress
- **THEN** the "Save" button SHALL:
- Text changes to "Saving..."
- Show a small spinner (ProgressView) inside the button
- Be disabled (prevent double-click)
- **AND** "Cancel" button SHALL remain enabled (allow cancel)

#### Scenario: Loading state during test
- **GIVEN** the user clicks "Test Connection"
- **WHEN** the test is in progress
- **THEN** the "Test Connection" button SHALL:
- Text changes to "Testing..."
- Show spinner inside button OR inline below
- Be disabled until test completes
- **AND** "Save" button SHALL remain enabled (user can save without testing)

### Requirement: Button Spacing and Sizing
Buttons SHALL have consistent dimensions and spacing.

#### Scenario: Horizontal spacing between buttons
- **GIVEN** buttons are arranged horizontally (edit mode)
- **WHEN** rendering the button row
- **THEN** spacing between buttons SHALL be:
- 12pt between "Test Connection" and "Cancel" (`DesignTokens.Spacing.md`)
- 8pt between "Cancel" and "Save" (`DesignTokens.Spacing.sm`)
- **AND** buttons SHALL have equal height (36pt minimum)

#### Scenario: Button width constraints
- **GIVEN** any action button in edit mode
- **WHEN** rendering
- **THEN** the button SHALL:
- Have minimum width: 100pt (accommodate "Test Connection" text)
- Have padding: 16pt horizontal, 8pt vertical
- Auto-size to content (not fixed width)
- **AND** icon (if present) SHALL be 16pt with 6pt trailing spacing

### Requirement: Fixed Button Bar
Action buttons SHALL remain visible when form content scrolls.

#### Scenario: Scrollable form with fixed footer
- **GIVEN** the edit form has many fields (Advanced Settings expanded)
- **WHEN** the user scrolls the form content
- **THEN** the action buttons SHALL:
- Remain fixed at the bottom of the panel (not scroll with content)
- Be separated from form content by a subtle divider line
- Have a semi-transparent background to overlay scrolling content
- OR be positioned outside the ScrollView in a fixed footer

#### Scenario: Button visibility on small screens
- **GIVEN** the Settings window is at minimum size (1200x800)
- **WHEN** the edit panel is displayed
- **THEN** the action buttons SHALL:
- Always be visible (not cut off)
- NOT require horizontal scrolling to reach
- Have adequate touch target size (44x44pt minimum on macOS)

## Related Specs
- `settings-ui-layout`: Defines overall panel dimensions
- `connection-test-inline`: Defines test result display below "Test Connection" button
- `provider-active-state`: Defines when "Save" button should be enabled
