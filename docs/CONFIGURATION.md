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

[cowork]
enabled = true                     # Enable Cowork task orchestration
require_confirmation = true        # Show task graph before execution
max_parallelism = 4                # Max concurrent tasks
dry_run = false                    # Plan without executing
planner_provider = "claude"        # AI provider for task planning
auto_execute_threshold = 0.95      # Confidence for auto-execution
max_tasks_per_graph = 20           # Maximum tasks per graph
task_timeout_seconds = 300         # Task execution timeout (5 min)
sandbox_enabled = true             # Sandbox code execution tasks
allowed_categories = []            # Empty = all allowed
blocked_categories = []            # Empty = none blocked

[cowork.model_profiles.claude-opus]
provider = "anthropic"
model = "claude-opus-4"
capabilities = ["reasoning", "code_generation", "long_context"]
cost_tier = "high"
latency_tier = "slow"
max_context = 200000

[cowork.model_profiles.claude-sonnet]
provider = "anthropic"
model = "claude-sonnet-4"
capabilities = ["code_generation", "code_review", "text_analysis"]
cost_tier = "medium"
latency_tier = "medium"

[cowork.model_profiles.claude-haiku]
provider = "anthropic"
model = "claude-haiku-3.5"
capabilities = ["fast_response", "simple_task"]
cost_tier = "low"
latency_tier = "fast"

[cowork.model_routing]
code_generation = "claude-opus"
code_review = "claude-sonnet"
quick_tasks = "claude-haiku"
reasoning = "claude-opus"
cost_strategy = "balanced"
enable_pipelines = true
default_model = "claude-sonnet"

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

### [cowork]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | Boolean | `true` | Enable Cowork task orchestration |
| `require_confirmation` | Boolean | `true` | Show task graph before execution |
| `max_parallelism` | Integer | `4` | Max concurrent tasks (1-32) |
| `dry_run` | Boolean | `false` | Plan without executing |
| `planner_provider` | String | `null` | AI provider for planning (uses default if null) |
| `auto_execute_threshold` | Float | `0.95` | Confidence for auto-execution (0.0-1.0) |
| `max_tasks_per_graph` | Integer | `20` | Maximum tasks per graph |
| `task_timeout_seconds` | Integer | `300` | Task execution timeout (0 = none) |
| `sandbox_enabled` | Boolean | `true` | Sandbox code execution tasks |
| `allowed_categories` | Array | `[]` | Allowed task types (empty = all) |
| `blocked_categories` | Array | `[]` | Blocked task types (overrides allowed) |

**Valid Categories**: `file_operation`, `code_execution`, `document_generation`, `app_automation`, `ai_inference`

See [COWORK.md](./COWORK.md) for detailed documentation.

### [cowork.model_profiles.*]

Define AI model profiles for intelligent task routing.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `provider` | String | Required | Provider name (anthropic, openai, google, ollama) |
| `model` | String | Required | Model name for API calls |
| `capabilities` | Array | `[]` | Model capabilities |
| `cost_tier` | String | `"medium"` | Cost tier: free, low, medium, high |
| `latency_tier` | String | `"medium"` | Latency tier: fast, medium, slow |
| `max_context` | Integer | `null` | Maximum context window in tokens |
| `local` | Boolean | `false` | Whether this is a local model |

**Valid Capabilities**: `code_generation`, `code_review`, `text_analysis`, `image_understanding`, `video_understanding`, `long_context`, `reasoning`, `local_privacy`, `fast_response`, `simple_task`, `long_document`

**Example**:
```toml
[cowork.model_profiles.claude-opus]
provider = "anthropic"
model = "claude-opus-4"
capabilities = ["reasoning", "code_generation", "long_context"]
cost_tier = "high"
latency_tier = "slow"
max_context = 200000

[cowork.model_profiles.claude-haiku]
provider = "anthropic"
model = "claude-haiku-3.5"
capabilities = ["fast_response", "simple_task"]
cost_tier = "low"
latency_tier = "fast"

[cowork.model_profiles.ollama-llama]
provider = "ollama"
model = "llama3.2"
capabilities = ["local_privacy", "fast_response"]
cost_tier = "free"
latency_tier = "fast"
local = true
```

### [cowork.model_routing]

Configure task-to-model routing rules.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `code_generation` | String | `null` | Model for code generation tasks |
| `code_review` | String | `null` | Model for code review tasks |
| `image_analysis` | String | `null` | Model for image analysis tasks |
| `video_understanding` | String | `null` | Model for video understanding tasks |
| `long_document` | String | `null` | Model for long document processing |
| `quick_tasks` | String | `null` | Model for quick/simple tasks |
| `privacy_sensitive` | String | `null` | Model for privacy-sensitive tasks |
| `reasoning` | String | `null` | Model for complex reasoning tasks |
| `cost_strategy` | String | `"balanced"` | Cost strategy: cheapest, balanced, best_quality |
| `enable_pipelines` | Boolean | `true` | Enable multi-model pipeline execution |
| `default_model` | String | `null` | Default model when no rule matches |

