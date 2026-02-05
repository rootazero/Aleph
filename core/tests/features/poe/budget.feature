Feature: POE Budget Management
  As an AI agent
  I want budget tracking and stuck detection
  So that I can manage resources effectively and know when to change strategy

  # ═══ Budget Exhaustion ═══

  Scenario: Budget exhausted after max attempts
    Given a POE task with impossible constraint and max 3 attempts
    And stuck window of 5
    When I execute the POE task
    Then the outcome should be BudgetExhausted
    And attempts should be 3

  Scenario: Token budget exhaustion stops execution
    Given a POE task with impossible constraint
    And worker consuming 50000 tokens per call
    And token budget of 80000
    And max 10 attempts
    And stuck window of 5
    When I execute the POE task
    Then the outcome should be BudgetExhausted
    And attempts should be 2

  # ═══ Stuck Detection ═══

  Scenario: Budget detects stuck pattern
    Given a POE budget with max 10 attempts
    When I record 5 attempts with same distance 0.8
    Then the budget should be stuck over window 3

  Scenario: Budget tracks improvement
    Given a POE budget with max 10 attempts
    When I record attempts with decreasing distances 0.9, 0.7, 0.5, 0.3
    Then the budget should not be stuck
    And best score should be approximately 0.3

  Scenario: Strategy switch on stuck
    Given a POE task with impossible constraint and stuck window 3
    And max 10 attempts
    When I execute the POE task
    Then the outcome should be StrategySwitch
    And switch reason should contain "No progress"

  # ═══ Edge Cases ═══

  Scenario: Empty manifest passes immediately
    Given a POE task with no constraints
    When I execute the POE task
    Then the outcome should be Success
    And worker should be called 1 time

  Scenario: Single attempt success
    Given a temporary directory
    And a file "quick.txt" with content "quick success"
    And a POE task requiring file "quick.txt" to exist with max 1 attempt
    When I execute the POE task
    Then the outcome should be Success
    And worker should be called 1 time

  Scenario: Worker execution count tracking
    Given a POE task with impossible constraint and max 4 attempts
    And stuck window of 10
    When I execute the POE task
    Then worker should be called 4 times
