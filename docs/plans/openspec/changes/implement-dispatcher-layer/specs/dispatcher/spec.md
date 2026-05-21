# Capability: Dispatcher

The Dispatcher Layer provides intelligent tool routing through multi-layer matching and user confirmation.

## ADDED Requirements

### Requirement: Multi-Layer Routing

The Dispatcher SHALL implement three routing layers with cascading fallback:

1. **L1 (Rule-based)**: Regex pattern matching with < 10ms latency
2. **L2 (Semantic)**: Keyword and similarity matching with 200-500ms latency
3. **L3 (AI-based)**: LLM inference with context awareness, > 1s latency

Each layer SHALL produce a confidence score (0.0 - 1.0). The first layer with confidence >= threshold SHALL be selected.

#### Scenario: L1 regex match
- **GIVEN** a user input starting with `/search`
- **WHEN** the dispatcher processes the input
- **THEN** L1 SHALL match immediately with confidence 1.0
- **AND** L2 and L3 SHALL be skipped

#### Scenario: L2 semantic fallback
- **GIVEN** a user input "find news about AI" (no slash command)
- **WHEN** L1 fails to match
- **THEN** L2 SHALL analyze keywords ("find", "news")
- **AND** L2 SHALL propose Search tool with confidence 0.7

#### Scenario: L3 context-aware routing
- **GIVEN** previous conversation mentioned "Keanu Reeves"
- **AND** current input is "search for his movies"
- **WHEN** L1 and L2 produce low confidence
- **THEN** L3 SHALL resolve "his" to "Keanu Reeves"
- **AND** L3 SHALL propose Search("Keanu Reeves movies")

### Requirement: Routing Match Result

The Dispatcher SHALL return an extended `RoutingMatch` containing:

- `tool_name: Option<String>` - Selected tool identifier
- `parameters: Option<Value>` - Extracted parameters
- `confidence: f32` - Match confidence (0.0 - 1.0)
- `routing_layer: RoutingLayer` - Which layer produced the match
- `routing_reason: Option<String>` - Human-readable explanation

#### Scenario: High confidence result
- **GIVEN** a slash command `/translate hello`
- **WHEN** routing completes
- **THEN** confidence SHALL be 1.0
- **AND** routing_layer SHALL be `L1Rule`

#### Scenario: Low confidence result
- **GIVEN** an ambiguous input "help me with this"
- **WHEN** routing completes
- **THEN** confidence MAY be < 0.5
- **AND** routing_reason SHALL explain the uncertainty

### Requirement: Confirmation Triggering

The Dispatcher SHALL trigger user confirmation when:

1. Confidence is below the configured threshold (default: 0.8)
2. The matched tool has `requires_confirmation: true` flag

#### Scenario: Confirmation triggered
- **GIVEN** dispatcher configuration with `confirmation_threshold = 0.8`
- **AND** a routing result with confidence 0.7
- **WHEN** the dispatcher prepares for execution
- **THEN** it SHALL call `on_clarification_needed()` callback
- **AND** it SHALL wait for user response

#### Scenario: Confirmation skipped
- **GIVEN** a slash command match with confidence 1.0
- **WHEN** the dispatcher prepares for execution
- **THEN** it SHALL proceed directly to execution
- **AND** it SHALL NOT call confirmation callback

### Requirement: Confirmation UI Format

The confirmation request SHALL include:

- Tool icon/badge based on `ToolSource`
- Tool name and description
- Extracted parameters in readable format
- Options: "Execute", "Cancel", "Chat instead"

#### Scenario: Search confirmation display
- **GIVEN** a proposed Search tool with query "AI news"
- **WHEN** confirmation is displayed
- **THEN** it SHALL show: `[🔍 Search] Query: "AI news"`
- **AND** it SHALL offer Execute and Cancel options

#### Scenario: MCP tool confirmation
- **GIVEN** a proposed MCP tool `git_commit` from server `github`
- **WHEN** confirmation is displayed
- **THEN** it SHALL show: `[⚡ MCP:github] git_commit`
- **AND** it SHALL display extracted parameters

### Requirement: Confirmation Response Handling

The Dispatcher SHALL handle confirmation responses:

1. **Execute**: Proceed to CapabilityExecutor with confirmed parameters
2. **Cancel**: Abort tool execution, treat as general chat
3. **Timeout**: After 30s, abort and notify user

#### Scenario: User confirms execution
- **GIVEN** a pending confirmation for Search tool
- **WHEN** user selects "Execute"
- **THEN** dispatcher SHALL proceed with Search capability
- **AND** parameters SHALL be passed to CapabilityExecutor

#### Scenario: User cancels
- **GIVEN** a pending confirmation
- **WHEN** user selects "Cancel"
- **THEN** dispatcher SHALL abort tool execution
- **AND** input SHALL be processed as GeneralChat intent

#### Scenario: Confirmation timeout
- **GIVEN** a pending confirmation
- **WHEN** 30 seconds pass without response
- **THEN** dispatcher SHALL abort
- **AND** dispatcher SHALL emit error event

### Requirement: Dynamic Prompt Generation

For L3 routing, the Dispatcher SHALL dynamically generate a system prompt containing all available tools from the `UnifiedToolRegistry`.

#### Scenario: Prompt includes all tool types
- **GIVEN** 2 Native tools, 3 MCP tools, and 1 Skill registered
- **WHEN** L3 router prepares its prompt
- **THEN** all 6 tools SHALL appear in the prompt
- **AND** each tool SHALL have name, description, and parameter hints

#### Scenario: Prompt format
- **GIVEN** a tool registry with entries
- **WHEN** prompt is generated
- **THEN** format SHALL be markdown list
- **AND** each entry SHALL follow: `- **name** [source]: description. Args: params`

### Requirement: Dispatcher Configuration

The Dispatcher SHALL be configurable via `config.toml`:

```toml
[dispatcher]
enabled = true                  # Enable/disable dispatcher
confirmation_threshold = 0.8    # 0.0 = always confirm, 1.0 = never
l3_enabled = true               # Enable LLM-based routing
l3_timeout_ms = 3000            # L3 request timeout
```

#### Scenario: Dispatcher disabled
- **GIVEN** `[dispatcher].enabled = false`
- **WHEN** input is processed
- **THEN** dispatcher layer SHALL be bypassed
- **AND** existing Router behavior SHALL apply

#### Scenario: L3 disabled
- **GIVEN** `[dispatcher].l3_enabled = false`
- **WHEN** L1 and L2 fail to match
- **THEN** L3 SHALL NOT be invoked
- **AND** input SHALL proceed as GeneralChat
