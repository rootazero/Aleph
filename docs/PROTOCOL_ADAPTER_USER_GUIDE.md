# Protocol Adapter User Guide

## Overview

Aleph's Protocol Adapter system allows you to add support for new AI providers **without modifying or recompiling the Rust codebase**. Define new protocols using YAML configuration files that are automatically loaded and hot-reloaded when you make changes.

### What is a Protocol Adapter?

A protocol adapter translates between Aleph's standardized internal representation of AI requests and the specific API format required by each LLM provider. Instead of hardcoding these translations in Rust, you can define them declaratively in YAML files.

### Key Benefits

- **No Compilation Required**: Add new providers by creating YAML files
- **Hot Reload**: Changes to protocol files are detected and applied automatically within 500ms
- **Two Modes**: Minimal configuration for OpenAI-compatible APIs, full templates for custom APIs
- **Type-Safe**: YAML configurations are validated against a strict schema
- **Maintainable**: Clear separation between protocol logic and application code
- **Extensible**: Template system supports complex transformations and custom authentication

## Quick Start

### Step 1: Create Protocol Configuration

Create `~/.aleph/protocols/my-provider.yaml`:

```yaml
name: my-custom-provider
extends: openai
base_url: https://api.myprovider.com/v1

differences:
  auth:
    header: X-API-Key
    prefix: ""  # No "Bearer " prefix
```

### Step 2: Use in Provider Config

Add to your `~/.aleph/config.toml`:

```toml
[[providers]]
name = "my-provider"
protocol = "my-custom-provider"  # References the protocol name
model = "my-model-name"
api_key = "your-api-key"
```

### Step 3: Hot Reload

Aleph watches `~/.aleph/protocols/` and automatically reloads when you edit files. Changes take effect within 500ms - no restart required!

**Try it**:
1. Edit `my-provider.yaml` (change base_url)
2. Save the file
3. Check logs: you'll see "Reloaded protocol 'my-custom-provider'"
4. Next request uses the updated configuration

## Configuration Modes

### Minimal Configuration Mode (Recommended)

Use this mode when your provider's API is similar to an existing protocol (OpenAI, Anthropic, or Gemini) but with minor differences.

#### When to Use

- Provider is OpenAI-compatible (most common)
- Only need to customize authentication headers
- Need different default parameters
- Different base URL
- Minor field name changes

#### Example 1: Custom Authentication

Many OpenAI-compatible providers use a different authentication header:

```yaml
name: groq-custom
extends: openai
base_url: https://api.groq.com/openai/v1

differences:
  auth:
    header: X-API-Key  # Instead of "Authorization"
    prefix: ""         # No "Bearer " prefix
```

#### Example 2: Field Customization

Override default parameter values and ranges:

```yaml
name: custom-openai
extends: openai
base_url: https://api.openai.com/v1

differences:
  request_fields:
    temperature:
      default: 0.7
      range: [0.0, 2.0]

    max_tokens:
      default: 4096
      range: [1, 32768]
```

#### Example 3: OpenRouter

OpenRouter is OpenAI-compatible but uses different headers:

```yaml
name: openrouter
extends: openai
base_url: https://openrouter.ai/api/v1

differences:
  auth:
    header: Authorization
    prefix: "Bearer "

  # OpenRouter-specific headers (future feature)
  # extra_headers:
  #   HTTP-Referer: https://aether.app
  #   X-Title: Aleph Assistant
```

#### Example 4: Together AI

```yaml
name: together-ai
extends: openai
base_url: https://api.together.xyz/v1

differences:
  auth:
    header: Authorization
    prefix: "Bearer "
```

### Full Template Mode (Advanced)

Use this mode when your provider's API is completely different from existing protocols.

#### When to Use

- Provider has a custom API format (not OpenAI-compatible)
- Non-standard request/response structure
- Custom authentication schemes (API key in URL, custom headers, etc.)
- Provider-specific parameters
- Different streaming protocol

#### Complete Example: Custom Provider

