Feature: Perception Watchers
  As a daemon component
  I want different types of watchers
  So that the daemon can monitor various system events

  # ═══ Time Watcher ═══

  Scenario: Time watcher emits heartbeat events
    Given a TimeWatcher with heartbeat interval 1 second
    And an event bus with capacity 10
    When I start the time watcher
    Then I should receive a heartbeat event within 2 seconds

  Scenario: Time watcher is not pausable
    Given a TimeWatcher with heartbeat interval 30 seconds
    Then the watcher id should be "time"
    And the watcher should not be pausable

  # ═══ Process Watcher ═══

  Scenario: Process watcher can be created
    Given a ProcessWatcher watching "Code"
    Then the watcher id should be "process"
    And the watcher should not be pausable

  # ═══ System Watcher ═══

  Scenario: System watcher can be created
    Given a SystemStateWatcher with poll interval 60 seconds
    Then the watcher id should be "system"
    And the watcher should not be pausable

  # ═══ Filesystem Watcher ═══

  Scenario: Filesystem watcher can be created and is pausable
    Given a FSEventWatcher watching "/tmp"
    Then the watcher id should be "filesystem"
    And the watcher should be pausable
