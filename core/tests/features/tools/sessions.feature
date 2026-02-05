Feature: Session Tools Integration
  As an AI agent
  I want to list and send messages to sessions
  So that I can coordinate with other agents and manage conversations

  # ==========================================================================
  # Sessions List Integration Tests
  # ==========================================================================

  Scenario: List sessions returns empty when no sessions exist
    Given a permissive gateway context
    When I call sessions_list with no filters and limit 50
    Then the sessions list count should be 0
    And the sessions list should be empty

  Scenario: List sessions returns all created sessions
    Given a permissive gateway context
    And a main session for agent "main"
    And a task session for agent "main" with kind "cron" and id "daily-summary"
    And a peer session for agent "main" with peer "user123"
    When I call sessions_list with no filters and limit 50
    Then the sessions list count should be 3

  Scenario: Filter sessions by kind task
    Given a permissive gateway context
    And a main session for agent "main"
    And a task session for agent "main" with kind "cron" and id "task-1"
    And a task session for agent "main" with kind "webhook" and id "task-2"
    And a peer session for agent "main" with peer "user"
    When I call sessions_list with kind filter "task" and limit 50
    Then the sessions list count should be 2
    And all sessions should have kind "task"

  Scenario: Filter sessions by multiple kinds
    Given a permissive gateway context
    And a main session for agent "main"
    And a task session for agent "main" with kind "cron" and id "task-1"
    And an ephemeral session for agent "main"
    When I call sessions_list with kind filters "main,ephemeral" and limit 50
    Then the sessions list count should be 2
    And sessions should include kind "main"
    And sessions should include kind "ephemeral"
    And sessions should not include kind "task"

  Scenario: Limit the number of returned sessions
    Given a permissive gateway context
    And 10 task sessions for agent "main" with kind "cron"
    When I call sessions_list with no filters and limit 5
    Then the sessions list count should be 5

  Scenario: List sessions with message history
    Given a permissive gateway context
    And a main session for agent "main"
    And the session has message from "user" with text "Hello!"
    And the session has message from "assistant" with text "Hi there!"
    And the session has message from "user" with text "How are you?"
    When I call sessions_list with message limit 5
    Then the sessions list count should be 1
    And the first session should have messages
    And the first session should have 3 messages

  # ==========================================================================
  # A2A Policy Integration Tests with sessions_list
  # ==========================================================================

  Scenario: Permissive policy allows listing all agent sessions
    Given a permissive gateway context
    And a main session for agent "main"
    And a main session for agent "work"
    And a main session for agent "personal"
    When I call sessions_list as agent "tester" with no filters and limit 50
    Then the sessions list count should be 3

  Scenario: Restrictive policy filters out unauthorized agent sessions
    Given a gateway context with policy allowing only agent "main"
    And a main session for agent "main"
    And a main session for agent "work"
    And a main session for agent "personal"
    When I call sessions_list as agent "tester" with no filters and limit 50
    Then the sessions list count should be 1
    And the first session key should contain "main"

  Scenario: Prefix pattern policy filters correctly
    Given a gateway context with policy allowing pattern "work-*"
    And a main session for agent "work-project1"
    And a main session for agent "work-project2"
    And a main session for agent "personal"
    When I call sessions_list as agent "tester" with no filters and limit 50
    Then the sessions list count should be 2
    And all session keys should contain "work-"

  Scenario: Disabled policy only allows same-agent communication
    Given a gateway context with disabled A2A policy
    And a main session for agent "main"
    And a main session for agent "work"
    When I call sessions_list as agent "main" with no filters and limit 50
    Then the sessions list count should be 1
    And the first session key should contain "agent:main:"

  # ==========================================================================
  # Sessions Send Integration Tests
  # ==========================================================================

  Scenario: sessions_send without context returns error
    Given a sessions_send tool without context
    When I call sessions_send to "agent:main:main" with message "Hello"
    Then the send status should be Error
    And the send error should contain "GatewayContext not configured"

  Scenario: sessions_send with invalid session key returns error
    Given a permissive gateway context
    And a sessions_send tool with context for agent "main"
    When I call sessions_send to "invalid:key:format" with message "Hello"
    Then the send status should be Error
    And the send error should contain "Invalid session key"

  Scenario: sessions_send with A2A policy denial returns forbidden
    Given a gateway context with disabled A2A policy
    And registered agent "translator"
    And a sessions_send tool with context for agent "main"
    When I call sessions_send to "agent:translator:main" with message "Translate this" and timeout 30
    Then the send status should be Forbidden
    And the send error should contain "A2A policy denies"

  Scenario: sessions_send fire-and-forget mode with permissive policy
    Given a permissive gateway context with tracking adapter
    And registered agent "main"
    And a sessions_send tool with context for agent "caller"
    When I call sessions_send to "agent:main:main" with message "Fire and forget message" and timeout 0
    Then the send status should be Accepted
    And the send result should have a session key
    And the send result should not have a reply
    And after 50ms the adapter should have been called at least 1 time

  Scenario: sessions_send to non-existent agent returns error
    Given a permissive gateway context
    And a sessions_send tool with context for agent "main"
    When I call sessions_send to "agent:nonexistent:main" with message "Hello" and timeout 30
    Then the send status should be Error
    And the send error should contain "not found in registry"

  Scenario: sessions_send wait mode with execution failure
    Given a permissive gateway context with failing adapter
    And registered agent "main"
    And a sessions_send tool with context for agent "caller"
    When I call sessions_send to "agent:main:main" with message "This will fail" and timeout 5
    Then the send status should be Error
    And the send error should contain "Execution failed"

  Scenario: sessions_send defaults to main session when no key provided
    Given a permissive gateway context with tracking adapter
    And registered agent "main"
    And a sessions_send tool with context for agent "caller"
    When I call sessions_send with no key and message "Hello default" and timeout 0
    Then the send status should be Accepted

  # ==========================================================================
  # Combined Workflow Tests
  # ==========================================================================

  Scenario: Full workflow - list sessions then send to discovered session
    Given a permissive gateway context with tracking adapter
    And a main session for agent "translator"
    And a main session for agent "coder"
    And registered agent "translator"
    And registered agent "coder"
    When I call sessions_list as agent "main" with kind filter "main" and limit 10
    Then the sessions list count should be at least 2
    When I find the session with key containing "translator"
    And I call sessions_send to the found session with message "Translate 'Hello' to French" and timeout 0
    Then the send status should be Accepted
    And after 50ms the adapter should have been called at least 1 time

  Scenario: Policy enforcement across list and send operations
    Given a gateway context with policy allowing pattern "work-*" and tracking adapter
    And a main session for agent "work-agent"
    And a main session for agent "personal-agent"
    And registered agent "work-agent"
    And registered agent "personal-agent"
    When I call sessions_list as agent "main" with no filters and limit 50
    Then the sessions list count should be 1
    And the first session key should contain "work-agent"
    When I setup sessions_send tool with context for agent "main"
    And I call sessions_send to "agent:work-agent:main" with message "Hello work agent" and timeout 0
    Then the send status should be Accepted
    When I call sessions_send to "agent:personal-agent:main" with message "Hello personal agent" and timeout 0
    Then the send status should be Forbidden

  Scenario: Same-agent communication always allowed even with restrictive policy
    Given a gateway context with disabled A2A policy and tracking adapter
    And a main session for agent "main"
    And registered agent "main"
    When I call sessions_list as agent "main" with no filters and limit 50
    Then the sessions list count should be 1
    When I setup sessions_send tool with context for agent "main"
    And I call sessions_send to "agent:main:main" with message "Self message" and timeout 0
    Then the send status should be Accepted