```yaml
name: exotic-ai
base_url: https://api.exotic.ai

custom:
  # Authentication
  auth:
    type: header
    header: X-API-Token
    value_template: "{{config.api_key}}"

  # Endpoints (relative to base_url)
  endpoints:
    chat: "/v2/completions"
    stream: "/v2/completions/stream"

  # Request structure mapping
  request_template:
    model_name: "{{config.model}}"
    input_text: "{{input}}"
    system_instruction: "{{system_prompt}}"
    parameters:
      temperature: "{{config.temperature}}"
      max_tokens: "{{config.max_tokens}}"
      # Provider-specific parameters
      creativity_level: "{{config.creativity_level | default: 'medium'}}"

  # Response parsing
  response_mapping:
    content: "$.output.generated_text"
    error: "$.error.message"

  # Streaming configuration
  stream_config:
    format: sse
    event_prefix: "data: "
    done_marker: "[DONE]"
    content_path: "$.chunk.text"
```

#### Real-World Example: Cohere

Cohere has a different API structure:

```yaml
name: cohere
base_url: https://api.cohere.ai/v1

custom:
  auth:
    type: header
    header: Authorization
    value_template: "Bearer {{config.api_key}}"

  endpoints:
    chat: "/chat"
    stream: "/chat"

  request_template:
    model: "{{config.model}}"
    message: "{{input}}"
    preamble: "{{system_prompt}}"
    temperature: "{{config.temperature}}"
    max_tokens: "{{config.max_tokens}}"
    stream: false  # Overridden for streaming

  response_mapping:
    content: "$.text"
    error: "$.message"

  stream_config:
    format: sse
    event_prefix: "data: "
    done_marker: "[DONE]"
    content_path: "$.text"
```

## Template Syntax

Aleph uses **Handlebars** template syntax for request transformation.

### Available Variables

#### Config Variables

Access provider configuration values:

| Variable | Description | Example |
|----------|-------------|---------|
| `{{config.model}}` | Model name | `"gpt-4"` |
| `{{config.api_key}}` | API key | `"sk-..."` |
| `{{config.base_url}}` | Base URL | `"https://api.openai.com/v1"` |
| `{{config.temperature}}` | Temperature parameter | `0.7` |
| `{{config.max_tokens}}` | Max tokens | `4096` |
| `{{config.*}}` | Any custom config field | `{{config.user_id}}` |

#### Input Variables

Access request data:

| Variable | Description | Example |
|----------|-------------|---------|
| `{{input}}` | User input text | `"Hello, world!"` |
| `{{system_prompt}}` | System prompt | `"You are a helpful assistant"` |
| `{{messages}}` | Structured message array | `[{"role": "user", "content": "..."}]` |

#### Session Variables

| Variable | Description | Example |
|----------|-------------|---------|
| `{{session_id}}` | Current session ID | `"sess_abc123"` |

#### Environment Variables

Access environment variables:

```yaml
auth:
  value_template: "Bearer {{env.MY_API_KEY}}"
```

Then set in your shell:
```bash
export MY_API_KEY="your-key"
```

### Default Values

Use the pipe operator for default values:

```yaml
request_template:
  temperature: "{{config.temperature | default: 0.7}}"
  creativity: "{{config.creativity | default: 'medium'}}"
```

If `config.temperature` is not set, it defaults to `0.7`.

### Complex Example

```yaml
request_template:
  # Model selection with fallback
  model: "{{config.model | default: 'gpt-3.5-turbo'}}"

  # Combine system prompt and input
  messages:
    - role: system
      content: "{{system_prompt | default: 'You are a helpful assistant.'}}"
    - role: user
      content: "{{input}}"

  # Parameters with defaults
  temperature: "{{config.temperature | default: 1.0}}"
  max_tokens: "{{config.max_tokens | default: 2048}}"

  # Custom metadata
  metadata:
    user_id: "{{config.user_id}}"
    session: "{{session_id}}"
    timestamp: "{{timestamp}}"
```

## JSONPath Syntax

Aleph uses **JSONPath** to extract values from provider responses.

### Basic Usage

Given a response:
```json
{
  "output": {
    "generated_text": "Hello, world!"
  }
}
```

Extract content:
```yaml
response_mapping:
  content: "$.output.generated_text"
```

### Common Patterns

#### Top-Level Field

```json
{"content": "Hello"}
```
```yaml
content: "$.content"
```

#### Nested Object

```json
{
  "data": {
    "result": {
      "text": "Hello"
    }
  }
}
```
```yaml
content: "$.data.result.text"
```

#### Array Access

```json
{
  "choices": [
    {"message": {"content": "Hello"}}
  ]
}
```
```yaml
content: "$.choices[0].message.content"
```

#### OpenAI Format

