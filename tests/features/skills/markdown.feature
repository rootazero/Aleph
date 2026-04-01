Feature: Markdown Skills System
  As the Aleph skill system
  I want to support markdown-based skill definitions
  So that skills can be defined, loaded, and managed dynamically

  # ==========================================================================
  # RPC Handler Registration Tests
  # From: markdown_skills_rpc_test.rs
  # ==========================================================================

  Scenario: All markdown skills handlers are registered
    Given a handler registry
    When I check registered markdown_skills handlers
    Then the markdown_skills.load handler should be registered
    And the markdown_skills.reload handler should be registered
    And the markdown_skills.list handler should be registered
    And the markdown_skills.unload handler should be registered

  Scenario: Load skill success
    Given a handler registry
    And a temp directory with a valid skill
    When I send markdown_skills.load with the skill path
    Then the skills RPC response should be successful
    And the response result count should be 1
    And the response skills array should have 1 items
    And the first skill name should be "test-skill"
    And the first skill description should be "Test skill for integration testing"
    And the first skill sandbox_mode should be "host"

  Scenario: Load skill invalid path returns error
    Given a handler registry
    When I send markdown_skills.load with invalid path
    Then the skills RPC response should be an error

  Scenario: List skills returns array
    Given a handler registry
    When I send markdown_skills.list
    Then the skills RPC response should be successful
    And the response result should have skills array
    And the response result should have count number

  Scenario: Reload nonexistent skill returns error
    Given a handler registry
    When I send markdown_skills.reload for skill "nonexistent-skill"
    Then the skills RPC response should be an error
    And the skills RPC error message should contain "not found"

  Scenario: Unload skill returns removed status
    Given a handler registry
    When I send markdown_skills.unload for skill "test-skill"
    Then the skills RPC response should be successful
    And the response result should have removed boolean

  Scenario: Load and reload skill flow
    Given a handler registry
    And a temp directory with a reload-test skill
    When I send markdown_skills.load with the skill path
    Then the skills RPC response should be successful
    When I update the skill description to "Updated description"
    And I send markdown_skills.reload for skill "reload-test"
    Then the skills RPC response should be successful
    And the reload was_replaced should be true
    And the reloaded skill description should be "Updated description"

  Scenario: Missing params returns error for load
    Given a handler registry
    When I send markdown_skills.load without params
    Then the skills RPC response should be an error
    And the skills RPC error message should contain "Missing params"

  Scenario: Missing params returns error for reload
    Given a handler registry
    When I send markdown_skills.reload without params
    Then the skills RPC response should be an error

  Scenario: Missing params returns error for unload
    Given a handler registry
    When I send markdown_skills.unload without params
    Then the skills RPC response should be an error

  # ==========================================================================
  # Skill Loading Integration Tests
  # From: markdown_skill_integration.rs
  # ==========================================================================

  Scenario: Load OpenClaw compatible skill
    Given the echo-basic fixture skill
    When I load skills from the directory
    Then the loaded tools count should be 1
    And the first loaded tool name should be "echo-basic"
    And the first loaded tool description should be "Basic echo command (OpenClaw compatible)"
    And the first loaded tool should require bin "echo"

  Scenario: Load Aleph enhanced skill with Docker
    Given the gh-pr-docker fixture skill
    When I load skills from the directory
    Then the loaded tools count should be 1
    And the first loaded tool name should be "gh-pr-docker"
    And the first loaded tool should have aleph extensions
    And the first loaded tool sandbox should be docker
    And the docker image should be "ghcr.io/cli/cli:latest"
    And the docker env_vars should include "GITHUB_TOKEN"
    And the input_hints should have key "action"
    And the input_hints should have key "repo"

  Scenario: Partial failure tolerance
    Given the markdown skills fixtures directory
    When I load skills with error handling
    Then the loaded tools count should be 2
    And the load errors count should be 1
    And the first error path should contain "invalid-yaml"

  Scenario: Tool definition includes LLM context
    Given the echo-basic fixture skill
    When I load skills from the directory
    Then the tool definition should have llm_context
    And the skill llm_context should contain "echo"

  Scenario: Tool server integration
    Given the echo-basic fixture skill
    When I create a tool server with the skill
    Then the tool server should have tool "echo-basic"
    And the server tool definition name should be "echo-basic"

  Scenario: Echo skill execution
    Given the echo-basic fixture skill
    When I load skills from the directory
    And I execute the echo skill with Hello World
    Then the execution result should contain Hello

  Scenario: Schema generation with input hints
    Given the gh-pr-docker fixture skill
    When I load skills from the directory
    Then the schema properties should have key "action"
    And the schema properties should have key "repo"
    And the schema properties should have key "number"
    And the schema required should include "action"
    And the schema required should include "repo"
    And the schema required should not include "number"

  # ==========================================================================
  # Hot Reload Tests
  # From: markdown_skill_hot_reload.rs
  # ==========================================================================

  Scenario: Watcher detects skill creation
    Given an empty temp directory for skills
    And a watcher config with debounce 100ms
    When I start a skill watcher
    And I create a new skill file
    Then the reloaded tools should not be empty
    And the first reloaded tool name should be "test-skill"

  Scenario: Watcher detects skill modification
    Given a skill directory with an existing skill
    And a watcher config with debounce 100ms
    When I start a counting skill watcher
    And I modify the existing skill file
    Then the reload count should be greater than 0

  Scenario: Watcher config defaults
    Then the default watcher config debounce should be 500ms
    And the default watcher config emit_initial_events should be false

  Scenario: Watcher ignores non-skill files
    Given an empty temp directory for skills
    And a watcher config with debounce 100ms
    When I start a counting skill watcher
    And I create non-skill files
    Then the reload count should be 0

  # ==========================================================================
  # Skill Generator Tests
  # From: markdown_skill_generator_integration.rs
  # ==========================================================================

  Scenario: Generate skill from suggestion
    Given a skill generator with temp output directory
    And a suggestion with name "Git Quick Commit" and description "Quickly commit changes with a message" and confidence 0.92
    And the suggestion has pattern_id "test-pattern-123"
    And the suggestion has sample contexts "git add . && git commit -m 'fix bug'|git add README.md && git commit -m 'update docs'"
    And the suggestion has instructions preview "Use git to add and commit changes with a message"
    When I generate the skill
    Then the generated skill path should exist
    And the generated skill path should end with SKILL.md
    And the generated content should contain "---"
    And the generated content should contain "name: git-quick-commit"
    And the generated content should contain "description:"
    And the generated content should contain "metadata:"
    And the generated content should contain "requires:"
    And the generated content should contain "bins:"
    And the generated content should contain "git"
    And the generated content should contain "aleph:"
    And the generated content should contain "security:"
    And the generated content should contain "evolution:"
    And the generated content should contain "source:"
    And the generated content should contain "auto-generated"
    And the generated content should contain "confidence_score: 0.92"
    And the generated content should contain "created_from_trace:"
    And the generated content should contain "test-pattern-123"
    And the generated content should contain "# Git Quick Commit"
    And the generated content should contain "## Description"
    And the generated content should contain "## Examples"
    And the generated content should contain "fix bug"
    And the generated content should contain "## Metrics"

  Scenario: Generated skill can be loaded
    Given a skill generator with temp output directory
    And a suggestion with name "Echo Tool" and description "Echo a message" and confidence 0.85
    And the suggestion has pattern_id "echo-test"
    And the suggestion has sample contexts "echo 'hello'"
    And the suggestion has instructions preview "Use echo command to print a message"
    When I generate the skill
    And I load the generated skill
    Then the loaded tools count should be 1
    And the loaded generated tool name should be "echo-tool"
    And the first loaded tool description should be "Echo a message"
    And the loaded generated tool should have evolution metadata
    And the evolution source should be auto-generated
    And the evolution confidence score should be approximately 0.85
    And the evolution created_from_trace should be "echo-test"
    And the tool definition should have llm_context

  Scenario Outline: Skill name conversion
    Given a skill generator with temp output directory
    And a suggestion with name "<input>" and description "Test" and confidence 0.8
    And the suggestion has pattern_id "test"
    When I generate the skill
    Then the generated skill directory name should be "<expected>"

    Examples:
      | input               | expected            |
      | Quick Fix           | quick-fix           |
      | Docker Build & Push | docker-build-push   |
      | search_files        | search-files        |
      | Git Commit --amend  | git-commit-amend    |
