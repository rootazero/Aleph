Feature: Message Builder
  As the Agent Loop
  I want to convert session parts to LLM messages
  So that the Thinker can communicate with language models

  # ═══════════════════════════════════════════════════════════════════════════
  # Parts to Messages Conversion (5 tests)
  # ═══════════════════════════════════════════════════════════════════════════

  Scenario: Convert user input to message
    Given a default message builder
    And a user input part with text "Hello, help me with a task"
    When I convert parts to messages
    Then there should be 1 message
    And message 0 should have role "user"
    And message 0 should have content "Hello, help me with a task"
    And message 0 should not have tool_call_id
    And message 0 should not have tool_calls

  Scenario: Convert user input with context to message
    Given a default message builder
    And a user input part with text "Find the bug" and context "Selected file: main.rs"
    When I convert parts to messages
    Then there should be 1 message
    And message 0 should contain "Find the bug"
    And message 0 should contain "Context: Selected file: main.rs"

  Scenario: Convert completed tool call to messages
    Given a default message builder
    And a tool call part:
      | id        | call_123     |
      | tool_name | search_files |
      | input     | {"query": "*.rs"} |
      | status    | Completed    |
      | output    | Found 5 files |
    When I convert parts to messages
    Then there should be 2 messages
    And message 0 should have role "assistant"
    And message 0 should have a tool call with id "call_123" and name "search_files"
    And message 1 should have role "tool"
    And message 1 should have tool_call_id "call_123"
    And message 1 should have content "Found 5 files"

  Scenario: Convert failed tool call to messages
    Given a default message builder
    And a tool call part:
      | id        | call_456     |
      | tool_name | read_file    |
      | input     | {"path": "/nonexistent"} |
      | status    | Failed       |
      | error     | File not found |
    When I convert parts to messages
    Then there should be 2 messages
    And message 1 should have content "Error: File not found"

  Scenario: Convert interrupted tool call to messages
    Given a default message builder
    And a tool call part:
      | id        | call_789        |
      | tool_name | long_running_task |
      | input     | {}              |
      | status    | Running         |
    When I convert parts to messages
    Then there should be 2 messages
    And message 1 should have content "[Tool execution was interrupted]"

  # ═══════════════════════════════════════════════════════════════════════════
  # Reminder Injection (2 tests)
  # ═══════════════════════════════════════════════════════════════════════════

  Scenario: Inject reminders when above threshold
    Given a message builder with reminder threshold 1
    And messages:
      | role      | content        |
      | user      | First message  |
      | assistant | Response       |
      | user      | Second message |
    And session iteration count is 2
    When I inject reminders
    Then message 2 should contain "<system-reminder>"
    And message 2 should contain "Second message"
    And message 2 should contain "Please address this message"
    And message 0 should not contain "<system-reminder>"

  Scenario: No reminder injection below threshold
    Given a message builder with reminder threshold 5
    And messages:
      | role | content |
      | user | Hello   |
    And session iteration count is 3
    When I inject reminders
    Then message 0 should not contain "<system-reminder>"
    And message 0 should have content "Hello"

  # ═══════════════════════════════════════════════════════════════════════════
  # Summary Handling (1 test)
  # ═══════════════════════════════════════════════════════════════════════════

  Scenario: Convert summary to Q&A pair
    Given a default message builder
    And a summary part with content "Previously, we analyzed the codebase and found issues with error handling."
    When I convert parts to messages
    Then there should be 2 messages
    And message 0 should have role "user"
    And message 0 should have content "What did we do so far?"
    And message 1 should have role "assistant"
    And message 1 should contain "error handling"

  # ═══════════════════════════════════════════════════════════════════════════
  # Message Factory (2 tests)
  # ═══════════════════════════════════════════════════════════════════════════

  Scenario: Message factory methods create correct messages
    Given I create a user message with content "Hello"
    Then the created message should have role "user"
    And the created message should have content "Hello"
    And the created message should not have tool_call_id
    And the created message should not have tool_calls

    Given I create an assistant message with content "Hi there"
    Then the created message should have role "assistant"
    And the created message should have content "Hi there"

    Given I create a tool result message with id "call_1" and content "Result data"
    Then the created message should have role "tool"
    And the created message should have content "Result data"
    And the created message should have tool_call_id "call_1"

    Given I create an assistant message with tool call id "call_2" name "search" arguments '{"query": "test"}'
    Then the created message should have role "assistant"
    And the created message should have empty content
    And the created message should have a tool call with name "search"

  Scenario: AI response with reasoning only
    Given a default message builder
    And an AI response part with empty content and reasoning "Let me think about this..."
    When I convert parts to messages
    Then there should be 1 message
    And message 0 should have content "Let me think about this..."

  # ═══════════════════════════════════════════════════════════════════════════
  # Max Messages Limit (1 test)
  # ═══════════════════════════════════════════════════════════════════════════

  Scenario: Respect max messages limit
    Given a message builder with max messages 3
    And a user input part with text "First" at timestamp 1000
    And an AI response part with content "Response 1" at timestamp 1100
    And an AI response part with content "Response 2" at timestamp 1200
    And an AI response part with content "Response 3" at timestamp 1300
    And an AI response part with content "Response 4" at timestamp 1400
    When I convert parts to messages
    Then there should be 3 messages
    And message 0 should have content "First"
    And message 2 should have content "Response 4"

  # ═══════════════════════════════════════════════════════════════════════════
  # Full Pipeline (1 test)
  # ═══════════════════════════════════════════════════════════════════════════

  Scenario: Full build pipeline with reminders
    Given a message builder with inject reminders enabled and threshold 1
    And a user input part with text "Help me find bugs"
    And a tool call part:
      | id        | call_1   |
      | tool_name | search   |
      | input     | {"query": "error"} |
      | status    | Completed |
      | output    | Found potential bug |
    And session iteration count is 5
    When I build messages from session
    Then there should be 3 messages
    And message 0 should contain "<system-reminder>"
    And message 0 should contain "Help me find bugs"

  # ═══════════════════════════════════════════════════════════════════════════
  # Serialization (2 tests)
  # ═══════════════════════════════════════════════════════════════════════════

  Scenario: ToolCall JSON round-trip serialization
    Given a tool call with id "call_1" name "read_file" arguments '{"path": "/test.rs"}'
    When I serialize the tool call to JSON
    Then the JSON should contain "call_1"
    And the JSON should contain "read_file"
    When I deserialize the JSON to a tool call
    Then the deserialized tool call should have id "call_1"
    And the deserialized tool call should have name "read_file"

  Scenario: Message JSON serialization skips None fields
    Given I create a user message with content "Test message"
    When I serialize the message to JSON
    Then the JSON should not contain "tool_call_id"
    And the JSON should not contain "tool_calls"
    When I deserialize the JSON to a message
    Then the deserialized message should have role "user"
    And the deserialized message should have content "Test message"

  # ═══════════════════════════════════════════════════════════════════════════
  # Compaction (3 tests)
  # ═══════════════════════════════════════════════════════════════════════════

  Scenario: Filter compacted excludes old messages after compaction
    Given a message builder with compactor
    And a user input part with text "Old message" at timestamp 1000
    And a compaction marker at timestamp 2000
    And a summary part with content "Previously we discussed old topics." compacted at 2001
    And a user input part with text "New message" at timestamp 3000
    When I build from session
    Then no message should contain "Old message"
    And some message should contain "Previously we discussed old topics."
    And some message should contain "What did we do so far?"
    And some message should contain "New message"

  Scenario: Build from session without compactor uses all parts
    Given a default message builder
    And a user input part with text "First message" at timestamp 1000
    And a user input part with text "Second message" at timestamp 2000
    When I build from session
    Then there should be 2 messages
    And message 0 should contain "First message"
    And message 1 should contain "Second message"

  Scenario: Builder has compactor when constructed with one
    Given a message builder with compactor
    Then the builder should have a compactor

  # ═══════════════════════════════════════════════════════════════════════════
  # Token Limit Warnings (6 tests)
  # ═══════════════════════════════════════════════════════════════════════════

  Scenario: Inject token limit warning at high usage
    Given a message builder with overflow detector and reminders enabled
    And messages:
      | role      | content        |
      | user      | First message  |
      | assistant | Response       |
      | user      | Second message |
    And session with model "test-model" and total tokens 6885
    And session iteration count is 2
    When I inject reminders
    Then some message should contain "Context usage is at"
    And some message should contain "85%"
    And there should be 4 messages

  Scenario: No token limit warning below threshold
    Given a message builder with overflow detector and reminders enabled
    And messages:
      | role      | content        |
      | user      | First message  |
      | assistant | Response       |
      | user      | Second message |
    And session with model "test-model" and total tokens 4050
    And session iteration count is 2
    When I inject reminders
    Then no message should contain "Context usage is at"
    And there should be 3 messages

  Scenario: Builder has overflow detector when constructed with one
    Given a message builder with overflow detector
    Then the builder should have an overflow detector
    And the builder should not have a compactor

  Scenario: Builder with all components has both
    Given a message builder with compactor and overflow detector
    Then the builder should have a compactor
    And the builder should have an overflow detector

  Scenario Outline: Optional components in with_all constructor
    Given a message builder with <compactor_state> compactor and <detector_state> overflow detector via with_all
    Then the builder <compactor_result> have a compactor
    And the builder <detector_result> have an overflow detector

    Examples:
      | compactor_state | detector_state | compactor_result | detector_result |
      | enabled         | disabled       | should           | should not      |
      | disabled        | enabled        | should not       | should          |
      | disabled        | disabled       | should not       | should not      |

  Scenario: Token warning at exactly 80% threshold
    Given a message builder with overflow detector and reminders enabled
    And messages:
      | role | content      |
      | user | Test message |
    And session with model "test-model" and total tokens 6480
    And session iteration count is 2
    When I inject reminders
    Then some message should contain "Context usage is at"

  Scenario: No token warning at 79% threshold
    Given a message builder with overflow detector and reminders enabled
    And messages:
      | role | content      |
      | user | Test message |
    And session with model "test-model" and total tokens 6399
    And session iteration count is 2
    When I inject reminders
    Then no message should contain "Context usage is at"

  # ═══════════════════════════════════════════════════════════════════════════
  # Max Steps Warnings (4 tests)
  # ═══════════════════════════════════════════════════════════════════════════

  Scenario: Inject max steps warning on last step
    Given a message builder with max iterations 10 and reminders enabled
    And a user input part with text "Continue"
    And session iteration count is 9
    When I build messages from session
    Then some message should contain "LAST step"
    And some message should contain "Complete the task"
    And some message should contain "Ask the user for guidance"
    And some message should contain "Do NOT start new tool calls"

  Scenario: No max steps warning before last step
    Given a message builder with max iterations 10 and reminders enabled
    And a user input part with text "Continue"
    And session iteration count is 8
    When I build messages from session
    Then no message should contain "LAST step"

  Scenario: No max steps warning after max iterations
    Given a message builder with max iterations 10 and reminders enabled
    And a user input part with text "Continue"
    And session iteration count is 10
    When I build messages from session
    Then no message should contain "LAST step"

  Scenario: Max steps warning with default config (50 iterations)
    Given a message builder with default max iterations
    And a user input part with text "Continue"
    And session iteration count is 49
    When I build messages from session
    Then some message should contain "LAST step"
