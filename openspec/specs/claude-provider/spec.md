# claude-provider Specification

## Purpose
TBD - created by archiving change integrate-ai-providers. Update Purpose after archive.
## Requirements
### Requirement: Claude API Client
The system SHALL implement a Claude API client that calls `/v1/messages` endpoint.

#### Scenario: Send messages request
- **WHEN** `ClaudeProvider::process("Hello", None)` is called
- **THEN** HTTP POST request is sent to `{base_url}/v1/messages`
- **AND** request body includes `{"model": "...", "messages": [...], "max_tokens": ...}`
- **AND** `messages` contains single message with role "user"
- **AND** `x-api-key` header includes API key

#### Scenario: Include system prompt separately
- **WHEN** `process("Hello", Some("You are a poet"))` is called
- **THEN** request body includes `"system": "You are a poet"` as top-level field
- **AND** system prompt is NOT in messages array (different from OpenAI)
- **AND** messages array contains only user message
- **AND** API receives both system and messages

#### Scenario: Parse successful response
- **WHEN** API returns 200 OK with valid JSON
- **THEN** response is parsed to extract `content[0].text`
- **AND** text is returned as String
- **AND** whitespace is preserved
- **AND** empty responses return empty string

### Requirement: Claude-Specific Headers
The system SHALL include Claude-specific HTTP headers required by Anthropic API.

#### Scenario: Set anthropic-version header
- **WHEN** creating HTTP request
- **THEN** `anthropic-version` header is set to `2023-06-01`
- **AND** header is sent with every request
- **AND** version is hardcoded (not configurable)

#### Scenario: Set x-api-key header
- **WHEN** authenticating request
- **THEN** `x-api-key` header is set to API key from config
- **AND** NO `Authorization: Bearer` header is used (different from OpenAI)
- **AND** header is sent with every request

#### Scenario: Set content-type header
- **WHEN** creating HTTP request
- **THEN** `Content-Type` header is set to `application/json`
- **AND** `User-Agent` header includes "Aether/{version}"
- **AND** headers are sent with every request

### Requirement: Request Parameters
The system SHALL support Claude-specific request parameters.

#### Scenario: Set model from config
- **WHEN** provider is initialized with config
- **THEN** `model` field is read from `[providers.claude]` section
- **AND** supported models include "claude-3-5-sonnet-20241022", "claude-3-opus-20240229"
- **AND** model is used in all requests

#### Scenario: Set max_tokens (required)
- **WHEN** building request body
- **THEN** `max_tokens` field is always included
- **AND** default is 4096 if not specified in config
- **AND** Claude API requires this field (unlike OpenAI where it's optional)

#### Scenario: Set temperature (optional)
- **WHEN** `temperature` is specified in config (0.0-1.0)
- **THEN** it is included in request body
- **AND** default is 0.7 if not specified
- **AND** Claude uses range 0.0-1.0 (different from OpenAI's 0.0-2.0)

### Requirement: Error Handling
The system SHALL handle all Claude API error responses appropriately.

#### Scenario: Handle 401 Unauthorized
- **WHEN** API returns 401 status
- **THEN** error type is `AetherError::AuthenticationError`
- **AND** error message suggests checking `x-api-key` configuration
- **AND** no retry is attempted

#### Scenario: Handle 429 Rate Limit
- **WHEN** API returns 429 status
- **THEN** error type is `AetherError::RateLimitError`
- **AND** `retry-after` header is parsed if present
- **AND** error message includes retry delay
- **AND** no automatic retry occurs

#### Scenario: Handle 529 Overloaded
- **WHEN** API returns 529 status (Claude-specific overload error)
- **THEN** error type is `AetherError::ProviderError`
- **AND** error message indicates "Claude is overloaded"
- **AND** retry with exponential backoff is recommended

#### Scenario: Handle 400 Bad Request
- **WHEN** API returns 400 status
- **THEN** error type is `AetherError::ProviderError`
- **AND** error message includes `error.message` from response body
- **AND** common issues: invalid model, missing max_tokens, prompt too long

#### Scenario: Handle network failures
- **WHEN** DNS lookup fails or connection refused
- **THEN** error type is `AetherError::NetworkError`
- **AND** error message includes underlying cause
- **AND** user is notified of connectivity issue

### Requirement: Custom Base URL
The system SHALL support custom base URL for proxies and compatible APIs.

#### Scenario: Use custom base URL
- **WHEN** config includes `base_url = "https://proxy.example.com/v1"`
- **THEN** requests are sent to custom URL instead of official API
- **AND** URL must end with `/v1` or be appended automatically
- **AND** authentication headers remain the same

#### Scenario: Default to official API
- **WHEN** `base_url` is not specified in config
- **THEN** default is `https://api.anthropic.com/v1`
- **AND** official Anthropic endpoint is used

### Requirement: Provider Metadata
The system SHALL provide metadata for Claude provider.

#### Scenario: Return provider name
- **WHEN** calling `provider.name()`
- **THEN** return value is `"claude"`
- **AND** name matches config section key
- **AND** name is used for logging and routing

#### Scenario: Return provider color
- **WHEN** calling `provider.color()`
- **THEN** return value is `"#d97757"` (Claude brand color)
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

### Requirement: HTTP Client Configuration
The system SHALL configure `reqwest` client with appropriate settings for Claude API.

#### Scenario: Configure timeout
- **WHEN** initializing ClaudeProvider
- **THEN** timeout is set from config `timeout_seconds` field
- **AND** default timeout is 30 seconds if not specified
- **AND** timeout applies to entire request (connect + read)

#### Scenario: Enable TLS verification
- **WHEN** making HTTPS requests
- **THEN** TLS 1.2+ is used
- **AND** certificate verification is enabled
- **AND** invalid certificates cause connection failure

### Requirement: Response Format Handling
The system SHALL correctly parse Claude's unique response format.

#### Scenario: Extract text from content array
- **WHEN** API returns response with `content: [{type: "text", text: "..."}]`
- **THEN** first content block with type "text" is extracted
- **AND** `text` field value is returned
- **AND** other content types (future: images) are ignored

#### Scenario: Handle multi-block responses
- **WHEN** response contains multiple content blocks
- **THEN** only first text block is used
- **AND** other blocks are logged but not returned
- **AND** future enhancement can concatenate all text blocks

#### Scenario: Handle stop_reason
- **WHEN** response includes `stop_reason: "end_turn"` or `"max_tokens"`
- **THEN** response is still valid and returned
- **AND** `max_tokens` stop reason is logged as warning
- **AND** user may see truncated responses

### Requirement: Logging and Debugging
The system SHALL log request/response details for debugging.

#### Scenario: Log request details
- **WHEN** sending API request
- **THEN** log includes model, input length, and timestamp
- **AND** API key is NOT logged (security)
- **AND** log level is DEBUG

#### Scenario: Log response metadata
- **WHEN** receiving API response
- **THEN** log includes status code, response length, latency, stop_reason
- **AND** first 100 chars of response are logged
- **AND** log level is DEBUG
- **AND** errors are logged at ERROR level

