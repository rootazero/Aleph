## ADDED Requirements

### Requirement: Window Style Customization
The Settings window SHALL support custom window styling to enable macOS 26 design language integration.

#### Scenario: Hidden title bar style
- **WHEN** the Settings window is opened
- **THEN** the window SHALL use `.windowStyle(.hiddenTitleBar)` to hide the native title bar
- **AND** the window SHALL use `.windowToolbarStyle(.unifiedCompact)` to align content to the window top edge
- **AND** the window SHALL maintain macOS 13+ compatibility

#### Scenario: Window dimensions
- **WHEN** the Settings window is displayed
- **THEN** the window SHALL have a minimum frame size of 800x500 points
- **AND** the window SHALL have a default size of 1200x800 points
- **AND** the window SHALL be resizable by the user
- **AND** the window SHALL restore the last user-positioned location on subsequent opens

### Requirement: Custom Traffic Light Buttons
The Settings window SHALL implement custom traffic light buttons (red, yellow, green) integrated into the sidebar.

#### Scenario: Traffic light visual design
- **WHEN** the custom traffic light buttons are rendered
- **THEN** each button SHALL be a circular shape with 13pt diameter (matching native size)
- **AND** the buttons SHALL use gradient fills (`.fill(color.gradient)`)
- **AND** the buttons SHALL have 8pt spacing between them
- **AND** the button colors SHALL be:
  - Red: close window
  - Yellow: minimize window
  - Green: toggle fullscreen

#### Scenario: Traffic light hover interaction
- **WHEN** the user hovers over a traffic light button
- **THEN** the button SHALL display the appropriate symbol icon:
  - Red button: `xmark` symbol
  - Yellow button: `minus` symbol
  - Green button: `arrow.up.left.and.arrow.down.right` symbol
- **AND** the icon SHALL use `.font(.system(size: 7, weight: .bold))`
- **AND** the icon SHALL use `.foregroundStyle(.black.opacity(0.7))` color

#### Scenario: Traffic light button actions
- **WHEN** the user clicks the red traffic light button
- **THEN** the window SHALL close (equivalent to `NSWindow.performClose(nil)`)
- **AND** when the user clicks the yellow traffic light button
- **THEN** the window SHALL minimize to Dock (equivalent to `NSWindow.miniaturize(nil)`)
- **AND** when the user clicks the green traffic light button
- **THEN** the window SHALL toggle fullscreen mode (equivalent to `NSWindow.toggleFullScreen(nil)`)

### Requirement: AppKit Window Control Bridge
The macOS client SHALL provide a bridge between SwiftUI and AppKit window control APIs.

#### Scenario: WindowController singleton
- **WHEN** the application initializes
- **THEN** a `WindowController` singleton SHALL be available
- **AND** the controller SHALL provide three methods:
  - `close()` - closes the key window
  - `minimize()` - minimizes the key window
  - `toggleFullscreen()` - toggles fullscreen for the key window
- **AND** the controller SHALL use `NSApp.keyWindow` to retrieve the current active window

#### Scenario: Window control error handling
- **WHEN** a window control method is called and no key window exists
- **THEN** the method SHALL fail silently (no crash or error alert)
- **AND** the method SHALL log a debug message indicating the operation was skipped

### Requirement: Sidebar with Integrated Traffic Lights
The Settings window SHALL display a rounded sidebar containing navigation items and traffic light buttons.

#### Scenario: Sidebar dimensions and styling
- **WHEN** the sidebar is rendered
- **THEN** the sidebar SHALL have a fixed width of 220pt
- **AND** the sidebar SHALL use a `RoundedRectangle(cornerRadius: 18, style: .continuous)` background
- **AND** the background SHALL have `padding(.leading: 8)` and `padding(.vertical: 8)`
- **AND** the sidebar SHALL have a subtle border using `.strokeBorder(.separator.opacity(0.25))`

#### Scenario: Sidebar background color
- **WHEN** the sidebar is rendered in Dark Mode
- **THEN** the sidebar background SHALL use `windowBackgroundColor.opacity(0.9)`
- **AND** when rendered in Light Mode
- **THEN** the sidebar background SHALL use `underPageBackgroundColor`

#### Scenario: Traffic lights positioning in sidebar
- **WHEN** the sidebar renders the traffic light buttons
- **THEN** the buttons SHALL be positioned at the top of the sidebar
- **AND** the buttons SHALL have `padding(.top: 14)` from the sidebar top edge
- **AND** the buttons SHALL have `padding(.leading: 18)` from the sidebar left edge
- **AND** the buttons SHALL be arranged in a horizontal row (HStack with 8pt spacing)

#### Scenario: Sidebar navigation items
- **WHEN** the sidebar displays navigation items
- **THEN** navigation items SHALL appear below the traffic lights with 12pt vertical spacing
- **AND** each navigation item SHALL display an SF Symbol icon and text label
- **AND** navigation items SHALL support selection state with visual feedback

### Requirement: Root Window Layout Structure
The Settings window SHALL use a two-panel layout with a sidebar and content area.

#### Scenario: Root layout composition
- **WHEN** the Settings window root view is rendered
- **THEN** the layout SHALL be an `HStack(spacing: 0)` containing:
  - Left: `SidebarWithTrafficLights` component (220pt)
  - Middle: `Divider` separator (1pt)
  - Right: `MainContentView` component (fills remaining space)
- **AND** the entire layout SHALL have `.background(.windowBackground)`

#### Scenario: Content area behavior
- **WHEN** the content area is rendered
- **THEN** the content area SHALL display the selected settings tab content
- **AND** the content SHALL be scrollable independently of the sidebar
- **AND** the content SHALL use `.frame(maxWidth: .infinity, maxHeight: .infinity)` to fill available space

## MODIFIED Requirements

### Requirement: Development Documentation
The macOS client SHALL include comprehensive documentation for developers, including the new window design implementation.

#### Scenario: Build instructions
- **WHEN** a new developer sets up the project
- **THEN** the README SHALL provide step-by-step instructions for building both the Rust core and Swift client

#### Scenario: System requirements documentation
- **WHEN** a developer reviews the README
- **THEN** it SHALL clearly specify minimum versions for macOS (13+), Xcode (15+), and Rust toolchain

#### Scenario: Architecture overview
- **WHEN** a developer needs to understand the system design
- **THEN** the README SHALL include a diagram or description of the Rust Core ↔ UniFFI ↔ Swift communication flow
- **AND** the README SHALL document the custom window design architecture (WindowGroup, traffic lights, sidebar integration)

#### Scenario: Window design implementation notes
- **WHEN** a developer reviews the window implementation
- **THEN** the documentation SHALL explain why `Settings` Scene was replaced with `WindowGroup`
- **AND** the documentation SHALL explain the AppKit bridge pattern for window controls
- **AND** the documentation SHALL provide guidance on testing across macOS versions (13-26)
