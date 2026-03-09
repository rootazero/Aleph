Feature: Basic Configuration
  As a user
  I want sensible default configurations
  So that the system works out of the box

  Scenario: Default config has expected hotkey
    Given a default Config
    Then the default_hotkey should be "Grave"
    And memory should be enabled

  Scenario: New config matches default
    Given a new Config
    Then the default_hotkey should be "Grave"

  Scenario: Memory config has expected defaults
    Given a default MemoryConfig
    Then memory should be enabled
    And the active_embedding_provider should be "siliconflow"
    And max_context_items should be 5
    And retention_days should be 90
    And vector_db should be "lancedb"
    And similarity_threshold should be 0.7
    And dreaming should be enabled
    And dreaming window_start should be "02:00"
    And dreaming window_end should be "05:00"

  Scenario: Shortcuts config has expected defaults
    Given a default ShortcutsConfig
    Then the summon shortcut should be "Command+Grave"
    And the cancel shortcut should be "Escape"

  Scenario: Behavior config has expected defaults
    Given a default BehaviorConfig
    Then the output_mode should be "typewriter"
    And typing_speed should be 50

  Scenario: Minimal config with provider passes validation
    Given a config parsed from:
      """
      [providers.openai]
      api_key = "sk-test"
      model = "gpt-4o"

      [general]
      default_provider = "openai"
      """
    Then the config should be valid
    And the default_hotkey should be "Grave"
    And smart_flow should be enabled
    And memory should be enabled
