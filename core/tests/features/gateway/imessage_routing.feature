Feature: iMessage Gateway Routing
  Integration tests for the iMessage Gateway routing system.
  Tests the complete message flow from InboundMessage through policy filtering.

  # ==========================================================================
  # DM Policy Tests
  # ==========================================================================

  @dm-policy
  Scenario: DM with open policy allows any sender
    Given an iMessage router with open DM policy
    When a DM message arrives from "+15551234567" with text "Hello!"
    Then the message should be accepted

  @dm-policy @allowlist
  Scenario: DM with allowlist policy allows listed sender
    Given an iMessage router with allowlist DM policy
    And the DM allowlist contains "+15551234567"
    When a DM message arrives from "+15551234567" with text "Hello!"
    Then the message should be accepted

  @dm-policy @allowlist
  Scenario: DM with allowlist policy denies unlisted sender
    Given an iMessage router with allowlist DM policy
    And the DM allowlist contains "+15559999999"
    When a DM message arrives from "+15551234567" with text "Hello!"
    Then the message should be filtered

  @dm-policy @pairing
  Scenario: DM with pairing policy triggers pairing request
    Given an iMessage router with pairing DM policy
    When a DM message arrives from "+15551234567" with text "Hello!"
    Then the message should be accepted
    And a pairing request should exist for sender containing "15551234567"

  @dm-policy @pairing
  Scenario: DM with pre-approved pairing passes through
    Given an iMessage router with pairing DM policy
    And sender "+15551234567" is pre-approved for pairing
    When a DM message arrives from "+15551234567" with text "Hello!"
    Then the message should be accepted

  # ==========================================================================
  # Group Policy Tests
  # ==========================================================================

  @group-policy @mention
  Scenario: Group message without mention is filtered when mention required
    Given an iMessage router with open group policy requiring mention
    And the bot name is "Aleph"
    When a group message arrives in "chat_id:42" from "+15551234567" with text "Hello everyone!"
    Then the message should be filtered

  @group-policy @mention
  Scenario: Group message with mention passes when mention required
    Given an iMessage router with open group policy requiring mention
    And the bot name is "Aleph"
    When a group message arrives in "chat_id:42" from "+15551234567" with text "Hey @aleph, help me!"
    Then the message should be accepted

  @group-policy
  Scenario: Group message is filtered when group policy disabled
    Given an iMessage router with disabled group policy
    When a group message arrives in "chat_id:42" from "+15551234567" with text "@aleph help!"
    Then the message should be filtered

  # ==========================================================================
  # DM Scope Tests
  # ==========================================================================

  @dm-scope
  Scenario: DM from multiple senders with PerPeer scope both allowed
    Given an iMessage router with open DM policy and PerPeer scope
    When a DM message arrives from "+15551111111" with text "Hello from user 1"
    Then the message should be accepted
    When a DM message arrives from "+15552222222" with text "Hello from user 2"
    Then the message should be accepted

  @dm-scope
  Scenario: DM from multiple senders with Main scope share session
    Given an iMessage router with open DM policy and Main scope
    When a DM message arrives from "+15551111111" with text "Hello"
    Then the message should be accepted
    When a DM message arrives from "+15552222222" with text "Hi"
    Then the message should be accepted

  # ==========================================================================
  # Group Allowlist Tests
  # ==========================================================================

  @group-policy @allowlist
  Scenario: Group message allowed when group in allowlist
    Given an iMessage router with allowlist group policy
    And the group allowlist contains "chat_id:42"
    When a group message arrives in "chat_id:42" from "+15551234567" with text "Hello!"
    Then the message should be accepted

  @group-policy @allowlist
  Scenario: Group message denied when group not in allowlist
    Given an iMessage router with allowlist group policy
    And the group allowlist contains "other_chat:99"
    When a group message arrives in "chat_id:42" from "+15551234567" with text "Hello!"
    Then the message should be filtered
