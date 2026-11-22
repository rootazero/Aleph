# General Configuration Guide

## File Path
- Main: `~/.aleph/config.toml` sections `[general]`, `[memory]`, `[policies]`, `[behavior]`

## Operation Rules
1. Before modification: `cp ~/.aleph/config.toml ~/.aleph/config.toml.bak`
2. After modification: call the `config.reload` RPC (or restart the daemon).
   The file watcher does **not** apply the new values: on a file event it
   swaps its own snapshot and hot-applies PII privacy only. A `config.reload`
   re-reads the file into the running config and additionally pushes the
   live-apply targets (`[behavior]`, `[execution]`, `[search]`,
   `[policies.spend]`, `[policies.terminal]`) into the running runtime.
3. Unknown keys are reported, not ignored: the loader's dead-key scan names
   every key that reaches no code. If the loader calls a key dead, it is dead
   — do not write it.

## [general]

```toml
[general]
default_provider = "openai"    # Fallback provider when no routing rule matches
language = "zh-Hans"           # UI/response language (null = system language)
fallback_providers = []        # Tried in order when the default provider fails transiently
session_store_backend = "file" # "file" (default) | "sqlite"
```

## [memory]

```toml
[memory]
enabled = true
rrf_k = 60                     # Reciprocal-rank-fusion constant for retrieval
bm25_bonus_weight = 0.5        # Bonus added to the BM25 score
```

Most memory knobs live in subsections (`[memory.embedding]`,
`[memory.dreaming]`, `[memory.memory_decay]`, `[memory.rerank]`,
`[memory.retrieval_scoring]`, `[memory.expansion]`, …). Read
`config.schema` for the full list — the top-level scalars above are the ones
that are safe to set by hand.

## [behavior]

```toml
[behavior]
output_mode = "typewriter"     # "typewriter" | "instant"
```

## [policies]

```toml
[policies]
# Execution permission tier: "plan" | "ask" | "auto" | "full"
exec_tier = "ask"
# Default session usage mode: "chat" | "work" | "code"
mode = "work"
# LLM risk triage in front of human approval prompts (off by default)
guardian_review = false

# Per-tool permission gate
[policies.tool_permissions]
default = "allow"              # "allow" | "deny" | "ask"
# overrides = { vault_store = "ask", agent_delete = "ask" }

[policies.web_fetch]
timeout_seconds = 30
max_content_length = 10000     # max chars of fetched body

[policies.memory.compression]
turn_threshold = 40

# [policies.memory] has no scalar fields — it is composed of subsections:
# [policies.memory.compression] and [policies.memory.session_compactor]
```

## Common Operations

### Change default language
Set `general.language = "en"` or `"zh-Hans"`.

### Switch to instant output
Set `behavior.output_mode = "instant"`, then call `config.reload`.

### Make one tool ask first
Add an entry to `[policies.tool_permissions].overrides`, e.g.
`overrides = { bash = "ask" }`.

## Caveats
- Memory settings affect all agents globally
- `[policies]` other than `spend` / `terminal` is not hot-applied into the
  running runtime; `config.reload` or a restart is required
