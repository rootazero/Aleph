Feature: Perception Configuration
  As a system administrator
  I want to configure perception watchers
  So that the daemon monitors relevant system events

  Scenario: Default perception config has expected values
    Given a default PerceptionConfig
    Then perception should be enabled
    And process watcher should be enabled
    And process poll interval should be 5 seconds
    And process watched apps should include "Code"
    And filesystem watcher should be enabled
    And filesystem debounce should be 500 ms
    And time watcher should be enabled
    And time heartbeat interval should be 30 seconds
    And system watcher should be enabled
    And system poll interval should be 60 seconds
    And system should track battery

  Scenario: Perception config serializes to TOML
    Given a default PerceptionConfig
    When I serialize the config to TOML
    Then the TOML should contain "enabled = true"
    And the TOML should contain "[process]"
    And the TOML should contain "[filesystem]"
