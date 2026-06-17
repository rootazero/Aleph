# Config Editing Rules & Reference

## Editing Rules

1. Use `bash` tool — `file_ops` cannot access `~/.aleph/config.toml` (denied_paths).
2. Use python3 for complex edits — safer than sed for TOML.
3. Subsections must appear after parent and before next top-level section.
4. Verify after edit: `grep -A10 'section' ~/.aleph/config.toml`
5. Config auto-reloads (fswatch 500ms) — no restart needed except generation providers.
6. Never write secrets to config.toml — use `vault_store`.

## config.toml Section Order

Sections should appear in this order to match the Config struct serialization:

```
default_hotkey
[general]
[memory]
[providers.*]
[rules]
[behavior]
[search]
[skills]
[tools]
[mcp]
[unified_tools]
[tool_service]
[sandbox]
[smart_flow]
[smart_matching]
[dispatcher]
[agent]                    # alias [cowork] for backward compatibility
[policies]
[generation]
  [generation.providers.*]
  [generation.image_providers.*]
  [generation.video_providers.*]
  [generation.speech_providers.*]
  [generation.audio_providers.*]
[orchestrator]
[subagent]
[route]                    # local-vs-cloud failover routing
[group_chat]
[cron]
[heartbeat]
[tasks_reaper]             # alias [task_reaper]
[personas]
[evolution]
[media]
[privacy]
[security]
[ssrf]
[profiles.*]
[secret_providers.*]
[secrets.*]
[secrets_config]
[prompt]
[channels.*]
[a2a]
[acp]
[execution]
[agents]
[bindings]
[plugin_marketplaces.*]
[stop_hooks]
[guardrails]
[stability]
[fallback_provider]
[context_budget]
[resume]
[projects]                 # project-workspace filesystem scope
```

## Process Management (CRITICAL)

**Kill all aleph processes before restart.** Multiple concurrent processes compete for `.shared_token`, causing HMAC failure and vault data loss.

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
ps aux | grep "[a]leph-server" | grep -v zsh | grep -v cp | grep -v tail
# Then start
target/release/aleph-server start
```

Never:
- Start new process with old process running
- Run multiple instances using same `~/.aleph/data/`
- `kill -9` without waiting 2s for file locks to release
