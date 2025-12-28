# settings-ui-layout Spec Delta

## ADDED Requirements

### Requirement: Provider List Toolbar
The provider list section SHALL include a toolbar at the top for quick actions and search.

#### Scenario: Toolbar layout and positioning
- **GIVEN** the Providers tab is displayed
- **WHEN** the left panel renders
- **THEN** a toolbar SHALL appear at the top of the provider list section
- **AND** the toolbar SHALL span the full width of the left panel
- **AND** the toolbar SHALL contain an HStack with:
  - "Add Custom Provider" button (left-aligned)
  - `Spacer()` to separate elements
  - Search bar (right-aligned)
- **AND** the toolbar SHALL use `DesignTokens.Spacing.md` for internal padding

#### Scenario: Add Custom Provider button appearance
- **GIVEN** the toolbar is rendered
- **WHEN** the "Add Custom Provider" button displays
- **THEN** the button SHALL use an icon (e.g., `plus.circle` or `plus.square.on.square`)
- **AND** the button SHALL have clear text label "Add Custom Provider"
- **AND** the button SHALL use ActionButton component style for consistency
- **AND** the button SHALL be visually prominent but not dominate the toolbar

#### Scenario: Add Custom Provider button interaction
- **GIVEN** the user clicks "Add Custom Provider" button
- **WHEN** the button action executes
- **THEN** the system SHALL:
  1. Clear any current provider selection
  2. Set `selectedPreset` to the "custom" preset provider
  3. Set `isAddingNew` to `true`
  4. Display an empty custom provider edit form in the right panel
- **AND** the form SHALL show editable fields for:
  - Provider Name (required, empty)
  - Theme Color (required, default gray)
  - Base URL (required, empty)
  - API Key (required, empty)
  - Model (required, empty)
  - Generation parameters (optional)

#### Scenario: Search bar integration in toolbar
- **GIVEN** the toolbar is rendered
- **WHEN** the search bar component displays
- **THEN** the search bar SHALL maintain its current SearchBar component implementation
- **AND** the search bar SHALL be right-aligned within the toolbar
- **AND** the search bar SHALL have a reasonable max width (e.g., 200-250 points)
- **AND** search functionality SHALL remain unchanged from current behavior

### Requirement: Visual Container Styling
The provider list and edit panel SHALL have visual container boundaries for clear functional separation.

#### Scenario: Provider list container styling
- **GIVEN** the left panel (provider list section) is rendered
- **WHEN** the container view is displayed
- **THEN** the container SHALL have:
  - Corner radius: `DesignTokens.CornerRadius.medium` (10pt)
  - Background color: `DesignTokens.Colors.sidebarBackground` (current)
  - No shadow effect
  - No border stroke
- **AND** the container SHALL wrap:
  - Toolbar (top)
  - Provider cards list (scrollable area)
- **AND** the container SHALL use `DesignTokens.Spacing.md` for outer padding

#### Scenario: Edit panel container styling
- **GIVEN** the right panel (edit panel) is rendered
- **WHEN** the container view is displayed
- **THEN** the container SHALL have:
  - Corner radius: `DesignTokens.CornerRadius.medium` (10pt)
  - Background color: `DesignTokens.Colors.contentBackground`
  - No shadow effect
  - No border stroke
- **AND** the container SHALL wrap the entire ProviderEditPanel content
- **AND** the container SHALL use `DesignTokens.Spacing.md` for outer padding

#### Scenario: Container spacing and layout
- **GIVEN** both left and right containers are rendered
- **WHEN** the Providers tab displays
- **THEN** the containers SHALL have:
  - Outer padding: `DesignTokens.Spacing.lg` from the window edges
  - Gap between containers: `DesignTokens.Spacing.md`
  - Divider between containers (current behavior preserved)
- **AND** containers SHALL grow proportionally with window resize
- **AND** minimum widths SHALL be preserved (left: 450pt, right: 500pt)

