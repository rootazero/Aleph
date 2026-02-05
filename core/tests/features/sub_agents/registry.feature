Feature: SubAgent Registry Lifecycle
  Tests for SubAgentRegistry registration and state transitions.

  @registry
  Scenario: Register and track a sub-agent run
    Given a fresh SubAgentRegistry
    When I register a sub-agent run with task "Explore codebase"
    Then the run should have status "Pending"
    And the run should be retrievable by its ID

  @registry
  Scenario: State transitions follow lifecycle rules
    Given a fresh SubAgentRegistry
    And a registered sub-agent run
    When I transition the run to "Running"
    Then the run should have status "Running"
    And the run should have a started_at timestamp

  @registry
  Scenario: Get children by parent session
    Given a fresh SubAgentRegistry
    When I register a sub-agent run with parent "parent-x" and task "Task 1"
    And I register a sub-agent run with parent "parent-x" and task "Task 2"
    And I register a sub-agent run with parent "parent-y" and task "Task 3"
    Then parent "parent-x" should have 2 children
    And parent "parent-y" should have 1 child

  @registry
  Scenario: Get active runs excludes completed
    Given a fresh SubAgentRegistry
    And a registered sub-agent run
    When I transition the run to "Running"
    And I transition the run to "Completed"
    Then there should be 0 active runs
