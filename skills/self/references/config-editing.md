# Config Editing Rules & Reference

## Editing Rules

1. Use `bash` tool — `file_ops` cannot access `~/.aleph/config.toml` (denied_paths).
2. Use python3 for complex edits — safer than sed for TOML.
3. Subsections must appear after parent and before next top-level section.
4. Verify after edit: `grep -A10 'section' ~/.aleph/config.toml`
5. Config auto-reloads (fswatch 500ms) — no restart needed except generation providers.
6. Never write secrets to config.toml — use `vault_store`.

## config.toml Section Order

```
default_hotkey
[a2a]
[acp]
[agent]
[agents]
[behavior]
[channels.*]
[cron]
[dispatcher]
[evolution]
[general]
[generation]
  [generation.providers.*]
  [generation.image_providers.*]
  [generation.video_providers.*]
  [generation.speech_providers.*]
  [generation.audio_providers.*]
[group_chat]
[mcp]
[media]
[memory]
[orchestrator]
[policies]
[privacy]
[prompt]
[providers.*]
[rules]
[search]
[skills]
[smart_flow]
[smart_matching]
[subagent]
[task_routing]
[tools]
[unified_tools]
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
