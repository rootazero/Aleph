Feature: Watcher Registry
  As a daemon component
  I want a registry to manage watchers
  So that watchers can be started and stopped together

  Scenario: Registry starts empty
    Given a new WatcherRegistry
    Then the registry watcher count should be 0

  Scenario: Empty registry can start and shutdown
    Given a new WatcherRegistry
    And an event bus with capacity 10
    When I start all watchers
    And I shutdown all watchers
    Then the operation should succeed
