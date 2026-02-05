Feature: Protocol Integration
  The configurable protocol system enables YAML-defined protocols that extend
  built-in protocols (OpenAI, Anthropic, Gemini) with custom endpoints,
  authentication, and request templates.

  # =========================================================================
  # End-to-End Protocol Tests
  # =========================================================================

  @e2e-protocol
  Scenario: End-to-end minimal protocol extending OpenAI
    Given a YAML protocol definition extending "openai" with custom auth header
    When I parse the protocol definition
    Then the protocol name should be "test-minimal"
    And the protocol should extend "openai"
    When I create a ConfigurableProtocol from the definition
    Then the protocol should be created successfully
    When I register the protocol in the registry
    Then the protocol should be retrievable as "test-minimal"
    When I create a provider using protocol "test-minimal"
    Then the provider should be created successfully
    And the provider name should be "test-minimal"
    When I build a request with the protocol
    Then the request should be built successfully

  @e2e-protocol
  Scenario: End-to-end custom protocol with template
    Given a YAML custom protocol definition with request template
    When I parse the protocol definition
    Then the protocol name should be "test-custom"
    And the protocol should have custom config
    When I create a ConfigurableProtocol from the definition
    Then the protocol should be created successfully
    When I register the protocol in the registry
    Then the protocol should be retrievable as "test-custom"
    When I create a provider using protocol "test-custom"
    Then the provider should be created successfully
    When I build a request with the protocol
    Then the request should be built successfully

  # =========================================================================
  # Hot Reload Simulation
  # =========================================================================

  @hot-reload
  Scenario: Protocol hot reload simulation
    Given a temporary protocol file with v1 configuration
    When I load the protocol from file
    Then the protocol should be registered as "reload-test"
    When I create a provider using protocol "reload-test"
    Then the provider should be created successfully
    When I update the protocol file with v2 configuration
    And I reload the protocol from file
    Then the protocol should be registered as "reload-test"
    When I create a provider with v2 configuration
    Then the provider should be created successfully

  # =========================================================================
  # Multiple Protocols Coexistence
  # =========================================================================

  @multi-protocol
  Scenario: Multiple protocols can coexist in the registry
    Given protocols extending "openai" "anthropic" and "gemini"
    When I register all protocols in the registry
    Then all protocols should be retrievable
    When I create providers for all protocols
    Then all providers should be created successfully

  # =========================================================================
  # Directory Loading
  # =========================================================================

  @directory-load
  Scenario: Protocols can be loaded from directory
    Given a temporary directory with protocol files
    When I load protocols from the directory
    Then protocol "dir-proto-1" should be registered
    And protocol "dir-proto-2" should be registered
    And protocol "dir-proto-3" should be registered

  # =========================================================================
  # Error Handling
  # =========================================================================

  @error-handling
  Scenario: Invalid YAML fails to parse
    Given invalid YAML content
    When I try to parse the protocol definition
    Then parsing should fail

  @error-handling
  Scenario: Protocol without extends or custom fails when building request
    Given a YAML protocol without extends or custom
    When I parse the protocol definition
    And I create a ConfigurableProtocol from the definition
    And I try to build a request
    Then building the request should fail

  @error-handling
  Scenario: Protocol with non-existent extends fails to create
    Given a YAML protocol extending non-existent base "non-existent-protocol"
    When I parse the protocol definition
    And I try to create a ConfigurableProtocol
    Then protocol creation should fail

  @error-handling
  Scenario: Provider with non-existent protocol fails to create
    Given a provider config with non-existent protocol
    When I try to create a provider
    Then provider creation should fail

  # =========================================================================
  # Authentication Variations
  # =========================================================================

  @auth-variations
  Scenario: Protocol with no auth prefix
    Given a YAML protocol with empty auth prefix
    When I create and register the protocol
    And I build a request with the protocol
    Then the request should be built successfully

  @auth-variations
  Scenario: Protocol with Bearer prefix
    Given a YAML protocol with Bearer auth prefix
    When I create and register the protocol
    And I build a request with the protocol
    Then the request should be built successfully

  # =========================================================================
  # Template Rendering
  # =========================================================================

  @template
  Scenario: Custom protocol template rendering with config values
    Given a YAML custom protocol with complex template
    When I create and register the protocol
    And I configure provider with max_tokens 1024 and temperature 0.7
    And I build a request with the protocol
    Then the request should be built successfully

  # =========================================================================
  # Registry Operations
  # =========================================================================

  @registry
  Scenario: Protocol registry lists built-in and custom protocols
    Given the protocol registry is initialized
    Then the registry should contain "openai"
    And the registry should contain "anthropic"
    And the registry should contain "gemini"
    When I register a custom protocol "list-test"
    Then the registry should contain "list-test"
    When I unregister protocol "list-test"
    Then the registry should not contain "list-test"

  # =========================================================================
  # Base URL Override
  # =========================================================================

  @url-override
  Scenario: Provider config base_url overrides protocol default
    Given a YAML protocol with default base_url "https://api.default.com"
    When I create and register the protocol
    And I create a provider with base_url "https://api.override.com"
    Then the provider should be created successfully
