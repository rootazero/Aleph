Feature: Smart Tool Discovery
  As an AI agent
  I want to discover and query tools efficiently
  So that I can use the right tools for each task without token overhead

  # ==========================================================================
  # 4.1 Unit Tests for ToolIndex Generation
  # ==========================================================================

  Scenario: Tool index generation basic
    Given a unified tool "builtin:search" named "search" with description "Search the web for information"
    And the tool source is Builtin
    When I generate a tool index entry with core tools "search"
    Then the index entry name should be "search"
    And the index entry category should be Core
    And the index entry should be marked as core
    And the index entry summary should be at most 50 characters

  Scenario: Tool index generation non-core
    Given a unified tool "mcp:github:pr_list" named "github:pr_list" with description "List all pull requests from a GitHub repository with filtering options"
    And the tool source is Mcp with server "github"
    When I generate a tool index entry with core tools "search,file_ops"
    Then the index entry name should be "github:pr_list"
    And the index entry category should be Mcp
    And the index entry should not be marked as core

  Scenario: Tool index truncates long descriptions
    Given a unified tool "builtin:test" named "test_tool" with description "This is a very long description that definitely exceeds the fifty character limit and should be truncated with ellipsis"
    And the tool source is Builtin
    When I generate a tool index entry with core tools ""
    Then the index entry summary should be exactly 50 characters
    And the index entry summary should end with "..."

  Scenario: Tool index extracts keywords
    Given a unified tool "mcp:github:pr_create" named "github:pr_create" with description "Create a pull request"
    And the tool source is Mcp with server "github"
    When I generate a tool index entry with core tools ""
    Then the index entry keywords should contain "github"
    And the index entry keywords should contain "create"

  Scenario: Tool index category mapping for Builtin
    Given a unified tool "test:tool" named "test" with description "Test tool"
    And the tool source is Builtin
    When I generate a tool index entry with core tools ""
    Then the index entry category should be Builtin

  Scenario: Tool index category mapping for Native
    Given a unified tool "test:tool" named "test" with description "Test tool"
    And the tool source is Native
    When I generate a tool index entry with core tools ""
    Then the index entry category should be Builtin

  Scenario: Tool index category mapping for Mcp
    Given a unified tool "test:tool" named "test" with description "Test tool"
    And the tool source is Mcp with server "test"
    When I generate a tool index entry with core tools ""
    Then the index entry category should be Mcp

  Scenario: Tool index category mapping for Skill
    Given a unified tool "test:tool" named "test" with description "Test tool"
    And the tool source is Skill with id "test"
    When I generate a tool index entry with core tools ""
    Then the index entry category should be Skill

  Scenario: Tool index category mapping for Custom
    Given a unified tool "test:tool" named "test" with description "Test tool"
    And the tool source is Custom with rule_index 0
    When I generate a tool index entry with core tools ""
    Then the index entry category should be Custom

  Scenario: Tool index add and count
    Given an empty tool index
    When I add entry "search" with category Core and summary "Web search"
    And I add entry "file_ops" with category Core and summary "File operations"
    And I add entry "github:pr_list" with category Mcp and summary "List PRs"
    And I add entry "code-review" with category Skill and summary "Review code"
    Then the tool index total count should be 4
    And the tool index core count should be 2
    And the tool index mcp count should be 1
    And the tool index skill count should be 1

  Scenario: Tool index to prompt format
    Given an empty tool index
    When I add entry "search" with category Core and summary "Web search" marked as core
    And I add entry "github:pr_list" with category Mcp and summary "List PRs"
    And I generate the prompt
    Then the tool prompt should contain "## Available Tools"
    And the tool prompt should contain "### Core"
    And the tool prompt should contain "- search: Web search"
    And the tool prompt should contain "### MCP"
    And the tool prompt should contain "- github:pr_list: List PRs"

  # ==========================================================================
  # 4.2 Unit Tests for Meta Tools
  # ==========================================================================

  Scenario: List tools on empty registry
    Given an empty tool registry
    When I call list_tools with no category filter
    Then the list result total count should be 0
    And the list result tools should be empty

  Scenario: List tools with registered tools
    Given an empty tool registry
    And I register tool "builtin:search" named "search" with description "Web search" and source Builtin
    And I register tool "mcp:github:pr_list" named "github:pr_list" with description "List PRs" and source Mcp with server "github"
    When I call list_tools with no category filter
    Then the list result total count should be 2
    And the list result tools should not be empty

  Scenario: List tools with category filter
    Given an empty tool registry
    And I register tool "builtin:search" named "search" with description "Web search" and source Builtin
    And I register tool "mcp:github:pr_list" named "github:pr_list" with description "List PRs" and source Mcp with server "github"
    When I call list_tools with category filter "mcp"
    Then all list result entries should have category Mcp

  Scenario: Get tool schema found
    Given an empty tool registry
    And I register tool "builtin:search" named "search" with description "Search the web for information" and source Builtin
    And the tool has a parameters schema with property "query" of type "string"
    When I call get_tool_schema for "search"
    Then the schema result should be found
    And the schema result name should be "search"
    And the schema result description should contain "Search"
    And the schema result parameters should have "properties"

  Scenario: Get tool schema not found
    Given an empty tool registry
    When I call get_tool_schema for "nonexistent_tool"
    Then the schema result should not be found
    And the schema result error should contain "not found"

  Scenario: Get tool schema with suggestions
    Given an empty tool registry
    And I register tool "builtin:search" named "search" with description "Web search" and source Builtin
    When I call get_tool_schema for "serach"
    Then the schema result should not be found
    And the schema result suggestions should contain "search"

  # ==========================================================================
  # 4.3 Integration Tests for Two-Stage Discovery
  # ==========================================================================

  Scenario: Two-stage discovery workflow
    Given an empty tool registry
    And I register tool "builtin:search" named "search" with description "Web search" and source Builtin with schema
    And I register tool "builtin:file_ops" named "file_ops" with description "File ops" and source Builtin with schema
    And I register 10 MCP tools for server "github"
    When I call list_tools with no category filter
    Then the list result total count should be 12
    When I call get_tool_schema for "search"
    Then the schema result should be found
    And the schema result name should be "search"

  Scenario: Tool index generation from registry
    Given an empty tool registry
    And I register tool "builtin:search" named "search" with description "Web search" and source Builtin
    And I register tool "mcp:github:pr_list" named "github:pr_list" with description "List PRs" and source Mcp with server "github"
    When I generate tool index from registry with core tools "search"
    Then the tool index total count should be 2
    And the tool index core count should be 1
    And the tool index mcp count should be 1
    And the first core tool should be "search"
    And the first core tool should be marked as core

  # ==========================================================================
  # 4.4 Benchmark: Token Consumption Comparison
  # ==========================================================================

  Scenario: Token consumption comparison
    Given an empty tool registry
    And I register 50 tools with realistic schemas
    When I calculate full schema text size
    And I calculate index-only text size
    Then the token savings should be greater than 50 percent

  # ==========================================================================
  # 4.5 Benchmark: Latency Comparison
  # ==========================================================================

  @benchmark @skip
  Scenario: Latency comparison
    Given an empty tool registry
    And I register 100 tools with simple schemas
    When I measure list_tools latency over 10 iterations
    Then the average list_tools latency should be under 100000 microseconds
    When I measure get_tool_schema latency over 10 iterations
    Then the average get_tool_schema latency should be under 50000 microseconds
    When I measure generate_index latency over 10 iterations
    Then the average generate_index latency should be under 100000 microseconds

  # ==========================================================================
  # 4.6 End-to-End Test with 50+ Tools
  # ==========================================================================

  Scenario: End-to-end with many tools
    Given an empty tool registry
    And I register a realistic tool mix with 5 builtin, 33 MCP, and 12 skill tools
    When I call list_tools with no category filter
    Then the list result total count should be at least 50
    When I call list_tools with category filter "mcp"
    Then the list result total count should be greater than 0
    When I call get_tool_schema for "search"
    Then the schema result should be found
    When I call get_tool_schema for "github:pr_list"
    Then the schema result should be found
    When I generate tool index from registry with core tools "search,file_ops,code_exec"
    Then the tool index core count should be 3
    And the tool prompt should contain "## Available Tools"
    And the tool prompt should contain "### Core"
    And the tool prompt should contain "### MCP"
    And the tool prompt should contain "### Skills"

  # ==========================================================================
  # Sub-Agent Integration Tests
  # ==========================================================================

  Scenario: Sub-agent delegate result parsing
    Given a delegate result JSON with success true and agent_id "mcp"
    And the result has 3 tools and 1 artifact and 1 tool call
    When I parse the delegate result
    Then the parsed result should be successful
    And the parsed result agent_id should be "mcp"
    And the parsed result should have 1 artifact
    And the parsed result should have 1 tool call
    And the parsed result iterations used should be 2

  Scenario: Sub-agent result merging
    Given a delegate result with success true and summary "Found matching tools"
    And the delegate result has agent_id "skill"
    And the delegate result has 1 artifact
    And the delegate result has 1 tool call
    When I merge the delegate result
    Then the merged result should be successful
    And the merged result summary should be "Found matching tools"
    And the merged result should have 1 artifact
    And the merged result should have 1 tool call
    And the merged result error should be none

  Scenario: Sub-agent context passing
    Given an execution context with working directory "/Users/test/project"
    And the context has current app "VSCode"
    And the context has original request "Help me find available GitHub tools"
    And the context has history summary "User asked about GitHub integration"
    And the context has metadata "theme" with value "dark"
    When I create a sub-agent request with prompt "List GitHub tools"
    And I set target to "github"
    And I set max iterations to 5
    And I set parent session to "session-123"
    And I set the execution context
    Then the request prompt should be "List GitHub tools"
    And the request target should be "github"
    And the request max iterations should be 5
    And the request execution context working directory should be "/Users/test/project"
    And the request execution context current app should be "VSCode"
