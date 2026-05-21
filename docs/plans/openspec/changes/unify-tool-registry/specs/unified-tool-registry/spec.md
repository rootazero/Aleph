# Capability: Unified Tool Registry

The Unified Tool Registry aggregates all tool types into a single queryable source of truth.

## ADDED Requirements

### Requirement: Tool Source Types

The system SHALL support the following tool source types:

| Source | Description | Example |
|--------|-------------|---------|
| Native | Built-in capabilities with execution logic | `native:search`, `native:video` |
| Builtin | System commands without special capability | `builtin:chat`, `builtin:mcp` |
| Mcp | Tools from MCP servers | `mcp:fs:read_file` |
| Skill | Claude Agent Skills | `skill:refine-text` |
| Custom | User-defined routing rules | `custom:en`, `custom:zh` |

#### Scenario: Tool source identification
- **WHEN** a tool is registered in the registry
- **THEN** it SHALL have a source type from the enumeration above
- **AND** the source type SHALL be queryable via `list_by_source_type()`

#### Scenario: Tool ID format
- **WHEN** a tool is registered
- **THEN** its ID SHALL follow the format `{source}:{name}` or `{source}:{namespace}:{name}`
- **AND** the ID SHALL be unique within the registry

### Requirement: Builtin Tools Registration

The system SHALL automatically register the following builtin commands on initialization:

| Command | Display Name | Icon | Has Subtools |
|---------|--------------|------|--------------|
| /search | Web Search | magnifyingglass | No |
| /mcp | MCP Tools | puzzlepiece.extension | Yes |
| /skill | Skills | wand.and.stars | Yes |
| /video | Video Transcript | play.rectangle | No |
| /chat | Chat | bubble.left.and.bubble.right | No |

#### Scenario: Builtin tools available on startup
- **WHEN** the application initializes
- **THEN** all 5 builtin tools SHALL be registered in ToolRegistry
- **AND** they SHALL be queryable via `list_builtin_tools()`
- **AND** they SHALL have `is_builtin = true`

#### Scenario: Builtin tools have metadata
- **WHEN** a builtin tool is queried
- **THEN** it SHALL include icon, usage, and localization_key
- **AND** the localization_key SHALL follow the format `tool.{name}`

### Requirement: Tool UI Metadata

Each UnifiedTool SHALL support the following UI metadata fields:

| Field | Type | Description |
|-------|------|-------------|
| icon | Option<String> | SF Symbol name for display |
| usage | Option<String> | Usage example (e.g., "/search <query>") |
| subtools | Vec<String> | IDs of nested tools |
| localization_key | Option<String> | Key for i18n lookup |
| is_builtin | bool | Whether this is a system builtin |
| sort_order | i32 | Display order (lower = first) |

#### Scenario: Tool metadata for UI rendering
- **WHEN** Swift UI requests tool metadata
- **THEN** the UnifiedToolInfo SHALL contain all UI metadata fields
- **AND** optional fields MAY be None/empty

#### Scenario: Sort order for display
- **WHEN** tools are listed for UI display
- **THEN** they SHALL be sorted by `sort_order` ascending
- **AND** tools with equal sort_order SHALL be sorted alphabetically by name

### Requirement: Dynamic Subtool Resolution

For tools with `has_subtools = true`, the system SHALL dynamically resolve subtools:

#### Scenario: MCP subtools resolution
- **WHEN** user requests children of `/mcp`
- **THEN** the system SHALL return all tools with `source = Mcp`
- **AND** results SHALL be grouped by MCP server name

#### Scenario: Skill subtools resolution
- **WHEN** user requests children of `/skill`
- **THEN** the system SHALL return all tools with `source = Skill`
- **AND** results SHALL reflect currently installed skills

#### Scenario: Subtools update on source change
- **WHEN** an MCP server connects or disconnects
- **THEN** the `/mcp` subtools list SHALL update immediately
- **AND** the system SHALL fire a `tools_changed` event

### Requirement: Tool Refresh on Events

The ToolRegistry SHALL refresh tool lists when:

#### Scenario: MCP server connection
- **WHEN** an MCP server connects successfully
- **THEN** its tools SHALL be added to the registry
- **AND** the `tools_changed` event SHALL be fired

#### Scenario: MCP server disconnection
- **WHEN** an MCP server disconnects
- **THEN** its tools SHALL be marked inactive (`is_active = false`)
- **AND** the `tools_changed` event SHALL be fired

#### Scenario: Skill installation
- **WHEN** a new skill is installed
- **THEN** it SHALL be added to the registry
- **AND** the `tools_changed` event SHALL be fired

#### Scenario: Configuration hot-reload
- **WHEN** config.toml is modified and reloaded
- **THEN** custom tools SHALL be re-registered
- **AND** the `tools_changed` event SHALL be fired

### Requirement: UniFFI API Exposure

The following APIs SHALL be exposed via UniFFI for Swift consumption:

| Method | Description | Returns |
|--------|-------------|---------|
| `list_builtin_tools()` | List system builtin tools | `Vec<UnifiedToolInfo>` |
| `list_all_tools()` | List all active tools | `Vec<UnifiedToolInfo>` |
| `list_tools_by_source(type)` | List tools by source type | `Vec<UnifiedToolInfo>` |
| `get_tool_metadata(id)` | Get single tool details | `Option<UnifiedToolInfo>` |

#### Scenario: Swift calls list_builtin_tools
- **WHEN** Swift UI calls `core.listBuiltinTools()`
- **THEN** it SHALL receive a list of UnifiedToolInfo for all builtin tools
- **AND** the list SHALL be sorted by sort_order

#### Scenario: Swift calls list_tools_by_source
- **WHEN** Swift UI calls `core.listToolsBySource(.mcp)`
- **THEN** it SHALL receive only tools with source type MCP
- **AND** inactive tools SHALL be excluded

### Requirement: Localization Support

Tool metadata SHALL support localization via localization keys:

#### Scenario: Localized tool hint
- **WHEN** a tool has `localization_key = "tool.search"`
- **AND** Swift UI renders the tool hint
- **THEN** Swift SHALL use `NSLocalizedString("tool.search.hint", ...)` for display

#### Scenario: Fallback to description
- **WHEN** a localization key is missing translation
- **THEN** the system SHALL fall back to the tool's `description` field
