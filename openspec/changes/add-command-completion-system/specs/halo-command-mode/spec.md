# Capability: Halo Command Mode

## Overview

Halo Command Mode provides a structured command browsing and execution interface as an alternative to free-form Chat Mode. It enables users to discover and invoke commands through hierarchical navigation with auto-completion.

## ADDED Requirements

### Requirement: Dual-Mode Interface

The Halo overlay SHALL support two distinct modes:

| Mode    | Entry Method           | Visual Indicator      | Behavior                    |
|---------|------------------------|-----------------------|-----------------------------|
| Chat    | Default, or Escape     | Purple border, ✨ icon | Natural language input      |
| Command | Cmd+Opt+/ hotkey       | Cyan border, 💻 icon  | Structured command browsing |

#### Scenario: Default to chat mode
- **WHEN** Halo is summoned via default hotkey
- **THEN** Halo SHALL display in Chat Mode
- **AND** show purple border styling

#### Scenario: Enter command mode
- **WHEN** user presses Cmd+Opt+/
- **THEN** Halo SHALL transition to Command Mode
- **AND** show cyan border styling
- **AND** display root command suggestions

#### Scenario: Exit command mode
- **WHEN** user presses Escape in Command Mode
- **THEN** Halo SHALL transition to Chat Mode (or hide if no pending input)

### Requirement: Command Session State

The system SHALL maintain a CommandSession state containing:
- **pathStack**: Array of selected CommandNode objects (breadcrumb trail)
- **currentInput**: String of current filter text
- **suggestions**: Array of CommandNode objects matching current context
- **selectedIndex**: Currently highlighted suggestion index

#### Scenario: Initial session state
- **WHEN** Command Mode is entered
- **THEN** pathStack SHALL be empty
- **AND** currentInput SHALL be empty
- **AND** suggestions SHALL contain root commands
- **AND** selectedIndex SHALL be 0

#### Scenario: Path navigation
- **WHEN** user selects "mcp" namespace
- **THEN** pathStack SHALL contain [mcp node]
- **AND** suggestions SHALL contain mcp children
- **AND** currentInput SHALL be cleared

### Requirement: Breadcrumb Navigation Display

The system SHALL display the current command path as chip-style breadcrumbs.

Each chip SHALL contain:
- Icon from CommandNode
- Key label from CommandNode
- Visual chip styling (rounded background, border)

#### Scenario: Empty path display
- **WHEN** pathStack is empty
- **THEN** only the input cursor SHALL be visible
- **AND** no breadcrumb chips displayed

#### Scenario: Multi-level path display
- **WHEN** pathStack contains [mcp, git]
- **THEN** display SHALL show: `[🔧 mcp] [🔧 git] █cursor`
- **AND** chips SHALL be visually distinct from input area

#### Scenario: Click to pop (optional)
- **WHEN** user clicks on a breadcrumb chip
- **THEN** path SHALL pop to that level
- **AND** subsequent chips SHALL be removed

### Requirement: Suggestion List Display

The system SHALL display filtered command suggestions below the input area.

Each suggestion row SHALL show:
- Icon from CommandNode
- Key label
- Hint text (if available and enabled, max 80px width)
- Arrow indicator if has_children = true

Row layout: `[Icon] [Key] [Hint] [Arrow]`

#### Scenario: Display suggestions
- **WHEN** suggestions array contains commands
- **THEN** suggestion list SHALL show one row per command
- **AND** maximum 8 rows visible (scrollable if more)

#### Scenario: Selected suggestion highlighting
- **WHEN** selectedIndex = 2
- **THEN** third suggestion row SHALL be visually highlighted
- **AND** highlight style SHALL be distinct (background color, border)

#### Scenario: Empty suggestions
- **WHEN** suggestions array is empty
- **THEN** display SHALL show "No matching commands" message
- **AND** message SHALL be styled as secondary text

#### Scenario: Loading state
- **WHEN** children are being fetched asynchronously
- **THEN** display SHALL show loading indicator
- **AND** input SHALL remain responsive

### Requirement: Hint Display in Suggestions

The system SHALL display command hints in the suggestion list when enabled.

Hints provide a brief description of each command's purpose.

#### Scenario: Hint display with width constraint
- **WHEN** CommandNode.hint is present
- **AND** show_command_hints setting is true
- **THEN** hint SHALL be displayed after key label
- **AND** hint width SHALL be constrained to maximum 80 pixels
- **AND** overflow text SHALL be truncated with ellipsis ("...")

#### Scenario: Hint styling
- **WHEN** hint is displayed
- **THEN** hint font size SHALL be smaller than key label (e.g., 11pt vs 13pt)
- **AND** hint color SHALL be secondary/gray
- **AND** hint SHALL be single line only

#### Scenario: No hint available
- **WHEN** CommandNode.hint is None
- **THEN** no hint space SHALL be reserved
- **AND** arrow indicator SHALL align to the right

#### Scenario: Hints disabled
- **WHEN** show_command_hints setting is false
- **THEN** hints SHALL NOT be displayed
- **AND** suggestion row SHALL show only icon, key, and arrow

