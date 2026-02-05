Feature: Embodiment Engine
  As an AI assistant with identity
  I want to load and apply soul definitions
  So that I maintain consistent personality and behavior

  # ═══ Soul File Parsing ═══

  Scenario: Parse minimal soul.md file
    Given a soul file with content:
      """
      # Identity

      I am a helpful AI assistant.

      ## Directives

      - Be helpful
      - Be concise
      """
    When I parse the soul file
    Then the soul identity should contain "helpful AI assistant"
    And the soul should have 2 directives

  Scenario: Parse soul.md with YAML frontmatter
    Given a soul file with content:
      """
      ---
      relationship: mentor
      expertise:
        - Rust
        - Python
      ---

      # Identity

      I am your programming mentor.

      ## Communication Style

      - **Tone**: encouraging
      - **Verbosity**: Balanced
      """
    When I parse the soul file
    Then the soul relationship should be "Mentor"
    And the soul should have 2 expertise areas
    And the soul should have expertise "Rust"

  Scenario: Parse soul.md with anti-patterns
    Given a soul file with content:
      """
      # Identity

      I am a professional assistant.

      ## Anti-Patterns

      - Never be condescending
      - Never skip important details
      """
    When I parse the soul file
    Then the soul should have 2 anti-patterns
    And the soul anti-patterns should contain "condescending"

  # ═══ Identity Resolution ═══

  Scenario: Session override takes highest priority
    Given a global soul with identity "Global identity"
    And a session override soul with identity "Session identity"
    When I resolve identity
    Then the effective identity should be "Session identity"

  Scenario: Global soul is used when no override
    Given a global soul with identity "Global identity"
    When I resolve identity
    Then the effective identity should be "Global identity"

  Scenario: Empty resolver returns default soul
    Given no soul files configured
    When I resolve identity
    Then the soul should be empty

  Scenario: Clear session override falls back to global
    Given a global soul with identity "Global identity"
    And a session override soul with identity "Session identity"
    When I clear the session override
    And I resolve identity
    Then the effective identity should be "Global identity"

  # ═══ Prompt Integration ═══

  Scenario: Soul section appears in system prompt
    Given a soul with identity "I am Aleph"
    And a soul with directive "Be helpful"
    When I build the system prompt with soul
    Then the prompt should contain "# Identity"
    And the prompt should contain "I am Aleph"
    And the prompt should contain "Behavioral Directives"
    And the prompt should contain "Be helpful"

  Scenario: Empty soul produces no soul section
    Given an empty soul
    When I build the system prompt with soul
    Then the prompt should not contain "# Identity"
    And the prompt should contain "AI assistant"

  Scenario: Soul appears before role section
    Given a soul with identity "I am Aleph"
    When I build the system prompt with soul
    Then "# Identity" should appear before "Your Role"
