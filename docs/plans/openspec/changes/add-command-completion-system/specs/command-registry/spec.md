# Capability: Command Registry

## Overview

The Command Registry provides a unified tree-based data structure for discovering and executing commands in Aleph. It aggregates commands from multiple sources (builtins, MCP tools, user prompts) into a navigable hierarchy.

## ADDED Requirements

### Requirement: Command Data Model

The system SHALL define a command data model with the following attributes:
- **key**: Unique identifier within parent namespace (e.g., "search", "git", "commit")
- **description**: Human-readable description for display
- **icon**: SF Symbol name for visual representation
- **hint**: Optional short hint text for command mode display (max ~80px width)
- **node_type**: One of Action, Prompt, or Namespace
- **has_children**: Boolean indicating if node has child commands
- **source_id**: Optional identifier for dynamic loading (e.g., "mcp:git")

#### Scenario: Command node creation
- **WHEN** a command node is created with key "search"
- **THEN** the node SHALL have a valid key, description, and icon
- **AND** node_type SHALL be one of the defined types
- **AND** hint MAY be present for display purposes

#### Scenario: Namespace node identification
- **WHEN** a command node has node_type = Namespace
- **THEN** has_children SHALL be true
- **AND** source_id MAY contain a loader identifier

### Requirement: Command Type Semantics

The system SHALL support three command types with distinct behaviors:

| Type      | Behavior                                    | Children |
|-----------|---------------------------------------------|----------|
| Action    | Execute immediately upon selection           | No       |
| Prompt    | Load system prompt, await user input        | No       |
| Namespace | Display children for further navigation     | Yes      |

#### Scenario: Action command execution
- **WHEN** user selects a command with node_type = Action
- **THEN** the system SHALL execute the associated action immediately
- **AND** return a CommandExecutionResult

#### Scenario: Prompt command selection
- **WHEN** user selects a command with node_type = Prompt
- **THEN** the system SHALL load the associated system prompt
- **AND** transition to input mode for user content

#### Scenario: Namespace command navigation
- **WHEN** user selects a command with node_type = Namespace
- **THEN** the system SHALL display children of that namespace
- **AND** allow further navigation

### Requirement: Root Command Retrieval

The system SHALL provide an API to retrieve all top-level commands.

Root commands SHALL include:
1. Builtin commands (derived from config.toml rules with `^/` prefix)
2. User-defined prompt commands (from config.toml rules)
3. `/mcp` namespace (if MCP support is enabled)

#### Scenario: Get root commands
- **WHEN** `get_root_commands()` is called
- **THEN** the system SHALL return a list of CommandNode objects
- **AND** the list SHALL be sorted alphabetically by key

#### Scenario: Empty configuration
- **WHEN** config.toml has no rules with `^/` prefix
- **THEN** `get_root_commands()` SHALL return an empty list
- **AND** the system SHALL NOT throw an error

### Requirement: Child Command Retrieval

The system SHALL provide an API to retrieve children of a namespace command.

#### Scenario: Get children of namespace
- **WHEN** `get_children("mcp")` is called for a namespace node
- **THEN** the system SHALL return child CommandNode objects
- **AND** children SHALL be filtered to direct descendants only

#### Scenario: Get children of leaf node
- **WHEN** `get_children()` is called for an Action or Prompt node
- **THEN** the system SHALL return an empty list

#### Scenario: Dynamic child loading
- **WHEN** `get_children()` is called for a node with source_id = "mcp:git"
- **THEN** the system SHALL query the MCP client for available tools
- **AND** return tools as CommandNode objects

### Requirement: Prefix Filtering

The system SHALL provide an API to filter commands by key prefix.

#### Scenario: Filter by prefix
- **WHEN** `filter_by_prefix([search, settings, share], "se")` is called
- **THEN** the system SHALL return `[search, settings]`
- **AND** results SHALL maintain original sort order

#### Scenario: No matching prefix
- **WHEN** `filter_by_prefix([search, settings], "xyz")` is called
- **THEN** the system SHALL return an empty list

#### Scenario: Empty prefix
- **WHEN** `filter_by_prefix([a, b, c], "")` is called
- **THEN** the system SHALL return all input nodes

### Requirement: Command Execution

