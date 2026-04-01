Feature: YAML Policies E2E
  As the policy system
  I want to evaluate MVP hardcoded policies
  So that automated actions trigger based on context and events

  # ==========================================================================
  # MVP Policy Tests (3 scenarios)
  # ==========================================================================

  Scenario: Meeting activity triggers auto-mute policy
    Given an MVP policy engine
    Then the engine should have 5 policies
    Given a default enhanced context
    And an activity changed event from "Idle" to "Meeting" with 5 participants
    When I evaluate all policies
    Then actions should be triggered
    And one action should be MuteSystemAudio

  Scenario: Low battery triggers notification policy
    Given an MVP policy engine
    And an enhanced context with battery level 15
    And a resource pressure changed event for battery from "Normal" to "Critical"
    When I evaluate all policies
    Then actions should be triggered
    And one action should be NotifyUser

  Scenario: Example YAML policy file exists and is valid
    Given the example policies YAML file path
    Then the file should exist
    And the file content should contain "Low Battery Alert"
    And the file content should contain "Meeting Auto-Mute"
