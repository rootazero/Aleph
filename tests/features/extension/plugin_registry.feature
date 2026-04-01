Feature: Plugin Registry
  As a plugin system developer
  I want to manage plugin registrations
  So that plugins can register tools, hooks, channels, and other components

  Background:
    Given a new plugin registry

  # =============================================================================
  # Registry Basics (4 tests)
  # =============================================================================

  Scenario: Register plugin and verify existence
    Given a test plugin "test-plugin"
    When I register the plugin
    Then the plugin "test-plugin" should exist
    And the plugin count should be 1

  Scenario: Disable and enable plugin toggles active state
    Given a test plugin "test-plugin"
    And I register the plugin
    Then the active plugin count should be 1
    When I disable the plugin "test-plugin"
    Then the active plugin count should be 0
    And the plugin "test-plugin" should exist
    When I enable the plugin "test-plugin"
    Then the active plugin count should be 1
    When I disable a non-existent plugin "non-existent"
    Then the last operation should have failed
    When I enable a non-existent plugin "non-existent"
    Then the last operation should have failed

  Scenario: Unregister plugin removes all components
    Given a test plugin "full-plugin"
    And I register the plugin
    And a tool "plugin_tool" for plugin "full-plugin"
    And I register the tool
    And a hook "BeforeAgentStart" with priority 0 for plugin "full-plugin"
    And I register the hook
    And a channel "test-channel" for plugin "full-plugin"
    And I register the channel
    And a diagnostic with level "warn" for plugin "full-plugin"
    And I add the diagnostic
    Then the plugin "full-plugin" should exist
    And the tool "plugin_tool" should exist
    And the hook count for event "BeforeAgentStart" should be 1
    And the channel "test-channel" should exist
    And the diagnostic count should be 1
    When I unregister the plugin "full-plugin"
    Then the plugin "full-plugin" should not exist
    And the tool "plugin_tool" should not exist
    And the hook count for event "BeforeAgentStart" should be 0
    And the channel "test-channel" should not exist
    And the diagnostic count should be 0

  Scenario: Clear registry removes all plugins and components
    Given a test plugin "plugin-1"
    And I register the plugin
    And a tool "tool" for plugin "plugin-1"
    And I register the tool
    When I clear the registry
    Then the plugin count should be 0
    And the tool count should be 0
    And the hook count should be 0
    And the channel count should be 0
    And the provider count should be 0
    And the gateway method count should be 0
    And the http route count should be 0
    And the http handler count should be 0
    And the cli command count should be 0
    And the service count should be 0
    And the in-chat command count should be 0
    And the diagnostic count should be 0

  # =============================================================================
  # Tool Registration (2 tests)
  # =============================================================================

  Scenario: Register tool and verify plugin updated
    Given a test plugin "tool-plugin"
    And I register the plugin
    And a tool "my_tool" for plugin "tool-plugin"
    When I register the tool
    Then the tool "my_tool" should exist
    And the tool count should be 1
    And the plugin "tool-plugin" should have tool "my_tool"

  Scenario: List tools filtered by plugin
    Given a test plugin "plugin-a"
    And I register the plugin
    Given a test plugin "plugin-b"
    And I register the plugin
    And a tool "tool_a1" for plugin "plugin-a"
    And I register the tool
    And a tool "tool_a2" for plugin "plugin-a"
    And I register the tool
    And a tool "tool_b1" for plugin "plugin-b"
    And I register the tool
    Then the tools for plugin "plugin-a" should be 2
    And the tools for plugin "plugin-b" should be 1
    And the tools for plugin "plugin-c" should be 0

  # =============================================================================
  # Hook Registration (2 tests)
  # =============================================================================

  Scenario: Hooks sorted by priority
    Given a test plugin "hook-plugin"
    And I register the plugin
    And a hook "BeforeToolCall" with priority 100 for plugin "hook-plugin"
    And I register the hook
    And a hook "BeforeToolCall" with priority -50 for plugin "hook-plugin"
    And I register the hook
    And a hook "BeforeToolCall" with priority 0 for plugin "hook-plugin"
    And I register the hook
    Then the hook count for event "BeforeToolCall" should be 3
    And the hooks for event "BeforeToolCall" should be sorted by priority as "-50,0,100"
    And the plugin "hook-plugin" hook count should be 3

  Scenario: Filter hooks by event type
    Given a test plugin "hook-plugin"
    And I register the plugin
    And a hook "BeforeToolCall" with priority 0 for plugin "hook-plugin"
    And I register the hook
    And a hook "AfterToolCall" with priority 0 for plugin "hook-plugin"
    And I register the hook
    Then the hook count for event "BeforeToolCall" should be 1
    And the hook count for event "AfterToolCall" should be 1
    And the hook count for event "SessionStart" should be 0

  # =============================================================================
  # Component Registration (7 tests)
  # =============================================================================

  Scenario: Register channel with aliases
    Given a test plugin "channel-plugin"
    And I register the plugin
    And a channel "telegram" with alias "tg" for plugin "channel-plugin"
    When I register the channel
    Then the channel "telegram" should exist
    And the channel count should be 1
    And the plugin "channel-plugin" should have channel "telegram"

  Scenario: Register provider with models
    Given a test plugin "provider-plugin"
    And I register the plugin
    And a provider "anthropic" with model "claude-opus-4-5" for plugin "provider-plugin"
    When I register the provider
    Then the provider "anthropic" should exist
    And the provider count should be 1
    And the plugin "provider-plugin" should have provider "anthropic"

  Scenario: Register gateway method
    Given a test plugin "gateway-plugin"
    And I register the plugin
    And a gateway method "myplugin.execute" for plugin "gateway-plugin"
    When I register the gateway method
    Then the gateway method "myplugin.execute" should exist
    And the gateway method count should be 1
    And the plugin "gateway-plugin" should have gateway method "myplugin.execute"

  Scenario: Register HTTP routes and handlers
    Given an http route "/api/v1/webhook" with methods "POST" for plugin "http-plugin"
    And I register the http route
    And an http handler "auth_middleware" with priority -100 for plugin "http-plugin"
    And I register the http handler
    And an http handler "logging_middleware" with priority 100 for plugin "http-plugin"
    And I register the http handler
    Then the http route count should be 1
    And the routes matching "/api/v1/webhook" should be 1
    And the http handler count should be 2
    And the http handlers should be sorted by priority as "-100,100"

  Scenario: Register CLI command
    Given a cli command "sync" for plugin "cli-plugin"
    When I register the cli command
    Then the cli command "sync" should exist
    And the cli command count should be 1

  Scenario: Register service with start/stop handlers
    Given a test plugin "service-plugin"
    And I register the plugin
    And a service "background-worker" for plugin "service-plugin"
    When I register the service
    Then the service "background-worker" should exist
    And the service count should be 1
    And the plugin "service-plugin" should have service "background-worker"

  Scenario: Register in-chat command
    Given an in-chat command "remind" for plugin "command-plugin"
    When I register the in-chat command
    Then the in-chat command "remind" should exist
    And the in-chat command count should be 1

  # =============================================================================
  # Diagnostics (1 test)
  # =============================================================================

  Scenario: Add and clear diagnostics
    Given a diagnostic with level "warn" for plugin "plugin-a"
    And I add the diagnostic
    And a diagnostic with level "error" for plugin "plugin-b"
    And I add the diagnostic
    Then the diagnostic count should be 2
    When I clear the diagnostics
    Then the diagnostic count should be 0

  # =============================================================================
  # Utility (2 tests)
  # =============================================================================

  Scenario: Get registry statistics
    Given a test plugin "plugin-1"
    And I register the plugin
    And a tool "tool" for plugin "plugin-1"
    And I register the tool
    When I get the registry stats
    Then the stats should show 1 plugins
    And the stats should show 1 active plugins
    And the stats should show 1 tools
    And the stats should show 0 hooks

  Scenario: Channels sorted by order field
    Given a channel "channel-c" with order 30 for plugin "p"
    And I register the channel
    And a channel "channel-a" with order 10 for plugin "p"
    And I register the channel
    And a channel "channel-b" with order 20 for plugin "p"
    And I register the channel
    Then the channels should be sorted by order as "channel-a,channel-b,channel-c"