**Cost Strategies**:
- `cheapest` - Always select the lowest cost model with required capabilities
- `balanced` - Balance between cost and quality (default)
- `best_quality` - Always select the highest quality model

**Fallback Chain**:

When routing a task, the Model Router uses a 5-level fallback chain:

1. **Task Type Mapping** - Explicit `[cowork.model_routing]` mappings (e.g., `code_generation = "claude-opus"`)
2. **Capability Matching** - Find model with required capability (e.g., `ImageUnderstanding`)
3. **Cost Strategy** - Apply cost strategy to all configured profiles
4. **Default Model** - Use `default_model` from `[cowork.model_routing]`
5. **Default Provider** - Use `default_provider` from `[general]` as final fallback

The final fallback level ensures that even users without `[cowork.model_profiles]` configured will have working model routing. The system uses `[general].default_provider` with sensible model defaults:

| Provider | Default Model |
|----------|---------------|
| `openai` | `gpt-4o` |
| `anthropic` / `claude` | `claude-sonnet-4-20250514` |
| `google` / `gemini` | `gemini-1.5-flash` |
| `ollama` | `llama3.2` |

**Example**:
```toml
[cowork.model_routing]
code_generation = "claude-opus"
code_review = "claude-sonnet"
image_analysis = "gpt-4o"
quick_tasks = "claude-haiku"
privacy_sensitive = "ollama-llama"
reasoning = "claude-opus"
cost_strategy = "balanced"
enable_pipelines = true
default_model = "claude-sonnet"
```

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
| `provider` | String | Required | AI provider name (legacy, use `preferred_model` for Model Router) |
| `system_prompt` | String | Optional | Base system prompt for this rule |
| `capabilities` | Array | `[]` | Capabilities: `["memory", "search", "mcp"]` |
| `intent_type` | String | `null` | Intent classification (maps to `TaskIntent` for Model Router) |
| `preferred_model` | String | `null` | Model profile ID to override automatic Model Router selection |
| `context_format` | String | `"markdown"` | Context format: markdown, xml, json |

### [policies]

The `[policies]` section enables fine-tuning of system behavior without modifying code. All policy values have sensible defaults and can be partially overridden.

#### [policies.tool_safety]

Configure tool safety classification keywords.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `high_risk_keywords` | Array | `["delete", "remove", "drop", ...]` | Keywords indicating high-risk (irreversible) operations |
| `low_risk_keywords` | Array | `["send", "notify", "post", ...]` | Keywords indicating low-risk operations |
| `reversible_keywords` | Array | `["create", "copy", "update", ...]` | Keywords indicating reversible operations |
| `readonly_keywords` | Array | `["search", "query", "read", ...]` | Keywords indicating read-only operations |
| `builtin_fallback` | String | `"readonly"` | Default safety level for builtin tools |
| `native_fallback` | String | `"reversible"` | Default safety level for native tools |
| `mcp_fallback` | String | `"irreversible_low_risk"` | Default safety level for MCP tools |
| `skill_fallback` | String | `"irreversible_low_risk"` | Default safety level for skills |
| `custom_fallback` | String | `"irreversible_low_risk"` | Default safety level for custom tools |

#### [policies.intent]

Configure AI intent detection behavior.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `confidence_threshold` | Float | `0.7` | Minimum confidence to accept detected intent |
| `timeout_ms` | Integer | `3000` | Timeout for intent detection requests |
| `min_input_length` | Integer | `3` | Minimum input length for intent detection |
| `video_url_patterns` | Array | `["youtube.com/watch", ...]` | URL patterns for video intent detection |

#### [policies.memory]

Configure memory subsystem behavior.

**[policies.memory.compression]**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `idle_timeout_seconds` | Integer | `300` | Idle time before triggering compression |
| `turn_threshold` | Integer | `20` | Conversation turns before compression |
| `background_interval_seconds` | Integer | `3600` | Background compression interval |

**[policies.memory.ai_retrieval]**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `timeout_ms` | Integer | `3000` | AI retrieval request timeout |
| `max_candidates` | Integer | `20` | Max candidates for AI re-ranking |
| `fallback_count` | Integer | `3` | Fallback count if AI re-ranking fails |
| `content_truncate_length` | Integer | `500` | Max content length per entry |

#### [policies.retry]

Configure retry behavior for AI provider requests.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_retries` | Integer | `3` | Maximum retry attempts |
| `initial_backoff_ms` | Integer | `1000` | Initial backoff duration |
| `backoff_multiplier` | Float | `2.0` | Exponential backoff multiplier |
| `retryable_status_codes` | Array | `[500, 502, 503, 504]` | HTTP status codes to retry |
| `retry_on_timeout` | Boolean | `true` | Retry on timeout errors |
| `retry_on_network_error` | Boolean | `true` | Retry on network errors |

#### [policies.web_fetch]

Configure web fetching behavior.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_content_length` | Integer | `10000` | Max extracted content length |
| `min_content_length` | Integer | `100` | Min content to accept selector match |
| `user_agent` | String | `"Aether/1.0"` | User agent string |
| `timeout_seconds` | Integer | `30` | Request timeout |

