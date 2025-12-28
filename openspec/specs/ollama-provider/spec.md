# ollama-provider Specification

## Purpose
TBD - created by archiving change integrate-ai-providers. Update Purpose after archive.
## Requirements
### Requirement: Ollama CLI Execution
The system SHALL execute Ollama CLI command to process input with local models.

#### Scenario: Execute ollama run command
- **WHEN** `OllamaProvider::process("Hello", None)` is called
- **THEN** command `ollama run {model} "{prompt}"` is executed
- **AND** model name is from config `[providers.ollama]` section
- **AND** prompt is the input text
- **AND** command runs asynchronously via `tokio::process::Command`

#### Scenario: Capture stdout as response
- **WHEN** ollama command completes successfully
- **THEN** stdout is captured as bytes
- **AND** bytes are converted to UTF-8 string
- **AND** string is returned as AI response
- **AND** trailing whitespace is trimmed

#### Scenario: Combine system prompt with input
- **WHEN** `process("Hello", Some("You are a poet"))` is called
- **THEN** prompt is formatted as `"{system_prompt}\n\nUser: {input}"`
- **AND** system prompt appears first
- **AND** user input is prefixed with "User: "
- **AND** combined prompt is passed to ollama

### Requirement: Command Execution Configuration
The system SHALL configure command execution with appropriate settings.

#### Scenario: Set working directory
- **WHEN** executing ollama command
- **THEN** working directory is set to user's home directory
- **AND** ollama can access its model directory
- **AND** relative paths are resolved correctly

#### Scenario: Inherit environment variables
- **WHEN** spawning ollama process
- **THEN** parent process environment is inherited
- **AND** `PATH` includes ollama binary location
- **AND** `OLLAMA_MODELS` path is respected if set

#### Scenario: Disable stdin
- **WHEN** spawning ollama process
- **THEN** stdin is set to null (no interactive input)
- **AND** process cannot block waiting for stdin
- **AND** execution completes automatically

### Requirement: Error Handling
The system SHALL handle all ollama execution errors appropriately.

#### Scenario: Handle command not found
- **WHEN** ollama binary is not in PATH
- **THEN** error type is `AetherError::ProviderError`
- **AND** error message indicates "ollama command not found"
- **AND** user is advised to install Ollama

#### Scenario: Handle model not found
- **WHEN** ollama exits with error "model not found"
- **THEN** error type is `AetherError::ProviderError`
- **AND** error message includes model name
- **AND** user is advised to run `ollama pull {model}`

#### Scenario: Handle non-zero exit code
- **WHEN** ollama exits with non-zero status
- **THEN** error type is `AetherError::ProviderError`
- **AND** stderr output is included in error message
- **AND** exit code is logged for debugging

#### Scenario: Handle UTF-8 decode errors
- **WHEN** ollama stdout is not valid UTF-8
- **THEN** error type is `AetherError::ProviderError`
- **AND** error message indicates "Invalid UTF-8 output"
- **AND** raw bytes are logged in hex

#### Scenario: Handle timeout
- **WHEN** ollama execution exceeds timeout
- **THEN** process is killed
- **AND** error type is `AetherError::Timeout`
- **AND** no partial output is returned

### Requirement: Timeout Configuration
The system SHALL support configurable timeout for ollama execution.

#### Scenario: Use default timeout
- **WHEN** `timeout_seconds` is not specified in config
- **THEN** default timeout is 120 seconds (2 minutes)
- **AND** timeout is longer than cloud APIs (local models are slower)

#### Scenario: Use custom timeout
- **WHEN** config includes `timeout_seconds = 60`
- **THEN** timeout is set to 60 seconds
- **AND** ollama process is killed if it exceeds timeout

#### Scenario: Apply timeout to entire execution
- **WHEN** timeout is triggered
- **THEN** ollama process receives SIGTERM
- **AND** if still running after 5 seconds, SIGKILL is sent
- **AND** process is guaranteed to terminate

### Requirement: Provider Metadata
The system SHALL provide metadata for Ollama provider.

#### Scenario: Return provider name
- **WHEN** calling `provider.name()`
- **THEN** return value is `"ollama"`
- **AND** name matches config section key

#### Scenario: Return provider color
- **WHEN** calling `provider.color()`
- **THEN** return value is `"#0000ff"` (default blue)
- **AND** color can be overridden in config
- **AND** color is used for Halo UI theming

### Requirement: Model Configuration
The system SHALL load model name from configuration.

#### Scenario: Load model from config
- **WHEN** initializing OllamaProvider
- **THEN** `model` field is read from `[providers.ollama]` section
- **AND** model name is required (no default)
- **AND** missing model returns `AetherError::InvalidConfig`

#### Scenario: Support all ollama models
- **WHEN** user configures any ollama model name
- **THEN** it is passed to `ollama run` command
- **AND** supported models include "llama3.2", "mistral", "codellama", etc.
- **AND** provider does not validate model name (ollama CLI will validate)

### Requirement: Async Implementation
The system SHALL implement all operations asynchronously using tokio.

#### Scenario: Non-blocking command execution
- **WHEN** `process()` is called
- **THEN** ollama command runs on tokio runtime
- **AND** no blocking operations occur
- **AND** other async tasks can run concurrently

#### Scenario: Support cancellation
- **WHEN** async task is cancelled before completion
- **THEN** ollama process is terminated
- **AND** resources are cleaned up
- **AND** no zombie processes remain

### Requirement: Output Processing
The system SHALL process ollama output correctly.

#### Scenario: Remove ANSI escape codes
- **WHEN** ollama output contains ANSI color codes
- **THEN** escape codes are stripped from response
- **AND** only plain text is returned
- **AND** formatting is preserved

#### Scenario: Handle multi-line output
- **WHEN** ollama returns multi-line response
- **THEN** all lines are preserved
- **AND** line breaks are maintained
- **AND** entire output is returned as single string

#### Scenario: Handle empty output
- **WHEN** ollama returns empty stdout
- **THEN** empty string is returned
- **AND** no error is raised
- **AND** warning is logged

### Requirement: Logging and Debugging
The system SHALL log execution details for debugging.

#### Scenario: Log command execution
- **WHEN** spawning ollama process
- **THEN** log includes model name, prompt length, and timestamp
- **AND** full prompt is NOT logged (may be large)
- **AND** log level is DEBUG

#### Scenario: Log execution result
- **WHEN** ollama completes
- **THEN** log includes exit code, execution time, output length
- **AND** first 100 chars of output are logged
- **AND** errors are logged at ERROR level

### Requirement: Prompt Escaping
The system SHALL properly escape prompt text for shell command.

#### Scenario: Escape double quotes
- **WHEN** input contains double quotes (`"hello"`)
- **THEN** quotes are escaped as `\"`
- **AND** command is `ollama run model "He said \"hello\""`
- **AND** ollama receives correct text

#### Scenario: Escape backslashes
- **WHEN** input contains backslashes (`C:\path`)
- **THEN** backslashes are escaped as `\\`
- **AND** ollama receives correct text

#### Scenario: Handle newlines
- **WHEN** input contains newlines
- **THEN** newlines are preserved in prompt
- **AND** command uses proper quoting
- **AND** ollama processes multi-line input correctly

