# Capability: Unified Tool Registry

The Unified Tool Registry aggregates all tool sources (Native, MCP, Skills, Custom) into a single queryable interface.

## ADDED Requirements

### Requirement: Unified Tool Structure

All tools SHALL be represented by a common `UnifiedTool` structure:

```rust
pub struct UnifiedTool {
    pub id: String,                      // Unique identifier
    pub name: String,                    // Command/tool name
    pub display_name: String,            // Human-readable name
    pub description: String,             // Purpose description
    pub source: ToolSource,              // Origin (Native/MCP/Skill/Custom)
    pub parameters_schema: Option<Value>,// JSON Schema for parameters
    pub is_active: bool,                 // User-enabled state
    pub requires_confirmation: bool,     // Force confirmation
}
```

#### Scenario: Native tool representation
- **GIVEN** the built-in Search tool
- **WHEN** registered in the registry
- **THEN** id SHALL be "native:search"
- **AND** source SHALL be `ToolSource::Native`
- **AND** parameters_schema SHALL define `query: string`

#### Scenario: MCP tool representation
- **GIVEN** an MCP tool `git_commit` from server `github-mcp`
- **WHEN** registered in the registry
- **THEN** id SHALL be "mcp:github-mcp:git_commit"
- **AND** source SHALL be `ToolSource::Mcp { server: "github-mcp" }`

#### Scenario: Skill representation
- **GIVEN** a skill with id `refine-text`
- **WHEN** registered in the registry
- **THEN** id SHALL be "skill:refine-text"
- **AND** source SHALL be `ToolSource::Skill { id: "refine-text" }`

### Requirement: Tool Source Enum

The registry SHALL distinguish tool origins via `ToolSource`:

```rust
pub enum ToolSource {
    Native,                        // Built-in capabilities
    Mcp { server: String },        // MCP server
    Skill { id: String },          // Local skill
    Custom { rule_index: usize },  // User-defined rule
}
```

#### Scenario: Source identification
- **GIVEN** tools from multiple sources
- **WHEN** listing all tools
- **THEN** each tool SHALL have distinct source
- **AND** source SHALL enable grouping in UI

### Requirement: Registry Initialization

The `ToolRegistry` SHALL aggregate tools from all sources during initialization:

1. Register Native tools (hardcoded)
2. Query MCP clients for available tools
3. Scan SkillsRegistry for installed skills
4. Parse config.toml for custom slash commands

#### Scenario: Initial registration
- **GIVEN** Aleph starts with 2 MCP servers and 3 skills
- **WHEN** ToolRegistry initializes
- **THEN** it SHALL contain all Native + MCP + Skill + Custom tools
- **AND** each tool SHALL have correct source attribution

#### Scenario: MCP tools discovery
- **GIVEN** an MCP server `github` with tools `git_status`, `git_commit`
- **WHEN** MCP client connects
- **THEN** registry SHALL receive both tools
- **AND** tools SHALL include JSON Schema from MCP protocol

### Requirement: Registry Refresh

The registry SHALL support dynamic refresh when configuration changes:

1. MCP server connects/disconnects → Refresh MCP tools
2. Skills installed/removed → Refresh skill entries
3. Config.toml modified → Refresh custom commands

#### Scenario: MCP reconnection
- **GIVEN** an MCP server disconnects
- **WHEN** it reconnects
- **THEN** registry SHALL refresh that server's tools
- **AND** other tools SHALL remain unchanged

#### Scenario: Skill installation
- **GIVEN** a new skill `code-review` is installed
- **WHEN** registry refreshes
- **THEN** `skill:code-review` SHALL appear in registry
- **AND** existing skills SHALL remain

### Requirement: Tool Query API

The registry SHALL provide query methods:

- `list_all()` → All active tools
- `list_by_source(source)` → Filter by source type
- `get_by_id(id)` → Single tool lookup
- `get_by_name(name)` → Match by command name
- `search(query)` → Fuzzy search by name/description

#### Scenario: List all tools
- **GIVEN** 10 tools registered
- **WHEN** calling `list_all()`
- **THEN** it SHALL return 10 `UnifiedTool` entries
- **AND** inactive tools SHALL be excluded by default

#### Scenario: Filter by source
- **GIVEN** 3 MCP tools and 2 Native tools
- **WHEN** calling `list_by_source(ToolSource::Mcp { .. })`
- **THEN** it SHALL return 3 MCP tools only

#### Scenario: Fuzzy search
- **GIVEN** tools including "search", "search-history", "git-search"
- **WHEN** calling `search("search")`
- **THEN** it SHALL return all 3 matching tools
- **AND** results SHALL be ordered by relevance

### Requirement: Tool Deactivation

Users SHALL be able to deactivate tools without uninstalling:

- Set `is_active = false` for specific tools
- Deactivated tools excluded from routing
- Deactivated tools hidden from L3 prompt

#### Scenario: Deactivate tool
- **GIVEN** user disables `mcp:github:git_push`
- **WHEN** registry updates
- **THEN** `is_active` SHALL be false
- **AND** tool SHALL not appear in L3 routing prompt

#### Scenario: Reactivate tool
- **GIVEN** a deactivated tool
- **WHEN** user enables it
- **THEN** `is_active` SHALL be true
- **AND** tool SHALL appear in routing again

### Requirement: Custom Command Registration

User-defined slash commands in config.toml SHALL be registered as tools:

```toml
[[rules]]
regex = "^/translate"
provider = "openai"
system_prompt = "You are a translator."
```

This SHALL create a tool with:
- id: "custom:translate"
- name: "translate"
- source: `ToolSource::Custom { rule_index: 0 }`

#### Scenario: Custom command as tool
- **GIVEN** a rule `^/summarize` in config.toml
- **WHEN** registry initializes
- **THEN** `custom:summarize` SHALL be registered
- **AND** description SHALL be derived from system_prompt

#### Scenario: Custom command update
- **GIVEN** user modifies a custom command's system_prompt
- **WHEN** config reloads
- **THEN** registry SHALL update the tool entry
- **AND** new description SHALL reflect the change

### Requirement: Thread-Safe Access

The registry SHALL be thread-safe for concurrent access:

- Use `Arc<RwLock<HashMap<String, UnifiedTool>>>`
- Read operations SHALL NOT block each other
- Write operations SHALL acquire exclusive lock

#### Scenario: Concurrent reads
- **GIVEN** multiple threads querying tools
- **WHEN** no write in progress
- **THEN** all reads SHALL proceed concurrently

#### Scenario: Write during reads
- **GIVEN** a refresh operation in progress
- **WHEN** a read is requested
- **THEN** read SHALL wait for write to complete
- **AND** read SHALL see updated data
