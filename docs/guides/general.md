# General Configuration Guide

## File Path
- Main: `~/.aleph/config.toml` sections `[general]`, `[memory]`, `[policies]`, `[behavior]`

## Operation Rules
1. Before modification: `cp ~/.aleph/config.toml ~/.aleph/config.toml.bak`
2. After modification: auto-reloads via fswatch, no restart needed

## [general]

```toml
[general]
default_provider = "openai"    # Fallback provider when no routing rule matches
language = "zh-Hans"           # UI/response language (null = system language)
queue_mode = "followup"        # "followup" | "steer" | "collect"
collect_window_ms = 3000       # Batch window for collect mode
```

## [memory]

```toml
[memory]
enabled = true
max_context_items = 10         # Past interactions to retrieve
retention_days = 0             # 0 = never delete
vector_db = "sqlite-vec"       # "sqlite-vec"
similarity_threshold = 0.5     # 0.0-1.0
compression_enabled = true
compression_turn_threshold = 20
max_facts_in_context = 5
backup_enabled = true
```

## [behavior]

```toml
[behavior]
output_mode = "typewriter"     # "typewriter" | "instant"
typing_speed = 200             # Characters per second (50-400)
```

## [policies]

```toml
[policies]
# Top-level policy toggles live in the subsections below.

# Per-tool permission gate
[policies.tool_permissions]
default = "allow"              # "allow" | "deny" | "ask"
# overrides = { vault_store = "ask", agent_delete = "ask" }

[policies.retry]
max_retries = 3
initial_backoff_ms = 1000      # field is initial_backoff_ms (not backoff_ms)
backoff_multiplier = 2.0
max_backoff_ms = 30000

[policies.web_fetch]
timeout_seconds = 30
max_content_length = 10000     # max chars of fetched body (there is no `enabled`/`max_body_bytes` field)

# [policies.memory] has no scalar fields — it is composed of subsections:
[policies.memory.compression]
turn_threshold = 40
[policies.memory.ai_retrieval]
timeout_ms = 5000
```

## [dispatcher]

> ⚠️ **Legacy / inert.** The dispatcher was dissolved in the Dispatcher-dissolution
> refactor (CLAUDE.md R7). This section still parses for backward compatibility but
> has no runtime consumer — tool routing is now handled by the LLM in the main loop.
> Setting these fields has no effect.

```toml
[dispatcher]
enabled = true                 # Master switch for the dispatcher (tool routing)
l3_enabled = true              # L3 = AI-powered tool inference
l3_timeout_ms = 5000
confirmation_threshold = 0.7   # Below this confidence → ask the user before running
confirmation_timeout_ms = 30000
```

## Common Operations

### Change default language
Set `general.language = "en"` or `"zh-Hans"`.

### Adjust memory retrieval
Change `memory.max_context_items` and `memory.similarity_threshold`.

### Switch to instant output
Set `behavior.output_mode = "instant"`.

## Caveats
- `queue_mode = "collect"` batches rapid messages — useful for chat platforms
- `retention_days = 0` means memories are kept forever
- Memory settings affect all agents globally
