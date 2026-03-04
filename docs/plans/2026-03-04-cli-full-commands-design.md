# CLI Full Commands Design

> Date: 2026-03-04
> Status: Approved
> Scope: Core commands + gateway call escape hatch + TUI bug fixes

## Context

CLI infrastructure (TUI, config, daemon, completion) is complete. 100+ Gateway RPC methods lack CLI command coverage. This design covers the core high-frequency commands (~40 RPC methods) plus a generic `gateway call` escape hatch for the rest.

## Architecture

### Global `--json` Flag

Top-level flag on `Cli` struct, inherited by all subcommands:

```rust
#[derive(Parser)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Commands>,
    // ...
}
```

### Output Helper (output.rs)

Shared module for consistent output formatting:

- `print_result(value, json_mode)` — JSON pretty-print or human-readable
- `print_table(headers, rows, json_mode, raw)` — Aligned table or JSON
- `print_error(msg)` — Unified error output to stderr

### Command Module Pattern

Each command group is a separate `commands/<name>.rs` file. All functions accept `json: bool`:

```rust
pub async fn list(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _) = AlephClient::connect(server_url).await?;
    let result: Value = client.call("method.name", None::<()>).await?;
    if json { output::print_result(&result, true); return Ok(()); }
    // Human-readable formatting...
}
```

### Unified Error Messages

Connection failures show consistent hint:

```
Error: Cannot connect to Gateway at ws://127.0.0.1:3578
  Hint: Run 'aleph daemon start' to start the server
```

## New CLI Commands

### 1. providers — AI Provider Management

```
aleph providers list                                    # providers.list
aleph providers get <name>                              # providers.get
aleph providers add <name> --type <type> --api-key <k>  # providers.create
aleph providers test <name>                             # providers.test
aleph providers set-default <name>                      # providers.setDefault
aleph providers remove <name>                           # providers.delete
```

### 2. models — Model Management

```
aleph models list                       # models.list
aleph models get <model-id>             # models.get
aleph models capabilities <model-id>    # models.capabilities
```

### 3. memory — Memory System

```
aleph memory search <query>             # memory.search
aleph memory stats                      # memory.stats
aleph memory clear [--facts-only]       # memory.clear / memory.clearFacts
aleph memory compress                   # memory.compress
aleph memory delete <id>                # memory.delete
```

### 4. plugins — Plugin Lifecycle

```
aleph plugins list                      # plugins.list
aleph plugins install <source>          # plugins.install / installFromZip
aleph plugins uninstall <name>          # plugins.uninstall
aleph plugins enable <name>             # plugins.enable
aleph plugins disable <name>            # plugins.disable
aleph plugins call <plugin> <tool>      # plugins.callTool
```

### 5. skills — Skill Management

```
aleph skills list                       # skills.list + markdown_skills.list
aleph skills install <source>           # skills.install / installFromZip / markdown_skills.install
aleph skills reload <name>              # markdown_skills.reload
aleph skills delete <name>              # skills.delete / markdown_skills.unload
```

### 6. workspace — Workspace Management

```
aleph workspace list                    # workspace.list
aleph workspace create <name>           # workspace.create
aleph workspace switch <name>           # workspace.switch
aleph workspace active                  # workspace.getActive
aleph workspace archive <name>          # workspace.archive
```

### 7. logs — Log Management

```
aleph logs level                        # logs.getLevel
aleph logs set-level <level>            # logs.setLevel
aleph logs dir                          # logs.getDirectory
```

### 8. system info — Enhanced System Information

Replace current partial `aleph info` with full `system.info` RPC data (CPU, memory, disk, version, platform, uptime).

### 9. gateway call — Generic RPC Escape Hatch

```
aleph gateway call <method> [params_json]
```

Examples:
```bash
aleph gateway call health
aleph gateway call config.get '{"section": "general"}'
aleph --json gateway call providers.list
```

Minimal implementation (~30 lines): connect, call, print result. Safety net for all unexposed RPC methods.

## Bug Fixes

### B1. Register `agent.respond` handler

`handle_respond_to_input` exists in `agent.rs` but is never wired in `start/builder/handlers.rs`. TUI dialog responses silently fail.

**Fix:** Register `agent.respond` → `handle_respond_to_input` in `register_agent_handlers`.

### B2. TUI `model.set` → nonexistent RPC

TUI `/model <name>` calls `model.set` (singular) but server only has `models.*` (plural). No `models.set` exists either.

**Fix:** Add `models.set` handler in Gateway that updates the active model config. Wire in handlers.rs.

### B3. TUI `model.list` → nonexistent RPC

TUI `/models` calls `model.list` (singular) but server registers `models.list` (plural).

**Fix:** Register `model.list` as alias for `models.list` in HandlerRegistry.

### B4. TUI `session.create` → nonexistent RPC

TUI `/new` calls `session.create` which doesn't exist.

**Fix:** Add `session.create` handler that creates a new session via SessionManager. Wire in handlers.rs.

## File Structure

```
apps/cli/src/
├── commands/
│   ├── mod.rs              # (modify) add new modules
│   ├── providers.rs        # NEW
│   ├── models_cmd.rs       # NEW
│   ├── memory_cmd.rs       # NEW
│   ├── plugins_cmd.rs      # NEW
│   ├── skills_cmd.rs       # NEW
│   ├── workspace_cmd.rs    # NEW
│   ├── logs_cmd.rs         # NEW
│   ├── gateway_cmd.rs      # NEW
│   └── [existing files]
├── output.rs               # NEW — shared output formatting
└── main.rs                 # (modify) add --json flag, new commands
```

## Non-Goals

- Config sub-domain commands (routing_rules, mcp_config, etc.) — use `gateway call`
- Discord-specific commands — use `gateway call`
- Device/pairing management — use `gateway call`
- Services/cron/channels — use `gateway call` (can be added later)
- POE commands — use `gateway call` (can be added later)

## Dependencies

- All RPC handlers already exist on server side
- No new Gateway handlers needed (except Bug Fixes B2-B4)
- Pure CLI-side work + minimal Gateway wiring for bugs
