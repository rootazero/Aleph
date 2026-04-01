Feature: Daemon Core Components
  As a system service
  I want reliable event bus, CLI parsing, and resource management
  So that the daemon operates correctly

  # ═══ Event Bus ═══

  Scenario: Event bus sends and receives events
    Given an event bus with capacity 10
    And a subscriber to the event bus
    When I send a heartbeat event
    Then the subscriber should receive a heartbeat event

  Scenario: Event bus supports multiple subscribers
    Given an event bus with capacity 10
    And 2 subscribers to the event bus
    When I send a heartbeat event
    Then all subscribers should receive a heartbeat event

  # ═══ CLI Parsing ═══

  Scenario: CLI parses install command
    When I parse CLI arguments "daemon install"
    Then the CLI parsing should succeed
    And the command should be Install

  # ═══ Service Manager ═══

  Scenario: Service manager trait can be implemented
    Given a mock service manager
    When I query the service status
    Then the query should succeed

  # ═══ Resource Governor ═══

  Scenario: Resource governor can be created with custom limits
    Given a resource governor with CPU threshold 20.0
    Then the governor CPU threshold should be 20.0

  Scenario: Resource governor makes proceed or throttle decisions
    Given a resource governor with default limits
    When I check the governor decision
    Then the decision should be Proceed or Throttle
