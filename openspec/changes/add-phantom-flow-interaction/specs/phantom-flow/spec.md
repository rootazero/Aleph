# Phantom Flow Interaction Specification

Phantom Flow is Aleph's global interaction mode for collecting user input through the Halo overlay. It provides a "Ghost-style" interaction pattern: in-place, ephemeral, and non-intrusive.

## ADDED Requirements

### Requirement: Clarification Request Types

The system SHALL support two types of clarification requests: `Select` (option list) and `Text` (free-form input).

#### Scenario: Create select-type clarification

- **GIVEN** a feature needs user to choose from options
- **WHEN** it creates a `ClarificationRequest` with `type = Select`
- **THEN** the request SHALL include a list of `ClarificationOption` items
- **AND** each option SHALL have `label` and `value` fields

#### Scenario: Create text-type clarification

- **GIVEN** a feature needs free-form text input
- **WHEN** it creates a `ClarificationRequest` with `type = Text`
- **THEN** the request SHALL include an optional `placeholder` field
- **AND** the `options` field SHALL be ignored

#### Scenario: Specify default value

- **GIVEN** a clarification request with `default_value` set
- **WHEN** the UI renders
- **THEN** the default option SHALL be pre-selected (select mode)
- **OR** the default text SHALL be pre-filled (text mode)

### Requirement: UniFFI Callback Interface

The system SHALL expose clarification through the `AlephEventHandler` callback interface.

#### Scenario: Trigger clarification from Rust

- **GIVEN** any Rust code holds a reference to `AlephEventHandler`
- **WHEN** it calls `handler.on_clarification_needed(request)`
- **THEN** the call SHALL block until user responds
- **AND** return a `ClarificationResult`

#### Scenario: Handle user selection

- **GIVEN** the user selects an option in select mode
- **WHEN** they press Enter or Tab
- **THEN** the callback SHALL return `ClarificationResult::Selected`
- **AND** include the selected `index` and `value`

#### Scenario: Handle user text input

- **GIVEN** the user enters text in text mode
- **WHEN** they press Enter
- **THEN** the callback SHALL return `ClarificationResult::TextInput`
- **AND** include the entered `value`

#### Scenario: Handle cancellation

- **GIVEN** the user presses Escape during clarification
- **WHEN** the UI dismisses
- **THEN** the callback SHALL return `ClarificationResult::Cancelled`

#### Scenario: Handle timeout

- **GIVEN** the user does not respond within timeout period
- **WHEN** the timeout expires (default 60 seconds)
- **THEN** the callback SHALL return `ClarificationResult::Timeout`
- **AND** the UI SHALL dismiss automatically

### Requirement: Halo Clarification State

The system SHALL add a new `HaloState.clarification` state for rendering the clarification UI.

#### Scenario: Transition to clarification state

- **GIVEN** the Halo is in any state
- **WHEN** `on_clarification_needed()` is called
- **THEN** the Halo SHALL transition to `.clarification` state
- **AND** display the clarification UI

#### Scenario: Transition from clarification on completion

- **GIVEN** the Halo is in `.clarification` state
- **WHEN** user completes or cancels the clarification
- **THEN** the Halo SHALL transition to `.idle` state
- **OR** to the previous processing state (if applicable)

### Requirement: Select Mode UI

The system SHALL render a vertical option list with keyboard navigation for select-type clarifications.

#### Scenario: Render option list

- **GIVEN** a select-type clarification with 4 options
- **WHEN** the UI renders
- **THEN** it SHALL display the prompt text at top
- **AND** show all 4 options in a vertical list
- **AND** highlight the currently selected option

#### Scenario: Navigate with arrow keys

- **GIVEN** the option list is displayed
- **WHEN** user presses Down arrow
- **THEN** selection SHALL move to next option
- **AND** wrap to first option if at end

#### Scenario: Navigate with Up arrow

- **GIVEN** the option list is displayed
- **WHEN** user presses Up arrow
- **THEN** selection SHALL move to previous option
- **AND** wrap to last option if at beginning

#### Scenario: Confirm with Enter

- **GIVEN** an option is highlighted
- **WHEN** user presses Enter
- **THEN** the selection SHALL be confirmed
- **AND** the result SHALL be returned to caller

#### Scenario: Quick jump with letter key

- **GIVEN** options include "Professional", "Casual", "Humorous"
- **WHEN** user presses "H" key
- **THEN** selection SHALL jump to "Humorous"

### Requirement: Text Mode UI

The system SHALL render a text input field with placeholder for text-type clarifications.

#### Scenario: Render text input

- **GIVEN** a text-type clarification with placeholder
- **WHEN** the UI renders
- **THEN** it SHALL display the prompt text at top
- **AND** show a text input field
- **AND** display the placeholder text when empty

#### Scenario: Focus input field

- **GIVEN** the text input UI is displayed
- **WHEN** the UI appears
- **THEN** the input field SHALL have keyboard focus
- **AND** user can immediately start typing

#### Scenario: Confirm with Enter

- **GIVEN** user has entered text
- **WHEN** user presses Enter
- **THEN** the text SHALL be returned as result
- **AND** the UI SHALL dismiss

### Requirement: Window Behavior

The system SHALL maintain proper window behavior during clarification mode.

#### Scenario: Window sizing for select mode

- **GIVEN** a select-type clarification is triggered
- **WHEN** the Halo window adjusts
- **THEN** the window size SHALL be approximately 350x280 pixels
- **AND** accommodate the option list

#### Scenario: Window sizing for text mode

- **GIVEN** a text-type clarification is triggered
- **WHEN** the Halo window adjusts
- **THEN** the window size SHALL be approximately 350x180 pixels

#### Scenario: No focus stealing

- **GIVEN** user is typing in another application
- **WHEN** clarification UI appears
- **THEN** the Halo window SHALL NOT steal focus
- **AND** keyboard events SHALL be captured without activation

### Requirement: Visual Consistency

The system SHALL maintain visual consistency with existing Halo modes.

#### Scenario: Match Command Mode styling

- **GIVEN** the clarification UI is displayed
- **WHEN** compared to Command Mode
- **THEN** they SHALL use consistent colors, fonts, and spacing
- **AND** selection highlight style SHALL match

#### Scenario: Show keyboard hints

- **GIVEN** the clarification UI is displayed
- **THEN** it SHALL show keyboard hints at bottom
- **AND** hints SHALL include: "↑↓ Navigate  ⏎ Select  ⎋ Cancel" (select mode)
- **OR** hints SHALL include: "⏎ Confirm  ⎋ Cancel" (text mode)

### Requirement: Performance

The system SHALL maintain UI responsiveness during clarification.

#### Scenario: UI appearance latency

- **GIVEN** `on_clarification_needed()` is called
- **WHEN** the UI renders
- **THEN** the clarification UI SHALL appear within 50ms

#### Scenario: Keyboard response time

- **GIVEN** the clarification UI is active
- **WHEN** user presses a key
- **THEN** the UI SHALL respond within 16ms (60fps)