### Requirement: Custom Provider List Integration
Custom provider instances SHALL be displayed alongside preset providers in a unified list.

#### Scenario: Unified provider list display
- **GIVEN** the user has configured custom provider instances
- **WHEN** the provider list renders
- **THEN** the list SHALL display:
  - All preset providers (OpenAI, Anthropic, Gemini, Ollama, etc.)
  - All configured custom provider instances
  - "Custom (OpenAI-compatible)" preset option
- **AND** custom providers SHALL use SimpleProviderCard component
- **AND** custom providers SHALL display user-defined name and color
- **AND** sorting SHALL be consistent (e.g., alphabetical or by type)

#### Scenario: Custom provider configuration status
- **GIVEN** a custom provider instance exists in the list
- **WHEN** the provider card renders
- **THEN** the card SHALL show:
  - User-defined provider name
  - User-defined theme color (icon background)
  - "Custom" or "OpenAI-compatible" type label
  - Configured status indicator (same as preset providers)
- **AND** clicking the card SHALL load its configuration in the edit panel
- **AND** the card SHALL be selectable/highlightable like preset cards

#### Scenario: Multiple custom provider instances
- **GIVEN** the user has created multiple custom providers (e.g., "Company API", "Local LLM", "Proxy")
- **WHEN** the provider list renders
- **THEN** each custom instance SHALL appear as a separate card
- **AND** each SHALL be independently selectable and editable
- **AND** each SHALL have its own unique name and visual identity
- **AND** deleting one custom instance SHALL NOT affect others

## MODIFIED Requirements

### Requirement: Providers Tab Layout Proportions
The Providers tab SHALL use a balanced two-panel layout with visual container styling.

#### Scenario: Left panel (provider list) dimensions
- **GIVEN** the Providers tab is selected
- **WHEN** the window is at minimum size (1200x800)
- **THEN** the left panel (provider list) SHALL have:
  - Minimum width: 450 points
  - Ideal width: 550 points
  - Maximum width: infinity (grows with window)
- **AND** the panel SHALL contain:
  - **Toolbar** with "Add Custom Provider" button and search bar (NEW)
  - Provider cards list (scrollable)
- **AND** the panel SHALL be wrapped in a container with:
  - Corner radius: `DesignTokens.CornerRadius.medium` (NEW)
  - Background: `DesignTokens.Colors.sidebarBackground`

#### Scenario: Right panel (edit panel) dimensions
- **GIVEN** a provider is selected or "Add Custom Provider" is clicked
- **WHEN** the edit panel is visible
- **THEN** the right panel SHALL have:
  - Minimum width: 500 points
  - Ideal width: 600 points
  - Maximum width: infinity (grows with window)
- **AND** the panel SHALL be wrapped in a container with:
  - Corner radius: `DesignTokens.CornerRadius.medium` (NEW)
  - Background: `DesignTokens.Colors.contentBackground` (NEW)
- **AND** the panel SHALL be separated from left panel by a visible divider (PRESERVED)

### Requirement: ScrollView Behavior
ScrollViews SHALL handle overflow content gracefully with the new toolbar layout.

#### Scenario: Provider list scrolling with toolbar
- **GIVEN** more than 8 provider cards exist
- **WHEN** the provider list exceeds the visible area
- **THEN** the list SHALL scroll vertically with native macOS scrollbars
- **AND** the **toolbar** (with "Add Custom Provider" button and search bar) SHALL remain fixed at the top (UPDATED)
- **AND** only the provider cards area SHALL scroll
- **AND** scrolling SHALL be smooth with momentum (native macOS behavior)

#### Scenario: Edit panel scrolling (unchanged)
- **GIVEN** the edit form has many fields (Advanced Settings expanded)
- **WHEN** the form exceeds the visible area
- **THEN** the form content SHALL scroll vertically
- **AND** the action buttons (Test, Save) SHALL remain visible at the bottom (footer behavior)
- **AND** scrolling SHALL be smooth with momentum (native macOS behavior)
