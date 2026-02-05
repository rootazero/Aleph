Feature: Prompt Builder
  As the Thinker subsystem
  I want to build structured prompts for LLM interaction
  So that agents receive clear instructions and tool information

  # ═══ System Prompt Generation ═══

  Scenario: Generate system prompt with tools
    Given a default prompt builder
    And tools:
      | name      | description    | schema              |
      | search    | Search the web | {"query": "string"} |
      | read_file | Read a file    | {"path": "string"}  |
    When I build the system prompt
    Then the prompt should contain "AI assistant"
    And the prompt should contain "search"
    And the prompt should contain "read_file"
    And the prompt should contain "Response Format"
    And the prompt should contain "JSON"

  # ═══ Message Building ═══

  Scenario: Build messages from observation
    Given a default prompt builder
    And an observation with history "Previously searched for Rust tutorials"
    And a recent step with action "tool:search" and result "Found 10 results"
    When I build messages for query "Find Rust tutorials"
    Then messages should have at least 3 entries
    And the first message should be from User
    And the first message should contain "Find Rust tutorials"

  # ═══ Runtime Capabilities ═══

  Scenario: System prompt with runtime capabilities
    Given a prompt builder with runtime capabilities:
      """
      **Python (via uv)**
      - Execute Python scripts
      - Executable: /path/to/python
      """
    When I build the system prompt
    Then the prompt should contain "Available Runtimes"
    And the prompt should contain "Python (via uv)"
    And the prompt should contain "/path/to/python"
    And "Available Runtimes" should appear before "Available Tools"

  Scenario: System prompt without runtime capabilities
    Given a default prompt builder
    When I build the system prompt
    Then the prompt should not contain "Available Runtimes"

  # ═══ Tool Index (Smart Discovery) ═══

  Scenario: System prompt with tool index
    Given a prompt builder with tool index:
      """
      - github:pr_list: List pull requests
      - github:issue_create: Create an issue
      - notion:page_read: Read a Notion page
      """
    And tools:
      | name   | description    | schema              |
      | search | Search the web | {"query": "string"} |
    When I build the system prompt
    Then the prompt should contain "search"
    And the prompt should contain "Search the web"
    And the prompt should contain '{"query": "string"}'
    And the prompt should contain "Additional Tools"
    And the prompt should contain "get_tool_schema"
    And the prompt should contain "github:pr_list"
    And the prompt should contain "notion:page_read"

  Scenario: Smart discovery with no full tools shows tool index
    Given a prompt builder with tool index:
      """
      - tool1: Description 1
      - tool2: Description 2
      """
    When I build the system prompt
    Then the prompt should not contain "No tools available"
    And the prompt should contain "Additional Tools"

  # ═══ Skill Mode ═══

  Scenario: System prompt with skill mode enabled
    Given a prompt builder with skill mode enabled
    When I build the system prompt
    Then the prompt should contain "Skill Execution Mode"
    And the prompt should contain "RESPONSE FORMAT"
    And the prompt should contain "EVERY response MUST be a valid JSON action object"
    And the prompt should contain "NEVER output raw content directly"
    And the prompt should contain "Complete ALL steps"
    And the prompt should contain "file_ops"

  Scenario: System prompt without skill mode
    Given a default prompt builder
    When I build the system prompt
    Then the prompt should not contain "Skill Execution Mode"
    And the prompt should not contain "NEVER output raw content directly"

  # ═══ Cached Prompt Building ═══

  Scenario: Build cached system prompt structure
    Given a default prompt builder
    When I build the cached system prompt
    Then cached parts should have 2 entries
    And the first cached part should be marked for caching
    And the second cached part should not be marked for caching

  Scenario: Cached header is static regardless of tools
    Given a default prompt builder
    When I build the cached system prompt
    And I build a second cached prompt with tools:
      | name | description | schema |
      | test | Test tool   | {}     |
    Then the first part headers should be identical
    And the dynamic parts should be different

  Scenario: Cached header contains core instructions
    Given a default prompt builder
    When I build the cached system prompt
    Then the header should contain "AI assistant executing tasks step by step"
    And the header should contain "Your Role"
    And the header should contain "Observe the current state"
    And the header should contain "Decision Framework"
    And the header should contain "What is the current state?"

  Scenario: Cached dynamic part contains tools and format
    Given a default prompt builder
    And tools:
      | name    | description  | schema              |
      | my_tool | My test tool | {"param": "value"}  |
    When I build the cached system prompt
    Then the dynamic part should contain "Available Tools"
    And the dynamic part should contain "my_tool"
    And the dynamic part should contain "My test tool"
    And the dynamic part should contain '{"param": "value"}'
    And the dynamic part should contain "Special Actions"
    And the dynamic part should contain "Response Format"

  Scenario: Combined cached parts match full prompt key sections
    Given a default prompt builder
    And tools:
      | name   | description    | schema              |
      | search | Search the web | {"query": "string"} |
    When I build the system prompt
    And I build the cached system prompt
    Then both prompts should contain "AI assistant"
    And both prompts should contain "Available Tools"
    And both prompts should contain "search"