```json
{
  "choices": [
    {
      "message": {
        "content": "Hello, world!"
      },
      "finish_reason": "stop"
    }
  ]
}
```
```yaml
response_mapping:
  content: "$.choices[0].message.content"
```

#### Anthropic Format

```json
{
  "content": [
    {
      "type": "text",
      "text": "Hello, world!"
    }
  ]
}
```
```yaml
response_mapping:
  content: "$.content[0].text"
```

#### Custom Provider

```json
{
  "result": {
    "completion": "Hello, world!",
    "metadata": {
      "tokens_used": 42
    }
  },
  "status": "success"
}
```
```yaml
response_mapping:
  content: "$.result.completion"
  error: "$.error.message"  # Will be empty if no error
```

### Error Handling

Always specify an error path:

```yaml
response_mapping:
  content: "$.output.text"
  error: "$.error.message"  # JSONPath for error messages
```

If the error path matches, Aleph will return an error instead of trying to extract content.

### Advanced JSONPath

| Expression | Description | Example |
|------------|-------------|---------|
| `$.field` | Root-level field | `$.content` |
| `$.nested.field` | Nested field | `$.data.result.text` |
| `$.array[0]` | First array element | `$.choices[0]` |
| `$.array[*]` | All array elements | `$.items[*].name` |
| `$..field` | Recursive search | `$..content` |

## Hot Reload

### How It Works

Aleph uses the `notify` crate to watch for filesystem changes:

1. **File Watch**: Monitors `~/.aleph/protocols/` directory
2. **Change Detection**: Detects Create/Modify/Delete events
3. **Debouncing**: 500ms delay to handle rapid successive changes
4. **Reload**: Re-parses YAML and updates registry
5. **Atomic Update**: New requests use the updated protocol immediately

### Watched Locations

#### Default Directory

```
~/.aleph/protocols/*.yaml
```

All `.yaml` files in this directory are automatically loaded on startup and watched for changes.

#### Explicit Paths (Future Feature)

You'll be able to specify additional paths in `config.toml`:

```toml
[protocol_extensions]
paths = [
    "./custom-protocols/my-provider.yaml",
    "/etc/aether/shared-protocols/company.yaml"
]
```

### Change Detection Timing

- **Detection Latency**: Changes detected within 500ms
- **Reload Time**: Parsing and registry update typically < 100ms
- **Total Latency**: < 600ms from file save to active protocol update

### Testing Hot Reload

1. **Create a test protocol**:
   ```bash
   cat > ~/.aleph/protocols/test.yaml << EOF
   name: test-protocol
   extends: openai
   base_url: https://api.test.com
   EOF
   ```

2. **Start Aleph** with logging:
   ```bash
   RUST_LOG=info aether gateway
   ```

3. **Verify initial load**:
   ```
   INFO  Loaded protocol 'test-protocol' from ~/.aleph/protocols/test.yaml
   ```

4. **Edit the file** (change base_url):
   ```bash
   sed -i 's/api.test.com/api.v2.test.com/' ~/.aleph/protocols/test.yaml
   ```

5. **Watch for reload**:
   ```
   INFO  Protocol file changed: ~/.aleph/protocols/test.yaml
   INFO  Reloaded protocol 'test-protocol'
   ```

6. **Next request uses updated protocol** automatically!

### Best Practices

- **Edit in place**: Modify existing files rather than deleting and recreating
- **Save once**: Some editors create temporary files; configure to save directly
- **Check logs**: Monitor Aleph logs to confirm reload
- **Test incrementally**: Make small changes and test between edits

## Troubleshooting

### Protocol Not Loading

**Symptom**: `Protocol 'my-provider' not found`

**Causes & Solutions**:

1. **File doesn't exist**
   ```bash
   ls -la ~/.aleph/protocols/my-provider.yaml
   ```
   **Fix**: Create the file or check the filename

2. **Wrong protocol name**
   - Protocol name in YAML must match the name you reference
   - File name doesn't matter (can be different from protocol name)

   ```yaml
   # File: ~/.aleph/protocols/groq.yaml
   name: groq-custom  # This is what you reference in config
   ```

3. **YAML not loading**
   - Check Aleph logs for parse errors
   - Validate YAML syntax:
     ```bash
     python -c "import yaml; yaml.safe_load(open('~/.aleph/protocols/my-provider.yaml'))"
     ```

### Invalid YAML Syntax

**Symptom**: `Failed to parse protocol YAML: ...`

**Common Mistakes**:

