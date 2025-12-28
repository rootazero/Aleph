# settings-ui-design-system Specification Delta

## ADDED Requirements

### Requirement: Design Tokens
The design system SHALL provide centralized constants for all visual parameters.

#### Scenario: Color definitions
- **WHEN** developers need to apply colors to UI elements
- **THEN** DesignTokens.Colors SHALL provide semantic color definitions including:
  - sidebarBackground: System control background color
  - cardBackground: Semi-transparent background for cards
  - accentBlue: Primary accent color (RGB: 0, 0.48, 1.0)
  - providerActive: Green indicator for active providers
  - providerInactive: Gray indicator for inactive providers
  - textPrimary: Primary text color
  - textSecondary: Secondary/subdued text color
  - textDisabled: Disabled state text color

#### Scenario: Spacing system
- **WHEN** developers need to add padding or spacing between elements
- **THEN** DesignTokens.Spacing SHALL provide a consistent scale:
  - xs: 4pt (tight spacing)
  - sm: 8pt (compact spacing)
  - md: 16pt (standard spacing)
  - lg: 24pt (comfortable spacing)
  - xl: 32pt (loose spacing)

#### Scenario: Corner radius scale
- **WHEN** developers apply rounded corners to UI elements
- **THEN** DesignTokens.CornerRadius SHALL provide:
  - small: 6pt (buttons, chips)
  - medium: 10pt (cards, inputs)
  - large: 16pt (large containers)

#### Scenario: Typography scale
- **WHEN** developers need to apply text styles
- **THEN** DesignTokens.Typography SHALL provide font definitions:
  - title: 22pt semibold (page titles)
  - heading: 17pt medium (section headers)
  - body: 14pt regular (body text)
  - caption: 12pt regular (supporting text)
  - code: monospaced font for code snippets

#### Scenario: Shadow definitions
- **WHEN** developers need to add depth with shadows
- **THEN** DesignTokens.Shadows SHALL provide shadow parameters:
  - card: subtle shadow for card elevations (radius: 4, opacity: 0.1)
  - elevated: stronger shadow for modals (radius: 8, opacity: 0.15)
  - dropdown: shadow for dropdown menus (radius: 6, opacity: 0.12)

#### Scenario: System appearance adaptation
- **WHEN** macOS system appearance changes between Light and Dark mode
- **THEN** DesignTokens color values SHALL automatically adapt
- **AND** the adaptation SHALL occur without requiring app restart

### Requirement: Atomic Components
The design system SHALL provide fundamental UI building blocks (atoms).

#### Scenario: SearchBar component
- **WHEN** a view needs search functionality
- **THEN** SearchBar SHALL provide:
  - Magnifying glass icon (leading)
  - Text input field with @Binding to searchText
  - Placeholder text customization
  - Clear button (trailing, visible when text is non-empty)
  - DesignTokens-based styling

#### Scenario: StatusIndicator component
- **WHEN** displaying status information
- **THEN** StatusIndicator SHALL provide:
  - Circular colored indicator (8pt diameter)
  - Color variants: success (green), warning (yellow), error (red), inactive (gray)
  - Optional text label next to indicator
  - Optional pulsing animation for "in progress" states

#### Scenario: ActionButton component
- **WHEN** implementing action buttons
- **THEN** ActionButton SHALL provide style variants:
  - primary: blue background, white text (prominent actions)
  - secondary: gray border, no fill (secondary actions)
  - danger: red background, white text (destructive actions)
- **AND** SHALL support icon + text combinations
- **AND** SHALL support disabled state with reduced opacity

#### Scenario: VisualEffectBackground component
- **WHEN** applying blur effects to backgrounds
- **THEN** VisualEffectBackground SHALL:
  - Wrap NSVisualEffectView for SwiftUI integration
  - Support material types (sidebar, headerView, menu, etc.)
  - Support blending modes (behindWindow, withinWindow)
  - Automatically adapt to system appearance

#### Scenario: ThemeSwitcher component
- **WHEN** displaying the theme switcher in the window toolbar
- **THEN** ThemeSwitcher SHALL provide:
  - Three icon buttons in a horizontal group (sun/moon/half-circle icons)
  - @Binding to ThemeMode (light/dark/auto)
  - Visual indication of selected mode (accent color background on active button)
  - Unified background container with rounded corners and subtle border
  - Smooth transition animation when switching themes (0.2s duration)
- **AND** ThemeMode SHALL be an enum with cases: light, dark, auto
- **AND** each button SHALL be 32pt wide × 28pt tall
- **AND** the switcher SHALL be positioned in window toolbar trailing area

### Requirement: Molecular Components
The design system SHALL provide composite UI components built from atoms.

#### Scenario: ProviderCard component
- **WHEN** displaying a provider in the list
- **THEN** ProviderCard SHALL show:
  - Provider icon (SF Symbol or custom image)
  - Provider name (Typography.heading)
  - Provider type badge (e.g., "OpenAI", "Claude")
  - Status indicator (online/offline/configured)
  - Brief description (Typography.caption)
- **AND** SHALL have visual states:
  - Default: medium corner radius, card shadow
  - Hover: scale 1.02, enhanced shadow
  - Selected: 2pt blue border, blue-tinted background
- **AND** SHALL support tap gesture to select
- **AND** SHALL support context menu (right-click) for actions

#### Scenario: SidebarItem component
- **WHEN** rendering navigation sidebar items
- **THEN** SidebarItem SHALL display:
  - SF Symbol icon (leading, 16pt)
  - Text label (Typography.body)
  - Selection indicator (filled background with small corner radius)
