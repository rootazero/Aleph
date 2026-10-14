# Config Editing Rules & Reference

## Editing Rules

0. **Config path — one spelling, both languages.** `ALEPH_HOME` points *at* the
   `.aleph` directory (not a parent to join `.aleph` onto); unset means `~/.aleph`.
   - bash: `"${ALEPH_HOME:-$HOME/.aleph}/config.toml"`
   - python3: `os.path.join(os.path.expanduser(os.environ.get("ALEPH_HOME", "~/.aleph")), "config.toml")`

   Never hardcode `~/.aleph` in a command that writes — on a host with
   `ALEPH_HOME` set it edits a file the daemon never reads, and reports success.
   Never hand `"~/.aleph/…"` or `"$HOME/.aleph/…"` to Python's `open()` — it
   expands neither: reads raise `FileNotFoundError`, writes create a literal
   `./~/.aleph/` tree in the cwd.
1. Use `bash` tool — `file_ops` cannot access `~/.aleph/config.toml` (denied_paths).
2. Use python3 for complex edits — safer than sed for TOML.
3. Subsections must appear after parent and before next top-level section.
4. Verify after edit: `grep -A10 'section' "${ALEPH_HOME:-$HOME/.aleph}/config.toml"`
5. Config auto-reloads (fswatch 500ms) — no restart needed except generation providers.
6. Never write secrets to config.toml — use `vault_store`, then reference the
   stored name with a `{{secret:NAME}}` placeholder.

## config.toml Section Order

Sections should appear in this order to match the Config struct serialization:

```
[general]
[memory]
[providers.*]
[rules]                    # slash commands only; a regex without a `^/` prefix is inert
                           # (warned at load, refused by routing_rules.create/.update)
[behavior]
[search]
[fetch]                    # URL→markdown fetch backends; NOT wired into web_fetch (BT-D-R4-22),
                           # only the fetch_config.test connection test reads them
[tools]
[mcp]
[unified_tools]
[tool_service]
[sandbox]
[policies]
[generation]
  [generation.providers.*]
  [generation.image_providers.*]
  [generation.video_providers.*]
  [generation.speech_providers.*]
  [generation.audio_providers.*]
[voice]                    # [voice.local] / [voice.streaming] / [voice.format]
[route]                    # local-vs-cloud failover routing
[group_chat]
[cron]
[heartbeat]
[tasks_reaper]             # alias [task_reaper]
[personas]
[privacy]
[security]
  [security.ssrf]          # foreign-owned: honoured by a raw-TOML bridge in load.rs
[ssrf]
[profiles.*]               # model + [profiles.*.smart_recall] only
[secrets_config]           # virtual_keys, custom_leak_patterns
[prompt]
[channels.*]
[a2a]
[acp]
[execution]
[agents]
[bindings]
[session]                  # dm_scope = "per-peer" (default) | "main"
[plugin_marketplaces.*]
[stop_hooks]
[guardrails]
[stability]
[fallback_provider]
[context_budget]
[strategy]                 # strategic planner for /goal, /loop, /workflow
[moa]                      # Mixture-of-Agents advisory presets
[team_dispatcher]
[team_broadcast]
[team_messages]
[resume]
[projects]                 # project-workspace filesystem scope
[desktop]                  # allow_global_pointer only (§7.6)
```

`[gateway]` is also a valid, load-bearing section of this same file — it is read
by `GatewayConfig::load_default` (`src/gateway/config.rs`), not by `Config`, so it
has no fixed position in the serialization order above. Because `Config` has no
`gateway` field, `self_config` cannot read or patch that path — edit `[gateway]`
with the `bash` tool instead.

**Retired — never write these.** `[agent]` / `[cowork]`, `[secret_providers]`,
`[desktop.presence]`, `[desktop.mic_level]`, `profiles.*.system_prompt`,
`profiles.*.tools`, `profiles.*.cache_strategy`, `agents.*.system_prompt`,
`memory.compound_ingest.replan_on_hash_conflict`,
`memory.compound_ingest.failure_cooldown_seconds`. They are listed in
`src/config/dead_keys.rs` as tolerated-and-retired: an existing file keeps parsing
(no `deny_unknown_fields`), the loader logs each at `info!` asking the operator to
delete the line, and none of them reach any code.

Anything **not** listed above and **not** in that retired list is reported dead —
`Config::load_from_file` warns and `doctor`'s `core/config-parse` raises a Warning
naming the path.

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
