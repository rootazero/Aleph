Feature: LaneState Queue and Semaphore Management
  Tests for LaneState queue operations and concurrency control.

  @lane_state
  Scenario: Enqueue and dequeue runs in FIFO order
    Given a LaneState with max_concurrent 2
    When I enqueue run "run-1"
    And I enqueue run "run-2"
    And I enqueue run "run-3"
    Then the queue should have 3 runs
    When I try to dequeue a run
    Then the dequeued run should be "run-1"
    And the queue should have 2 runs

  @lane_state
  Scenario: Semaphore limits concurrent execution
    Given a LaneState with max_concurrent 2
    And I enqueue run "run-1"
    And I enqueue run "run-2"
    And I enqueue run "run-3"
    When I try to dequeue a run with semaphore
    Then the dequeue should succeed
    When I try to dequeue a run with semaphore
    Then the dequeue should succeed
    When I try to dequeue a run with semaphore
    Then the dequeue should fail due to semaphore limit

  @lane_state
  Scenario: Complete run releases semaphore permit
    Given a LaneState with max_concurrent 2
    And I enqueue run "run-1"
    And I enqueue run "run-2"
    And I enqueue run "run-3"
    When I try to dequeue a run with semaphore
    And I try to dequeue a run with semaphore
    And I try to dequeue a run with semaphore
    Then the dequeue should fail due to semaphore limit
    When I complete run "run-1"
    And I try to dequeue a run with semaphore
    Then the dequeue should succeed

  @lane_state
  Scenario: Track running runs
    Given a LaneState with max_concurrent 2
    And I enqueue run "run-1"
    And I enqueue run "run-2"
    When I try to dequeue a run with semaphore
    Then there should be 1 running run
    And run "run-1" should be running
    When I complete run "run-1"
    Then there should be 0 running runs

  @lane_state
  Scenario: Calculate priority boost based on wait time
    Given a LaneState with max_concurrent 2
    And I enqueue run "run-1" at timestamp 1000
    When I calculate priority boost for "run-1" at timestamp 41001
    Then the priority boost should be at least 1
    When I calculate priority boost for "run-1" at timestamp 51000
    Then the priority boost should be at least 2
