Feature: TaskGraph Aggregate Root
  As a developer
  I want TaskGraph to act as a proper DDD Aggregate Root
  So that I can ensure the integrity of task dependencies and state transitions

  @domain @task_graph
  Scenario: TaskGraph maintains identity
    Given a new TaskGraph with ID "graph-123"
    Then the TaskGraph should have identity "graph-123"
    And it should be recognized as an Aggregate Root

  @domain @task_graph
  Scenario: TaskGraph validates its internal consistency
    Given a TaskGraph with 3 tasks
    And a dependency cycle between the tasks
    When I validate the TaskGraph
    Then the validation should fail with a "CycleDetected" error

  @domain @task_graph
  Scenario: TaskGraph tracks overall progress
    Given a TaskGraph with tasks:
      | id | status    | progress |
      | T1 | Completed | 1.0      |
      | T2 | Running   | 0.5      |
    Then the overall progress should be 0.75
