# Protocol Configuration Examples

This directory contains example YAML protocol configurations for the Aether Protocol Adapter system.

## Overview

The Protocol Adapter allows you to integrate any LLM provider with Aether by defining custom protocol configurations. There are two modes:

### 1. Minimal Configuration Mode (Protocol Extension)

**Use when**: The provider's API is mostly OpenAI-compatible, and you only need to customize a few fields.

**Example**: `groq-custom.yaml`

This mode extends an existing protocol (like OpenAI) and only specifies the differences:

```yaml
name: groq-custom
extends: openai
base_url: https://api.groq.com/openai/v1

differences:
  auth:
    header: X-API-Key
    prefix: ""

  request_fields:
    temperature:
      default: 0.7
      range: [0.0, 2.0]
```

**Benefits**:
- Minimal configuration needed
- Inherits all OpenAI protocol features
- Automatically gets future OpenAI protocol updates
- Easy to maintain

### 2. Full Template Mode (Custom Protocol)

**Use when**: The provider's API is completely different from OpenAI, requiring full control over request/response structure.

**Example**: `exotic-ai.yaml`

This mode defines a completely custom protocol from scratch:

```yaml
name: exotic-ai
base_url: https://api.exotic.ai

custom:
  auth:
    type: header
    header: X-API-Token
    value_template: "{{config.api_key}}"

  endpoints:
    chat: "/v2/completions"

  request_template:
    model_name: "{{config.model}}"
    input_text: "{{input}}"
    parameters:
      temperature: "{{config.temperature}}"

  response_mapping:
    content: "$.output.generated_text"
    error: "$.error.message"
```

**Benefits**:
- Full control over request/response structure
- Support for any API design
- Custom authentication methods
- Provider-specific features

## Usage Instructions

### Step 1: Choose an Example

1. **For OpenAI-compatible providers** (Groq, Together, Perplexity, etc.):
   - Start with `groq-custom.yaml`
   - Modify the `base_url` and `auth` section
   - Add any provider-specific customizations

2. **For custom API providers**:
   - Start with `exotic-ai.yaml`
   - Replace all sections with your provider's API structure
   - Test incrementally: auth → basic request → response → streaming

### Step 2: Copy to Configuration Directory

```bash
# Create the protocols directory if it doesn't exist
mkdir -p ~/.aether/protocols

# Copy and rename the example
cp groq-custom.yaml ~/.aether/protocols/my-provider.yaml

# Or for custom protocols
cp exotic-ai.yaml ~/.aether/protocols/my-custom-provider.yaml
```

### Step 3: Edit the Configuration

Open the file in your text editor and customize:

```bash
# Using your preferred editor
nano ~/.aether/protocols/my-provider.yaml
# or
code ~/.aether/protocols/my-provider.yaml
```

**Key fields to update**:
- `name`: Unique identifier for this protocol
- `base_url`: Provider's API base URL
- `auth`: Authentication header and format
- `request_template`: How to structure requests
- `response_mapping`: How to extract responses

### Step 4: Reference in Provider Configuration

#### Option A: In `config.toml`

```toml
[[providers]]
name = "my-provider"
protocol = "my-provider"  # References ~/.aether/protocols/my-provider.yaml
api_key = "your-api-key-here"
model = "provider-model-name"
```

#### Option B: Using CLI Flags

```bash
aether chat --protocol my-provider --model provider-model-name
```

### Step 5: Test the Configuration

```bash
# Test with a simple prompt
aether chat --provider my-provider "Hello, world!"

# Test streaming
aether chat --provider my-provider --stream "Tell me a story"

# Enable debug logging to troubleshoot
RUST_LOG=debug aether chat --provider my-provider "Test"
```

## Configuration Sections Explained

### Required Sections

- **name**: Unique identifier for this protocol
- **base_url**: API base URL
- **auth**: Authentication configuration
- **endpoints**: API endpoint paths
- **request_template**: Request structure mapping
- **response_mapping**: Response extraction rules

### Optional Sections

- **stream_config**: Streaming configuration (SSE or NDJSON)

### For Extended Protocols Only

- **extends**: Base protocol to extend (e.g., `openai`)
- **differences**: Only the fields that differ from base protocol
  - **auth**: Authentication configuration override
  - **request_fields**: Parameter defaults and validation (supported in differences only)

