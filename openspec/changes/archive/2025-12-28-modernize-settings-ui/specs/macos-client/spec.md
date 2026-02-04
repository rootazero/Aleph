# macos-client Specification Delta

## MODIFIED Requirements

### Requirement: Settings Window UI
The settings window SHALL provide a modern, native macOS interface for configuring Aleph.

#### Scenario: Modern visual design
- **WHEN** the user opens the settings window
- **THEN** the UI SHALL follow contemporary macOS design patterns including:
  - Dark theme support with system appearance integration
  - Card-based layouts with subtle shadows and rounded corners
  - Blur effects for backgrounds (NSVisualEffectView)
  - Proper visual hierarchy using consistent spacing and typography

#### Scenario: Three-column layout
- **WHEN** the settings window is displayed at ideal size (1200x800)
- **THEN** it SHALL show a three-column layout:
  - Left sidebar (200pt fixed width) with navigation items
  - Center content area (flexible width) displaying selected settings
  - Right detail panel (350pt ideal width) showing contextual information for selected items

#### Scenario: Responsive layout
- **WHEN** the user resizes the settings window below 1000pt width
- **THEN** the detail panel SHALL automatically collapse/hide to maintain usability
- **AND** the minimum window size SHALL be 800x600

#### Scenario: Sidebar navigation
- **WHEN** the user views the settings sidebar
- **THEN** it SHALL display navigation items with:
  - SF Symbol icons for each section (General, Providers, Routing, etc.)
  - Clear selected state visual feedback (background highlight + icon color change)
  - Hover states for better interactivity
  - Bottom action area with Import/Export/Reset buttons

#### Scenario: Provider list display
- **WHEN** the user navigates to the Providers tab
- **THEN** providers SHALL be displayed as cards showing:
  - Provider icon and name
  - Provider type label (OpenAI/Claude/Ollama/etc.)
  - Status indicator (online/offline/configured)
  - Brief description
- **AND** cards SHALL have hover effects (scale + shadow)
- **AND** selected card SHALL show a highlight border

#### Scenario: Search functionality
- **WHEN** the user types in the search bar on Providers tab
- **THEN** the provider list SHALL filter in real-time
- **AND** the search SHALL match against provider name and type
- **AND** response time SHALL be under 50ms

#### Scenario: Detail panel information
- **WHEN** the user selects a provider card
- **THEN** the right detail panel SHALL display:
  - Provider name and status
  - Full configuration details (API endpoint, model, etc.)
  - Usage examples (e.g., "Use with Claude Code" environment variables)
  - Action buttons (Edit, Delete, Test Connection, Copy Config)

#### Scenario: Theme consistency
- **WHEN** the user selects a theme mode using the theme switcher
- **THEN** all settings UI SHALL apply the selected theme (Light/Dark/Auto)
- **AND** if Auto mode is selected, the UI SHALL follow macOS system appearance
- **AND** the theme preference SHALL persist across app restarts (saved to UserDefaults)

#### Scenario: Theme switcher visibility
- **WHEN** the user opens the settings window
- **THEN** a theme switcher SHALL be visible in the top-right corner of the window
- **AND** it SHALL display three icon buttons (sun/moon/half-circle) representing Light/Dark/Auto modes
- **AND** the currently selected mode SHALL be visually highlighted with accent color background

#### Scenario: Theme switching interaction
- **WHEN** the user clicks a theme button in the switcher
- **THEN** the application appearance SHALL immediately update to the selected theme
- **AND** the transition SHALL be smooth without flickering or layout jumps
- **AND** all UI elements (cards, backgrounds, text) SHALL adapt to the new theme

#### Scenario: Performance requirements
- **WHEN** the settings window is rendered and interacted with
- **THEN** the frame rate SHALL maintain 60fps during animations
- **AND** search filtering SHALL complete within 50ms
- **AND** window opening time SHALL be under 500ms
- **AND** memory usage SHALL not increase by more than 10MB compared to legacy UI

