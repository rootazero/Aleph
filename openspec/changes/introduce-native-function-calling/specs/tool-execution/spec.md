# Tool Execution Specification

## ADDED Requirements

### Requirement: AgentTool Trait Interface

The system SHALL provide a unified `AgentTool` trait that all native tools implement for LLM function calling.

The trait MUST define:
1. `definition()` - Returns tool metadata and JSON Schema for parameters
2. `execute(args)` - Executes the tool with JSON string arguments
3. `name()` - Returns the tool identifier
4. `requires_confirmation()` - Indicates if user confirmation is needed

#### Scenario: Tool definition retrieval
- **WHEN** a tool's `definition()` method is called
- **THEN** it returns a `ToolDefinition` containing:
  - `name`: Unique tool identifier (e.g., "file_read")
  - `description`: Human-readable description for LLM
  - `parameters`: JSON Schema object describing input parameters
  - `requires_confirmation`: Boolean indicating destructive operation
  - `category`: Tool category for UI grouping

#### Scenario: Tool execution with valid arguments
- **WHEN** `execute(args)` is called with valid JSON arguments
- **THEN** the tool deserializes arguments to typed parameters
- **AND** executes the operation
- **AND** returns `ToolResult::success(content)` with result data

#### Scenario: Tool execution with invalid arguments
- **WHEN** `execute(args)` is called with invalid JSON
- **THEN** the tool returns an error with descriptive message
- **AND** includes field name if a specific field is invalid

### Requirement: Tool Result Structure

The system SHALL use a standardized `ToolResult` struct for all tool execution results.

The result MUST contain:
1. `success`: Boolean indicating operation success
2. `content`: String result for LLM consumption
3. `data`: Optional structured JSON data
4. `error`: Optional error message if failed

#### Scenario: Successful tool result
- **WHEN** a tool operation completes successfully
- **THEN** `ToolResult` has `success: true`
- **AND** `content` contains human-readable result
- **AND** `error` is `None`

#### Scenario: Failed tool result
- **WHEN** a tool operation fails
- **THEN** `ToolResult` has `success: false`
- **AND** `error` contains descriptive error message
- **AND** `content` may contain partial results or empty string

### Requirement: Native Tool Registry

The system SHALL maintain a registry of all native `AgentTool` implementations.

The registry MUST support:
1. Registering new tools via `register_native(tool)`
2. Executing tools by name via `execute(name, args)`
3. Listing all tool definitions for LLM prompt generation
4. Querying tools by category or capability

#### Scenario: Tool registration
- **WHEN** `register_native(tool)` is called with an `AgentTool`
- **THEN** the tool is stored in the registry
- **AND** its definition is available via `get_definitions()`
- **AND** it can be executed via `execute(name, args)`

#### Scenario: Tool execution by name
- **WHEN** `execute(name, args)` is called with a registered tool name
- **THEN** the registry locates the tool
- **AND** calls `tool.execute(args)`
- **AND** returns the `ToolResult`

#### Scenario: Unknown tool execution
- **WHEN** `execute(name, args)` is called with unknown tool name
- **THEN** the registry returns `Err(ToolNotFound)`

### Requirement: Filesystem Tools

The system SHALL provide filesystem operation tools as `AgentTool` implementations.

Required tools:
1. `file_read` - Read file contents (no confirmation)
2. `file_write` - Write content to file (requires confirmation)
3. `file_list` - List directory contents (no confirmation)
4. `file_delete` - Delete file or directory (requires confirmation)
5. `file_search` - Search files by glob pattern (no confirmation)

All filesystem tools MUST validate paths against `allowed_roots` configuration.

#### Scenario: File read within allowed roots
- **WHEN** `file_read` is executed with a path inside allowed_roots
- **THEN** the file content is returned successfully
- **AND** the content is included in `ToolResult.content`

#### Scenario: File read outside allowed roots
- **WHEN** `file_read` is executed with a path outside allowed_roots
- **THEN** the operation is rejected
- **AND** error message indicates "Path not allowed"

#### Scenario: File write requires confirmation
- **WHEN** `file_write` tool definition is queried
- **THEN** `requires_confirmation` is `true`

### Requirement: Git Tools

The system SHALL provide Git operation tools as `AgentTool` implementations.

Required tools:
1. `git_status` - Show working tree status
2. `git_diff` - Show changes between commits
3. `git_log` - Show commit history
4. `git_branch` - List or manipulate branches

All git tools MUST validate repository paths against `allowed_repos` configuration.

#### Scenario: Git status in allowed repository
- **WHEN** `git_status` is executed in an allowed repository
- **THEN** the status output is returned
- **AND** includes modified, staged, and untracked files

#### Scenario: Git operation in disallowed repository
- **WHEN** any git tool is executed in a non-allowed repository
- **THEN** the operation is rejected
- **AND** error message indicates repository not allowed

### Requirement: Shell Execution Tool

The system SHALL provide a shell command execution tool as `AgentTool` implementation.

The `shell_execute` tool MUST:
1. Execute shell commands with configurable timeout
2. Support command whitelist/blacklist validation
3. Capture stdout and stderr
4. Return exit code in result

#### Scenario: Allowed command execution
- **WHEN** `shell_execute` is called with an allowed command
- **THEN** the command is executed
- **AND** stdout/stderr are captured
- **AND** exit code is included in result

#### Scenario: Blocked command execution
- **WHEN** `shell_execute` is called with a blocked command
- **THEN** execution is rejected
- **AND** error indicates command not allowed

#### Scenario: Command timeout
- **WHEN** `shell_execute` runs longer than configured timeout
- **THEN** the process is terminated
- **AND** error indicates timeout exceeded

### Requirement: MCP Tool Bridge

The system SHALL provide an `McpToolBridge` that implements `AgentTool` for external MCP server tools.

The bridge MUST:
1. Implement `AgentTool` trait
2. Convert JSON string args to MCP tool call format
3. Delegate execution to MCP server via JSON-RPC
4. Convert MCP result to `ToolResult`

#### Scenario: External MCP tool execution
- **WHEN** an MCP tool is executed via `McpToolBridge`
- **THEN** the bridge sends JSON-RPC request to MCP server
- **AND** waits for response
- **AND** converts MCP result to `ToolResult`

#### Scenario: MCP server disconnection during execution
- **WHEN** MCP server disconnects during tool execution
- **THEN** the bridge returns error result
- **AND** error indicates connection failure

### Requirement: Tool Category Classification

The system SHALL categorize tools for UI grouping and filtering.

Supported categories:
1. `Filesystem` - File and directory operations
2. `Git` - Version control operations
3. `Shell` - Command execution
4. `System` - System information
5. `Clipboard` - Clipboard operations
6. `Screen` - Screen capture
7. `Search` - Web search
8. `External` - External MCP server tools

#### Scenario: Tool category query
- **WHEN** a tool's definition is queried
- **THEN** the `category` field indicates the tool's category
- **AND** UI can filter/group tools by category

### Requirement: Tool Definition JSON Schema

Each tool's `parameters` field MUST be a valid JSON Schema object.

The schema MUST:
1. Have `type: "object"` at root
2. Define `properties` for each parameter
3. List `required` parameters
4. Include `description` for each property

#### Scenario: JSON Schema validation
- **WHEN** a tool definition's `parameters` is examined
- **THEN** it is valid JSON Schema Draft-07
- **AND** can be used by LLM for function calling

#### Scenario: LLM tool list generation
- **WHEN** all tool definitions are collected
- **THEN** they can be serialized to OpenAI/Anthropic tool format
- **AND** LLM can select and invoke tools correctly