1. **Incorrect indentation** (YAML is sensitive to spaces)
   ```yaml
   # WRONG
   custom:
   auth:  # Should be indented
     type: header

   # CORRECT
   custom:
     auth:  # Properly indented
       type: header
   ```

2. **Missing colons**
   ```yaml
   # WRONG
   name my-protocol

   # CORRECT
   name: my-protocol
   ```

3. **Unquoted special characters**
   ```yaml
   # WRONG
   base_url: https://api.example.com/v1?key=value

   # CORRECT
   base_url: "https://api.example.com/v1?key=value"
   ```

**Tools**:
- Use a YAML validator: https://www.yamllint.com/
- Most editors have YAML syntax highlighting

### Template Rendering Errors

**Symptom**: `Template render error: ...`

**Common Causes**:

1. **Undefined variable**
   ```yaml
   request_template:
     api_key: "{{config.api_key}}"  # Error if api_key not in config
   ```

   **Fix**: Use default values
   ```yaml
   request_template:
     api_key: "{{config.api_key | default: ''}}"
   ```

2. **Syntax error**
   ```yaml
   # WRONG
   value: {{config.model}}  # Missing quotes

   # CORRECT
   value: "{{config.model}}"
   ```

3. **Nested templates**
   ```yaml
   # Handlebars doesn't support nested templates
   # Use separate fields instead
   ```

**Debugging**:
- Enable debug logging: `RUST_LOG=debug`
- Check what variables are available in logs
- Test with simple templates first

### Response Parsing Errors

**Symptom**: `Failed to extract content from response` or `JSONPath '...' matched no values`

**Debugging Steps**:

1. **Enable debug logging**
   ```bash
   RUST_LOG=debug aether chat --provider my-provider "test"
   ```

2. **Examine actual response** in logs:
   ```
   DEBUG Response body: {"result": {"text": "..."}}
   ```

3. **Update JSONPath** to match actual structure:
   ```yaml
   # Was:
   content: "$.output.text"

   # Should be:
   content: "$.result.text"
   ```

4. **Test JSONPath** separately:
   ```bash
   # Use jq to test JSONPath-like queries
   echo '{"result":{"text":"Hello"}}' | jq '.result.text'
   ```

**Common Issues**:

- **Wrong path**: Response structure doesn't match your JSONPath
- **Array without index**: Use `[0]` for first element
- **Null values**: Path exists but value is null
- **Type mismatch**: Trying to extract object as string

**Solutions**:

1. **Add fallbacks** (future feature):
   ```yaml
   response_mapping:
     content_alternatives:
       - "$.output.text"
       - "$.result.content"
       - "$.data.message"
   ```

2. **Check for errors first**:
   ```yaml
   response_mapping:
     error: "$.error.message"  # Checked first
     content: "$.output.text"
   ```

### Streaming Issues

**Symptom**: `Stream ended without content` or chunks not being parsed

**Common Causes**:

1. **Wrong stream format**
   ```yaml
   stream_config:
     format: sse  # But provider uses ndjson
   ```

   **Fix**: Check provider docs for stream format (SSE vs NDJSON)

2. **Incorrect done marker**
   ```yaml
   stream_config:
     done_marker: "[DONE]"  # But provider uses "data: [DONE]"
   ```

   **Fix**: Check actual stream output in debug logs

3. **Wrong content path**
   ```yaml
   stream_config:
     content_path: "$.delta.text"  # But chunk structure is different
   ```

   **Fix**: Examine streaming response structure

**Debugging**:

1. **Test non-streaming first**:
   ```bash
   aether chat --provider my-provider --no-stream "test"
   ```

2. **Enable verbose logging**:
   ```bash
   RUST_LOG=trace aether chat --provider my-provider --stream "test"
   ```

3. **Check stream format** in logs:
   ```
   TRACE SSE event: data: {"chunk":{"text":"Hello"}}
   ```

4. **Verify done marker**:
   ```
   TRACE SSE event: data: [DONE]
   ```

### Authentication Errors

**Symptom**: `401 Unauthorized` or `403 Forbidden`

**Checklist**:

1. **API key is correct**
   ```bash
   # Test with provider's official client/curl
   curl -H "X-API-Key: your-key" https://api.provider.com/v1/models
   ```

2. **Header name matches**
   ```yaml
   differences:
     auth:
       header: X-API-Key  # Must match provider's requirement exactly
   ```