#### [policies.text]

Configure text formatting behavior.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default_truncate_length` | Integer | `200` | Default text truncation length |
| `search_snippet_length` | Integer | `300` | Search result snippet length |
| `mcp_result_length` | Integer | `2000` | MCP tool result truncation length |

#### [policies.metrics]

Configure performance monitoring.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `target_hotkey_to_clipboard_ms` | Integer | `50` | Target: hotkey to clipboard |
| `target_clipboard_to_memory_ms` | Integer | `100` | Target: clipboard to memory |
| `target_memory_to_ai_ms` | Integer | `500` | Target: memory to AI request |
| `target_ai_to_paste_ms` | Integer | `50` | Target: AI response to paste |
| `target_paste_to_complete_ms` | Integer | `100` | Target: paste to completion |
| `warning_multiplier` | Float | `2.0` | Multiplier for warning threshold |
| `enable_logging` | Boolean | `true` | Enable performance logging |
| `enable_warnings` | Boolean | `true` | Enable performance warnings |

**Example Policies Configuration**:

```toml
[policies.intent]
confidence_threshold = 0.75
timeout_ms = 2500

[policies.memory.compression]
idle_timeout_seconds = 180
turn_threshold = 15

[policies.retry]
max_retries = 5
initial_backoff_ms = 500

[policies.metrics]
warning_multiplier = 1.5
```

---

**Model Router Integration**:

When `preferred_model` is set, it overrides the Model Router's automatic model selection based on `intent_type`. This is useful for:
- Forcing specific models for certain commands
- Testing different models for the same intent
- Legacy provider migration

Example with Model Router:
```toml
[[rules]]
regex = "^/code"
intent_type = "code_generation"        # Maps to TaskIntent::CodeGeneration
preferred_model = "claude-opus"        # Override: use claude-opus instead of auto-selection
system_prompt = "You are an expert programmer."
```

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

## [generation]

Configure media generation providers for image, video, audio, and speech generation.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default_image_provider` | String | `null` | Default provider for image generation |
| `default_video_provider` | String | `null` | Default provider for video generation |
| `default_audio_provider` | String | `null` | Default provider for audio generation |
| `default_speech_provider` | String | `null` | Default provider for speech/TTS |
| `output_dir` | String | `~/Downloads/aether-gen` | Output directory for generated files |
| `auto_paste_threshold_mb` | Integer | `5` | Files larger than this are saved to disk |
| `background_task_threshold_seconds` | Integer | `30` | Duration threshold for background tasks |
| `smart_routing_enabled` | Boolean | `true` | Enable smart routing based on capabilities |

### [generation.providers.*]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `provider_type` | String | Required | Provider type (openai, replicate, recraft, ideogram, kimi, etc.) |
| `model` | String | Required | Model name for generation |
| `api_key` | String | Required | API key for the provider |
| `enabled` | Boolean | `true` | Enable/disable this provider |
| `color` | String | Provider default | UI color for this provider |
| `capabilities` | Array | `[]` | Supported generation types: image, video, audio, speech |

**Supported Providers**:
- `openai` - DALL-E 3 (image)
- `replicate` - Flux, SDXL (image), Various video models
- `recraft` - Recraft V3 (image)
- `ideogram` - Ideogram V2 (image)
- `kimi` - Kimi Visions (image)
- `gemini` - Imagen (image)
- `elevenlabs` - Text-to-Speech

**Example**:
```toml
[generation]
default_image_provider = "dalle"
output_dir = "~/Downloads/aether-gen"
auto_paste_threshold_mb = 5
smart_routing_enabled = true

[generation.providers.dalle]
provider_type = "openai"
model = "dall-e-3"
enabled = true
color = "#10a37f"
capabilities = ["image"]

[generation.providers.replicate]
provider_type = "replicate"
model = "flux-schnell"
api_key = "r8_..."
enabled = true
capabilities = ["image", "video"]

[generation.providers.recraft]
provider_type = "recraft"
model = "recraft-v3"
api_key = "..."
enabled = true
capabilities = ["image"]
```

---

## Runtime Management

Runtimes (Python, Node.js, yt-dlp) are managed automatically by Aether. No user configuration is required.

**Auto-Installed Runtimes**:
| Runtime | Tool | Purpose |
|---------|------|---------|
| Python | uv | MCP servers, scripts |
| Node.js | fnm | JavaScript MCP servers |
| yt-dlp | yt-dlp | Video download |

**Features**:
- Automatic installation on first use
- Background update checking
- Version management via Settings → Runtimes tab

**Runtime Requirements in MCP Config**:
```toml
[mcp.servers.my-server]
name = "my-server"
command = "npx"
args = ["-y", "my-mcp-server"]
requires_runtime = "node"  # Ensures Node.js is installed before running
```

---

**Last Updated**: 2026-01-18
