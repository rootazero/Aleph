Feature: Sub-Agent Orchestration
  Tests for RunEventBus lifecycle, AuthProfileManager, and SessionsSpawnTool.

  # ==========================================================================
  # RunEventBus Lifecycle Tests
  # ==========================================================================

  @run-event-bus
  Scenario: RunEventBus emits completion event correctly
    Given a new RunEventBus handle with run_id "run-123"
    When I subscribe to the event bus
    And I emit a status changed event to Running
    And I emit a run completed event with summary "Task done" and tokens 150
    Then waiting for run end should return Completed with summary "Task done" and tokens 150

  @run-event-bus
  Scenario: RunEventBus emits failure event correctly
    Given a new RunEventBus handle with run_id "run-fail-test"
    When I subscribe to the event bus
    And I emit a run failed event with error "Something went wrong" and code "ERR_TEST"
    Then waiting for run end should return Failed with error "Something went wrong" and code "ERR_TEST"

  @run-event-bus
  Scenario: RunEventBus emits cancellation event correctly
    Given a new RunEventBus handle with run_id "run-cancel-test"
    When I subscribe to the event bus
    And I emit a run cancelled event with reason "User cancelled"
    Then waiting for run end should return Cancelled with reason "User cancelled"

  @run-event-bus
  Scenario: RunEventBus supports multiple subscribers
    Given a new RunEventBus handle with run_id "run-multi-sub"
    When I create two subscribers
    And I emit a status changed event to Running
    Then both subscribers should receive the Running status event

  @run-event-bus
  Scenario: RunEventBus sequence counter increments correctly
    Given a new RunEventBus handle with run_id "run-seq-test"
    Then the sequence counter should start at 0
    And incrementing the sequence should return 0, 1, 2 in order
    And the current sequence should be 3

  @run-event-bus
  Scenario: RunEventBus input sender works correctly
    Given a new RunEventBus handle with run_id "run-input-test"
    When I get the input sender
    And I send "user input message" through the input sender
    Then the input receiver should receive "user input message"

  # ==========================================================================
  # AuthProfileManager Tests
  # ==========================================================================

  @auth-profile
  Scenario: AuthProfileManager applies cooldown after rate limit failure
    Given a temp profiles config with profile "test_profile" for provider "openai"
    When I get available profile for provider "openai" and agent "main"
    Then the profile id should be "test_profile"
    When I mark profile "test_profile" as failed due to rate limit
    Then profile "test_profile" should be in cooldown
    And profile "test_profile" should have failure count 1

  @auth-profile
  Scenario: AuthProfileManager success clears cooldown
    Given a temp profiles config with profile "test_profile" for provider "anthropic"
    When I mark profile "test_profile" as failed due to rate limit
    Then profile "test_profile" should be in cooldown
    When I mark profile "test_profile" as success
    Then profile "test_profile" should not be in cooldown
    And profile "test_profile" should have failure count 0

  @auth-profile
  Scenario: AuthProfileManager falls back to backup profile
    Given a temp profiles config with primary "primary_profile" and backup "backup_profile" for provider "anthropic"
    When I get available profile for provider "anthropic" and agent "main"
    Then the profile id should be "primary_profile"
    When I mark profile "primary_profile" as failed due to rate limit
    And I get available profile for provider "anthropic" and agent "main"
    Then the profile id should be "backup_profile"

  @auth-profile
  Scenario: AuthProfileManager tracks usage correctly
    Given a temp profiles config with profile "usage_test" for provider "openai"
    When I record usage for agent "main" profile "usage_test" with 1000 input tokens and 500 output tokens and cost 0.015
    Then the usage state file should exist
    When I record usage for agent "main" profile "usage_test" with 2000 input tokens and 1000 output tokens and cost 0.030
    Then the usage state file should contain "usage_test"

  @auth-profile
  Scenario: AuthProfileManager returns error when no profiles available
    Given an empty profiles config
    When I try to get available profile for provider "anthropic" and agent "main"
    Then it should return an error containing "No profiles available"

  # ==========================================================================
  # SessionsSpawnTool Tests (Gateway Feature)
  # ==========================================================================

  @sessions-spawn @gateway
  Scenario: SessionsSpawnTool session key format is correct
    Given an agent id "poet" and label "translator"
    Then the subagent session key prefix should be "agent:poet:subagent:translator"

  @sessions-spawn @gateway
  Scenario: SessionsSpawnTool authorization with wildcard allows all
    Given a sessions spawn tool with default wildcard authorization
    Then authorization for "any_agent" should succeed
    And authorization for "translator" should succeed

  @sessions-spawn @gateway
  Scenario: SessionsSpawnTool authorization with explicit list
    Given a sessions spawn tool with allowed agents "translator" and "summarizer"
    Then authorization for "translator" should succeed
    And authorization for "summarizer" should succeed
    And authorization for "other" should fail

  @sessions-spawn @gateway
  Scenario: SessionsSpawnTool authorization with empty list denies all
    Given a sessions spawn tool with empty allowed agents list
    Then authorization for "any" should fail

  @sessions-spawn @gateway
  Scenario: SessionsSpawnTool without context returns error
    Given a sessions spawn tool without gateway context
    When I call spawn with task "Test task"
    Then the spawn status should be Error
    And the spawn error should contain "GatewayContext not configured"

  @sessions-spawn @gateway
  Scenario: CleanupPolicy defaults to Ephemeral
    When I get the default cleanup policy
    Then the cleanup policy should be Ephemeral

  @sessions-spawn @gateway
  Scenario: SessionsSpawnArgs defaults are correct
    When I parse spawn args from JSON with only task "Do something"
    Then the spawn args task should be "Do something"
    And the spawn args label should be none
    And the spawn args agent_id should be none
    And the spawn args model should be none
    And the spawn args thinking should be none
    And the spawn args timeout should be 300
    And the spawn args cleanup should be Ephemeral
