Feature: Inbound Message Router
  The InboundMessageRouter handles incoming messages from various channels,
  resolves session keys, checks permissions, and executes agents.

  # =========================================================================
  # Session Key Resolution Tests
  # =========================================================================

  @session-key
  Scenario: DM message with PerPeer scope resolves to peer session key
    Given a router with DmScope PerPeer
    And a DM message from "+15551234567"
    When I resolve the session key
    Then the session key should be "agent:main:peer:dm:+15551234567"

  @session-key
  Scenario: DM message with Main scope resolves to main session key
    Given a router with DmScope Main
    And a DM message from "+15551234567"
    When I resolve the session key
    Then the session key should be "agent:main:main"

  @session-key
  Scenario: Group message resolves to group session key
    Given a basic inbound router
    And a group message with conversation "chat_id:42"
    When I resolve the session key
    Then the session key should be "agent:main:peer:imessage:group:chat_id:42"

  # =========================================================================
  # Allowlist Tests
  # =========================================================================

  @allowlist
  Scenario: Sender in allowlist is permitted
    Given a basic inbound router
    And an allowlist containing "+15551234567" and "user@example.com"
    When I check if "+15551234567" is in the allowlist
    Then the sender should be allowed
    When I check if "5551234567" is in the allowlist
    Then the sender should be allowed
    When I check if "user@example.com" is in the allowlist
    Then the sender should be allowed
    When I check if "+19999999999" is in the allowlist
    Then the sender should not be allowed

  @allowlist
  Scenario: Wildcard allowlist permits all senders
    Given a basic inbound router
    And an allowlist containing "*"
    When I check if "+19999999999" is in the allowlist
    Then the sender should be allowed

  # =========================================================================
  # Mention Detection Tests
  # =========================================================================

  @mention
  Scenario: Bot mention is detected in various formats
    Given a basic inbound router
    And a channel config with bot_name "MyBot"
    When I check for mention in "Hey @aleph, help me"
    Then a mention should be detected
    When I check for mention in "MyBot can you help?"
    Then a mention should be detected
    When I check for mention in "Hello ALEPH"
    Then a mention should be detected
    When I check for mention in "Hello world"
    Then a mention should not be detected

  # =========================================================================
  # Execution Integration Tests
  # =========================================================================

  @execution
  Scenario: Execute without execution support gracefully degrades
    Given a basic inbound router
    And a DM message from "+15551234567"
    And a test context for the message
    When I execute for the context
    Then the execution should succeed with graceful degradation

  @execution
  Scenario: Execute with empty agent registry returns AgentNotFound
    Given a router with execution support but empty registry
    And a DM message from "+15551234567"
    And a test context for the message
    When I execute for the context
    Then the execution should fail with AgentNotFound "main"

  @execution
  Scenario: Execute with registered agent calls adapter
    Given a router with execution support and registered agent "main"
    And a DM message from "+15551234567"
    And a test context for the message
    When I execute for the context
    Then the execution should succeed
    And the execution adapter should have been called once

  @execution
  Scenario: SimpleExecutionEngine can be used as trait object
    Given a SimpleExecutionEngine
    When I get status for run "nonexistent-run"
    Then the status should be None
    When I cancel run "nonexistent-run"
    Then the cancel should fail with RunNotFound

  @execution
  Scenario: Router with execution configured returns AgentNotFound not graceful degradation
    Given a router with execution support but empty registry
    And a DM message from "+15551234567"
    And a test context for the message
    When I execute for the context
    Then the execution should fail with AgentNotFound "main"

  @execution
  Scenario: Router backward compatible with new constructor
    Given a basic inbound router
    And a DM message from "+15551234567"
    When I register channel config "test" with default settings
    And I resolve the session key
    Then the session key should contain "main"

  # =========================================================================
  # Unified Routing Tests
  # =========================================================================

  @unified-routing
  Scenario: Unified routing respects AgentRouter bindings
    Given a router with unified routing
    And agent "work" is registered
    And a binding "imessage:*" to agent "work"
    When I resolve agent ID for channel "imessage"
    Then the resolved agent ID should be "work"
    When I resolve agent ID for channel "telegram"
    Then the resolved agent ID should be "main"

  @unified-routing
  Scenario: Unified routing falls back to default when no router configured
    Given a router with default agent "assistant" and no agent router
    When I resolve agent ID for channel "imessage"
    Then the resolved agent ID should be "assistant"

  @unified-routing
  Scenario: with_unified_routing constructor sets all fields correctly
    Given a router with unified routing
    And a binding "test:*" to agent "custom"
    When I resolve agent ID for channel "test:channel"
    Then the resolved agent ID should be "custom"