3. **Prefix is correct**
   ```yaml
   differences:
     auth:
       prefix: "Bearer "  # Some providers don't use prefix
       # Use prefix: "" for no prefix
   ```

4. **API key in config**
   ```toml
   [[providers]]
   api_key = "your-key-here"  # Make sure it's set
   ```

5. **Template renders correctly**
   ```yaml
   custom:
     auth:
       value_template: "{{config.api_key}}"  # Check variable name
   ```

**Testing**:
```bash
# Enable debug to see actual headers sent
RUST_LOG=debug aether chat --provider my-provider "test" 2>&1 | grep -i auth
```

## Examples

### Example Files

See `examples/protocols/` for complete, working examples:

- **`groq-custom.yaml`** - Minimal configuration mode extending OpenAI
- **`exotic-ai.yaml`** - Full template mode with custom protocol

### Copy and Modify

```bash
# Create protocols directory
mkdir -p ~/.aleph/protocols

# Copy example as starting point
cp examples/protocols/groq-custom.yaml ~/.aleph/protocols/my-provider.yaml

# Edit for your provider
nano ~/.aleph/protocols/my-provider.yaml
```

### Provider-Specific Examples

#### Groq

```yaml
name: groq
extends: openai
base_url: https://api.groq.com/openai/v1

differences:
  auth:
    header: Authorization
    prefix: "Bearer "

  request_fields:
    temperature:
      default: 0.7
```

#### Together AI

```yaml
name: together
extends: openai
base_url: https://api.together.xyz/v1

differences:
  auth:
    header: Authorization
    prefix: "Bearer "
```

#### Perplexity

```yaml
name: perplexity
extends: openai
base_url: https://api.perplexity.ai

differences:
  auth:
    header: Authorization
    prefix: "Bearer "
```

#### OpenRouter

```yaml
name: openrouter
extends: openai
base_url: https://openrouter.ai/api/v1

differences:
  auth:
    header: Authorization
    prefix: "Bearer "
```

#### AI21

```yaml
name: ai21
base_url: https://api.ai21.com/studio/v1

custom:
  auth:
    type: header
    header: Authorization
    value_template: "Bearer {{config.api_key}}"

  endpoints:
    chat: "/chat/completions"

  request_template:
    model: "{{config.model}}"
    messages:
      - role: system
        content: "{{system_prompt}}"
      - role: user
        content: "{{input}}"
    temperature: "{{config.temperature}}"
    max_tokens: "{{config.max_tokens}}"

  response_mapping:
    content: "$.choices[0].message.content"
    error: "$.error.message"
```

## Architecture

### Protocol Resolution Order

When you specify a protocol in your provider config, Aleph resolves it in this order:

```
1. Dynamic Protocols (YAML-loaded)
   └─> ConfigurableProtocol
       ├─> Minimal mode: base protocol + differences
       └─> Custom mode: full template rendering

2. Built-in Protocols (Compiled Rust)
   ├─> OpenAI
   ├─> Anthropic
   ├─> Gemini
   └─> Ollama

3. Not Found
   └─> Error: "Protocol 'name' not found. Available: [...]"
```

### Component Architecture

```
┌─────────────────────────────────────────────────────────┐
│                  Provider Config                         │
│  protocol: "my-provider"                                 │
│  model: "my-model"                                       │
└─────────────────┬───────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────────────┐
│             ProtocolRegistry                             │
│  - Manages all available protocols                       │
│  - Dynamic (YAML) + Built-in (Rust)                     │
│  - Thread-safe concurrent access                         │
└─────────────────┬───────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────────────┐
│           ConfigurableProtocol                           │
│  ┌──────────────────┬──────────────────┐               │
│  │  Minimal Mode    │  Custom Mode     │               │
│  │  - Load base     │  - Render        │               │
│  │  - Apply diffs   │    templates     │               │
│  │  - Delegate      │  - Parse with    │               │
│  │                  │    JSONPath      │               │
│  └──────────────────┴──────────────────┘               │
└─────────────────┬───────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────────────┐
│              HTTP Request/Response                       │
│  - Build request with authentication                     │
│  - Execute HTTP call                                     │
│  - Parse response                                        │
│  - Extract content via JSONPath                          │
└─────────────────────────────────────────────────────────┘
```

### Hot Reload Flow

