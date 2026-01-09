# Keyboard Simulation Capability

## Overview
This capability handles keyboard event simulation for the typewriter output mode, including character input, special keys, and shortcut simulation.

## MODIFIED Requirements

### Requirement: Typewriter Newline Handling
The typewriter mode MUST handle newline characters (`\n`, `\r`) via Unicode string input rather than virtual key codes to ensure compatibility with rich text applications.

#### Scenario: Newline in Notes.app
- **Given**: User has Notes.app active with cursor in a note
- **When**: Typewriter mode outputs text containing `\n`
- **Then**: A line break is inserted without triggering paragraph formatting
- **And**: No system beep sound is produced
- **And**: Subsequent characters continue to type normally

#### Scenario: Multiple newlines
- **Given**: Typewriter mode is outputting multi-paragraph text
- **When**: The text contains consecutive newlines (`\n\n`)
- **Then**: Two line breaks are inserted correctly
- **And**: No special formatting is triggered

### Requirement: Typewriter Character Input Reliability
The typewriter mode MUST implement retry logic with fallback for character input failures to ensure complete text output.

#### Scenario: Transient input failure
- **Given**: Typewriter mode is outputting text
- **When**: A CGEvent fails to post for a character
- **Then**: The system retries up to 3 times with exponential backoff
- **And**: If retries fail, clipboard-based paste is used as fallback
- **And**: The original clipboard content is restored after fallback

#### Scenario: Event queue saturation
- **Given**: Typewriter mode is outputting text rapidly
- **When**: The event queue becomes saturated
- **Then**: Inter-event delays are sufficient to prevent queue overflow
- **And**: No system beep sounds are produced

### Requirement: Typewriter Tab Handling
The typewriter mode MUST use virtual key codes for Tab characters to ensure proper behavior in text navigation and indentation contexts.

#### Scenario: Tab in text editor
- **Given**: User has a text editor active
- **When**: Typewriter mode outputs text containing `\t`
- **Then**: A Tab character is inserted using kVK_Tab
- **And**: The editor's tab behavior is respected (indentation, field navigation)

## Related Capabilities
- `output-coordinator`: Uses keyboard simulation for typewriter output
- `behavior-settings`: Controls typewriter speed and mode selection
