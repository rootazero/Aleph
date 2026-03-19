# Cron Configuration Guide

## File Path
- Main: `~/.aleph/config.toml` section `[cron]`

## Operation Rules
1. Before modification: `cp ~/.aleph/config.toml ~/.aleph/config.toml.bak`
2. Runtime job management (create/delete/enable/disable): use `cron_manage` tool
3. Config only controls global cron behavior, not individual jobs

## Structure

```toml
[cron]
enabled = true             # Master switch for cron system
max_concurrent = 3         # Max jobs running simultaneously
default_timeout_seconds = 300  # Per-job timeout
```

## Job Management

Individual cron jobs are managed via the `cron_manage` tool, not config files:

```
cron_manage(action="create", name="Morning Report",
  prompt="Generate today's task summary",
  schedule={"type":"cron","expr":"0 9 * * *","timezone":"Asia/Shanghai"})

cron_manage(action="list")

cron_manage(action="delete", job_id="abc-123")

cron_manage(action="enable", job_id="abc-123")
cron_manage(action="disable", job_id="abc-123")
```

## Schedule Types

| Type | Description | Example |
|------|-------------|---------|
| `cron` | Standard cron expression | `{"type":"cron","expr":"0 9 * * *"}` |
| `every` | Interval-based | `{"type":"every","interval_ms":3600000}` |
| `at` | One-shot at specific time | `{"type":"at","at_ms":1711944000000}` |

## Caveats
- Jobs are stored in runtime state, not in config.toml
- `cron.enabled = false` stops all scheduled jobs
- Jobs run as the default agent unless specified otherwise
- Use `cron_manage(action="list")` to see all active jobs
