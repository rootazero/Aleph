Feature: WorldModel Dispatcher E2E
  As the daemon subsystem
  I want WorldModel and Dispatcher to work together
  So that state changes are tracked and dispatched correctly

  # ========================================================================
  # WorldModel + Dispatcher Integration
  # ========================================================================

  @async
  Scenario: WorldModel and Dispatcher process IDE start event
    Given a daemon event bus with capacity 1000
    And a WorldModel with test configuration
    And a Dispatcher with default configuration
    And I subscribe to the event bus
    When I spawn WorldModel loop with 5 second timeout
    And I spawn Dispatcher loop with 5 second timeout
    And I wait 100 milliseconds for startup
    And I send a process started event for "Code" with pid 12345
    And I wait 200 milliseconds for processing
    Then the WorldModel state should be "Programming" activity
    And I should receive an event from the bus within 1 second
    When I abort spawned tasks
    And I wait 100 milliseconds for cleanup

  # ========================================================================
  # Dispatcher Mode Transitions
  # ========================================================================

  Scenario: Dispatcher mode transitions
    Given a daemon event bus with capacity 100
    And a WorldModel with test configuration
    And a Dispatcher with default configuration
    Then the dispatcher mode should be "Running"
    When I set dispatcher mode to "Reconciling" with empty pending actions
    Then the dispatcher mode should be "Reconciling"
    When I set dispatcher mode to "Running"
    Then the dispatcher mode should be "Running"

  # ========================================================================
  # WorldModel Persistence
  # ========================================================================

  @async
  Scenario: WorldModel persistence across restarts
    Given a temporary state file for persistence testing
    And a daemon event bus with capacity 100
    When I create a WorldModel with the state file
    And I add a pending action "MuteSystemAudio" with reason "Test persistence"
    Then the WorldModel should have 1 pending action
    When I drop the WorldModel instance
    And I create a new WorldModel with the same state file
    Then the WorldModel should have 1 pending action
    And the pending action reason should be "Test persistence"
