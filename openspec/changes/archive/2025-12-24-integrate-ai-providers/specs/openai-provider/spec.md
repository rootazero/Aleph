# openai-provider Specification Delta

## ADDED Requirements

### Requirement: OpenAI API Client
The system SHALL implement an OpenAI API client that calls `/v1/chat/completions` endpoint.

#### Scenario: Send chat completion request
- **WHEN** `OpenAiProvider::process("Hello", None)` is called
- **THEN** HTTP POST request is sent to `{base_url}/v1/chat/completions`
- **AND** request body includes `{"model": "...", "messages": [...]}`
- **AND** `messages` contains user message with input text
- **AND** authorization header includes `Bearer {api_key}`

#### Scenario: Include system prompt
- **WHEN** `process("Hello", Some("You are a poet"))` is called
- **THEN** `messages` array includes system message
- **AND** system message has role "system" and content from parameter
- **AND** user message follows system message
- **AND** API receives both messages in order

#### Scenario: Parse successful response
- **WHEN** API returns 200 OK with valid JSON
- **THEN** response is parsed to extract `choices[0].message.content`
- **AND** content is returned as String
- **AND** whitespace is preserved
- **AND** empty responses return empty string

### Requirement: HTTP Client Configuration
The system SHALL configure `reqwest` client with appropriate settings for OpenAI API.

#### Scenario: Set request headers
- **WHEN** creating HTTP request
- **THEN** `Authorization` header is set to `Bearer {api_key}`
- **AND** `Content-Type` header is set to `application/json`
- **AND** `User-Agent` header includes "Aether/{version}"
- **AND** headers are sent with every request

#### Scenario: Configure timeout
- **WHEN** initializing OpenAiProvider
- **THEN** timeout is set from config `timeout_seconds` field
- **AND** default timeout is 30 seconds if not specified
- **AND** timeout applies to entire request (connect + read)
- **AND** timeout error returns `AetherError::Timeout`

#### Scenario: Enable TLS verification
- **WHEN** making HTTPS requests
- **THEN** TLS 1.2+ is used
- **AND** certificate verification is enabled
- **AND** invalid certificates cause connection failure
- **AND** error is `AetherError::NetworkError`

### Requirement: Request Parameters
The system SHALL support configurable request parameters for OpenAI API.

#### Scenario: Set model from config
- **WHEN** provider is initialized with config
- **THEN** `model` field is read from `[providers.openai]` section
- **AND** model is used in all requests
- **AND** supported models include "gpt-4o", "gpt-4o-mini", "gpt-3.5-turbo"
- **AND** invalid model names are accepted (API will validate)

#### Scenario: Set max_tokens
- **WHEN** `max_tokens` is specified in config
- **THEN** it is included in request body
- **AND** default is 4096 if not specified
- **AND** API will limit response to this length

#### Scenario: Set temperature
- **WHEN** `temperature` is specified in config (0.0-2.0)
- **THEN** it is included in request body
- **AND** default is 0.7 if not specified
- **AND** controls randomness of responses

### Requirement: Error Handling
The system SHALL handle all OpenAI API error responses appropriately.

#### Scenario: Handle 401 Unauthorized
- **WHEN** API returns 401 status
- **THEN** error type is `AetherError::AuthenticationError`
- **AND** error message suggests checking API key
- **AND** no retry is attempted
- **AND** user is notified via callback

#### Scenario: Handle 429 Rate Limit
- **WHEN** API returns 429 status
- **THEN** error type is `AetherError::RateLimitError`
- **AND** `Retry-After` header is parsed if present
- **AND** error message includes retry delay
- **AND** no automatic retry occurs

#### Scenario: Handle 500+ Server Errors
- **WHEN** API returns 500, 502, 503, or 504 status
- **THEN** error type is `AetherError::ProviderError`
- **AND** error message includes status code
- **AND** retry can be attempted by caller
- **AND** exponential backoff is recommended

#### Scenario: Handle network failures
- **WHEN** DNS lookup fails or connection refused
- **THEN** error type is `AetherError::NetworkError`
- **AND** error message includes underlying cause
- **AND** user is notified of connectivity issue

#### Scenario: Handle malformed responses
- **WHEN** API returns invalid JSON or missing fields
- **THEN** error type is `AetherError::ProviderError`
- **AND** error message indicates "Invalid API response"
- **AND** raw response is logged for debugging

### Requirement: Custom Base URL
The system SHALL support custom base URL for proxies and compatible APIs.

#### Scenario: Use custom base URL
- **WHEN** config includes `base_url = "https://proxy.example.com/v1"`
- **THEN** requests are sent to custom URL instead of official API
- **AND** URL must end with `/v1` or be appended automatically
- **AND** authentication headers remain the same

#### Scenario: Default to official API
- **WHEN** `base_url` is not specified in config
- **THEN** default is `https://api.openai.com/v1`
- **AND** official OpenAI endpoint is used
- **AND** no environment variable override (explicit config only)

### Requirement: Provider Metadata
The system SHALL provide metadata for OpenAI provider.

#### Scenario: Return provider name
- **WHEN** calling `provider.name()`
- **THEN** return value is `"openai"`
- **AND** name matches config section key
- **AND** name is used for logging and routing

#### Scenario: Return provider color
- **WHEN** calling `provider.color()`
- **THEN** return value is `"#10a37f"` (OpenAI brand color)
- **AND** color is used for Halo UI theming
- **AND** color can be overridden in config

### Requirement: Async Implementation
The system SHALL implement all operations asynchronously using tokio.

#### Scenario: Non-blocking API calls
- **WHEN** `process()` is called
- **THEN** HTTP request runs on tokio runtime
- **AND** no blocking operations occur
- **AND** other async tasks can run concurrently

#### Scenario: Support cancellation
- **WHEN** async task is cancelled before response
- **THEN** HTTP request is aborted
- **AND** resources are cleaned up
- **AND** no memory leak occurs

### Requirement: Logging and Debugging
The system SHALL log request/response details for debugging.

#### Scenario: Log request details
- **WHEN** sending API request
- **THEN** log includes model, input length, and timestamp
- **AND** API key is NOT logged (security)
- **AND** log level is DEBUG
- **AND** logs are written to stderr

#### Scenario: Log response metadata
- **WHEN** receiving API response
- **THEN** log includes status code, response length, latency
- **AND** first 100 chars of response are logged
- **AND** log level is DEBUG
- **AND** errors are logged at ERROR level