## ADDED Requirements

### Requirement: Design Token System
The UI SHALL use a centralized design token system for visual consistency.

#### Scenario: Centralized design constants
- **WHEN** developers implement new UI components
- **THEN** they SHALL use DesignTokens.swift for all visual parameters including:
  - Colors (backgrounds, accents, status indicators, text colors)
  - Spacing (xs: 4pt, sm: 8pt, md: 16pt, lg: 24pt, xl: 32pt)
  - Corner radii (small: 6pt, medium: 10pt, large: 16pt)
  - Typography (title, heading, body, caption font definitions)
  - Shadows (card, elevated, dropdown shadow parameters)

#### Scenario: Theme adaptation
- **WHEN** the system appearance changes
- **THEN** DesignTokens SHALL automatically provide appropriate color values
- **AND** NO hard-coded color values SHALL exist outside of DesignTokens

### Requirement: Reusable UI Components
The settings UI SHALL be built from reusable SwiftUI components organized in atomic design hierarchy.

#### Scenario: Atomic components
- **WHEN** building the settings UI
- **THEN** it SHALL use atomic components (Atoms) including:
  - SearchBar: search icon + text field + clear button
  - StatusIndicator: colored circle with optional label
  - ActionButton: primary/secondary/danger button styles with icon support
  - VisualEffectBackground: NSVisualEffectView wrapper for blur effects

#### Scenario: Molecular components
- **WHEN** building complex UI sections
- **THEN** it SHALL use molecular components (Molecules) including:
  - ProviderCard: complete provider card layout with icon, text, status, actions
  - SidebarItem: navigation item with icon, text, selection state
  - DetailPanel: contextual information panel for selected items

#### Scenario: Component reusability
- **WHEN** implementing new settings tabs (Routing, Memory, etc.)
- **THEN** developers SHALL reuse existing atomic and molecular components
- **AND** component code SHALL not exceed 200 lines per file
- **AND** each component SHALL include PreviewProvider for visual testing

### Requirement: Enhanced Interactions
The settings UI SHALL provide smooth, responsive interactions with visual feedback.

#### Scenario: Micro-interactions
- **WHEN** the user interacts with UI elements
- **THEN** the system SHALL provide visual feedback including:
  - Button press: scale to 0.95 with spring animation
  - Card hover: scale to 1.02 with shadow enhancement
  - Sidebar selection: smooth background slide animation
  - Panel transitions: fade + offset animations

#### Scenario: Loading states
- **WHEN** the UI is loading data or performing async operations
- **THEN** it SHALL display appropriate loading indicators:
  - Skeleton loading for provider list
  - Spinning icon for test connection button
  - Toast notification for successful save operations
  - Shake animation for error states

#### Scenario: Keyboard shortcuts
- **WHEN** the settings window is active
- **THEN** it SHALL support keyboard navigation:
  - Tab key to navigate between controls
  - Cmd+F to focus search bar (if applicable)
  - Cmd+W to close window
  - Arrow keys to navigate sidebar items

### Requirement: Import/Export/Reset Settings
The settings window SHALL provide operations for managing configurations.

#### Scenario: Export settings
- **WHEN** the user clicks "Export Settings" button
- **THEN** the system SHALL:
  - Open a save file dialog
  - Export current configuration to a JSON file
  - Include providers, routing rules, and behavior settings
  - Show success notification

#### Scenario: Import settings
- **WHEN** the user clicks "Import Settings" button
- **THEN** the system SHALL:
  - Open a file picker dialog
  - Validate the selected JSON file format
  - Merge or replace current settings based on user choice
  - Show error message if file is invalid
  - Reload UI to reflect imported settings

#### Scenario: Reset settings
- **WHEN** the user clicks "Reset Settings" button
- **THEN** the system SHALL:
  - Display a confirmation dialog warning data loss
  - Upon confirmation, restore default configuration
  - Update config.toml with defaults
  - Reload UI to show default values
