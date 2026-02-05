Feature: Evolution Auto-Load
  As the skill evolution system
  I want to automatically detect patterns and generate skills
  So that frequently used patterns become permanent skills

  # ==========================================================================
  # Pattern Detection to Auto-Load E2E (1 scenario)
  # ==========================================================================

  Scenario: Full e2e pattern detection to auto-load
    Given an in-memory evolution tracker
    And an empty tool server for evolution
    And an evolution auto-loader with temp output directory
    When I log 10 successful executions for skill "git-quick-commit" across 3 sessions
    And I run the solidification pipeline with low thresholds
    Then suggestions should be generated
    And the candidate count should be 1
    When I auto-load the first suggestion
    Then exactly 1 tool should be loaded
    And the tool should be registered in the tool server
    And the tool should have a description
    And the tool should have a parameters schema
    And the tool should have LLM context
    And the generated skills count should be 1

  # ==========================================================================
  # Batch Auto-Load Test (1 scenario)
  # ==========================================================================

  Scenario: Batch auto-load multiple patterns
    Given an in-memory evolution tracker
    And an empty tool server for evolution
    And an evolution auto-loader with temp output directory
    When I log 8 successful executions for 3 different patterns
    And I run the solidification pipeline with low thresholds
    Then 3 suggestions should be generated
    When I batch auto-load all suggestions
    Then the batch result should show 3 total and 3 loaded
    And the batch success rate should be 100%
    And all generated tools should be registered

  # ==========================================================================
  # Existing Tool Replacement Test (1 scenario)
  # ==========================================================================

  Scenario: Auto-load replaces existing tool on reload
    Given an in-memory evolution tracker
    And an empty tool server for evolution
    And an evolution auto-loader with temp output directory
    When I log 7 successful executions for skill "test-skill" across 7 sessions
    And I run the solidification pipeline with low thresholds
    Then suggestions should be generated
    When I auto-load the first suggestion
    Then exactly 1 tool should be loaded
    When I auto-load the same suggestion again
    Then exactly 1 tool should be loaded
    And the tool should still exist after reload
