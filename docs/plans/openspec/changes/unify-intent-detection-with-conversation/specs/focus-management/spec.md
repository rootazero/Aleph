# Focus Management Specification

## ADDED Requirements

### Requirement: Cursor Auto-Focus in Continuation Input

The system SHALL automatically focus the cursor in the input field when showing the multi-turn conversation continuation input.

#### Scenario: Show continuation input after AI response

**Given** AI has generated a response
**And** the response is pasted to the target application
**When** the continuation input window appears
**Then** the text input field automatically receives focus
**And** user can immediately start typing

#### Scenario: Focus returns to Halo after paste

**Given** user is in a multi-turn conversation
**And** AI response is being pasted to target app
**When** paste operation completes
**Then** Halo continuation input is shown
**And** cursor is positioned in the input field

### Requirement: Focus Returns to Target App During Response

The system SHALL return focus to the target application when the AI response is ready to be output, before performing the paste operation.

#### Scenario: Activate target app before paste

**Given** user started interaction from Notes app
**And** AI has generated a response
**When** output begins
**Then** Notes app is activated first
**And** response is pasted into Notes
**And** then Halo continuation input appears

#### Scenario: ESC dismisses Halo and ends conversation

**Given** Halo continuation input is shown
**When** user presses ESC key
**Then** Halo input is hidden
**And** conversation session is ended
**And** focus remains in the last active application
