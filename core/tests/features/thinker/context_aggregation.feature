Feature: Context Aggregation
  As an AI system
  I need to combine interaction and security contexts
  So I can generate appropriate system prompts

  # This feature tests the ContextAggregator which reconciles:
  # - InteractionManifest (what's technically possible)
  # - SecurityContext (what's allowed by policy)
  # To produce a ResolvedContext with available/disabled tools

  Scenario: Web environment with standard security
    Given a web rich interaction manifest
    And a standard sandbox security context
    And tools "file_ops,exec,web_search,canvas"
    When I aggregate the context
    Then the environment contract paradigm should be "WebRich"
    And "canvas" should be available
    And "exec" should require approval

  Scenario: CLI environment with strict security
    Given a CLI interaction manifest
    And a strict readonly security context
    And tools "file_ops,exec,read"
    When I aggregate the context
    Then "file_ops" should be blocked by policy
    And "exec" should be blocked by policy
    And "read" should be available

  Scenario: Generated prompt includes environment contract
    Given a messaging interaction manifest with inline buttons
    And a permissive security context
    And tools "message,file_ops"
    When I build the system prompt with context
    Then the prompt should contain "Environment Contract"
    And the prompt should contain "Messaging"
    And the prompt should contain "inline_buttons"

  # Additional scenarios for comprehensive coverage

  Scenario: Canvas tool filtered by CLI channel
    Given a CLI interaction manifest
    And a permissive security context
    And tools "web_search,canvas,read_file"
    When I aggregate the context
    Then "web_search" should be available
    And "read_file" should be available
    And "canvas" should be unsupported by channel

  Scenario: Network tools blocked by strict security
    Given a web rich interaction manifest
    And a strict readonly security context
    And tools "web_search,http_request,read_file"
    When I aggregate the context
    Then "web_search" should be blocked by policy
    And "http_request" should be blocked by policy
    And "read_file" should be available

  Scenario: Background paradigm includes silent capability
    Given a background interaction manifest
    And a permissive security context
    And tools "notify,file_ops"
    When I aggregate the context
    Then the environment contract paradigm should be "Background"
    And the environment contract should have "silent_reply" capability

  Scenario: Prompt includes security notes for strict context
    Given a CLI interaction manifest
    And a strict readonly security context
    And tools "read_file"
    When I build the system prompt with context
    Then the prompt should contain "Security"
    And the prompt should contain "Strict"
    And the prompt should contain "Network Access: Disabled"

  Scenario: Approval-required tools in both available and disabled
    Given a web rich interaction manifest
    And a standard sandbox security context
    And tools "bash,web_search"
    When I aggregate the context
    Then "bash" should be available
    And "bash" should require approval
    And "web_search" should be available
