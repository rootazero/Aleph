Feature: Agent Loop Core
  As an AI assistant
  I want to execute Observe-Think-Act cycles
  So that I can complete complex tasks

  # ═══ Basic Loop Execution ═══

  Scenario: Loop completes with Complete decision
    Given a mock thinker that returns Complete with summary "Task done"
    When I run the agent loop with request "Test request"
    Then the loop result should be Completed
    And the summary should be "Task done"
    And the steps should be 0

  Scenario: Loop executes tool and completes
    Given a mock thinker that returns UseTool "search" then Complete
    When I run the agent loop with request "Search for something"
    Then the loop result should be Completed
    And the steps should be 1
    And events should include ActionStart
    And events should include ActionDone with success

  Scenario: Loop triggers max steps guard
    Given a mock thinker that always returns tool calls
    And a loop config with max steps 5
    When I run the agent loop with request "Run many steps"
    Then the loop result should be GuardTriggered
    And the guard violation should be MaxSteps

  # ═══ Event Bus Integration ═══

  Scenario: Loop emits LoopContinue event
    Given a mock thinker that returns UseTool "search" then Complete
    And an event bus subscribed to LoopContinue
    When I run the agent loop with event bus
    Then a LoopContinue event should be emitted

  Scenario: Loop emits ToolCallCompleted event
    Given a mock thinker that returns UseTool "search" then Complete
    And an event bus subscribed to ToolCallCompleted
    When I run the agent loop with event bus
    Then a ToolCallCompleted event should be emitted for "search"

  Scenario: Loop emits LoopStop on completion
    Given a mock thinker that returns Complete with summary "Done"
    And an event bus subscribed to LoopStop
    When I run the agent loop with event bus
    Then a LoopStop event should be emitted with reason Completed

  Scenario: Loop emits LoopStop on guard trigger
    Given a mock thinker that always returns tool calls
    And a loop config with max steps 3
    And an event bus subscribed to LoopStop
    When I run the agent loop with event bus
    Then a LoopStop event should be emitted with reason MaxIterationsReached

  # ═══ Compaction Trigger ═══

  Scenario: Optional compaction trigger works without event bus
    Given no event bus configured
    When I create an optional compaction trigger
    Then triggering loop continue should not panic
    And triggering loop stop should not panic

  Scenario: Compaction trigger can be created with event bus
    Given an event bus subscribed to LoopStop
    When I create a compaction trigger
    Then the trigger should emit events successfully

  # ═══ Overflow Detection ═══

  Scenario: Loop integrates with overflow detector
    Given a mock thinker that returns UseTool "search" then Complete
    And an overflow detector with 100K context limit
    When I run the agent loop with overflow detector
    Then the loop result should be Completed
    And the steps should be 1

  Scenario: Should compact returns true above 85 percent usage
    Given an overflow detector with context 10K output 1K reserve 10 percent
    And token usage of 7000 tokens
    When I check should compact
    Then should compact should be true

  Scenario: Should compact returns false below 85 percent usage
    Given an overflow detector with context 10K output 1K reserve 10 percent
    And token usage of 4000 tokens
    When I check should compact
    Then should compact should be false

  Scenario: Is overflow returns true above limit
    Given an overflow detector with context 10K output 1K reserve 10 percent
    And token usage of 9000 tokens
    When I check is overflow
    Then is overflow should be true

  Scenario: Usage percent calculates correctly
    Given an overflow detector with context 10K output 1K reserve 10 percent
    And token usage of 4050 tokens
    When I check usage percent
    Then usage percent should be 50

  Scenario: Unified session constructor works with all options
    Given an event bus subscribed to LoopStop
    And an overflow detector with 100K context limit
    When I create agent loop with unified session
    Then the loop should have overflow detector configured
    And the loop config should have realtime overflow enabled

  # ═══ Rich User Questions ═══

  Scenario: Loop handles AskUserRich decision
    Given a mock thinker that returns AskUserRich then Complete
    When I run the agent loop with request "Test rich question"
    Then the loop result should be Completed
    And events should include UserQuestionRequired
