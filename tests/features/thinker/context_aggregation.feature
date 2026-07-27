Feature: Context Aggregation
  As an AI system
  I need to combine interaction and security contexts
  So I can generate appropriate system prompts

  # This feature tests the ContextAggregator, which reconciles:
  # - InteractionManifest (what the channel can render)
  # - SecurityContext (what policy allows)
  # into the EnvironmentContract the system prompt states.
  #
  # The scenarios that asserted per-tool availability were removed 2026-07-27
  # along with the two-phase tool filter itself: production never fed it a tool
  # list, and the enforced permission model lives in src/tools/scoped/.

  Scenario: Generated prompt includes environment contract
    Given a messaging interaction manifest with inline buttons
    And a permissive security context
    When I build the system prompt with context
    Then the prompt should contain "Environment Contract"
    And the prompt should contain "Messaging"
    And the prompt should contain "inline_buttons"

  Scenario: Background paradigm includes silent capability
    Given a background interaction manifest
    And a permissive security context
    When I aggregate the context
    Then the environment contract paradigm should be "Background"
    And the environment contract should have "silent_reply" capability

  Scenario: Prompt includes security notes for strict context
    Given a CLI interaction manifest
    And a strict readonly security context
    When I build the system prompt with context
    Then the prompt should contain "Security"
    And the prompt should contain "Strict"
    And the prompt should contain "Network Access: Disabled"