```
File Change (Create/Modify/Delete)
    │
    ▼
notify crate detects event (< 500ms)
    │
    ▼
ProtocolLoader.load_from_file()
    │
    ├─> Parse YAML → ProtocolDefinition
    ├─> Create ConfigurableProtocol
    └─> Registry.register() (atomic update)
    │
    ▼
New requests use updated protocol
```

### For More Details

See comprehensive architecture documentation:
- **[ARCHITECTURE.md](./ARCHITECTURE.md)** - Full system architecture
- **[Provider System](./ARCHITECTURE.md#provider-system)** - Provider abstraction layer
- **[Protocol Adapter Design](../docs/plans/2026-02-04-protocol-adapter-phase4-design.md)** - Detailed design document

## Advanced Topics

### Environment Variable Substitution

Use environment variables for sensitive data:

```yaml
custom:
  auth:
    value_template: "Bearer {{env.MY_PROVIDER_API_KEY}}"
```

```bash
export MY_PROVIDER_API_KEY="secret-key"
aether chat --provider my-provider "Hello"
```

**Benefits**:
- Keep API keys out of config files
- Different keys for different environments
- Easier key rotation

### Session Context

Access session-specific data:

```yaml
request_template:
  metadata:
    session_id: "{{session_id}}"
    user: "{{config.user_id}}"
```

### Complex Request Templates

Build sophisticated request structures:

```yaml
request_template:
  # Conditional fields (using defaults)
  model: "{{config.model | default: 'gpt-3.5-turbo'}}"

  # Nested structures
  messages:
    - role: system
      content: "{{system_prompt | default: 'You are helpful.'}}"
    - role: user
      content: "{{input}}"

  # Arrays of values
  stop_sequences: ["###", "---", "END"]

  # Provider-specific parameters
  options:
    temperature: "{{config.temperature | default: 1.0}}"
    top_p: "{{config.top_p | default: 1.0}}"
    presence_penalty: "{{config.presence_penalty | default: 0.0}}"
```

### Multiple Protocols for One Provider

You can define multiple protocol variants:

```yaml
# ~/.aleph/protocols/openai-gpt4.yaml
name: openai-gpt4
extends: openai
differences:
  request_fields:
    temperature:
      default: 0.2  # Lower for GPT-4

# ~/.aleph/protocols/openai-gpt3.yaml
name: openai-gpt3
extends: openai
differences:
  request_fields:
    temperature:
      default: 0.8  # Higher for GPT-3.5
```

Then use different protocols for different use cases:

```toml
[[providers]]
name = "gpt4-precise"
protocol = "openai-gpt4"
model = "gpt-4"

[[providers]]
name = "gpt3-creative"
protocol = "openai-gpt3"
model = "gpt-3.5-turbo"
```

## Future Features

These features are planned but not yet implemented:

### Content Alternatives

Fallback paths for extracting content:

```yaml
response_mapping:
  content_alternatives:
    - "$.choices[0].message.content"
    - "$.result.text"
    - "$.output.completion"
```

### Model Aliases

Map common model names to provider-specific identifiers:

```yaml
model_aliases:
  gpt-4: provider-premium-v2
  gpt-3.5-turbo: provider-fast-v1
```

### Rate Limiting

Provider-specific rate limit hints:

```yaml
rate_limits:
  requests_per_minute: 60
  tokens_per_minute: 100000
  concurrent_requests: 5
```

### Retry Configuration

Custom retry behavior per protocol:

```yaml
retry:
  max_attempts: 3
  initial_delay_ms: 1000
  backoff_multiplier: 2.0
  retry_on_status: [429, 500, 502, 503]
```

### Usage Metadata Extraction

Extract token usage from responses:

```yaml
response_mapping:
  usage:
    prompt_tokens: "$.usage.input_tokens"
    completion_tokens: "$.usage.output_tokens"
```

## Contributing

Found a useful protocol configuration? Please contribute it!

1. Test thoroughly with the provider
2. Add helpful comments explaining customizations
3. Submit a PR to `examples/protocols/`
4. Include usage instructions in comments

## Additional Resources

- **[Protocol Examples](../examples/protocols/)** - Working example configurations
- **[ARCHITECTURE.md](./ARCHITECTURE.md)** - System architecture overview
- **[Phase 4 Design](../docs/plans/2026-02-04-protocol-adapter-phase4-design.md)** - Implementation design document
- **[Phase 4 Implementation](../docs/plans/2026-02-04-protocol-adapter-phase4-implementation.md)** - Step-by-step implementation plan
