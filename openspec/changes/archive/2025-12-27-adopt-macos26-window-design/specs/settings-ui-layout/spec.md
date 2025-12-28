## MODIFIED Requirements

### Requirement: Settings Window Dimensions
The Settings window SHALL provide sufficient space for rich content display with custom window styling.

#### Scenario: Minimum window size
- **GIVEN** the user opens Settings
- **WHEN** the window appears with custom styling (`.windowStyle(.hiddenTitleBar)`)
- **THEN** the window SHALL have a minimum frame size of 800x500 points (updated from 1200x800)
- **AND** the window SHALL have a default size of 1200x800 points
- **AND** the window SHALL be resizable by the user
- **AND** the window SHALL NOT have a native title bar (hidden via `.windowStyle(.hiddenTitleBar)`)

#### Scenario: Window positioning
- **GIVEN** the Settings window is opened for the first time
- **WHEN** no saved position exists
- **THEN** the window SHALL appear centered on the main screen
- **AND** subsequent opens SHALL restore the last user-positioned location

## ADDED Requirements

### Requirement: macOS 26 Window Design with Custom Traffic Lights
The Settings window SHALL adopt macOS 26 design language with integrated traffic light buttons in a rounded sidebar.

#### Scenario: Rounded sidebar with traffic lights
- **GIVEN** the Settings window is displayed
- **WHEN** the sidebar renders
- **THEN** the sidebar SHALL have a fixed width of 220pt
- **AND** the sidebar SHALL use a rounded rectangle background with 18pt corner radius (`.continuous` style)
- **AND** three custom traffic light buttons (red, yellow, green) SHALL be displayed at the top
- **AND** the buttons SHALL be positioned 14pt from the top and 18pt from the left edge
- **AND** the buttons SHALL be 13pt diameter circles with gradient fills

#### Scenario: Traffic light functionality
- **GIVEN** traffic light buttons are rendered
- **WHEN** the user clicks the red button
- **THEN** the window SHALL close (via `NSWindow.performClose(nil)`)
- **AND** when the user clicks the yellow button
- **THEN** the window SHALL minimize to Dock (via `NSWindow.miniaturize(nil)`)
- **AND** when the user clicks the green button
- **THEN** the window SHALL toggle fullscreen mode (via `NSWindow.toggleFullScreen(nil)`)
- **AND** when the user hovers over any button
- **THEN** an appropriate icon SHALL appear (xmark, minus, or fullscreen arrow)

#### Scenario: Horizontal split layout
- **GIVEN** the Settings window is opened
- **WHEN** the root view is rendered
- **THEN** the layout SHALL be an `HStack(spacing: 0)` containing:
  - Sidebar component (220pt fixed width)
  - Divider separator (1pt)
  - Content area (fills remaining space)
- **AND** the entire layout SHALL have `.background(.windowBackground)`
