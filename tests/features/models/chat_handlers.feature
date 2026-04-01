Feature: Models and Chat RPC Handlers
  The models.* and chat.* RPC handlers manage AI model listings,
  capabilities, and chat parameter deserialization.

  # =========================================================================
  # models.list Tests
  # =========================================================================

  @models-list
  Scenario: models.list with empty config returns empty models array
    Given an empty config for models testing
    When I call models.list with no params
    Then the response should be successful
    And the models array should be empty

  @models-list
  Scenario: models.list with providers configured returns all models
    Given a config with providers "openai" "claude" and "gemini"
    And the default provider is "openai"
    When I call models.list with no params
    Then the response should be successful
    And the models array should have 3 models
    And each model should have required fields
    And one model should be marked as default with provider "openai"

  @models-list
  Scenario: models.list with enabled_only filter excludes disabled providers
    Given a config with enabled provider "openai" and disabled provider "claude"
    When I call models.list with enabled_only filter
    Then the response should be successful
    And the models array should have 1 model
    And the model provider should be "openai"

  @models-list
  Scenario: models.list with provider filter returns only matching provider
    Given a config with providers "openai" and "claude"
    When I call models.list with provider filter "claude"
    Then the response should be successful
    And the models array should have 1 model
    And the model provider should be "claude"

  # =========================================================================
  # models.get Tests
  # =========================================================================

  @models-get
  Scenario: models.get with existing provider returns model info
    Given a config with provider "openai" as default
    When I call models.get for provider "openai"
    Then the response should be successful
    And the returned model id should be "gpt-4o"
    And the returned model provider should be "openai"
    And the returned model provider_type should be "openai"
    And the returned model should be enabled
    And the returned model should be marked as default
    And the returned model capabilities should include "chat" "vision" and "tools"

  @models-get
  Scenario: models.get with non-existent provider returns error
    Given an empty config for models testing
    When I call models.get for provider "nonexistent"
    Then the response should have an error
    And the models error should contain "not found"

  @models-get
  Scenario: models.get without required params returns error
    Given an empty config for models testing
    When I call models.get with no params
    Then the response should have an error
    And the models error should contain "params"

  # =========================================================================
  # models.capabilities Tests
  # =========================================================================

  @models-capabilities
  Scenario: models.capabilities with Claude provider returns all capabilities
    Given a config with provider "claude" with model "claude-3-5-sonnet-20241022"
    When I call models.capabilities for provider "claude"
    Then the response should be successful
    And the capabilities should include "chat"
    And the capabilities should include "vision"
    And the capabilities should include "tools"
    And the capabilities should include "thinking"

  @models-capabilities
  Scenario: models.capabilities with Gemini provider returns thinking capability
    Given a config with provider "gemini" with model "gemini-2.0-flash"
    When I call models.capabilities for provider "gemini"
    Then the response should be successful
    And the capabilities should include "chat"
    And the capabilities should include "thinking"

  @models-capabilities
  Scenario: models.capabilities with non-existent provider returns error
    Given an empty config for models testing
    When I call models.capabilities for provider "nonexistent"
    Then the response should have an error

  # =========================================================================
  # chat.* Param Deserialization Tests
  # =========================================================================

  @chat-params
  Scenario: SendParams deserialization with all fields
    Given JSON for SendParams with all fields
    When I deserialize the SendParams
    Then the message should be "Hello, world!"
    And the session_key should be "agent:main:main"
    And the channel should be "gui:window1"
    And stream should be true
    And thinking should be "high"

  @chat-params
  Scenario: SendParams deserialization with minimal fields uses defaults
    Given JSON for SendParams with only message "Test message"
    When I deserialize the SendParams
    Then the message should be "Test message"
    And the session_key should be none
    And the channel should be none
    And stream should be true
    And thinking should be none

  @chat-params
  Scenario: SendParams deserialization with stream false
    Given JSON for SendParams with stream false
    When I deserialize the SendParams
    Then stream should be false

  @chat-params
  Scenario: HistoryParams deserialization with all fields
    Given JSON for HistoryParams with all fields
    When I deserialize the HistoryParams
    Then the history session_key should be "agent:main:main"
    And the limit should be 100
    And the before should be "2024-01-01T00:00:00Z"

  @chat-params
  Scenario: HistoryParams deserialization with minimal fields
    Given JSON for HistoryParams with only session_key "agent:main:task:cron:daily"
    When I deserialize the HistoryParams
    Then the history session_key should be "agent:main:task:cron:daily"
    And the limit should be none
    And the before should be none

  @chat-params
  Scenario: ClearParams deserialization with all fields
    Given JSON for ClearParams with keep_system false
    When I deserialize the ClearParams
    Then the clear session_key should be "agent:main:main"
    And keep_system should be false

  @chat-params
  Scenario: ClearParams deserialization with defaults
    Given JSON for ClearParams with only session_key
    When I deserialize the ClearParams
    Then the clear session_key should be "agent:main:main"
    And keep_system should be true

  # === Model Discovery Scenarios ===

  Scenario: List models returns models with source field
    Given a config with provider "openai" with model "gpt-4o"
    When I call models.list with no params
    Then the response should be successful
    And the models array should contain models with source field

  Scenario: Anthropic returns preset models
    Given a config with provider "anthropic" with model "claude-opus-4-20250514"
    When I call models.list with provider filter "anthropic"
    Then the response should be successful
    And the models should include preset models

  Scenario: Refresh for single provider
    Given a config with provider "openai" with model "gpt-4o"
    When I call models.refresh for provider "openai"
    Then the response should be successful

  Scenario: Refresh unknown provider returns error
    Given an empty config for models testing
    When I call models.refresh for provider "nonexistent"
    Then the response should have an error

  Scenario: Set model backward compatibility
    Given a mutable config with providers "openai" and "anthropic" default "openai"
    When I call models.set with model "anthropic"
    Then the response should be successful
