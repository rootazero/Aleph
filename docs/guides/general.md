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
vector_db = "lancedb"          # "lancedb" | "sqlite-vec"
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
# Tool safety
tool_confirmation_required = ["vault_store", "file_ops"]

[policies.retry]
max_retries = 3
backoff_ms = 1000

[policies.web_fetch]
enabled = true
timeout_seconds = 30
max_body_bytes = 1048576       # 1MB

[policies.memory]
auto_save = true
```

## [dispatcher]

```toml
[dispatcher]
# Tool routing configuration
max_tool_calls_per_turn = 10
parallel_tool_calls = true
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
