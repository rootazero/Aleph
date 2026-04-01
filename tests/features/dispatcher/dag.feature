Feature: DAG Scheduler Integration
  As the dispatcher subsystem
  I want to evaluate risk, manage task context, and execute DAG-based task graphs
  So that multi-step tasks are executed safely and in the correct order

  # ========================================================================
  # Risk Evaluator Tests
  # ========================================================================

  Scenario: AI inference tasks are low risk
    Given a risk evaluator
    When I evaluate an AI task "Analyze text" with prompt "Analyze: {input}"
    Then the risk level should be "low"

  Scenario: Code execution tasks are high risk
    Given a risk evaluator
    When I evaluate a code task "Execute shell command" with code "echo test"
    Then the risk level should be "high"

  Scenario: Tasks with API keywords are high risk
    Given a risk evaluator
    When I evaluate an AI task "Call external API to fetch data" with prompt "Fetch data"
    Then the risk level should be "high"

  Scenario: Tasks with HTTP keywords are high risk
    Given a risk evaluator
    When I evaluate an AI task "Make HTTP request to server" with prompt "Request data"
    Then the risk level should be "high"

  # ========================================================================
  # Task Context Tests
  # ========================================================================

  Scenario: Task context creation with user input
    Given a task context with user input "User wants to analyze document"
    When I build prompt context for "task_1" with no dependencies
    Then the prompt context should contain "User wants to analyze document"

  Scenario: Task context records output for dependent tasks
    Given a task context with user input "Test input"
    And task "task_1" has output "First result"
    When I build prompt context for "task_2" depending on "task_1"
    Then the prompt context should contain "First result"

  Scenario: Task context records named output for explicit reference
    Given a task context with user input "Test input"
    And task "task_1" named "Analysis Task" has output "Analysis complete"
    When I build prompt context for "task_2" depending on "task_1"
    Then the prompt context should contain "Analysis complete"

  # ========================================================================
  # DAG Task Plan Tests
  # ========================================================================

  Scenario: Task plan created from graph
    Given a task graph "plan_1" titled "Test Plan"
    And the graph has AI task "t1" named "First task"
    And the graph has AI task "t2" named "Second task"
    And task "t1" depends on "t2"
    When I create a task plan without confirmation required
    Then the plan should have id "plan_1"
    And the plan should have title "Test Plan"
    And the plan should have 2 tasks
    And the plan should not require confirmation

  Scenario: Task plan detects high risk tasks
    Given a task graph "plan_1" titled "High Risk Plan"
    And the graph has code task "t1" named "Run code" with code "print('hello')"
    When I create a task plan with confirmation required
    Then the plan should have high risk tasks
    And the plan should require confirmation

  # ========================================================================
  # DAG Task Info Tests
  # ========================================================================

  Scenario: Task info creation with dependencies
    Given a task info "task_1" named "Read file" with status "pending" and risk "low"
    And the task info has dependency "task_0"
    Then the task info id should be "task_1"
    And the task info name should be "Read file"
    And the task info status should be "pending"
    And the task info risk level should be "low"
    And the task info should have dependency "task_0"

  Scenario: Task info with high risk
    Given a task info "task_1" named "Execute command" with status "running" and risk "high"
    Then the task info risk level should be "high"

  # ========================================================================
  # Task Display Status Tests
  # ========================================================================

  Scenario Outline: Task display status string representation
    Given a task display status <status>
    Then the status string should be "<expected>"

    Examples:
      | status    | expected  |
      | pending   | pending   |
      | running   | running   |
      | completed | completed |
      | failed    | failed    |
      | cancelled | cancelled |

  # ========================================================================
  # NoOp Callback Tests
  # ========================================================================

  Scenario: NoOp callback methods complete without error
    Given a no-op execution callback
    And an empty task plan "test" titled "Test Plan"
    When I call all callback methods
    Then all callback methods should complete without error
    And confirmation should return "confirmed"

  # ========================================================================
  # DAG Scheduler Integration Tests
  # ========================================================================

  Scenario: Linear DAG execution
    Given a task graph "linear_plan" titled "Linear Execution"
    And the graph has AI task "t1" named "Task 1"
    And the graph has AI task "t2" named "Task 2"
    And the graph has AI task "t3" named "Task 3"
    And task "t1" depends on "t2"
    And task "t2" depends on "t3"
    And a mock task executor
    And a collecting callback
    And a task context with user input "Test input"
    When I execute the graph
    Then the DAG execution should succeed
    And 3 tasks should be executed
    And 3 tasks should be completed
    And 0 tasks should have failed
    And the execution should not be cancelled
    And plan_ready callback should be called 1 time
    And task_start callback should be called 3 times
    And task_complete callback should be called 3 times
    And all_complete callback should be called 1 time

  Scenario: Parallel DAG execution
    Given a task graph "parallel_plan" titled "Parallel Execution"
    And the graph has AI task "t1" named "Task 1"
    And the graph has AI task "t2" named "Task 2"
    And the graph has AI task "t3" named "Task 3"
    And the graph has AI task "t4" named "Task 4"
    And task "t1" depends on "t2"
    And task "t1" depends on "t3"
    And task "t2" depends on "t4"
    And task "t3" depends on "t4"
    And a mock task executor
    And a collecting callback
    And a task context with user input "Test input"
    When I execute the graph
    Then the DAG execution should succeed
    And 4 tasks should be executed
    And 4 tasks should be completed

  # ========================================================================
  # End-to-End Tests
  # ========================================================================

  Scenario: Full workflow risk evaluation
    Given a task graph "mixed_risk" titled "Mixed Risk Plan"
    And the graph has AI task "t1" named "Analyze document"
    And the graph has code task "t2" named "Execute analysis script" with code "analyze()"
    And task "t1" depends on "t2"
    And a risk evaluator
    When I evaluate the graph for risk
    Then the graph should have high risk
    When I create a task plan with high risk flag
    Then the plan should require confirmation
    And the plan should have high risk tasks

  Scenario: Context propagation through tasks
    Given a task context with user input "Analyze the following text and summarize"
    And task "analysis" named "Document Analysis" has output "The text discusses AI safety"
    When I build prompt context for "summarize" depending on "analysis"
    Then the prompt context should contain "AI safety"
    When task "summarize" has output "Summary: AI safety is important"
    And I build prompt context for "generate_report" depending on "summarize"
    Then the prompt context should contain "Summary"
