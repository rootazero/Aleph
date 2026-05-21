# Capability: Command Completion from Registry

Command completion derives data from the Unified Tool Registry.

## ADDED Requirements

### Requirement: Registry-Based Command Source

CommandRegistry SHALL query ToolRegistry for command data instead of maintaining separate lists.

#### Scenario: Root commands from registry
- **WHEN** user types `/` in input field
- **THEN** CommandRegistry SHALL query ToolRegistry for all root-level tools
- **AND** convert them to CommandNode structures for display

#### Scenario: Subcommands from registry
- **WHEN** user types `/mcp ` (with space)
- **THEN** CommandRegistry SHALL query ToolRegistry for tools with source type MCP
- **AND** return them as children of the `/mcp` command

#### Scenario: Skill subcommands from registry
- **WHEN** user types `/skill ` (with space)
- **THEN** CommandRegistry SHALL query ToolRegistry for tools with source type Skill
- **AND** return them as children of the `/skill` command

### Requirement: UniFFI Completion APIs

The following completion APIs SHALL be exposed via UniFFI:

| Method | Description | Returns |
|--------|-------------|---------|
| `get_command_completions(prefix)` | Filter root commands by prefix | `Vec<CommandNode>` |
| `get_subcommand_completions(parent)` | Get children of namespace command | `Vec<CommandNode>` |

#### Scenario: Prefix filtering
- **WHEN** Swift calls `core.getCommandCompletions("se")`
- **THEN** it SHALL receive commands starting with "se" (e.g., "search")
- **AND** filtering SHALL be case-insensitive

#### Scenario: Empty prefix returns all
- **WHEN** Swift calls `core.getCommandCompletions("")`
- **THEN** it SHALL receive all root-level commands
- **AND** they SHALL be sorted by sort_order then alphabetically

#### Scenario: Subcommand completion
- **WHEN** Swift calls `core.getSubcommandCompletions("mcp")`
- **THEN** it SHALL receive all MCP tools as CommandNodes
- **AND** each SHALL include icon and hint from the tool metadata

### Requirement: Dynamic Completion Updates

Command completion SHALL update dynamically when tools change.

#### Scenario: MCP tool addition reflected in completion
- **WHEN** a new MCP server connects and registers tools
- **THEN** the `tools_changed` event SHALL be fired
- **AND** subsequent `get_subcommand_completions("mcp")` SHALL include new tools

#### Scenario: Skill addition reflected in completion
- **WHEN** a new skill is installed
- **THEN** the `tools_changed` event SHALL be fired
- **AND** subsequent `get_subcommand_completions("skill")` SHALL include the new skill

#### Scenario: Swift refresh on tools_changed
- **WHEN** Swift receives `tools_changed` notification
- **THEN** CommandCompletionManager SHALL call `refreshCommands()`
- **AND** the displayed completion list SHALL update

### Requirement: Tool-to-CommandNode Conversion

ToolRegistry tools SHALL be convertible to CommandNode structures:

| UnifiedTool Field | CommandNode Field |
|-------------------|-------------------|
| name | key |
| description | description |
| icon | icon |
| localization_key + ".hint" | hint |
| has subtools? | node_type (Namespace vs Action) |

#### Scenario: Tool conversion preserves metadata
- **WHEN** a UnifiedTool is converted to CommandNode
- **THEN** all relevant metadata SHALL be preserved
- **AND** the hint SHALL use localized string if available

#### Scenario: Namespace vs Action determination
- **WHEN** a tool has `subtools.is_empty() == false` or is `/mcp` or `/skill`
- **THEN** its CommandNode type SHALL be `Namespace`
- **OTHERWISE** its CommandNode type SHALL be `Action` or `Prompt`

### Requirement: Remove Hardcoded Command Lists

CommandRegistry SHALL NOT maintain hardcoded command lists.

#### Scenario: No hardcoded builtin hints
- **WHEN** CommandRegistry initializes
- **THEN** it SHALL NOT contain hardcoded `BUILTIN_HINTS` array
- **AND** all hint data SHALL come from ToolRegistry's localization_key

#### Scenario: No hardcoded builtin commands
- **WHEN** CommandRegistry queries root commands
- **THEN** it SHALL NOT use a hardcoded `builtin_commands` list
- **AND** all commands SHALL come from ToolRegistry queries
