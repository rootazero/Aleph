Feature: POE Execution Cycle
  As an AI agent
  I want to execute POE cycles with validation
  So that I can ensure task completion meets requirements

  # ═══ Basic File Validation ═══

  Scenario: POE succeeds when file exists
    Given a temporary directory
    And a file "output.txt" with content "Hello"
    And a POE task requiring file "output.txt" to exist
    When I execute the POE task
    Then the outcome should be Success
    And all hard constraints should pass

  Scenario: POE fails when file missing
    Given a temporary directory
    And a POE task requiring file "missing.txt" to exist with max 3 attempts
    When I execute the POE task
    Then the outcome should be BudgetExhausted or StrategySwitch

  Scenario: POE succeeds with multiple file constraints
    Given a temporary directory
    And a file "config.json" with content '{"version": "1.0"}'
    And a file "README.md" with content "# Project"
    And a POE task requiring files "config.json" and "README.md" to exist
    When I execute the POE task
    Then the outcome should be Success

  # ═══ Command Validation ═══

  Scenario: POE validates successful command
    Given a POE task requiring command "echo hello" to pass
    When I execute the POE task
    Then the outcome should be Success

  Scenario: POE fails on failing command
    Given a POE task requiring command "false" to pass with max 2 attempts
    When I execute the POE task
    Then the outcome should be BudgetExhausted or StrategySwitch

  Scenario: POE validates command output contains pattern
    Given a POE task requiring command "echo hello world" output to contain "world"
    When I execute the POE task
    Then the outcome should be Success
