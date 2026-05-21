# core-library Spec Delta

## ADDED Requirements

### Requirement: Multimodal Input Processing
The Rust core SHALL process multimodal input containing text and media attachments.

#### Scenario: Process text with image
- **GIVEN** userInput = "What's in this image?"
- **AND** context.attachments contains one PNG image
- **WHEN** process_input is called
- **THEN** Rust extracts image from context.attachments
- **AND** converts to provider-specific format
- **AND** sends multimodal request to vision provider
- **AND** returns text response

#### Scenario: Process text without attachments
- **GIVEN** userInput = "Translate: Hello"
- **AND** context.attachments is nil
- **WHEN** process_input is called
- **THEN** Rust processes as text-only (unchanged behavior)

#### Scenario: Multiple attachments
- **GIVEN** userInput = "Compare these images"
- **AND** context.attachments contains two images
- **WHEN** process_input is called
- **THEN** Rust includes both images in multimodal request
- **AND** provider processes all images

---

### Requirement: Vision Provider Detection
The Rust core SHALL detect when a provider supports vision/multimodal input.

#### Scenario: OpenAI GPT-4o supports vision
- **GIVEN** provider name = "openai"
- **AND** model = "gpt-4o"
- **WHEN** checking vision capability
- **THEN** returns true

#### Scenario: Claude 3.5 Sonnet supports vision
- **GIVEN** provider name = "claude"
- **AND** model = "claude-3-5-sonnet-20241022"
- **WHEN** checking vision capability
- **THEN** returns true

#### Scenario: Ollama text model
- **GIVEN** provider name = "ollama"
- **AND** model = "llama3"
- **WHEN** checking vision capability
- **THEN** returns false

---

### Requirement: Vision Provider Fallback
The Rust core SHALL fallback to vision-capable provider when image is present but routed provider lacks vision.

#### Scenario: Fallback to default vision provider
- **GIVEN** routing rule matches "ollama" provider (text-only)
- **AND** input includes image attachment
- **AND** default_provider is "openai" with GPT-4o
- **WHEN** process_input is called
- **THEN** Rust logs "Falling back to openai for vision capability"
- **AND** routes request to OpenAI instead of Ollama

#### Scenario: No fallback needed
- **GIVEN** routing rule matches "claude" provider
- **AND** input includes image attachment
- **AND** Claude model supports vision
- **WHEN** process_input is called
- **THEN** Rust routes to Claude as specified

---

### Requirement: Image Size Handling
The Rust core SHALL validate image sizes before sending to providers.

#### Scenario: Image within provider limit
- **GIVEN** image size = 5MB
- **AND** provider limit = 20MB (OpenAI)
- **WHEN** building provider request
- **THEN** image is included in request

#### Scenario: Image exceeds limit - log warning
- **GIVEN** image size = 25MB
- **AND** provider limit = 20MB
- **WHEN** building provider request
- **THEN** warning is logged
- **AND** image may be skipped or truncated
- **AND** processing continues with text only

---

## MODIFIED Requirements

### Requirement: AgentPayload Context
The AgentPayload SHALL include media attachments in its context for provider consumption.

#### Scenario: Payload with image attachment
- **GIVEN** CapturedContext includes one image MediaAttachment
- **WHEN** PayloadBuilder creates AgentPayload
- **THEN** payload.context includes attachments field
- **AND** PromptAssembler can access attachments for provider

#### Scenario: Payload without attachments
- **GIVEN** CapturedContext has no attachments
- **WHEN** PayloadBuilder creates AgentPayload
- **THEN** payload.context.attachments is None
- **AND** provider receives text-only request

