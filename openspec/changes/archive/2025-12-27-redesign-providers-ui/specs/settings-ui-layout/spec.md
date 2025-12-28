# Specification: Settings UI Layout

## Purpose
Defines the visual layout, sizing, and proportions for the Settings window and Providers tab to match the reference design language.

## ADDED Requirements

### Requirement: Settings Window Dimensions
The Settings window SHALL provide sufficient space for rich content display.

#### Scenario: Minimum window size
- **GIVEN** the user opens Settings
- **WHEN** the window appears
- **THEN** the window SHALL have a minimum frame size of 1200x800 points
- **AND** the window SHALL be resizable by the user
- **AND** the window SHALL respect macOS standard title bar and toolbar heights

#### Scenario: Window positioning
- **GIVEN** the Settings window is opened for the first time
- **WHEN** no saved position exists
- **THEN** the window SHALL appear centered on the main screen
- **AND** subsequent opens SHALL restore the last user-positioned location

### Requirement: Providers Tab Layout Proportions
The Providers tab SHALL use a balanced two-panel layout.

#### Scenario: Left panel (provider list) width
- **GIVEN** the Providers tab is selected
- **WHEN** the window is at minimum size (1200x800)
- **THEN** the left panel (provider list) SHALL have:
- Minimum width: 450 points
- Ideal width: 550 points
- Maximum width: infinity (grows with window)
- **AND** the panel SHALL contain search bar, provider cards, and "Add Provider" button

#### Scenario: Right panel (edit panel) width
- **GIVEN** a provider is selected or "Add Provider" is clicked
- **WHEN** the edit panel is visible
- **THEN** the right panel SHALL have:
- Minimum width: 500 points
- Ideal width: 600 points
- Maximum width: infinity (grows with window)
- **AND** the panel SHALL be separated from left panel by a visible divider

#### Scenario: Responsive layout
- **GIVEN** the user resizes the Settings window
- **WHEN** the window width changes
- **THEN** both panels SHALL grow proportionally
- **AND** neither panel SHALL shrink below its minimum width
- **AND** content SHALL remain readable without horizontal scrolling (except for long URLs)

### Requirement: Vertical Spacing Consistency
All vertical spacing SHALL follow the design token system.

#### Scenario: Section spacing
- **GIVEN** any section in the Providers UI (header, cards, edit form)
- **WHEN** rendering vertical layouts
- **THEN** spacing between major sections SHALL use `DesignTokens.Spacing.lg` (16 points)
- **AND** spacing within sections SHALL use `DesignTokens.Spacing.md` (12 points)
- **AND** spacing between form fields SHALL use `DesignTokens.Spacing.sm` (8 points)

#### Scenario: Padding consistency
- **GIVEN** any container view (cards, panels, edit form)
- **WHEN** rendering content
- **THEN** container padding SHALL use `DesignTokens.Spacing.lg` for outer padding
- **AND** inner padding SHALL use `DesignTokens.Spacing.md` or `sm` based on hierarchy

### Requirement: ScrollView Behavior
ScrollViews SHALL handle overflow content gracefully.

#### Scenario: Provider list scrolling
- **GIVEN** more than 8 provider cards exist
- **WHEN** the provider list exceeds the visible area
- **THEN** the list SHALL scroll vertically with native macOS scrollbars
- **AND** the search bar and "Add Provider" button SHALL remain fixed at the top

#### Scenario: Edit panel scrolling
- **GIVEN** the edit form has many fields (Advanced Settings expanded)
- **WHEN** the form exceeds the visible area
- **THEN** the form content SHALL scroll vertically
- **AND** the action buttons (Test, Cancel, Save) SHALL remain visible at the bottom
- **AND** scrolling SHALL be smooth with momentum (native macOS behavior)

## Related Specs
- `macos-client`: Defines overall macOS UI requirements
- `provider-card-component`: Defines individual card sizing
- `provider-edit-panel`: Defines edit panel content structure
