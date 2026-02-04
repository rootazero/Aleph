Feature: Configuration Validation
  As a system administrator
  I want configuration validation
  So that invalid configs are rejected before runtime

  # ═══ Valid Configurations ═══

  Scenario: Valid config with provider passes validation
    Given a config with a valid openai provider
    Then the config should be valid

  # ═══ Provider Validation ═══

  Scenario: Missing default provider fails validation
    Given a config with default_provider "nonexistent"
    Then the config should be invalid

  Scenario: Provider without API key fails validation
    Given a config with openai provider without api_key
    Then the config should be invalid

  Scenario: Invalid temperature fails validation
    Given a config with openai provider with temperature 3.0
    Then the config should be invalid

  Scenario: Zero timeout fails validation
    Given a config with openai provider with timeout 0
    Then the config should be invalid
    And the error should contain "timeout must be greater than 0"

  Scenario: Ollama provider without API key passes validation
    Given a config with ollama provider without api_key
    Then the config should be valid

  # ═══ Routing Rules Validation ═══

  Scenario: Invalid regex in rule fails validation
    Given a config with a valid openai provider
    And a routing rule with regex "[invalid("
    Then the config should be invalid

  Scenario: Rule referencing unknown provider fails validation
    Given a config with a routing rule referencing "nonexistent" provider
    Then the config should be invalid

  Scenario Outline: Valid regex patterns pass validation
    Given a config with a valid openai provider
    And a routing rule with regex "<pattern>"
    Then the config should be valid

    Examples:
      | pattern              |
      | .*                   |
      | ^/code               |
      | \\d+                 |
      | [a-zA-Z]+            |
      | ^test$               |

  Scenario: Valid regex pattern with pipe passes validation
    Given a config with a valid openai provider
    And a routing rule with regex "hello|world"
    Then the config should be valid

  Scenario Outline: Invalid regex patterns fail validation
    Given a config with a valid openai provider
    And a routing rule with regex "<pattern>"
    Then the config should be invalid

    Examples:
      | pattern      |
      | [invalid(    |
      | (unclosed    |
      | **           |
      | [z-a]        |

  # ═══ Memory Config Validation ═══

  Scenario: Zero max_context_items fails validation
    Given a config with memory max_context_items 0
    Then the config should be invalid
    And the error should contain "max_context_items must be greater than 0"

  Scenario: Invalid similarity threshold fails validation
    Given a config with memory similarity_threshold 1.5
    Then the config should be invalid
    And the error should contain "similarity_threshold must be between 0.0 and 1.0"

  Scenario: Invalid dreaming window fails validation
    Given a config with dreaming window_start "25:00"
    Then the config should be invalid
    And the error should contain "window_start_local must be HH:MM"

  Scenario: Invalid graph decay fails validation
    Given a config with graph_decay node_decay_per_day 1.5
    Then the config should be invalid
    And the error should contain "graph_decay.node_decay_per_day"