### Requirement: Keyboard Navigation

The system SHALL support keyboard-only navigation in Command Mode.

| Key              | Behavior                                           |
|------------------|----------------------------------------------------|
| Cmd+Opt+/        | Enter Command Mode, clear path, show root commands |
| Tab              | Select highlighted suggestion, push to path        |
| Enter            | Execute if Action/Prompt, else same as Tab         |
| Backspace        | Delete character, or pop path if input empty       |
| Escape           | Exit Command Mode                                  |
| ↑ (Up Arrow)     | Move selection up                                  |
| ↓ (Down Arrow)   | Move selection down                                |
| Any printable    | Append to currentInput, filter suggestions         |

#### Scenario: Tab selection
- **WHEN** user presses Tab with suggestion highlighted
- **AND** suggestion is a Namespace
- **THEN** selected node SHALL be pushed to pathStack
- **AND** children SHALL be loaded as new suggestions
- **AND** currentInput SHALL be cleared

#### Scenario: Tab on Action
- **WHEN** user presses Tab with Action suggestion highlighted
- **THEN** command SHALL be executed
- **AND** Command Mode SHALL exit on success

#### Scenario: Backspace path pop
- **WHEN** user presses Backspace
- **AND** currentInput is empty
- **AND** pathStack is not empty
- **THEN** last node SHALL be popped from pathStack
- **AND** parent-level suggestions SHALL be displayed

#### Scenario: Backspace at root
- **WHEN** user presses Backspace
- **AND** currentInput is empty
- **AND** pathStack is empty
- **THEN** Command Mode SHALL exit (return to Chat Mode)

#### Scenario: Arrow key wraparound
- **WHEN** user presses Down Arrow at last suggestion
- **THEN** selection SHALL wrap to first suggestion

### Requirement: Input Filtering

The system SHALL filter suggestions based on currentInput prefix.

#### Scenario: Prefix filtering
- **WHEN** currentInput = "se"
- **AND** suggestions contain [search, settings, share]
- **THEN** filtered suggestions SHALL be [search, settings]

#### Scenario: Case insensitive filtering
- **WHEN** currentInput = "SE"
- **AND** suggestions contain [search, settings]
- **THEN** filtered suggestions SHALL be [search, settings]

#### Scenario: Real-time filtering
- **WHEN** user types a character
- **THEN** suggestions SHALL update within 16ms (one frame)
- **AND** selectedIndex SHALL reset to 0

### Requirement: Command Execution

The system SHALL execute commands based on selection.

#### Scenario: Execute action
- **WHEN** user selects Action command (Tab or Enter)
- **THEN** system SHALL call `execute_command(path, currentInput)`
- **AND** transition to processing state on success

#### Scenario: Execute prompt
- **WHEN** user selects Prompt command
- **THEN** system SHALL load associated system prompt
- **AND** if currentInput is non-empty, use as user input
- **AND** if currentInput is empty, prompt for input

#### Scenario: Execute with argument
- **WHEN** user types "/mcp/git/commit" then "fix login bug"
- **AND** presses Enter
- **THEN** command SHALL execute with argument "fix login bug"

### Requirement: Visual Styling

Command Mode SHALL have distinct visual styling from Chat Mode.

#### Scenario: Command mode border
- **WHEN** Halo is in Command Mode
- **THEN** border color SHALL be cyan (#00BCD4 or similar)
- **AND** border width SHALL match Chat Mode

#### Scenario: Mode icon
- **WHEN** Halo is in Command Mode
- **THEN** mode indicator icon SHALL be terminal/laptop symbol
- **AND** icon position SHALL match Chat Mode sparkle icon

#### Scenario: Suggestion list styling
- **WHEN** suggestion list is displayed
- **THEN** background SHALL use frosted glass effect (vibrancy)
- **AND** rows SHALL have consistent padding and alignment

### Requirement: Accessibility

Command Mode SHALL be accessible to users with assistive technologies.

#### Scenario: VoiceOver navigation
- **WHEN** VoiceOver is active in Command Mode
- **THEN** suggestions SHALL be announced with key and hint (if available)
- **AND** selected state SHALL be announced

#### Scenario: VoiceOver hint announcement
- **WHEN** VoiceOver is active
- **AND** suggestion has a hint
- **THEN** VoiceOver SHALL announce: "[key], [hint]"
- **AND** announcement SHALL include navigation state (e.g., "has submenu")

#### Scenario: Reduced motion
- **WHEN** system has Reduce Motion enabled
- **THEN** mode transitions SHALL be instant (no animation)
- **AND** suggestion list updates SHALL not animate

### Requirement: Focus Preservation

Command Mode SHALL NOT steal focus from the active application.

#### Scenario: Click-through disabled
- **WHEN** Halo is in Command Mode
- **THEN** `ignoresMouseEvents` SHALL be false (allow interaction)
- **BUT** window SHALL NOT become key window

#### Scenario: Return focus on exit
- **WHEN** user exits Command Mode
- **THEN** focus SHALL return to previously active application
- **AND** Halo SHALL return to click-through mode
