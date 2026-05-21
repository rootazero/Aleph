# CLI Bug Fix + JSON Unification Design

> Date: 2026-03-04
> Status: Approved
> Scope: Bug fixes + --json support unification

## Bug Fixes

### B1. `session new` calls wrong RPC method
- File: `apps/cli/src/commands/session.rs`
- Issue: Calls `sessions.create` (plural) but server registers `session.create` (singular)
- Fix: Change to `session.create`

### B2. TUI dialog calls wrong method name
- File: `apps/cli/src/tui/mod.rs`
- Issue: `Action::RespondToDialog` calls `agent.respond` but server registers `agent.respondToInput`
- Fix: Change to `agent.respondToInput`

### B3. TUI hardcoded model name
- File: `apps/cli/src/tui/mod.rs`
- Issue: Hardcoded `"claude-3"` as model name on startup
- Fix: After connect, call `models.list` to get first available model name. Fallback to `"unknown"`.

## --json Unification

Add `json: bool` parameter to all commands that currently lack it:

| Command | File | Functions to update |
|---------|------|-------------------|
| `health` | `health.rs` | `run()` |
| `tools` | `tools.rs` | `run()` |
| `session` | `session.rs` | `list()`, `new_session()`, `delete()` |
| `connect` | `connect.rs` | `run()` |
| `daemon` | `daemon.rs` | `status()`, `stop()`, `logs()` (start/restart are local) |
| `config` | `config_cmd.rs` | `get()`, `validate()` |

Update `main.rs` dispatch arms to pass `cli.json`.

## Guests Migration

Remove per-command `--format` parameter from `GuestsAction` variants.
Use `cli.json` passed from main.rs instead.
Update all functions in `guests.rs`.

## Tasks

1. Fix B1 + B2 + B3 (bugs)
2. Add --json to health, tools, session, connect
3. Add --json to daemon, config
4. Migrate guests to global --json
5. Update all main.rs dispatch arms
