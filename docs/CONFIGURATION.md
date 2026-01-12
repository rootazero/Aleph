# Configuration Schema

This document describes Aether's configuration schema stored in `~/.config/aether/config.toml`.

## Complete Configuration Example

```toml
[general]
theme = "cyberpunk"                # cyberpunk | zen | jarvis
default_provider = "openai"

[shortcuts]
summon = "Command+Grave"           # Cmd + ~
cancel = "Escape"

[behavior]
input_mode = "cut"                 # cut | copy
output_mode = "typewriter"         # typewriter | instant
typing_speed = 50                  # chars per second

[memory]
enabled = true                     # Enable/disable memory module
embedding_model = "bge-small-zh-v1.5"
max_context_items = 5
retention_days = 90
vector_db = "sqlite-vec"
similarity_threshold = 0.7

[search]
enabled = true                     # Enable/disable search capability
default_provider = "tavily"        # Default search provider
fallback_providers = ["searxng"]   # Fallback providers if default fails
max_results = 5                    # Maximum search results
timeout_seconds = 10               # Search timeout

[search.backends.tavily]
provider_type = "tavily"
api_key = "tvly-..."

[search.backends.searxng]
provider_type = "searxng"
base_url = "http://localhost:8888"

[search.backends.google]
provider_type = "google"
api_key = "AIzaSy..."
engine_id = "012345..."            # Custom Search Engine ID

[dispatcher]
enabled = true                     # Enable Dispatcher Layer
l3_enabled = true                  # Enable L3 AI inference routing
l3_timeout_ms = 5000               # L3 inference timeout (ms)
confirmation_enabled = true        # Enable tool confirmation UI
confirmation_threshold = 0.7       # Confidence below this triggers confirmation
confirmation_timeout_ms = 30000    # Auto-cancel after timeout (ms)

[dispatcher.agent]
enabled = true                     # Enable L3 Agent multi-step planning
max_steps = 10                     # Maximum steps in a plan
step_timeout_ms = 30000            # Timeout per step (30s)
enable_rollback = true             # Attempt rollback on failure
plan_confirmation_required = true  # Require user confirmation before execution
allow_irreversible_without_confirmation = false  # Show warning for irreversible steps
heuristics_threshold = 2           # Action signals needed to trigger planning

[routing.pipeline]
enabled = true

[routing.pipeline.cache]
enabled = true
max_size = 1000
ttl_seconds = 3600
decay_half_life_seconds = 600

[routing.pipeline.confidence]
auto_execute = 0.9
requires_confirmation = 0.6
no_match = 0.3

[providers.openai]
api_key = "sk-..."
model = "gpt-4o"
base_url = "https://api.openai.com/v1"
color = "#10a37f"

[providers.claude]
api_key = "sk-ant-..."
model = "claude-3-5-sonnet-20241022"
color = "#d97757"

[[rules]]
regex = "^/translate"
provider = "openai"
system_prompt = "You are a translator."
capabilities = ["memory"]          # Enable Memory capability for context
intent_type = "translation"        # Custom intent classification
context_format = "markdown"        # Context format (markdown | xml | json)

[[rules]]
regex = "^/search"
provider = "openai"
system_prompt = "You are a search assistant. Answer based on search results."
capabilities = ["search"]          # Enable Search capability
intent_type = "web_search"
context_format = "markdown"

[[rules]]
regex = "^/research"
provider = "claude"
system_prompt = "You are a research analyst. Use memory and search to provide comprehensive answers."
capabilities = ["memory", "search"]  # Enable both Memory and Search
intent_type = "research"
context_format = "markdown"

[[rules]]
regex = "^/draw"
provider = "openai"
system_prompt = "You are DALL-E. Generate images."

[[rules]]
regex = ".*"                       # Catch-all
provider = "openai"
capabilities = ["memory"]          # Enable memory for all requests
```

---

## Section Reference

### [general]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `theme` | String | `"cyberpunk"` | UI theme: cyberpunk, zen, jarvis |
| `default_provider` | String | Required | Default AI provider name |

### [shortcuts]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `summon` | String | `"Command+Grave"` | Hotkey to activate Aether |
| `cancel` | String | `"Escape"` | Hotkey to cancel operation |

### [behavior]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `input_mode` | String | `"cut"` | cut (removes text) or copy (keeps text) |
| `output_mode` | String | `"typewriter"` | typewriter (char-by-char) or instant |
| `typing_speed` | Integer | `50` | Characters per second for typewriter mode |

