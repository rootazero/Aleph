Feature: LaneScheduler Integration Tests
  Comprehensive tests for multi-lane scheduling with priority, concurrency limits,
  anti-starvation, and recursion depth enforcement.

  @lane_scheduler
  Scenario: Enqueue and schedule runs by priority
    Given a LaneScheduler with default config
    When I enqueue run "run-1" to lane "Main"
    And I enqueue run "run-2" to lane "Subagent"
    And I enqueue run "run-3" to lane "Main"
    Then the scheduler should have 3 queued runs
    When I schedule the next run
    Then the scheduled run should be "run-1" from lane "Main"
    And the scheduler should have 2 queued runs
    And the scheduler should have 1 running run

  @lane_scheduler
  Scenario: Priority-based scheduling across all lanes
    Given a LaneScheduler with default config
    When I enqueue run "cron-1" to lane "Cron"
    And I enqueue run "subagent-1" to lane "Subagent"
    And I enqueue run "main-1" to lane "Main"
    And I enqueue run "nested-1" to lane "Nested"
    Then the scheduler should have 4 queued runs
    When I schedule the next run
    Then the scheduled run should be from lane "Main"
    When I schedule the next run
    Then the scheduled run should be from lane "Nested"
    When I schedule the next run
    Then the scheduled run should be from lane "Subagent"
    When I schedule the next run
    Then the scheduled run should be from lane "Cron"

  @lane_scheduler
  Scenario: Respect lane concurrency limits
    Given a LaneScheduler with Main lane limit 2
    When I enqueue 5 runs to lane "Main"
    And I schedule runs until no more can be scheduled
    Then exactly 2 runs should be running
    And 3 runs should remain queued

  @lane_scheduler
  Scenario: Global concurrency limit enforcement
    Given a LaneScheduler with global limit 2
    When I enqueue run "main-1" to lane "Main"
    And I enqueue run "main-2" to lane "Main"
    And I enqueue run "sub-1" to lane "Subagent"
    And I enqueue run "sub-2" to lane "Subagent"
    And I enqueue run "sub-3" to lane "Subagent"
    And I schedule runs until no more can be scheduled
    Then exactly 2 runs should be running
    And 3 runs should remain queued

  @lane_scheduler
  Scenario: Anti-starvation priority boost
    Given a LaneScheduler with 30 second starvation threshold
    When I enqueue run "starving-run" to lane "Cron"
    And I wait for anti-starvation conditions
    And I sweep anti-starvation
    Then the anti-starvation sweep should complete

  @lane_scheduler
  Scenario: Anti-starvation no boost below threshold
    Given a LaneScheduler with 30 second starvation threshold
    When I enqueue run "recent-run" to lane "Cron"
    And I sweep anti-starvation immediately
    Then 0 runs should receive priority boost

  @lane_scheduler
  Scenario: Recursion depth limit enforcement
    Given a LaneScheduler with max recursion depth 3
    When I spawn child "c1" from parent "p0"
    And I spawn child "c2" from parent "c1"
    And I spawn child "c3" from parent "c2"
    Then spawning child "c4" from parent "c3" should fail
    And run "c3" should have recursion depth 3

  @lane_scheduler
  Scenario: Recursion depth allows spawning below limit
    Given a LaneScheduler with max recursion depth 5
    When I spawn child "c1" from parent "p0"
    And I spawn child "c2" from parent "c1"
    And I spawn child "c3" from parent "c2"
    Then spawning child "c4" from parent "c3" should succeed
    And run "c3" should have recursion depth 3

  @lane_scheduler
  Scenario: Multiple lanes scheduling with different priorities
    Given a LaneScheduler with default config
    When I enqueue 2 runs to lane "Main"
    And I enqueue 3 runs to lane "Nested"
    And I enqueue 4 runs to lane "Subagent"
    And I enqueue 2 runs to lane "Cron"
    Then the scheduler should have 11 queued runs
    When I schedule runs until no more can be scheduled
    Then the scheduler should have 11 running runs
    And 0 runs should remain queued

  @lane_scheduler
  Scenario: Complete run releases permits
    Given a LaneScheduler with Main lane limit 2
    When I enqueue 3 runs to lane "Main"
    And I schedule runs until no more can be scheduled
    Then exactly 2 runs should be running
    When I complete run "run-1" in lane "Main"
    And I schedule the next run
    Then exactly 2 runs should be running
    And 0 runs should remain queued

  @lane_scheduler
  Scenario: Statistics tracking
    Given a LaneScheduler with default config
    When I enqueue run "main-1" to lane "Main"
    And I enqueue run "main-2" to lane "Main"
    And I enqueue run "sub-1" to lane "Subagent"
    Then the scheduler should have 3 queued runs
    And the scheduler should have 0 running runs
    When I schedule the next run
    Then the scheduler should have 2 queued runs
    And the scheduler should have 1 running run
    And lane "Main" should have 1 queued run
    And lane "Main" should have 1 running run

  @lane_scheduler
  Scenario: Recursion cleanup on completion
    Given a LaneScheduler with default config
    When I spawn child "child" from parent "parent"
    Then run "child" should have recursion depth 1
    When I complete run "child" in lane "Main"
    Then run "child" should have recursion depth 0

  @lane_scheduler
  Scenario: Multiple children from same parent
    Given a LaneScheduler with default config
    When I spawn child "child-1" from parent "parent"
    And I spawn child "child-2" from parent "parent"
    And I spawn child "child-3" from parent "parent"
    Then run "child-1" should have recursion depth 1
    And run "child-2" should have recursion depth 1
    And run "child-3" should have recursion depth 1
    And spawning child "child-4" from parent "parent" should succeed
