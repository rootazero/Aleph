## ADDED Requirements

### Requirement: Manual Testing Coverage
The macOS client SHALL undergo comprehensive manual testing to verify all critical user flows.

#### Scenario: Application launch testing
- **WHEN** the application is launched on a clean macOS system
- **THEN** the app SHALL appear in the menu bar without a Dock icon, as configured by LSUIElement

#### Scenario: Menu bar functionality testing
- **WHEN** the user clicks the menu bar icon
- **THEN** the dropdown menu SHALL display all menu items (Settings, About, Quit) and respond correctly to user interaction

#### Scenario: Halo overlay appearance testing
- **WHEN** the global hotkey is triggered
- **THEN** the Halo overlay SHALL appear at the current cursor location within 100ms

#### Scenario: Focus protection testing
- **WHEN** the Halo overlay is displayed
- **THEN** the active application SHALL retain focus and the Halo window SHALL remain click-through

#### Scenario: Animation smoothness testing
- **WHEN** Halo state transitions occur (listening, processing, success, error)
- **THEN** all animations SHALL render at 60fps without visual jank

#### Scenario: Multi-monitor support testing
- **WHEN** the system has multiple displays configured
- **THEN** the Halo SHALL appear on the screen where the cursor is located, with position clamped to screen bounds

#### Scenario: Permission flow testing
- **WHEN** the app is launched without Accessibility permissions
- **THEN** a permission alert SHALL appear with clear instructions and a button to open System Settings

#### Scenario: Stability testing
- **WHEN** the app runs for 30 minutes with 50+ Halo triggers
- **THEN** the app SHALL remain stable without crashes, with consistent memory usage

### Requirement: Automated Test Infrastructure
The project SHALL include automated tests for Swift components where feasible.

#### Scenario: State machine unit tests
- **WHEN** Halo state transitions are tested
- **THEN** the state machine SHALL correctly handle all valid transitions and reject invalid ones

#### Scenario: Error callback testing
- **WHEN** the Rust core returns an error via callback
- **THEN** the Swift error handler SHALL log the error and display user-friendly feedback without crashing

### Requirement: Testing Documentation
The project SHALL document the complete manual testing procedure.

#### Scenario: Test checklist availability
- **WHEN** a developer needs to verify app functionality
- **THEN** the README SHALL provide a step-by-step testing checklist covering all critical paths

#### Scenario: Known issues documentation
- **WHEN** testing reveals limitations or bugs
- **THEN** the README SHALL document known issues with workarounds or planned fixes