### [memory]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | Boolean | `true` | Enable/disable memory module |
| `embedding_model` | String | `"bge-small-zh-v1.5"` | Embedding model for vector search |
| `max_context_items` | Integer | `5` | Max memory entries to retrieve |
| `retention_days` | Integer | `90` | Auto-delete entries older than N days |
| `vector_db` | String | `"sqlite-vec"` | Vector database engine |
| `similarity_threshold` | Float | `0.7` | Min cosine similarity (0.0-1.0) |

### [search]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | Boolean | `true` | Enable/disable search capability |
| `default_provider` | String | Required | Default search provider |
| `fallback_providers` | Array | `[]` | Fallback providers if default fails |
| `max_results` | Integer | `5` | Maximum search results |
| `timeout_seconds` | Integer | `10` | Search request timeout |

**Supported Search Providers**: tavily, searxng, google, bing, brave, exa

### [dispatcher]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | Boolean | `true` | Enable Dispatcher Layer |
| `l3_enabled` | Boolean | `true` | Enable L3 AI inference routing |
| `l3_timeout_ms` | Integer | `5000` | L3 inference timeout (ms) |
| `confirmation_enabled` | Boolean | `true` | Enable tool confirmation UI |
| `confirmation_threshold` | Float | `0.7` | Confidence below this triggers confirmation |
| `confirmation_timeout_ms` | Integer | `30000` | Auto-cancel after timeout |

### [dispatcher.agent]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | Boolean | `true` | Enable L3 Agent multi-step planning |
| `max_steps` | Integer | `10` | Maximum steps in a plan |
| `step_timeout_ms` | Integer | `30000` | Timeout per step |
| `enable_rollback` | Boolean | `true` | Attempt rollback on failure |
| `plan_confirmation_required` | Boolean | `true` | Require user confirmation |
| `heuristics_threshold` | Integer | `2` | Action signals for planning trigger |

### [providers.*]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `api_key` | String | Required | API key for the provider |
| `model` | String | Required | Model identifier |
| `base_url` | String | Provider default | Custom API endpoint |
| `color` | String | Provider default | UI color for this provider |
| `system_prompt_mode` | String | `"normal"` | `"normal"` or `"prepend"` |

### [[rules]]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `regex` | String | Required | Regex pattern to match user input |
| `provider` | String | Required | AI provider name |
| `system_prompt` | String | Optional | Base system prompt for this rule |
| `capabilities` | Array | `[]` | Capabilities: `["memory", "search", "mcp"]` |
| `intent_type` | String | `null` | Intent classification for routing |
| `context_format` | String | `"markdown"` | Context format: markdown, xml, json |

---

## System Prompt Mode

Some APIs (like certain OpenAI-compatible endpoints) ignore the `system` role message. For these providers, configure `system_prompt_mode = "prepend"`:

```toml
[providers.my_provider]
provider_type = "openai"
system_prompt_mode = "prepend"  # Prepend system prompt to user message
```

**Prepend Mode Logic**:

```
Normal Mode:
  system_message = "You are a helpful AI assistant." + memory_context + search_results
  user_message = user_input

Prepend Mode (with rule prompt):
  system_message = (none, or ignored by API)
  user_message = [Instruction] rule_prompts + memory_context + search_results
                 ---
                 [User Input] user_input
```

---

## Routing Rules & Prompt Assembly

### Rule Types

1. **Builtin Commands** (not user-customizable):
   - `/search` → Search capability
   - `/mcp` → MCP capability (future)
   - `/skill` → Skill capability (future)

2. **User-Defined Slash Commands** (in config.toml):
   - `/zh`, `/en`, `/draw`, etc.
   - Only have `system_prompt`, no special capabilities
   - Only ONE slash command can match per request

3. **Keyword Rules** (in config.toml):
   - Match by regex patterns in user input
   - Multiple keyword rules can match simultaneously
   - Can be combined with slash commands

### Prompt Assembly Order

```
final_system_prompt = slash_command_prompt + keyword_rule1_prompt + keyword_rule2_prompt + ...
                      (separated by \n\n)
```

### Memory Behavior

- Memory is available in EVERY conversation (regardless of command used)
- Memory provides context for accuracy and continuity
- Memory should NOT directly interfere with the response

---

**Last Updated**: 2026-01-11