The system SHALL provide an API to execute a command by path.

#### Scenario: Execute action command
- **WHEN** `execute_command("/search", Some("weather today"))` is called
- **THEN** the system SHALL route to the search capability
- **AND** return CommandExecutionResult with success status

#### Scenario: Execute prompt command
- **WHEN** `execute_command("/en", Some("你好世界"))` is called
- **THEN** the system SHALL load the translation prompt
- **AND** route to AI provider with combined prompt + input

#### Scenario: Execute with missing argument
- **WHEN** `execute_command("/search", None)` is called for a command requiring input
- **THEN** the system SHALL return CommandExecutionResult with error
- **AND** error message SHALL indicate argument requirement

### Requirement: Builtin Command Parsing

The system SHALL parse config.toml rules into CommandNode objects.

Rules with `regex` starting with `^/` SHALL be converted to builtin commands:
- Key: Extracted from regex (e.g., `^/search` → "search")
- Description: Derived from system_prompt or capabilities
- Icon: From rule's `icon` field (default: "command" symbol)
- Type: Action if capabilities non-empty, else Prompt

#### Scenario: Parse search command rule
- **WHEN** config contains rule `{ regex: "^/search", capabilities: ["search"], system_prompt: "..." }`
- **THEN** CommandNode SHALL have key = "search"
- **AND** node_type = Action
- **AND** has_children = false

#### Scenario: Parse translation prompt rule
- **WHEN** config contains rule `{ regex: "^/en", system_prompt: "Translate to English" }`
- **THEN** CommandNode SHALL have key = "en"
- **AND** node_type = Prompt
- **AND** description SHALL reflect translation purpose

### Requirement: Icon Field in Routing Rules

The system SHALL support an `icon` field in RoutingRuleConfig for command visualization.

#### Scenario: Custom icon specified
- **WHEN** rule has `icon = "globe"`
- **THEN** CommandNode.icon SHALL be "globe"

#### Scenario: Default icon fallback
- **WHEN** rule has no `icon` field
- **THEN** CommandNode.icon SHALL default to:
  - "bolt" for Action type
  - "text.quote" for Prompt type
  - "folder" for Namespace type

### Requirement: Hint Field in Routing Rules

The system SHALL support a `hint` field in RoutingRuleConfig for command mode display.

The hint provides a short description shown next to the command key in the suggestion list.

#### Scenario: User-defined hint
- **WHEN** rule has `hint = "译英文"`
- **THEN** CommandNode.hint SHALL be "译英文"
- **AND** the hint SHALL be used as-is without localization

#### Scenario: No hint specified
- **WHEN** rule has no `hint` field
- **AND** rule is a user-defined command
- **THEN** CommandNode.hint SHALL be None

### Requirement: Builtin Command Hints with Localization

The system SHALL provide localized hints for builtin commands.

Builtin commands SHALL have hardcoded hint keys that map to Localizable.strings:

| Command Key | Hint Key (i18n) | English | 简体中文 |
|-------------|-----------------|---------|---------|
| search | command.hint.search | Web search | 网页搜索 |
| en | command.hint.en | To English | 译成英文 |
| zh | command.hint.zh | To Chinese | 译成中文 |
| draw | command.hint.draw | Gen image | 生成图片 |
| mcp | command.hint.mcp | MCP tools | MCP工具 |
| code | command.hint.code | Code help | 代码助手 |

#### Scenario: Builtin command hint localization
- **WHEN** system language is "zh-Hans"
- **AND** user requests CommandNode for builtin "search" command
- **THEN** CommandNode.hint SHALL be "网页搜索"

#### Scenario: Builtin command hint fallback
- **WHEN** system language has no translation for hint key
- **THEN** CommandNode.hint SHALL use English fallback

### Requirement: Hint Visibility Setting

The system SHALL provide a setting to control hint visibility in command mode.

Setting location: `config.toml` → `[general]` → `show_command_hints`

#### Scenario: Hints enabled (default)
- **WHEN** `show_command_hints` is true or not specified
- **THEN** CommandNode.hint values SHALL be populated

#### Scenario: Hints disabled
- **WHEN** `show_command_hints` is false
- **THEN** CommandNode.hint values MAY still be populated
- **BUT** UI layer SHALL hide hint display
