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
enabled = true               # Master switch for cron system
db_path = "..."              # SQLite store for jobs (defaults under ~/.aleph/data)
check_interval_secs = 60     # Scheduler tick interval
max_concurrent_jobs = 5      # Max jobs running simultaneously
job_timeout_secs = 900       # Per-job timeout (default 900)
```

## Job Management

Individual cron jobs are managed via the `cron_manage` tool, not config files:

```
cron_manage(action="create", name="Morning Report",
  prompt="Generate today's task summary",
  schedule={"type":"cron","expr":"0 0 9 * * *","timezone":"Asia/Shanghai"})

cron_manage(action="list")

cron_manage(action="run", job_id="abc-123")      # Trigger immediately
cron_manage(action="delete", job_id="abc-123")

cron_manage(action="enable", job_id="abc-123")
cron_manage(action="disable", job_id="abc-123")
```

## Schedule Types

| Type | Description | Example |
|------|-------------|---------|
| `cron` | Standard **6-field** cron (sec min hour dom mon dow) | `{"type":"cron","expr":"0 0 9 * * *"}` (daily at 09:00) |
| `every` | Interval-based (field is `every_ms`, min 1000) | `{"type":"every","every_ms":3600000}` |
| `at` | One-shot at specific time | `{"type":"at","at_ms":1711944000000}` |

> **Cron is 6-field, not 5** — the leading field is **seconds**. A 5-field
> expression (`"0 9 * * *"`) is rejected. Daily-at-09:00 = `"0 0 9 * * *"`.

## Manual Execution

You can trigger a job to run immediately without waiting for its schedule:

```
cron_manage(action="run", job_id="abc-123")
```

This sets the job's next execution time to "now". The job will run on the next timer tick (within 60 seconds). The job must be enabled and not already running.

## Caveats
- Jobs are stored in runtime state, not in config.toml
- `cron.enabled = false` stops all scheduled jobs
- Jobs run as the default agent unless specified otherwise
- Use `cron_manage(action="list")` to see all active jobs
