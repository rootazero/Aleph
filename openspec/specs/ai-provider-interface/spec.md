# ai-provider-interface Specification

## Purpose
TBD - created by archiving change integrate-ai-providers. Update Purpose after archive.
## Requirements
### Requirement: AiProvider Trait Definition
The system SHALL define a trait `AiProvider` that provides a unified interface for all AI backends (OpenAI, Claude, Ollama, etc.).

#### Scenario: Define trait with async process method
- **WHEN** implementing a new AI provider
- **THEN** it must implement `async fn process(&self, input: &str, system_prompt: Option<&str>) -> Result<String, AlephError>`
- **AND** the method accepts plain text input and optional system prompt
- **AND** the method returns AI-generated response as String
- **AND** errors are wrapped in `AlephError`

#### Scenario: Provide metadata methods
- **WHEN** accessing provider information
- **THEN** trait must provide `fn name(&self) -> &str` for logging
- **AND** trait must provide `fn color(&self) -> &str` for UI theming
- **AND** color is a hex string (e.g., `"#10a37f"`)

#### Scenario: Ensure thread safety
- **WHEN** provider is shared across async tasks
- **THEN** trait must extend `Send + Sync` bounds
- **AND** provider can be wrapped in `Arc<dyn AiProvider>`
- **AND** concurrent calls are safe

### Requirement: Error Type for AI Operations
The system SHALL define error types specific to AI provider operations.

#### Scenario: Network errors
- **WHEN** HTTP request to AI API fails due to network issues
- **THEN** error type is `AlephError::NetworkError(String)`
- **AND** error message includes underlying cause
- **AND** error is propagated to caller

#### Scenario: Authentication errors
- **WHEN** API key is invalid or missing
- **THEN** error type is `AlephError::AuthenticationError(String)`
- **AND** error message suggests checking API key configuration
- **AND** no retry is attempted

#### Scenario: Rate limit errors
- **WHEN** provider returns 429 Too Many Requests
- **THEN** error type is `AlephError::RateLimitError(String)`
- **AND** error message includes retry-after information if available
- **AND** no automatic retry is attempted

#### Scenario: Provider-specific errors
- **WHEN** AI API returns 5xx server error
- **THEN** error type is `AlephError::ProviderError(String)`
- **AND** error message includes HTTP status code
- **AND** caller can decide retry strategy

#### Scenario: Timeout errors
- **WHEN** API request exceeds configured timeout
- **THEN** error type is `AlephError::Timeout`
- **AND** request is cancelled
- **AND** no partial response is returned

### Requirement: Mock Provider for Testing
The system SHALL provide a mock implementation of `AiProvider` for testing purposes.

#### Scenario: Create mock with predefined response
- **WHEN** testing AI-dependent logic
- **THEN** `MockProvider::new("test response")` creates a mock
- **AND** `process()` returns the predefined response
- **AND** no actual API call is made

#### Scenario: Simulate delays
- **WHEN** testing timeout behavior
- **THEN** mock can simulate processing delay with `with_delay(Duration)`
- **AND** `process()` sleeps for specified duration before returning
- **AND** timeout can be tested

#### Scenario: Simulate errors
- **WHEN** testing error handling
- **THEN** mock can simulate specific error types with `with_error(AlephError)`
- **AND** `process()` returns the configured error
- **AND** error paths can be tested

### Requirement: Async Runtime Integration
The system SHALL ensure all provider operations run on tokio runtime without blocking.

#### Scenario: Run HTTP requests on tokio
- **WHEN** calling `provider.process()`
- **THEN** HTTP client uses tokio async I/O
- **AND** no blocking operations occur
- **AND** runtime is not blocked

#### Scenario: Support cancellation
- **WHEN** async task is dropped before completion
- **THEN** HTTP request is cancelled
- **AND** resources are cleaned up
- **AND** no memory leak occurs

### Requirement: Provider Configuration
The system SHALL support configuring provider-specific settings (API keys, models, timeouts).

#### Scenario: Load provider config from TOML
- **WHEN** initializing a provider
- **THEN** config is read from `[providers.<name>]` section
- **AND** API key is loaded from `api_key` field
- **AND** model name is loaded from `model` field
- **AND** timeout is loaded from `timeout_seconds` field with default 30

#### Scenario: Validate required fields
- **WHEN** loading provider config
- **THEN** API key must not be empty for cloud providers
- **AND** model must not be empty
- **AND** missing required fields return `AlephError::InvalidConfig`
- **AND** error message lists missing fields

#### Scenario: Support optional fields
- **WHEN** provider config omits optional fields
- **THEN** reasonable defaults are used
- **AND** `base_url` defaults to official API endpoint
- **AND** `temperature` defaults to 0.7 (if applicable)
- **AND** `max_tokens` defaults to 4096 (if applicable)

### Requirement: Provider Registry
The system SHALL maintain a registry of available providers for router lookup.

#### Scenario: Register provider by name
- **WHEN** initializing AlephCore
- **THEN** providers are registered with unique names (e.g., "openai", "claude")
- **AND** names match config section keys
- **AND** duplicate names are rejected

#### Scenario: Lookup provider by name
- **WHEN** router needs to find provider
- **THEN** `providers.get("openai")` returns `Option<Arc<dyn AiProvider>>`
- **AND** None is returned if provider not registered
- **AND** lookup is O(1) with HashMap

#### Scenario: List all providers
- **WHEN** debugging or logging
- **THEN** `providers.keys()` returns iterator of provider names
- **AND** names are sorted alphabetically
- **AND** count can be queried

