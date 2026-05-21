# Tool Execution Capability

## ADDED Requirements

### Requirement: Web page content fetching SHALL be supported
The system SHALL provide a WebFetchTool that fetches and extracts readable content from web URLs.

#### Scenario: User requests webpage summary
Given the user inputs "总结这个网页：https://example.com"
When the intent routing detects a web fetch need
Then the system shall:
  - Execute the WebFetchTool with the URL
  - Convert HTML content to Markdown format
  - Truncate content exceeding 50KB limit
  - Pass the content to AI for summarization

#### Scenario: Invalid URL handling
Given the user provides an invalid or blocked URL
When WebFetchTool attempts to fetch
Then the system shall:
  - Return an error with clear message
  - Fall back to general chat mode
  - Not crash or hang

#### Scenario: Network timeout
Given the target URL is slow or unresponsive
When the fetch timeout (30s) is exceeded
Then the system shall:
  - Cancel the request gracefully
  - Return a timeout error
  - Suggest the user try again

---

### Requirement: Unified tool execution layer SHALL route to backends
The system SHALL provide a UnifiedToolExecutor that routes tool calls to the appropriate execution backend (builtin, native, MCP).

#### Scenario: Builtin tool execution
Given a tool matched as "search" or "video"
When UnifiedToolExecutor.execute() is called
Then the system shall:
  - Route to existing CapabilityExecutor
  - Maintain backward compatibility
  - Return results in standard format

#### Scenario: Native tool execution
Given a tool matched as a NativeToolRegistry tool (e.g., web_fetch, file_read)
When UnifiedToolExecutor.execute() is called
Then the system shall:
  - Look up the tool in NativeToolRegistry
  - Execute the AgentTool with serialized parameters
  - Return ToolExecutionResult with content or error

#### Scenario: MCP tool execution
Given a tool matched as an MCP server tool
When UnifiedToolExecutor.execute() is called
Then the system shall:
  - Route to McpClient.call_tool()
  - Convert MCP result to standard format
  - Handle MCP-specific errors

#### Scenario: Unknown tool fallback
Given a matched tool_name is not found in any registry
When UnifiedToolExecutor.execute() is called
Then the system shall:
  - Return a tool_not_found error
  - Log the unrecognized tool name
  - Allow caller to fall back to general chat

---

### Requirement: Tool result synthesis SHALL use AI
After executing a tool, the system SHALL pass tool results to the AI provider for natural language synthesis.

#### Scenario: Successful tool result synthesis
Given WebFetchTool returns webpage content
When the AI provider receives the tool result
Then the system shall:
  - Inject tool output into system prompt
  - Request AI to synthesize a user-facing response
  - Return the synthesized response to user

#### Scenario: Failed tool execution synthesis
Given a tool execution fails with an error
When the AI provider receives the error
Then the system shall:
  - Include error context in system prompt
  - Request AI to explain the failure to user
  - Offer alternative suggestions if possible

---

### Requirement: L3 prompt SHALL include all available tools
The L3 AI routing layer SHALL be aware of all available tools across builtin, native, and MCP sources.

#### Scenario: L3 sees native tools
Given WebFetchTool is registered in NativeToolRegistry
When L3 builds its routing prompt
Then the tool list shall include:
  - "web_fetch" with description
  - Tool source indication (native)
  - Parameter schema for accurate matching

#### Scenario: Dynamic tool discovery
Given new MCP tools are connected at runtime
When L3 processes the next user request
Then the updated tool list shall be visible to L3
And L3 may route to newly available tools

---

## MODIFIED Requirements

### Requirement: execute_matched_tool SHALL use unified executor
The existing execute_matched_tool() in processing.rs SHALL route through UnifiedToolExecutor instead of hardcoded Capability mapping.

#### Scenario: Existing builtin tools unchanged
Given a user triggers /search or /youtube
When execute_matched_tool() processes the tool
Then behavior shall be identical to current implementation
And existing tests shall pass without modification

#### Scenario: Native tool execution enabled
Given a user triggers a native tool (e.g., web_fetch)
When execute_matched_tool() processes the tool
Then UnifiedToolExecutor shall handle execution
And tool results shall be synthesized by AI

---

## Related Capabilities

- [ai-routing](../../../specs/ai-routing/spec.md) - Intent detection and routing
- [core-library](../../../specs/core-library/spec.md) - Core processing pipeline
