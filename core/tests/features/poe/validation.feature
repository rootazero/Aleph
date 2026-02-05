Feature: POE Validation Rules
  As an AI agent
  I want various validation rules
  So that I can verify different aspects of task completion

  # ═══ File Content Validation ═══

  Scenario: FileContains validates regex pattern
    Given a temporary directory
    And a file "source.rs" with content "fn main() { println!(\"Hello\"); }"
    And a POE task requiring file "source.rs" to contain pattern "fn\s+main\(\)"
    When I execute the POE task
    Then the outcome should be Success

  Scenario: FileNotContains validates absence of pattern
    Given a temporary directory
    And a file "safe.rs" with content "fn safe_function() {}"
    And a POE task requiring file "safe.rs" to not contain pattern "unsafe\s*\{"
    When I execute the POE task
    Then the outcome should be Success

  # ═══ Directory Structure ═══

  Scenario: Directory structure validation passes
    Given a temporary directory
    And directories "src" and "tests" exist
    And a file "Cargo.toml" with content "[package]"
    And a POE task requiring directory structure "src/, tests/, Cargo.toml"
    When I execute the POE task
    Then the outcome should be Success

  # ═══ JSON Schema Validation ═══

  Scenario: JSON schema validation passes for valid JSON
    Given a temporary directory
    And a file "config.json" with content '{"name": "test", "version": "1.0.0"}'
    And a POE task requiring file "config.json" to match schema with fields "name:string, version:string"
    When I execute the POE task
    Then the outcome should be Success

  Scenario: JSON schema validation fails for missing field
    Given a temporary directory
    And a file "bad.json" with content '{"name": "test"}'
    And a POE task requiring file "bad.json" to match schema with fields "name:string, version:string"
    When I execute the POE task
    Then the outcome should be BudgetExhausted or StrategySwitch

  # ═══ Mixed Validation ═══

  Scenario: Mixed validation rules with partial failure
    Given a temporary directory
    And a file "exists.txt" with content "content"
    And a POE task requiring file "exists.txt" and "missing.txt" to both exist with max 2 attempts
    When I execute the POE task
    Then the outcome should be BudgetExhausted or StrategySwitch