### Future Features (Not Yet Implemented)

The following features are documented in examples but not yet implemented:
- **model_aliases**: Model name mapping
- **rate_limits**: Rate limiting hints
- **retry**: Retry configuration
- **content_alternatives**: Fallback response paths
- **finish_reason mapping**: Provider-specific finish reason translation
- **usage metadata extraction**: Token usage tracking
- **content_mode**: Delta vs. full content streaming

These are commented out in the example files and will be added in future versions.

## Template Variables

The Protocol Adapter supports template variables in `auth.value_template` and `request_template`:

- `{{config.api_key}}`: API key from provider config
- `{{config.model}}`: Model name from provider config
- `{{config.*}}`: Any provider config field
- `{{env.VAR_NAME}}`: Environment variable
- `{{input}}`: User input message
- `{{system_prompt}}`: System prompt
- `{{messages}}`: Structured message array
- `{{session_id}}`: Current session ID

### Example with Environment Variable

```yaml
auth:
  type: header
  header: Authorization
  value_template: "Bearer {{env.MY_PROVIDER_API_KEY}}"
```

Then set the environment variable:

```bash
export MY_PROVIDER_API_KEY="your-api-key"
aether chat --provider my-provider "Hello"
```

## JSONPath for Response Mapping

Response mapping uses JSONPath syntax to extract data:

```yaml
response_mapping:
  # Extract from nested object
  content: "$.output.generated_text"

  # Extract from array (first element)
  content: "$.choices[0].message.content"

  # With fallbacks
  content_alternatives:
    - "$.result.text"
    - "$.data.completion"

  # Nested metadata
  usage:
    prompt_tokens: "$.usage.input_tokens"
    completion_tokens: "$.usage.output_tokens"
```

### Common JSONPath Expressions

- `$.field`: Root level field
- `$.nested.field`: Nested field
- `$.array[0]`: First array element
- `$.array[*]`: All array elements
- `$..field`: Recursive search for field

## Troubleshooting

### Protocol Not Found

**Error**: `Protocol 'my-provider' not found`

**Solution**:
1. Check file exists: `ls ~/.aether/protocols/my-provider.yaml`
2. Verify filename matches protocol name in YAML
3. Check YAML syntax: `aether protocol validate my-provider`

### Authentication Errors

**Error**: `401 Unauthorized` or `403 Forbidden`

**Solution**:
1. Verify API key is correct
2. Check `auth.header` matches provider's requirements
3. Verify `auth.prefix` (some providers don't use "Bearer ")
4. Test with provider's official examples

### Response Parsing Errors

**Error**: `Failed to extract content from response`

**Solution**:
1. Enable debug logging: `RUST_LOG=debug aether chat ...`
2. Examine actual response structure in logs
3. Update `response_mapping.content` JSONPath
4. Add `content_alternatives` for fallbacks

### Streaming Not Working

**Error**: `Stream ended without content`

**Solution**:
1. Verify `stream_config.format` (sse vs. ndjson)
2. Check `stream_config.content_path` matches chunk structure
3. Verify `stream_config.done_marker` is correct
4. Test non-streaming first to validate basic connectivity

## Additional Resources

- **Full Documentation**: See `docs/PROTOCOL_ADAPTER_USER_GUIDE.md` (coming soon - Task 9)
- **Built-in Protocols**: `core/src/providers/protocols/` directory
- **Architecture Documentation**: See `docs/ARCHITECTURE.md` for system overview

## Contributing

Found a useful protocol configuration? Consider contributing it:

1. Test thoroughly with the provider
2. Add helpful comments
3. Submit a PR to `examples/protocols/`
4. Include usage instructions in comments

## Examples by Provider Type

### OpenAI-Compatible Providers
- Groq → `groq-custom.yaml` (extend OpenAI)
- Together AI → extend OpenAI, change base_url
- Perplexity → extend OpenAI, change base_url + auth
- OpenRouter → extend OpenAI, change base_url

### Custom Protocol Providers
- Anthropic (if not built-in) → custom protocol
- Cohere → custom protocol
- AI21 → custom protocol
- Custom enterprise APIs → `exotic-ai.yaml` template

## License

These examples are provided as-is for reference. Modify freely for your use case.