- **AND** SHALL have visual states:
  - Unselected: transparent background, gray icon
  - Selected: tinted background, accent-colored icon
  - Hover: light background tint

#### Scenario: DetailPanel component
- **WHEN** showing detailed information for selected items
- **THEN** DetailPanel SHALL provide:
  - Section header with title (Typography.heading)
  - Collapsible sections (chevron indicator)
  - Content area with proper padding (Spacing.md)
  - Action buttons at bottom (ActionButton components)
  - Visual separation between sections (dividers)

### Requirement: Layout Guidelines
The design system SHALL define standard layout patterns for consistency.

#### Scenario: Card layout pattern
- **WHEN** creating card-based UI
- **THEN** the layout SHALL use:
  - CornerRadius.medium for rounded corners
  - Spacing.md for internal padding
  - Spacing.sm to Spacing.md for spacing between cards
  - Shadows.card for depth
  - cardBackground color for fill

#### Scenario: Form layout pattern
- **WHEN** creating settings forms
- **THEN** the layout SHALL use:
  - Spacing.lg between form sections
  - Spacing.sm between label and input
  - Spacing.xs between related inputs (grouped fields)
  - Left-aligned labels (Typography.body)
  - Full-width inputs with CornerRadius.small

#### Scenario: List layout pattern
- **WHEN** displaying scrollable lists
- **THEN** the layout SHALL use:
  - LazyVStack for performance (not VStack)
  - Spacing.md between list items
  - ScrollView with edge padding (Spacing.md)
  - Dividers only when content is not card-based

### Requirement: Animation Standards
The design system SHALL define consistent animation behaviors.

#### Scenario: Button interactions
- **WHEN** user presses a button
- **THEN** it SHALL scale to 0.95 using spring animation:
  - response: 0.3 seconds
  - dampingFraction: 0.6

#### Scenario: Card hover effects
- **WHEN** user hovers over a card
- **THEN** it SHALL:
  - Scale to 1.02 using easeInOut animation (0.2s duration)
  - Enhance shadow from radius 4 to radius 8
  - Transition smoothly when hover ends

#### Scenario: Panel transitions
- **WHEN** detail panel appears or disappears
- **THEN** it SHALL animate using:
  - Combined opacity (0 to 1) and offset (20pt to 0)
  - Spring animation with response: 0.4s, dampingFraction: 0.8

#### Scenario: Search filtering
- **WHEN** search results update
- **THEN** filtered items SHALL:
  - Fade in/out with 0.2s duration
  - Use matched geometry effect for smooth reordering (if applicable)

### Requirement: Accessibility Compliance
The design system SHALL ensure accessibility for all users.

#### Scenario: Color contrast ratios
- **WHEN** displaying text on backgrounds
- **THEN** the contrast ratio SHALL meet WCAG 2.1 AA standards:
  - Normal text: minimum 4.5:1
  - Large text (18pt+): minimum 3:1
- **AND** DesignTokens SHALL provide pre-validated color pairs

#### Scenario: VoiceOver support
- **WHEN** users navigate with VoiceOver enabled
- **THEN** all interactive components SHALL:
  - Provide descriptive accessibility labels
  - Announce state changes (selected, expanded, etc.)
  - Support standard VoiceOver gestures

#### Scenario: Keyboard navigation
- **WHEN** users navigate with keyboard only
- **THEN** all interactive elements SHALL:
  - Be reachable via Tab key
  - Show focus indicators (blue outline)
  - Support standard keyboard shortcuts (Enter to activate, Esc to dismiss)

#### Scenario: Reduced motion support
- **WHEN** user enables "Reduce Motion" in system preferences
- **THEN** the UI SHALL:
  - Replace spring/bounce animations with simple fades
  - Reduce animation durations by 50%
  - Disable hover scale effects

### Requirement: Component Documentation
All design system components SHALL be documented for developer use.

#### Scenario: ThemeManager service
- **WHEN** the application initializes
- **THEN** ThemeManager SHALL:
  - Load saved theme preference from UserDefaults (key: "app_theme")
  - Default to "auto" mode if no preference is saved
  - Apply the selected theme by setting NSApp.appearance
  - Observe system appearance changes when in auto mode
  - Persist theme changes to UserDefaults when user switches modes
- **AND** ThemeManager SHALL be an ObservableObject with @Published currentTheme property

#### Scenario: Component file structure
- **WHEN** developers browse the Components directory
- **THEN** it SHALL be organized as:
  - Components/Atoms/ (SearchBar, StatusIndicator, ActionButton, etc.)
  - Components/Molecules/ (ProviderCard, SidebarItem, DetailPanel, etc.)
  - Components/Organisms/ (ModernSidebar, ProviderListView, etc.)

#### Scenario: Preview providers
- **WHEN** developers work on components in Xcode
- **THEN** each component file SHALL include PreviewProvider showing:
  - Default state
  - All visual variants (primary/secondary, light/dark)
  - Different sizes (if applicable)
  - Interactive states (hover, pressed, disabled)

#### Scenario: Code documentation
- **WHEN** developers reference component APIs
- **THEN** each component SHALL have:
  - Documentation comment describing purpose
  - @Parameter annotations for all public properties
  - Usage example in comments or README

#### Scenario: Design guide document
- **WHEN** developers need to understand design decisions
- **THEN** docs/ui-design-guide.md SHALL provide:
  - Visual reference (uisample.png)
  - Design token usage examples
  - Layout pattern examples
  - Animation guidelines
  - When to create new components vs. reuse existing ones
