# Spec Delta: Halo Visual System

## Capability
`halo-visual-system`

## REMOVED Requirements

### Requirement: Multi-theme support
The system SHALL NOT support multiple Halo themes (Cyberpunk, Zen, Jarvis). Theme system adds complexity without user value. Single unified style is sufficient.

#### Scenario: Theme selection in settings
- **GIVEN** the user is in Settings
- **WHEN** they navigate to the Appearance section
- **THEN** there is no Halo theme selector (Cyberpunk, Zen, Jarvis options removed)

#### Scenario: Theme persistence
- **GIVEN** the app restarts
- **WHEN** Halo is shown
- **THEN** it uses the unified visual style (no theme preference loaded)

---

### Requirement: Success state visual feedback
The system SHALL NOT display a success icon after AI response completion. Success is already visible through the pasted result or SubPanel output.

#### Scenario: Single-turn completion
- **GIVEN** user is in single-turn mode
- **WHEN** AI response is received and pasted
- **THEN** Halo transitions directly to idle state (no success checkmark shown)

#### Scenario: Multi-turn completion
- **GIVEN** user is in multi-turn conversation mode
- **WHEN** AI response is received
- **THEN** response appears in SubPanel CLI output (no success checkmark shown)

---

## ADDED Requirements

### Requirement: Unified Halo visual style
The system MUST use a single, unified visual style for all Halo states. This simplifies codebase while maintaining clear visual feedback.

#### Scenario: Processing indicator appearance
- **GIVEN** AI is processing a request
- **WHEN** processing indicator is displayed
- **THEN** it shows a minimal rotating spinner (16x16 px, purple color)

#### Scenario: Listening state appearance
- **GIVEN** Halo is in listening state
- **WHEN** displayed
- **THEN** it shows a simple pulsing circle (purple color)

#### Scenario: Error state appearance
- **GIVEN** an error occurs during processing
- **WHEN** Halo displays the error
- **THEN** it shows error icon with action buttons (Retry, Open Settings, Dismiss)

---

### Requirement: Smart position tracking for processing indicator
The processing indicator MUST track cursor position via Accessibility API with mouse position as fallback. Processing indicator should appear near where the result will be inserted.

#### Scenario: Cursor position available
- **GIVEN** user triggers AI processing in a text field
- **WHEN** the app supports Accessibility API for cursor position
- **THEN** Halo appears at the text caret position

#### Scenario: Cursor position unavailable (fallback)
- **GIVEN** user triggers AI processing
- **WHEN** the app does not support Accessibility API (e.g., WeChat, Electron apps)
- **THEN** Halo appears at the current mouse position

#### Scenario: Invalid cursor position
- **GIVEN** Accessibility API returns coordinates near (0,0)
- **WHEN** these coordinates are outside valid screen bounds
- **THEN** Halo falls back to mouse position

---

## MODIFIED Requirements

### Requirement: HaloState enum simplification
The HaloState enum MUST be simplified to remove the success case. This removes unused states and simplifies the state machine.

#### Scenario: Processing state without provider color
- **GIVEN** AI is processing a request
- **WHEN** HaloState is set to processing
- **THEN** no provider-specific color is used (unified purple color)

#### Scenario: State transitions skip success
- **GIVEN** AI response is complete
- **WHEN** output is delivered (paste or SubPanel)
- **THEN** state transitions directly from processing/typewriting to idle

---

### Requirement: Multi-turn conversation mode preservation
The multi-turn conversation mode MUST remain unchanged. This preserves existing multi-turn behavior which works well.

#### Scenario: SubPanel visibility during processing
- **GIVEN** user is in multi-turn conversation mode
- **WHEN** AI is processing a response
- **THEN** SubPanel remains visible with CLI output mode

#### Scenario: ESC key behavior
- **GIVEN** user is in multi-turn conversation mode
- **WHEN** ESC is pressed
- **THEN** conversation is dismissed and Halo hides

---

### Requirement: Replace HaloWindow with minimal ProcessingIndicatorWindow
The HaloWindow component MUST be replaced with a minimal ProcessingIndicatorWindow. HaloWindow and related components (660+ lines each) are overly complex for showing a simple spinner.

#### Scenario: Processing indicator window initialization
- **GIVEN** the app launches
- **WHEN** ProcessingIndicatorWindow is created
- **THEN** it is a borderless, transparent, floating window with 16x16 spinner

#### Scenario: Processing indicator positioning
- **GIVEN** AI processing starts
- **WHEN** CaretPositionHelper returns a valid position
- **THEN** ProcessingIndicatorWindow appears at that position

#### Scenario: Processing indicator hide
- **GIVEN** AI processing completes or is cancelled
- **WHEN** hide is called
- **THEN** ProcessingIndicatorWindow fades out and orders out

---

## Cross-References

- **Related Capability:** `event-handler` - Rust→Swift callbacks for state changes
- **Related Capability:** `context-capture` - Uses CaretPositionHelper for position detection
- **Supersedes:** `enhance-processing-indicator-and-multiturn-visibility` (processing indicator parts)
