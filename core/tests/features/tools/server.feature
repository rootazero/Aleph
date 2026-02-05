Feature: Tool Server
  As the tool system
  I want to manage tool registration and replacement
  So that tools can be dynamically updated at runtime

  # ==========================================================================
  # Tool Examples - Tests from tool_examples_integration.rs
  # ==========================================================================

  Scenario: Bash tool has usage examples
    Given a BashExecTool
    When I get its definition
    Then the tool name should be "bash"
    And the llm_context should contain "## Usage Examples"
    And the llm_context should contain "bash(cmd='ls -la /tmp')"
    And the llm_context should contain "bash(cmd='pwd && ls -l', working_dir='/home/user')"

  Scenario: Search tool has usage examples
    Given a SearchTool
    When I get its definition
    Then the tool name should be "search"
    And the llm_context should contain "## Usage Examples"
    And the llm_context should contain "search(query='latest Rust async trends', limit=5)"
    And the llm_context should contain "search(query='Claude AI capabilities 2025')"

  # ==========================================================================
  # Tool Replacement - Tests from tool_server_replace_test.rs
  # ==========================================================================

  Scenario: Replace tool adds new tool
    Given a tool server
    When I replace with TestToolV1
    Then the update info should indicate a new tool
    And the update info tool name should be "test_tool"
    And the update info new description should be "Test tool version 1"
    And the tool "test_tool" should be registered

  Scenario: Replace tool updates existing
    Given a tool server
    And TestToolV1 is added to the server
    When I replace with TestToolV2
    Then the update info should indicate a replacement
    And the update info tool name should be "test_tool"
    And the update info old description should be "Test tool version 1"
    And the update info new description should be "Test tool version 2 (updated)"
    And the tool definition description should be "Test tool version 2 (updated)"

  Scenario: Replace tool execution is updated
    Given a tool server
    And TestToolV1 is added to the server
    When I call "test_tool" with message "hello"
    Then the call result should be "v1: hello"
    When I replace with TestToolV2
    And I call "test_tool" with message "hello"
    Then the call result should be "v2: hello"

  Scenario: Replace tool via handle works
    Given a tool server
    And a server handle
    When I replace TestToolV1 via the handle
    Then the update info should indicate a new tool
    When I replace TestToolV2 via the handle
    Then the update info should indicate a replacement
    And the tool definition description should be "Test tool version 2 (updated)"

  Scenario: Multiple replacements are tracked
    Given a tool server
    When I replace with TestToolV1
    Then the update info should indicate a new tool
    When I replace with TestToolV2
    Then the update info should indicate a replacement
    When I replace with TestToolV1
    Then the update info should indicate a replacement
    And the update info old description should be "Test tool version 2 (updated)"
