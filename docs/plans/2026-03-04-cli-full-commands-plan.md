# CLI Full Commands Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add 9 CLI command groups (~40 RPC methods), a generic `gateway call` escape hatch, global `--json` output, and fix 4 TUI RPC bugs.

**Architecture:** Each command group is a `commands/<name>.rs` file following the existing connect → call → format → close pattern. A shared `output.rs` handles `--json` flag and table formatting. Bug fixes wire missing handlers and correct method names.

**Tech Stack:** Rust, clap (CLI parsing), serde_json, tokio-tungstenite (WebSocket client via AlephClient)

---

## Dependency Graph

```
Task 1 (output.rs + --json) ──┬── Task 3 (gateway call)
                               ├── Task 4 (providers)
                               ├── Task 5 (models)
                               ├── Task 6 (memory)
                               ├── Task 7 (plugins)
                               ├── Task 8 (skills)
                               ├── Task 9 (workspace)
                               ├── Task 10 (logs)
                               └── Task 11 (info enhance)

Task 2 (bug fixes) ── independent
```

Tasks 3-11 are independent of each other (parallelizable). All depend on Task 1.
Task 2 is fully independent.

---

### Task 1: Output Helper + Global --json Flag

**Files:**
- Create: `apps/cli/src/output.rs`
- Modify: `apps/cli/src/main.rs:35-53` (Cli struct), `apps/cli/src/main.rs:209-280` (dispatch)
- Modify: `apps/cli/src/commands/mod.rs` (no change needed for output.rs — it's a sibling module)

**Step 1: Create `output.rs`**

```rust
// apps/cli/src/output.rs
use serde_json::Value;

/// Print a JSON value — pretty JSON in --json mode, pretty JSON in normal mode too
/// (commands that want custom formatting should handle it before calling this)
pub fn print_json(value: &Value) {
    if let Ok(s) = serde_json::to_string_pretty(value) {
        println!("{}", s);
    }
}

/// Print either raw JSON (--json mode) or a formatted table
pub fn print_table(headers: &[&str], rows: &[Vec<String>], json_mode: bool, raw: &Value) {
    if json_mode {
        print_json(raw);
        return;
    }

    if rows.is_empty() {
        println!("(no results)");
        return;
    }

    // Calculate column widths
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }

    // Print header
    let header_line: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| format!("{:<width$}", h, width = widths[i]))
        .collect();
    println!("{}", header_line.join("  "));
    let separator: Vec<String> = widths.iter().map(|&w| "─".repeat(w)).collect();
    println!("{}", separator.join("  "));

    // Print rows
    for row in rows {
        let line: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let w = widths.get(i).copied().unwrap_or(cell.len());
                format!("{:<width$}", cell, width = w)
            })
            .collect();
        println!("{}", line.join("  "));
    }
}

/// Print a key-value detail view
pub fn print_detail(pairs: &[(&str, String)], json_mode: bool, raw: &Value) {
    if json_mode {
        print_json(raw);
        return;
    }
    let max_key = pairs.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (key, value) in pairs {
        println!("{:<width$}  {}", format!("{}:", key), value, width = max_key + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_table_empty_rows() {
        // Should not panic
        let raw = serde_json::json!([]);
        print_table(&["Name"], &[], false, &raw);
    }

    #[test]
    fn print_table_column_widths() {
        let raw = serde_json::json!([]);
        let rows = vec![vec!["short".to_string(), "a very long value".to_string()]];
        // Should not panic
        print_table(&["Col1", "Col2"], &rows, false, &raw);
    }
}
```

**Step 2: Add global `--json` flag to Cli struct**

In `apps/cli/src/main.rs`, modify the `Cli` struct (currently lines 35-53):

```rust
#[derive(Parser)]
#[command(name = "aleph", about = "Aleph AI Assistant CLI")]
pub(crate) struct Cli {
    /// Gateway server URL
    #[arg(short, long, default_value = "ws://127.0.0.1:18789")]
    server: String,

    /// Enable verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Path to config file
    #[arg(short, long)]
    config: Option<String>,

    /// Output raw JSON instead of human-readable format
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}
```

Add `pub mod output;` in `main.rs` or make it a sibling module accessible from commands.

**Step 3: Pass `json` flag through dispatch**

Update the dispatch match block to pass `cli.json` to commands that will use it. Existing commands don't need it yet — only new commands will receive it.

**Step 4: Run tests**

```bash
cargo test -p aleph-cli
```

**Step 5: Commit**

```bash
git add apps/cli/src/output.rs apps/cli/src/main.rs
git commit -m "cli: add output helper and global --json flag"
```

---

### Task 2: Fix TUI RPC Bugs (B1-B4)

**Files:**
- Modify: `apps/cli/src/tui/mod.rs:697` (model.set → models.set)
- Modify: `apps/cli/src/tui/mod.rs:713` (model.list → models.list)
- Modify: `apps/cli/src/tui/mod.rs:652` (session.create)
- Modify: `core/src/bin/aleph/commands/start/builder/handlers.rs` (wire agent.respondToInput)
- Create: `core/src/gateway/handlers/models.rs` — add `handle_set` function
- Modify: `core/src/gateway/handlers/session.rs` — add `handle_create_db` function

**Step 1: Fix B2 — `model.list` → `models.list`**

In `apps/cli/src/tui/mod.rs` line 713, change:
```rust
// FROM:
match client.call::<_, Value>("model.list", None::<()>).await {
// TO:
match client.call::<_, Value>("models.list", None::<()>).await {
```

**Step 2: Fix B1 — `model.set` → `models.set` + create server handler**

In `apps/cli/src/tui/mod.rs` line 697, change:
```rust
// FROM:
.call::<_, Value>("model.set", Some(params))
// TO:
.call::<_, Value>("models.set", Some(params))
```

Add `handle_set` to `core/src/gateway/handlers/models.rs`:
```rust
/// Handle models.set — update active model in config
pub async fn handle_set(request: JsonRpcRequest, config: Arc<Config>) -> JsonRpcResponse {
    let params: serde_json::Value = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let model_name = match params.get("model").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'model' param"),
    };
    // Validate model exists in config providers
    // Return success with model name
    JsonRpcResponse::success(request.id, serde_json::json!({
        "model": model_name,
        "status": "active"
    }))
}
```

Wire in `core/src/bin/aleph/commands/start/builder/handlers.rs`:
```rust
// In register_config_handlers or a new section
register_handler!(server, "models.set", models_handlers::handle_set, config);
```

**Step 3: Fix B3 — `session.create` handler**

In `core/src/gateway/handlers/session.rs`, add:
```rust
/// Handle session.create — create a new session
pub async fn handle_create_db(
    request: JsonRpcRequest,
    manager: Arc<SessionManager>,
) -> JsonRpcResponse {
    let params: serde_json::Value = request.params.clone().unwrap_or(serde_json::json!({}));
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("default");

    // Create session via SessionManager
    let session_key = format!("session_{}", chrono::Utc::now().timestamp_millis());

    JsonRpcResponse::success(request.id, serde_json::json!({
        "session_key": session_key,
        "name": name,
    }))
}
```

Wire `session.create` in handlers.rs alongside `session.usage` and `session.compact`.

**Step 4: Fix B4 — Wire `agent.respondToInput`**

In `core/src/bin/aleph/commands/start/builder/handlers.rs`, in the agent handlers section, add:
```rust
register_handler!(server, "agent.respondToInput", agent_handlers::handle_respond_to_input);
```

The handler already exists at `core/src/gateway/handlers/agent.rs:405`.

**Step 5: Run tests**

```bash
cargo check -p alephcore && cargo check --bin aleph && cargo test -p aleph-cli
```

**Step 6: Commit**

```bash
git commit -m "fix: wire missing RPC handlers and correct TUI method names

- Fix model.set/model.list → models.set/models.list (TUI)
- Add models.set handler for model switching
- Add session.create handler for TUI /new command
- Wire agent.respondToInput handler (existed but unregistered)"
```

---

### Task 3: `aleph gateway call` Command

**Files:**
- Create: `apps/cli/src/commands/gateway_cmd.rs`
- Modify: `apps/cli/src/main.rs` (add Gateway variant + dispatch)
- Modify: `apps/cli/src/commands/mod.rs` (add module)

**Step 1: Create `gateway_cmd.rs`**

```rust
//! Generic Gateway RPC call command

use serde_json::Value;
use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// Call any Gateway RPC method directly
pub async fn call(server_url: &str, method: &str, params_json: Option<&str>, json_mode: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params: Option<Value> = match params_json {
        Some(s) => {
            let v: Value = serde_json::from_str(s).map_err(|e| {
                crate::error::CliError::Other(format!("Invalid JSON params: {}", e).into())
            })?;
            Some(v)
        }
        None => None,
    };

    let result: Value = client.call(method, params).await?;

    if json_mode {
        output::print_json(&result);
    } else {
        // Always pretty-print for gateway call
        output::print_json(&result);
    }

    client.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse_valid_json_params() {
        let json_str = r#"{"section": "general"}"#;
        let v: serde_json::Value = serde_json::from_str(json_str).unwrap();
        assert!(v.is_object());
    }

    #[test]
    fn parse_invalid_json_params() {
        let json_str = "not json";
        assert!(serde_json::from_str::<serde_json::Value>(json_str).is_err());
    }
}
```

**Step 2: Add to main.rs**

In `Commands` enum:
```rust
/// Call any Gateway RPC method directly
Gateway {
    #[command(subcommand)]
    action: GatewayAction,
},
```

Sub-enum:
```rust
#[derive(Subcommand)]
enum GatewayAction {
    /// Call an RPC method
    Call {
        /// RPC method name (e.g., "health", "providers.list")
        method: String,
        /// JSON params (optional)
        params: Option<String>,
    },
}
```

Dispatch:
```rust
Some(Commands::Gateway { action }) => match action {
    GatewayAction::Call { method, params } => {
        commands::gateway_cmd::call(&server_url, &method, params.as_deref(), cli.json).await?
    }
},
```

**Step 3: Add module, test, commit**

```bash
# In commands/mod.rs add: pub mod gateway_cmd;
cargo test -p aleph-cli
git commit -m "cli: add gateway call command for generic RPC access"
```

---

### Task 4: `aleph providers` Command

**Files:**
- Create: `apps/cli/src/commands/providers_cmd.rs`
- Modify: `apps/cli/src/main.rs` (add Providers variant + ProvidersAction enum + dispatch)
- Modify: `apps/cli/src/commands/mod.rs`

**Implementation:**

Sub-commands and their RPC methods:
- `list` → `providers.list` — table output (Name, Type, Default)
- `get <name>` → `providers.get` — detail view
- `add <name> --type <type> --api-key <key> [--base-url <url>]` → `providers.create`
- `test <name>` → `providers.test` — connectivity test
- `set-default <name>` → `providers.setDefault`
- `remove <name>` → `providers.delete` — with confirmation prompt

```rust
#[derive(Subcommand)]
pub enum ProvidersAction {
    List,
    Get { name: String },
    Add {
        name: String,
        #[arg(long, rename_all = "kebab-case")]
        r#type: String,
        #[arg(long)]
        api_key: String,
        #[arg(long)]
        base_url: Option<String>,
    },
    Test { name: String },
    SetDefault { name: String },
    Remove { name: String },
}
```

Each function follows connect → call → format pattern. `list` uses `output::print_table`. Others use `output::print_detail` or simple println.

**Commit:** `cli: add providers command for AI provider management`

---

### Task 5: `aleph models` Command

**Files:**
- Create: `apps/cli/src/commands/models_cmd.rs`
- Modify: `apps/cli/src/main.rs`, `apps/cli/src/commands/mod.rs`

**Sub-commands:**
- `list` → `models.list` — table (ID, Provider, Context Window)
- `get <model-id>` → `models.get` — detail view
- `capabilities <model-id>` → `models.capabilities` — capability detail

```rust
#[derive(Subcommand)]
pub enum ModelsAction {
    List,
    Get { model_id: String },
    Capabilities { model_id: String },
}
```

**Commit:** `cli: add models command for model listing and capabilities`

---

### Task 6: `aleph memory` Command

**Files:**
- Create: `apps/cli/src/commands/memory_cmd.rs`
- Modify: `apps/cli/src/main.rs`, `apps/cli/src/commands/mod.rs`

**Sub-commands:**
- `search <query> [--limit N]` → `memory.search` — results table
- `stats` → `memory.stats` — detail view
- `clear [--facts-only]` → `memory.clear` or `memory.clearFacts`
- `compress` → `memory.compress`
- `delete <id>` → `memory.delete`

```rust
#[derive(Subcommand)]
pub enum MemoryAction {
    Search {
        query: String,
        #[arg(long, default_value = "10")]
        limit: usize,
    },
    Stats,
    Clear {
        #[arg(long)]
        facts_only: bool,
    },
    Compress,
    Delete { id: String },
}
```

**Commit:** `cli: add memory command for search, stats, and management`

---

### Task 7: `aleph plugins` Command

**Files:**
- Create: `apps/cli/src/commands/plugins_cmd.rs`
- Modify: `apps/cli/src/main.rs`, `apps/cli/src/commands/mod.rs`

**Sub-commands:**
- `list` → `plugins.list` — table (Name, Version, Status, Type)
- `install <source>` → `plugins.install` or `plugins.installFromZip` (detect .zip extension)
- `uninstall <name>` → `plugins.uninstall`
- `enable <name>` → `plugins.enable`
- `disable <name>` → `plugins.disable`
- `call <plugin> <tool> [params_json]` → `plugins.callTool`

```rust
#[derive(Subcommand)]
pub enum PluginsAction {
    List,
    Install { source: String },
    Uninstall { name: String },
    Enable { name: String },
    Disable { name: String },
    Call {
        plugin: String,
        tool: String,
        params: Option<String>,
    },
}
```

**Commit:** `cli: add plugins command for plugin lifecycle management`

---

### Task 8: `aleph skills` Command

**Files:**
- Create: `apps/cli/src/commands/skills_cmd.rs`
- Modify: `apps/cli/src/main.rs`, `apps/cli/src/commands/mod.rs`

**Sub-commands:**
- `list` → calls both `skills.list` + `markdown_skills.list`, merges results — table (Name, Type, Description)
- `install <source>` → `skills.install` (or `markdown_skills.install` if .md source)
- `reload <name>` → `markdown_skills.reload`
- `delete <name>` → `skills.delete` (or `markdown_skills.unload`)

```rust
#[derive(Subcommand)]
pub enum SkillsAction {
    List,
    Install { source: String },
    Reload { name: String },
    Delete { name: String },
}
```

**Commit:** `cli: add skills command for skill management`

---

### Task 9: `aleph workspace` Command

**Files:**
- Create: `apps/cli/src/commands/workspace_cmd.rs`
- Modify: `apps/cli/src/main.rs`, `apps/cli/src/commands/mod.rs`

**Sub-commands:**
- `list` → `workspace.list` — table (Name, Status, Created)
- `create <name> [--description <desc>]` → `workspace.create`
- `switch <name>` → `workspace.switch`
- `active` → `workspace.getActive` — detail view
- `archive <name>` → `workspace.archive`

```rust
#[derive(Subcommand)]
pub enum WorkspaceAction {
    List,
    Create {
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
    Switch { name: String },
    Active,
    Archive { name: String },
}
```

**Commit:** `cli: add workspace command for workspace management`

---

### Task 10: `aleph logs` Command

**Files:**
- Create: `apps/cli/src/commands/logs_cmd.rs`
- Modify: `apps/cli/src/main.rs`, `apps/cli/src/commands/mod.rs`

**Sub-commands:**
- `level` → `logs.getLevel` — prints current level
- `set-level <level>` → `logs.setLevel` — changes level
- `dir` → `logs.getDirectory` — prints log directory path

```rust
#[derive(Subcommand)]
pub enum LogsAction {
    Level,
    SetLevel { level: String },
    Dir,
}
```

Small module, ~50 lines. All three functions are simple single-value outputs.

**Commit:** `cli: add logs command for log level and directory management`

---

### Task 11: Enhance `aleph info`

**Files:**
- Modify: `apps/cli/src/commands/info.rs`

**Current state:** `info.rs` calls `health` and `providers.list` only.

**Enhancement:** Also call `system.info` to display CPU, memory, disk, version, platform, uptime. Use `output::print_detail` for consistent formatting.

```rust
// Add to existing run() function:
let sys_info: Value = client.call("system.info", None::<()>).await?;

println!("\nSystem Information");
println!("──────────────────");
if let Some(cpu) = sys_info.get("cpu_usage") { println!("CPU:         {}%", cpu); }
if let Some(mem) = sys_info.get("memory_usage") { println!("Memory:      {}", mem); }
if let Some(disk) = sys_info.get("disk_usage") { println!("Disk:        {}", disk); }
// ... etc
```

Also add `json` parameter support to `run()`.

**Commit:** `cli: enhance info command with system metrics`

---

## Summary

| Task | Command | RPC Methods | Est. Lines | Parallelizable |
|------|---------|-------------|------------|----------------|
| 1 | output.rs + --json | — | ~100 | Foundation |
| 2 | Bug fixes (B1-B4) | 3 new handlers | ~120 | Independent |
| 3 | gateway call | any | ~50 | After Task 1 |
| 4 | providers | 6 | ~150 | After Task 1 |
| 5 | models | 3 | ~80 | After Task 1 |
| 6 | memory | 5 | ~120 | After Task 1 |
| 7 | plugins | 6 | ~130 | After Task 1 |
| 8 | skills | 4 | ~100 | After Task 1 |
| 9 | workspace | 5 | ~100 | After Task 1 |
| 10 | logs | 3 | ~50 | After Task 1 |
| 11 | info enhance | 1 | ~40 | After Task 1 |

**Total: ~1040 new lines, 11 tasks, 36+ RPC methods covered**
