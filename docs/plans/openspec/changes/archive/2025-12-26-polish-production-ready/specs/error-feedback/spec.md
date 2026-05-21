# error-feedback Specification

## Purpose

Enhance error handling by providing actionable recovery suggestions alongside error messages. Errors become learning opportunities with clear guidance on resolution, improving user experience and reducing support burden.

## ADDED Requirements

### Requirement: Error Suggestions Framework

The system SHALL extend `AlephError` type with optional `suggestion` field containing actionable recovery guidance.

#### Scenario: Error with suggestion

- **WHEN** `AlephError::ApiKeyInvalid` is created
- **THEN** error includes suggestion: "Please check your API key in Settings → Providers"
- **AND** suggestion is accessible via `error.suggestion()`
- **AND** suggestion is optional (may be None)

#### Scenario: Error without suggestion

- **WHEN** generic error occurs without specific recovery path
- **THEN** error has `suggestion = None`
- **AND** only error message is displayed
- **AND** no empty suggestion text shown

### Requirement: API Provider Error Suggestions

The system SHALL provide specific suggestions for common API provider errors.

#### Scenario: Invalid API key error

- **WHEN** OpenAI returns 401 Unauthorized
- **THEN** error message: "API authentication failed"
- **AND** suggestion: "Please verify your OpenAI API key in Settings → Providers → OpenAI"
- **AND** Halo displays both message and suggestion

#### Scenario: Rate limit exceeded

- **WHEN** provider returns 429 Too Many Requests
- **THEN** error message: "API rate limit exceeded"
- **AND** suggestion: "Please wait 60 seconds and try again, or upgrade your API plan"
- **AND** retry countdown is displayed in Halo

#### Scenario: Network timeout

- **WHEN** HTTP request times out after 30 seconds
- **THEN** error message: "Request timed out"
- **AND** suggestion: "Check your internet connection or try a different AI provider"
- **AND** Halo shows timeout icon

#### Scenario: Model not found

- **WHEN** provider returns 404 for model name
- **THEN** error message: "Model 'gpt-5' not found"
- **AND** suggestion: "Available models: gpt-4o, gpt-4-turbo. Update your config in Settings → Providers"

#### Scenario: Content policy violation

- **WHEN** provider returns content filtering error
- **THEN** error message: "Content violates provider policy"
- **AND** suggestion: "Please rephrase your request or try a different AI provider"

### Requirement: Configuration Error Suggestions

The system SHALL provide suggestions for configuration-related errors.

#### Scenario: Invalid regex in routing rule

- **WHEN** config validation detects invalid regex pattern
- **THEN** error message: "Invalid regex pattern: '^[a-z'"
- **AND** suggestion: "Check syntax at regex101.com or use simple text matching instead"
- **AND** Settings UI highlights problematic rule

#### Scenario: Missing required config field

- **WHEN** provider config lacks `api_key` field
- **THEN** error message: "OpenAI provider missing required field: api_key"
- **AND** suggestion: "Add your API key in Settings → Providers → OpenAI → Edit"

#### Scenario: Config file parse error

- **WHEN** TOML parser fails on malformed config.toml
- **THEN** error message: "Failed to parse config: unexpected character at line 42"
- **AND** suggestion: "Restore default config or fix syntax error in ~/.aleph/config.toml"

### Requirement: Memory Module Error Suggestions

The system SHALL provide suggestions for memory-related errors.

#### Scenario: Database locked error

- **WHEN** memory database is locked by another process
- **THEN** error message: "Memory database is locked"
- **AND** suggestion: "Close other Aleph instances or restart the application"

#### Scenario: Disk space error

- **WHEN** memory storage fails due to disk full
- **THEN** error message: "Failed to store memory: disk full"
- **AND** suggestion: "Free up disk space or adjust retention policy in Settings → Memory"

#### Scenario: Embedding model not found

- **WHEN** embedding model file is missing
- **THEN** error message: "Embedding model not found: all-MiniLM-L6-v2.onnx"
- **AND** suggestion: "Reinstall Aleph or download model manually from GitHub releases"

### Requirement: System Permission Error Suggestions

The system SHALL provide suggestions for macOS permission-related errors.

#### Scenario: Accessibility permission denied

- **WHEN** keyboard simulation fails due to missing permission
- **THEN** error message: "Accessibility permission required"
- **AND** suggestion: "Grant permission in System Settings → Privacy & Security → Accessibility"
- **AND** button to open System Settings directly

#### Scenario: Keychain access denied

- **WHEN** API key read from Keychain fails
- **THEN** error message: "Failed to read API key from Keychain"
- **AND** suggestion: "Re-enter your API key in Settings → Providers to save it again"

### Requirement: Clipboard Error Suggestions

The system SHALL provide suggestions for clipboard operation failures.

#### Scenario: Clipboard empty error

- **WHEN** user triggers hotkey with empty clipboard and no selection
- **THEN** error message: "No content to process"
- **AND** suggestion: "Select text or image before pressing Cmd+~"

#### Scenario: Clipboard format error

- **WHEN** clipboard contains unsupported format (e.g., video)
- **THEN** error message: "Unsupported clipboard format"
- **AND** suggestion: "Aleph supports text and images only (PNG, JPEG, GIF)"

#### Scenario: Image size exceeded

- **WHEN** clipboard image exceeds max_image_size_mb
- **THEN** error message: "Image too large (15MB)"
- **AND** suggestion: "Resize image to under 10MB or adjust limit in config.toml"

### Requirement: Error Display in Halo UI

The system SHALL display error messages and suggestions in the Halo overlay with appropriate visual styling.

#### Scenario: Show error with suggestion

- **WHEN** `on_error(message, Some(suggestion))` callback is invoked
- **THEN** Halo displays red shake animation
- **AND** error message appears in bold red text
- **AND** suggestion appears below in smaller gray text
- **AND** both texts are visible for 5 seconds before fade out

#### Scenario: Show error without suggestion

- **WHEN** `on_error(message, None)` callback is invoked
- **THEN** only error message is displayed
- **AND** no empty suggestion space is shown
- **AND** layout is vertically centered

#### Scenario: Multi-line suggestions

- **WHEN** suggestion contains multiple lines
- **THEN** each line is displayed with proper formatting
- **AND** maximum 3 lines are shown (truncate longer suggestions)
- **AND** Halo expands vertically to fit content


## MODIFIED Requirements

### Requirement: AlephError Structure (Extended)

The system SHALL extend the existing `AlephError` enum with suggestion support.

#### Scenario: Error variants with suggestions

- **WHEN** `AlephError` is defined
- **THEN** each variant can optionally include `suggestion: Option<String>` field
- **AND** helper methods `with_suggestion()` and `suggestion()` are provided
- **AND** existing error creation code remains compatible

### Requirement: UniFFI Error Callback (Extended)

The system SHALL extend the `on_error` callback in `AlephEventHandler` to include suggestion parameter.

#### Scenario: Extended callback signature

- **WHEN** UniFFI interface is defined
- **THEN** callback signature is `on_error(message: String, suggestion: Option<String>)`
- **AND** Swift implementation receives both parameters
- **AND** backwards compatibility is maintained (suggestion defaults to None)

## References

- **Related Spec**: `event-handler` - Extends error callback signature
- **Related Spec**: `core-library` - Extends AlephError type
- **Integration**: HaloView.swift for error display UI
