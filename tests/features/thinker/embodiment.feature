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

  # Identity Resolution scenarios were removed: the layered `IdentityResolver`
  # (session-override / global-soul → SoulManifest) it exercised was a
  # disconnected island never wired into prompt assembly and has been dissolved
  # in favor of the single file-based source of truth (agent-dir SOUL.md). The
  # `identity.*` RPC / CLI now read/write those files directly; see
  # `src/gateway/handlers/identity.rs`.

  # Prompt Integration scenarios were removed: they exercised the
  # `SoulManifest`→prompt path (`build_system_prompt_with_soul`) that was
  # dissolved with the rest of System B. The live identity-injection source is
  # now the agent-dir SOUL.md file rendered raw by `SoulLayer`. That file-based
  # path is covered end-to-end by `SoulLayer` unit tests (`src/thinker/layers/
  # soul.rs`) and the production cached-path regression
  # (`cached_full_prompt_injects_soul_and_agents_identity_files` in
  # `src/thinker/prompt_builder/cache.rs`). Parsing above still tests the live
  # `SoulManifest` parser consumed by the `identity.get` RPC.
