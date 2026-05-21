## ADDED Requirements

### Requirement: Tool Index System

The system SHALL maintain a lightweight tool index that provides minimal metadata for all registered tools.

The tool index entry SHALL contain:
- `name`: Tool command name (e.g., "github:pr_list")
- `category`: Tool category (Core, MCP, Skill, Custom)
- `summary`: One-line description (max 50 chars)
- `keywords`: Search keywords for relevance matching

The system SHALL generate tool index in two formats:
1. **Structured**: For programmatic access (`Vec<ToolIndexEntry>`)
2. **Prompt**: For LLM consumption (markdown format)

#### Scenario: Generate tool index from registry
- **WHEN** tool registry contains 50 tools
- **THEN** generate index with all 50 entries
- **AND** each entry contains name, category, summary, keywords

#### Scenario: Generate prompt-format index
- **WHEN** generating index for LLM prompt
- **THEN** format as categorized markdown list
- **AND** total token count is less than 1000 tokens

---

### Requirement: Meta Tools for Tool Discovery

The system SHALL provide meta tools that allow LLM to discover and query tool information.

#### Meta Tool: list_tools

The `list_tools` tool SHALL:
- Accept optional `category` parameter
- Return list of tools matching category (or all if no category)
- Include name, category, and summary for each tool

#### Scenario: List tools without category filter
- **WHEN** LLM calls `list_tools()` without parameters
- **THEN** return all available tools grouped by category
- **AND** include count per category

#### Scenario: List tools with category filter
- **WHEN** LLM calls `list_tools(category: "mcp")`
- **THEN** return only MCP tools
- **AND** include tool name and summary for each

#### Meta Tool: get_tool_schema

The `get_tool_schema` tool SHALL:
- Accept required `tool_name` parameter
- Return full JSON Schema for the specified tool
- Return error if tool not found

#### Scenario: Get schema for existing tool
- **WHEN** LLM calls `get_tool_schema("github:pr_create")`
- **THEN** return complete tool definition including:
  - Full description
  - JSON Schema for parameters
  - Required/optional parameter list
  - Usage examples

#### Scenario: Get schema for non-existent tool
- **WHEN** LLM calls `get_tool_schema("nonexistent_tool")`
- **THEN** return error with message "Tool not found: nonexistent_tool"
- **AND** suggest similar tool names if available

---

### Requirement: Core Tool Set

The system SHALL define a core tool set that is always available with full schema in LLM context.

Core tools SHALL include:
- `search`: Web search capability
- `file_ops`: File read/write/delete operations
- `list_tools`: Meta tool for discovery
- `get_tool_schema`: Meta tool for schema retrieval

Core tools SHALL:
- Always be included in LLM context with full schema
- Not require two-stage discovery
- Be configurable via configuration file

#### Scenario: Core tools always available
- **WHEN** agent loop starts with any user input
- **THEN** core tools are included in LLM context
- **AND** core tools have full parameter schema

#### Scenario: Configure core tools
- **WHEN** user sets `tool_discovery.core_tools = ["search", "shell", "custom_tool"]`
- **THEN** only specified tools are treated as core
- **AND** custom_tool is included with full schema

---

### Requirement: Intent-Based Tool Filtering

The system SHALL filter tools based on detected user intent to reduce tool set size.

The filtering process SHALL:
1. Extract `required_capabilities` from intent analysis
2. Map capabilities to tool categories
3. Score tools by relevance to intent
4. Select top-K tools (configurable, default: 10)

#### Scenario: Filter tools by intent
- **WHEN** user input is "帮我创建一个 GitHub PR"
- **AND** intent analysis detects capabilities: ["github", "git"]
- **THEN** filter returns github-related MCP tools with higher scores
- **AND** filtered set contains at most 10 tools

#### Scenario: Include core tools in filtered set
- **WHEN** filtering tools by intent
- **THEN** core tools are always included regardless of intent match
- **AND** filtered tools are added to core set (not replacing)

---

### Requirement: Two-Stage Tool Discovery Flow

The system SHALL support a two-stage tool discovery flow for LLM interactions.

Stage 1: Initial Context
- Provide core tools with full schema
- Provide filtered tools with full schema (if intent detected)
- Provide tool index for remaining tools (name + summary only)

Stage 2: On-Demand Expansion
- LLM can call `get_tool_schema()` to get full schema
- Expanded schema is cached for session duration
- Subsequent calls use cached schema

#### Scenario: Initial LLM context
- **WHEN** starting agent loop with 50 registered tools
- **AND** intent filtering selects 8 relevant tools
- **THEN** LLM context contains:
  - 6 core tools with full schema
  - 8 filtered tools with full schema (may overlap with core)
  - 40+ tools in index format (name + summary)
- **AND** total context tokens < 3000

#### Scenario: On-demand schema expansion
- **WHEN** LLM needs tool not in full-schema set
- **THEN** LLM calls `get_tool_schema("tool_name")`
- **AND** system returns full schema
- **AND** LLM can then call tool with correct parameters

---

### Requirement: Tool Discovery Configuration

The system SHALL provide configuration options for tool discovery behavior.

Configuration options SHALL include:
- `tool_discovery.enabled`: Enable/disable smart discovery (default: true)
- `tool_discovery.core_tools`: List of core tool names
- `tool_discovery.max_filtered_tools`: Maximum filtered tools (default: 10)
- `tool_discovery.index_format`: "markdown" | "json" (default: "markdown")

#### Scenario: Disable smart discovery
- **WHEN** `tool_discovery.enabled = false`
- **THEN** use legacy full-tool-set behavior
- **AND** all tools passed to LLM with full schema

#### Scenario: Custom core tools
- **WHEN** `tool_discovery.core_tools = ["search", "notion:page_read"]`
- **THEN** specified tools treated as core
- **AND** notion:page_read always has full schema in context
