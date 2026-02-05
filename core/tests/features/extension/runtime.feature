Feature: Plugin Runtime Integration
  As the extension system
  I want to manage plugin lifecycle and execution
  So that plugins can be loaded, executed, and unloaded correctly

  # =============================================================================
  # Manifest Parsing (4 tests)
  # =============================================================================

  Scenario: Parse JSON manifest content
    Given a JSON manifest content:
      """
      {
        "id": "test-plugin",
        "name": "Test Plugin",
        "version": "1.0.0",
        "kind": "nodejs",
        "entry": "index.js"
      }
      """
    When I parse the JSON manifest
    Then the manifest id should be "test-plugin"
    And the manifest name should be "Test Plugin"
    And the manifest version should be "1.0.0"

  Scenario: Parse minimal JSON manifest
    Given a JSON manifest content:
      """
      {"id": "minimal-plugin"}
      """
    When I parse the JSON manifest
    Then the manifest id should be "minimal-plugin"
    And the manifest name should be "minimal-plugin"
    And the manifest version should be empty

  Scenario: Invalid plugin id is rejected
    Given a JSON manifest content:
      """
      {"id": "Invalid-Plugin"}
      """
    When I parse the JSON manifest expecting error
    Then the parse should have failed

  Scenario: Missing plugin id is rejected
    Given a JSON manifest content:
      """
      {"name": "Test Plugin"}
      """
    When I parse the JSON manifest expecting error
    Then the parse should have failed

  # =============================================================================
  # Directory-based Manifest Loading (2 tests)
  # =============================================================================

  Scenario: Load manifest from directory with aleph.plugin.json
    Given a temp directory with manifest files:
      | file              | content                                                    |
      | aleph.plugin.json | {"id": "my-plugin", "name": "My Plugin", "version": "2.0.0", "kind": "nodejs"} |
    When I parse the manifest from the directory
    Then the manifest id should be "my-plugin"
    And the manifest name should be "My Plugin"
    And the manifest version should be "2.0.0"
    And the manifest root_dir should match the temp directory

  Scenario: Directory without manifest returns error
    Given an empty temp directory
    When I parse the manifest from the directory expecting error
    Then the parse should have failed

  # =============================================================================
  # Extension Manager (4 tests)
  # =============================================================================

  Scenario: Extension manager registry access
    Given an extension manager with default config
    When I get the plugin registry
    Then the registry should be empty
    And the registry tools should be empty

  Scenario: Extension manager loader access
    Given an extension manager with default config
    When I get the plugin loader
    Then no runtime should be active
    And no plugins should be loaded

  Scenario: Plugin not found error
    Given an extension manager with default config
    When I call tool on non-existent plugin "nonexistent-plugin"
    Then the error should be PluginNotFound with id "nonexistent-plugin"

  Scenario: Hook execution on non-existent plugin
    Given an extension manager with default config
    When I execute hook on non-existent plugin "nonexistent-plugin"
    Then the error should be PluginNotFound with id "nonexistent-plugin"

  # =============================================================================
  # Plugin Loader (2 tests)
  # =============================================================================

  Scenario: Plugin loader standalone initialization
    Given a new plugin loader
    Then no runtime should be active
    And nodejs runtime should not be active
    And wasm runtime should not be active
    And loaded plugin count should be 0

  Scenario: Unload non-existent plugin returns error
    Given a new plugin loader
    When I unload plugin "nonexistent"
    Then the error should be PluginNotFound with id "nonexistent"

  # =============================================================================
  # Plugin Registry (1 test)
  # =============================================================================

  Scenario: Plugin registry standalone initialization
    Given a new standalone plugin registry
    Then the registry should be empty
    And the registry tools should be empty
    And the registry hooks should be empty
    And the registry stats plugins should be 0
    And the registry stats tools should be 0
    And the registry stats hooks should be 0

  # =============================================================================
  # Full Flow (ignored - requires Node.js)
  # =============================================================================

  @ignore @requires-nodejs
  Scenario: Full Node.js plugin flow
    Given a temp directory with a test Node.js plugin
    And an extension manager with node runtime enabled
    When I load the plugin from the directory
    Then the plugin should be loaded successfully
    When I call tool "handleEcho" with message "Hello, Plugin!"
    Then the tool result should contain echoed "Hello, Plugin!"
    And the tool result should contain timestamp
