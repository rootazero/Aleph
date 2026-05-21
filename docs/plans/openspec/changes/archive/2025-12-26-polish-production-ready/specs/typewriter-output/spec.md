# typewriter-output Specification

## Purpose

Deliver AI responses to the user through natural character-by-character typing animation, replacing the current instant paste behavior. This creates a more human-like, less jarring user experience while maintaining the ability to skip animations for power users.

## ADDED Requirements

### Requirement: Character-by-Character Typing Simulation

The system SHALL simulate individual keyboard presses to type AI responses character-by-character using the enigo crate.

#### Scenario: Type short response

- **WHEN** AI response is "Hello, world!" (13 characters)
- **AND** `typing_speed` config is 50 chars/second
- **THEN** response is typed character-by-character
- **AND** total typing duration is ~260ms (13 / 50)
- **AND** each character triggers individual key press event
- **AND** cursor remains in original application

#### Scenario: Type long response with punctuation

- **WHEN** AI response contains 500 characters including newlines and punctuation
- **AND** `typing_speed` is 50 chars/second
- **THEN** typing completes in ~10 seconds
- **AND** newlines trigger Enter key press
- **AND** special characters (quotes, symbols) are handled correctly
- **AND** no characters are dropped or duplicated

#### Scenario: Type Unicode characters

- **WHEN** AI response contains Unicode (emoji, Chinese, Arabic)
- **AND** client calls `typewriter_output()`
- **THEN** Unicode characters are typed correctly
- **AND** no mojibake or encoding errors occur
- **AND** multi-byte characters count as single unit for speed calculation

### Requirement: Configurable Typing Speed

The system SHALL allow users to configure typing speed from 10 to 200 characters per second.

#### Scenario: Slow typing speed

- **WHEN** user sets `typing_speed` to 10 chars/second in config
- **AND** AI response is 100 characters
- **THEN** typing takes ~10 seconds
- **AND** animation feels deliberate and readable

#### Scenario: Fast typing speed

- **WHEN** user sets `typing_speed` to 200 chars/second
- **AND** AI response is 100 characters
- **THEN** typing takes ~0.5 seconds
- **AND** animation is smooth but rapid

#### Scenario: Invalid typing speed

- **WHEN** user sets `typing_speed` to 0 or negative value
- **THEN** config validation returns error
- **AND** default speed (50 cps) is used
- **AND** user is notified via error message

#### Scenario: Speed out of range

- **WHEN** user sets `typing_speed` to 500 (exceeds max 200)
- **THEN** config validation clamps to 200
- **AND** warning log is generated
- **AND** Settings UI shows clamped value

### Requirement: Animation Skip Mechanism

The system SHALL allow users to skip typewriter animation mid-typing and paste remaining content instantly.

#### Scenario: Skip animation with Escape key

- **WHEN** typewriter animation is in progress
- **AND** user presses Escape key
- **THEN** animation stops immediately
- **AND** remaining text is pasted instantly via clipboard
- **AND** final text matches original AI response exactly

#### Scenario: Skip animation with new hotkey

- **WHEN** typewriter animation is in progress
- **AND** user presses global hotkey (Cmd+~) again
- **THEN** current animation is cancelled
- **AND** remaining text is pasted instantly
- **AND** new request processing begins

#### Scenario: Complete animation without skip

- **WHEN** typewriter animation completes naturally
- **THEN** Halo overlay fades out with success state
- **AND** no residual text in clipboard
- **AND** ProcessingState transitions to Success

### Requirement: Output Mode Configuration

The system SHALL support two output modes: instant paste and typewriter animation.

#### Scenario: Instant paste mode

- **WHEN** user sets `output_mode` to "instant" in config
- **AND** AI response is ready
- **THEN** response is pasted immediately via Cmd+V simulation
- **AND** no typing animation occurs
- **AND** behavior matches Phase 1-6 functionality

#### Scenario: Typewriter mode (default)

- **WHEN** user sets `output_mode` to "typewriter" (or uses default)
- **AND** AI response is ready
- **THEN** response is typed character-by-character
- **AND** typing speed uses `typing_speed` config
- **AND** Halo shows typing progress

#### Scenario: Invalid output mode

- **WHEN** config contains invalid `output_mode` value
- **THEN** config validation returns error
- **AND** default mode ("typewriter") is used
- **AND** user is notified via error log

### Requirement: Typewriter Progress Feedback

The system SHALL provide real-time progress updates during typing animation via UniFFI callbacks.

#### Scenario: Report typing progress

- **WHEN** typewriter animation is 25% complete
- **THEN** `on_typewriter_progress(0.25)` callback is invoked
- **AND** Swift UI updates Halo progress indicator
- **AND** callback is invoked at ~10% intervals (avoid excessive updates)

#### Scenario: Report completion

- **WHEN** typewriter animation reaches 100%
- **THEN** `on_typewriter_progress(1.0)` callback is invoked
- **AND** `on_state_changed(ProcessingState::Success)` follows
- **AND** Halo transitions to success state

#### Scenario: Report cancellation

- **WHEN** typewriter animation is cancelled mid-typing
- **THEN** `on_typewriter_cancelled()` callback is invoked
- **AND** Swift UI immediately hides Halo
- **AND** remaining text is pasted via clipboard

### Requirement: Typewriter State Management

The system SHALL manage typewriter state transitions to handle cancellations and errors gracefully.

#### Scenario: Transition from AI processing to typing

- **WHEN** AI provider returns response
- **AND** `output_mode` is "typewriter"
- **THEN** state transitions from `ProcessingWithAI` to `TypingOutput`
- **AND** `on_state_changed(ProcessingState::TypingOutput)` callback is invoked
- **AND** Halo changes from spinner to typing indicator

#### Scenario: Handle typing error

- **WHEN** typewriter encounters keyboard simulation error (e.g., permissions lost)
- **THEN** typing stops immediately
- **AND** remaining text is pasted via clipboard fallback
- **AND** error is logged but user sees complete response
- **AND** state transitions to Success (graceful degradation)

### Requirement: Typewriter Integration with Input Simulator

The system SHALL extend the `InputSimulator` trait with typewriter-specific methods.

#### Scenario: Implement typewriter trait method

- **WHEN** `InputSimulator` implements `type_string_animated(text, speed)`
- **THEN** method types text character-by-character
- **AND** simulates inter-character delays based on speed
- **AND** returns `Result<(), AlephError>` for error handling
- **AND** method is cancellable via async/tokio cancellation token

## MODIFIED Requirements

### Requirement: Processing State Enumeration (Extended)

The system SHALL add new processing state for typewriter animation to existing `ProcessingState` enum.

#### Scenario: New TypingOutput state

- **WHEN** state machine includes typewriter support
- **THEN** `ProcessingState::TypingOutput` variant exists
- **AND** UniFFI exports state to Swift
- **AND** HaloView handles TypingOutput with typing animation UI

## References

- **Related Spec**: `event-handler` - Extends ProcessingState enum
- **Related Spec**: `core-library` - Integrates typewriter into AlephCore
- **Depends On**: `input/` module and enigo crate for keyboard simulation
