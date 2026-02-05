@macos
Feature: macOS Launchd Service
  As a macOS user
  I want the daemon to integrate with launchd
  So that it starts automatically and runs as a service

  Scenario: Launchd service can be created
    When I create a LaunchdService
    Then the plist path should contain "LaunchAgents"

  Scenario: Launchd service generates valid plist
    Given a default DaemonConfig
    When I create a LaunchdService
    And I generate the plist
    Then the plist should contain "com.aleph.daemon"
    And the plist should contain "RunAtLoad"
    And the plist should contain "KeepAlive"
