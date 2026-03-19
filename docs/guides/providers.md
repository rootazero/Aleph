# Providers Configuration Guide

## File Path
- Main: `~/.aleph/config.toml` section `[providers.<name>]`
- API keys: encrypted vault (key: `provider:{name}`)

## Operation Rules
1. Before modification: `cp ~/.aleph/config.toml ~/.aleph/config.toml.bak`
2. API keys via `vault_store(action="store", key="provider:<name>", secret="<key>")`
3. Never write API keys to config.toml — the `api_key` field is runtime-only
4. After modification: auto-reloads via fswatch, no restart needed

## Structure

```toml
[providers.openai]
protocol = "openai"        # "openai" | "anthropic" | "gemini" | "ollama"
models = ["gpt-4o", "gpt-4o-mini"]  # First model is default
base_url = "https://api.openai.com/v1"  # Optional, defaults to official
color = "#10a37f"          # Hex color for UI
timeout_seconds = 300
enabled = true             # Must be true to use
# api_key — DO NOT SET HERE, use vault_store

# Optional generation parameters
max_tokens = 4096
temperature = 0.7          # 0.0-2.0
top_p = 0.9                # 0.0-1.0
```

## Common Operations

### Add a new provider
1. Add `[providers.<name>]` section to config.toml
2. Set protocol, models, enabled = true
3. Store API key: `vault_store(action="store", key="provider:<name>", secret="...")`

### Change default provider
```toml
[general]
default_provider = "claude"
```

### Disable a provider
Set `enabled = false` in the provider section.

### Delete a provider
1. Remove `[providers.<name>]` section from config.toml
2. Remove API key: `vault_store(action="delete", key="provider:<name>")`

## Protocol-Specific Fields
- **anthropic**: `max_tokens` (required by API), `stop_sequences`
- **gemini**: `thinking_level = "LOW"|"HIGH"`, `media_resolution = "LOW"|"MEDIUM"|"HIGH"`
- **ollama**: `base_url = "http://localhost:11434"`, `repeat_penalty = 1.1`

## Caveats
- `models` accepts a single string `model = "gpt-4o"` for backward compat
- Provider is disabled by default — always set `enabled = true`
- `verified = false` resets after config change; set true after successful test
