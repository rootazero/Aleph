# CLI Full RPC Coverage Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Cover remaining 29 Gateway RPC methods with 7 new CLI command groups + session extensions + identity handler wiring.

**Architecture:** Pure I/O CLI commands following established R4 pattern (connect → call → format → close). All commands accept global `--json` flag. One Gateway-side change: wire identity handlers with SharedIdentityResolver.

**Tech Stack:** Rust, clap (CLI), serde_json, AlephClient (WebSocket JSON-RPC), rpassword (vault)

---

### Task 1: Wire identity handlers in Gateway

**Files:**
- Modify: `core/src/bin/aleph/commands/start/builder/handlers.rs`
- Modify: `core/src/bin/aleph/commands/start/mod.rs`

**Context:** Identity handlers exist in `core/src/gateway/handlers/identity.rs` with 4 functions: `handle_get`, `handle_set`, `handle_clear`, `handle_list`. All take `SharedIdentityResolver = Arc<RwLock<IdentityResolver>>`. Placeholders are registered in `HandlerRegistry::new()`. We need to create the real resolver at startup and overwrite the placeholders.

**Step 1: Add `register_identity_handlers` in `handlers.rs`**

At the top of `handlers.rs`, add the import:
```rust
use alephcore::gateway::handlers::identity as identity_handlers;
use alephcore::gateway::handlers::identity::SharedIdentityResolver;
```

After `register_workspace_handlers`, add:
```rust
pub(in crate::commands::start) fn register_identity_handlers(
    server: &mut GatewayServer,
    resolver: &SharedIdentityResolver,
) {
    register_handler!(server, "identity.get", identity_handlers::handle_get, resolver);
    register_handler!(server, "identity.set", identity_handlers::handle_set, resolver);
    register_handler!(server, "identity.clear", identity_handlers::handle_clear, resolver);
    register_handler!(server, "identity.list", identity_handlers::handle_list, resolver);
}
```

**Step 2: Create resolver and call registration in `start/mod.rs`**

After `register_workspace_handlers` call (~line 1150), add:
```rust
// Identity resolver (shared for session-level overrides)
let identity_resolver: SharedIdentityResolver = Arc::new(
    tokio::sync::RwLock::new(
        alephcore::thinker::identity::IdentityResolver::with_defaults()
    )
);
register_identity_handlers(&mut server, &identity_resolver);
```

Add the import at the top of the file (with existing use statements):
```rust
use alephcore::gateway::handlers::identity::SharedIdentityResolver;
```

And import the function from handlers:
```rust
use super::builder::handlers::register_identity_handlers;
```

**Step 3: Build and test**

Run: `cargo check -p alephcore && cargo check -p aleph`
Expected: No compilation errors.

**Step 4: Commit**
```bash
git add core/src/bin/aleph/commands/start/builder/handlers.rs core/src/bin/aleph/commands/start/mod.rs
git commit -m "gateway: wire identity handlers with SharedIdentityResolver"
```

---

### Task 2: Add chat command

**Files:**
- Create: `apps/cli/src/commands/chat_cmd.rs`
- Modify: `apps/cli/src/commands/mod.rs`
- Modify: `apps/cli/src/main.rs`

**Context:** The existing `aleph chat` (interactive TUI) and `aleph ask` (one-shot) remain unchanged. The new `ChatControl` subcommands provide direct RPC access to `chat.send/abort/history/clear`. These require authentication (session-aware).

RPC signatures:
- `chat.send`: params `{message, session_key?, stream?, thinking?}` → `{run_id, session_key, streaming}`
- `chat.abort`: params `{run_id}` → `{run_id, aborted}`
- `chat.history`: params `{session_key, limit?, before?}` → `{session_key, messages[], count}`
- `chat.clear`: params `{session_key, keep_system?}` → `{session_key, cleared}`

**Important design note:** Since `Commands::Chat` already exists as the interactive chat variant, add a NEW variant `ChatControl` with subcommand name alias `chat-control` to avoid conflict. CLI usage: `aleph chat-control send/abort/history/clear`.

Actually, a better approach: rename the existing `Chat` variant's CLI name to be just the default (when no subcommand) and add the new subcommands as a separate `ChatControl` command group. This avoids breaking the existing `aleph chat` command.

**Step 1: Create `chat_cmd.rs`**

```rust
//! Chat control commands (send, abort, history, clear)

use serde_json::Value;

use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;
use crate::output;

/// Send a message via RPC (non-interactive)
pub async fn send(
    server_url: &str,
    message: &str,
    session: Option<&str>,
    stream: bool,
    thinking: Option<&str>,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let mut params = serde_json::json!({ "message": message });
    if let Some(s) = session {
        params["session_key"] = serde_json::Value::String(s.to_string());
    }
    if stream {
        params["stream"] = serde_json::Value::Bool(true);
    }
    if let Some(t) = thinking {
        params["thinking"] = serde_json::Value::String(t.to_string());
    }

    let result: Value = client.call("chat.send", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let run_id = result.get("run_id").and_then(|v| v.as_str()).unwrap_or("-");
        let session_key = result.get("session_key").and_then(|v| v.as_str()).unwrap_or("-");
        println!("Message sent.");
        println!("  Run ID:  {}", run_id);
        println!("  Session: {}", session_key);
    }

    client.close().await?;
    Ok(())
}

/// Abort a running chat
pub async fn abort(server_url: &str, run_id: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let params = serde_json::json!({ "run_id": run_id });
    let result: Value = client.call("chat.abort", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let aborted = result.get("aborted").and_then(|v| v.as_bool()).unwrap_or(false);
        if aborted {
            println!("Run '{}' aborted.", run_id);
        } else {
            println!("Run '{}' was not running or already completed.", run_id);
        }
    }

    client.close().await?;
    Ok(())
}

/// Show chat history for a session
pub async fn history(
    server_url: &str,
    session_key: &str,
    limit: Option<usize>,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let mut params = serde_json::json!({ "session_key": session_key });
    if let Some(l) = limit {
        params["limit"] = serde_json::Value::Number(serde_json::Number::from(l));
    }

    let result: Value = client.call("chat.history", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let count = result.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
        println!("=== Chat History ({}) ===", session_key);
        println!();
        if let Some(messages) = result.get("messages").and_then(|v| v.as_array()) {
            for msg in messages {
                let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("?");
                let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let truncated = if content.len() > 200 {
                    format!("{}...", &content.chars().take(200).collect::<String>())
                } else {
                    content.to_string()
                };
                println!("[{}] {}", role, truncated);
            }
        }
        println!();
        println!("Total: {} messages", count);
    }

    client.close().await?;
    Ok(())
}

/// Clear chat history for a session
pub async fn clear(
    server_url: &str,
    session_key: &str,
    keep_system: bool,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let mut params = serde_json::json!({ "session_key": session_key });
    if keep_system {
        params["keep_system"] = serde_json::Value::Bool(true);
    }

    let result: Value = client.call("chat.clear", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let cleared = result.get("cleared").and_then(|v| v.as_bool()).unwrap_or(false);
        if cleared {
            println!("Chat history cleared for session '{}'.", session_key);
        } else {
            println!("No history to clear for session '{}'.", session_key);
        }
    }

    client.close().await?;
    Ok(())
}
```

**Step 2: Add module to `mod.rs`**

Add to `apps/cli/src/commands/mod.rs`:
```rust
pub mod chat_cmd;
```

**Step 3: Add enum and dispatch to `main.rs`**

Add the `ChatControlAction` enum after existing enums:
```rust
#[derive(Subcommand)]
enum ChatControlAction {
    /// Send a message (non-interactive)
    Send {
        /// The message to send
        message: String,
        /// Session key
        #[arg(short, long)]
        session: Option<String>,
        /// Enable streaming
        #[arg(long)]
        stream: bool,
        /// Thinking level (none, concise, verbose)
        #[arg(long)]
        thinking: Option<String>,
    },
    /// Abort a running chat
    Abort {
        /// Run ID to abort
        run_id: String,
    },
    /// Show chat history
    History {
        /// Session key
        session_key: String,
        /// Maximum messages to return
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Clear chat history
    Clear {
        /// Session key
        session_key: String,
        /// Keep system messages
        #[arg(long)]
        keep_system: bool,
    },
}
```

Add variant to `Commands` enum:
```rust
    /// Chat control (send, abort, history, clear)
    #[command(name = "chat-control")]
    ChatControl {
        #[command(subcommand)]
        action: ChatControlAction,
    },
```

Add dispatch in `main()`:
```rust
        Some(Commands::ChatControl { action }) => match action {
            ChatControlAction::Send { message, session, stream, thinking } => {
                commands::chat_cmd::send(
                    &server_url, &message, session.as_deref(), stream,
                    thinking.as_deref(), &config, cli.json,
                ).await?;
            }
            ChatControlAction::Abort { run_id } => {
                commands::chat_cmd::abort(&server_url, &run_id, &config, cli.json).await?;
            }
            ChatControlAction::History { session_key, limit } => {
                commands::chat_cmd::history(&server_url, &session_key, limit, &config, cli.json).await?;
            }
            ChatControlAction::Clear { session_key, keep_system } => {
                commands::chat_cmd::clear(&server_url, &session_key, keep_system, &config, cli.json).await?;
            }
        },
```

**Step 4: Build and test**

Run: `cargo test -p aleph-cli`
Expected: All existing tests pass, no compilation errors.

**Step 5: Commit**
```bash
git add apps/cli/src/commands/chat_cmd.rs apps/cli/src/commands/mod.rs apps/cli/src/main.rs
git commit -m "cli: add chat-control command for send, abort, history, and clear"
```

---

### Task 3: Extend session with usage + compact

**Files:**
- Modify: `apps/cli/src/commands/session.rs`
- Modify: `apps/cli/src/main.rs`

**Context:** `session.rs` already has `list`, `create`, `delete`. Add `usage` and `compact`. Both require authentication. `SessionAction` enum is in `main.rs`.

RPC signatures:
- `session.usage`: params `{session_key}` → `{session_key, tokens, input_tokens, output_tokens, messages, created_at?, last_active_at?}`
- `session.compact`: params `{session_key}` → `{message, before_messages, after_messages, tokens_saved}`

**Step 1: Add `usage` and `compact` to `session.rs`**

Append after the `delete` function:

```rust
/// Show session usage statistics
pub async fn usage(server_url: &str, key: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let params = serde_json::json!({ "session_key": key });
    let result: serde_json::Value = client.call("session.usage", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let pairs = vec![
            ("Session", result.get("session_key").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
            ("Total Tokens", result.get("tokens").and_then(|v| v.as_u64()).map(|n| n.to_string()).unwrap_or("-".to_string())),
            ("Input Tokens", result.get("input_tokens").and_then(|v| v.as_u64()).map(|n| n.to_string()).unwrap_or("-".to_string())),
            ("Output Tokens", result.get("output_tokens").and_then(|v| v.as_u64()).map(|n| n.to_string()).unwrap_or("-".to_string())),
            ("Messages", result.get("messages").and_then(|v| v.as_u64()).map(|n| n.to_string()).unwrap_or("-".to_string())),
            ("Created", result.get("created_at").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
            ("Last Active", result.get("last_active_at").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
        ];
        output::print_detail(&pairs, false, &result);
    }

    client.close().await?;
    Ok(())
}

/// Compact a session (compress history)
pub async fn compact(server_url: &str, key: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let params = serde_json::json!({ "session_key": key });
    let result: serde_json::Value = client.call("session.compact", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let msg = result.get("message").and_then(|v| v.as_str()).unwrap_or("Compacted.");
        let before = result.get("before_messages").and_then(|v| v.as_u64()).unwrap_or(0);
        let after = result.get("after_messages").and_then(|v| v.as_u64()).unwrap_or(0);
        let saved = result.get("tokens_saved").and_then(|v| v.as_u64()).unwrap_or(0);
        println!("{}", msg);
        println!("  Before: {} messages", before);
        println!("  After:  {} messages", after);
        println!("  Tokens saved: {}", saved);
    }

    client.close().await?;
    Ok(())
}
```

**Step 2: Add variants to `SessionAction` in `main.rs`**

```rust
    /// Show session usage statistics
    Usage {
        /// Session key
        key: String,
    },
    /// Compact session (compress history)
    Compact {
        /// Session key
        key: String,
    },
```

**Step 3: Add dispatch arms in `main.rs`**

Add inside the `Some(Commands::Session { action }) => match action {` block:
```rust
            SessionAction::Usage { key } => {
                commands::session::usage(&server_url, &key, &config, cli.json).await?;
            }
            SessionAction::Compact { key } => {
                commands::session::compact(&server_url, &key, &config, cli.json).await?;
            }
```

**Step 4: Build and test**

Run: `cargo test -p aleph-cli`
Expected: All tests pass.

**Step 5: Commit**
```bash
git add apps/cli/src/commands/session.rs apps/cli/src/main.rs
git commit -m "cli: add session usage and compact subcommands"
```

---

### Task 4: Add POE command

**Files:**
- Create: `apps/cli/src/commands/poe_cmd.rs`
- Modify: `apps/cli/src/commands/mod.rs`
- Modify: `apps/cli/src/main.rs`

**Context:** POE is the Principle-Operation-Evaluation execution engine. 8 RPC methods split into two groups: execution (run/status/cancel/list) and contract signing (prepare/sign/reject/pending). All are already wired in `register_poe_handlers`.

RPC signatures:
- `poe.run`: params `{instruction, manifest?, stream?, config?}` → `{task_id, session_key}`
- `poe.status`: params `{task_id}` → full task status object
- `poe.cancel`: params `{task_id}` → `{cancelled, reason?}`
- `poe.list`: no params → `{tasks[], count}`
- `poe.prepare`: params `{instruction}` → `{contract_id, manifest}`
- `poe.sign`: params `{contract_id}` → `{task_id, session_key}`
- `poe.reject`: params `{contract_id}` → `{success}`
- `poe.pending`: no params → `{pending_contracts[]}`

**Step 1: Create `poe_cmd.rs`**

```rust
//! POE (Principle-Operation-Evaluation) commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// Run a POE task
pub async fn run(
    server_url: &str,
    instruction: &str,
    manifest: Option<&str>,
    stream: bool,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let mut params = serde_json::json!({ "instruction": instruction });
    if let Some(m) = manifest {
        let manifest_value: Value = serde_json::from_str(m)
            .map_err(|e| crate::error::CliError::Other(format!("Invalid manifest JSON: {}", e)))?;
        params["manifest"] = manifest_value;
    }
    if stream {
        params["stream"] = Value::Bool(true);
    }

    let result: Value = client.call("poe.run", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let task_id = result.get("task_id").and_then(|v| v.as_str()).unwrap_or("-");
        let session_key = result.get("session_key").and_then(|v| v.as_str()).unwrap_or("-");
        println!("POE task started.");
        println!("  Task ID: {}", task_id);
        println!("  Session: {}", session_key);
    }

    client.close().await?;
    Ok(())
}

/// Get POE task status
pub async fn status(server_url: &str, task_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "task_id": task_id });
    let result: Value = client.call("poe.status", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let status = result.get("status").and_then(|v| v.as_str()).unwrap_or("-");
        let pairs = vec![
            ("Task ID", result.get("task_id").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
            ("Status", status.to_string()),
            ("Session", result.get("session_key").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
            ("Elapsed", result.get("elapsed_ms").and_then(|v| v.as_u64()).map(|ms| format!("{}ms", ms)).unwrap_or("-".to_string())),
        ];
        output::print_detail(&pairs, false, &result);
    }

    client.close().await?;
    Ok(())
}

/// Cancel a POE task
pub async fn cancel(server_url: &str, task_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "task_id": task_id });
    let result: Value = client.call("poe.cancel", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let cancelled = result.get("cancelled").and_then(|v| v.as_bool()).unwrap_or(false);
        if cancelled {
            println!("Task '{}' cancelled.", task_id);
        } else {
            let reason = result.get("reason").and_then(|v| v.as_str()).unwrap_or("unknown");
            println!("Could not cancel task '{}': {}", task_id, reason);
        }
    }

    client.close().await?;
    Ok(())
}

/// List all POE tasks
pub async fn list(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("poe.list", None::<()>).await?;

    let mut rows = Vec::new();
    if let Some(tasks) = result.get("tasks").and_then(|v| v.as_array()) {
        for t in tasks {
            rows.push(vec![
                t.get("task_id").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
                t.get("status").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
                t.get("session_key").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
                t.get("elapsed_ms").and_then(|v| v.as_u64()).map(|ms| format!("{}ms", ms)).unwrap_or("-".to_string()),
            ]);
        }
    }

    output::print_table(&["Task ID", "Status", "Session", "Elapsed"], &rows, json, &result);

    client.close().await?;
    Ok(())
}

/// Prepare a POE contract
pub async fn prepare(server_url: &str, instruction: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "instruction": instruction });
    let result: Value = client.call("poe.prepare", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let contract_id = result.get("contract_id").and_then(|v| v.as_str()).unwrap_or("-");
        println!("Contract prepared: {}", contract_id);
        if let Some(manifest) = result.get("manifest") {
            println!("  Manifest: {}", serde_json::to_string_pretty(manifest).unwrap_or_default());
        }
    }

    client.close().await?;
    Ok(())
}

/// Sign (approve) a POE contract
pub async fn sign(server_url: &str, contract_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "contract_id": contract_id });
    let result: Value = client.call("poe.sign", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let task_id = result.get("task_id").and_then(|v| v.as_str()).unwrap_or("-");
        println!("Contract signed. Task started: {}", task_id);
    }

    client.close().await?;
    Ok(())
}

/// Reject a POE contract
pub async fn reject(server_url: &str, contract_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "contract_id": contract_id });
    let result: Value = client.call("poe.reject", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Contract '{}' rejected.", contract_id);
    }

    client.close().await?;
    Ok(())
}

/// List pending POE contracts
pub async fn pending(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("poe.pending", None::<()>).await?;

    let mut rows = Vec::new();
    if let Some(contracts) = result.get("pending_contracts").and_then(|v| v.as_array()) {
        for c in contracts {
            let instruction = c.get("instruction").and_then(|v| v.as_str()).unwrap_or("-");
            let truncated = if instruction.chars().count() > 60 {
                format!("{}...", instruction.chars().take(60).collect::<String>())
            } else {
                instruction.to_string()
            };
            rows.push(vec![
                c.get("contract_id").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
                truncated,
            ]);
        }
    }

    output::print_table(&["Contract ID", "Instruction"], &rows, json, &result);

    client.close().await?;
    Ok(())
}
```

**Step 2: Add module to `mod.rs`**

```rust
pub mod poe_cmd;
```

**Step 3: Add enum and dispatch to `main.rs`**

Add `PoeAction` enum:
```rust
#[derive(Subcommand)]
enum PoeAction {
    /// Run a POE task
    Run {
        /// Task instruction
        instruction: String,
        /// Success manifest (JSON)
        #[arg(long)]
        manifest: Option<String>,
        /// Enable streaming
        #[arg(long)]
        stream: bool,
    },
    /// Get POE task status
    Status {
        /// Task ID
        task_id: String,
    },
    /// Cancel a running POE task
    Cancel {
        /// Task ID
        task_id: String,
    },
    /// List all POE tasks
    List,
    /// Prepare a POE contract
    Prepare {
        /// Task instruction
        instruction: String,
    },
    /// Sign (approve) a pending contract
    Sign {
        /// Contract ID
        contract_id: String,
    },
    /// Reject a pending contract
    Reject {
        /// Contract ID
        contract_id: String,
    },
    /// List pending contracts
    Pending,
}
```

Add `Poe` variant to `Commands`:
```rust
    /// POE (Principle-Operation-Evaluation) execution engine
    Poe {
        #[command(subcommand)]
        action: PoeAction,
    },
```

Add dispatch:
```rust
        Some(Commands::Poe { action }) => match action {
            PoeAction::Run { instruction, manifest, stream } => {
                commands::poe_cmd::run(&server_url, &instruction, manifest.as_deref(), stream, cli.json).await?;
            }
            PoeAction::Status { task_id } => {
                commands::poe_cmd::status(&server_url, &task_id, cli.json).await?;
            }
            PoeAction::Cancel { task_id } => {
                commands::poe_cmd::cancel(&server_url, &task_id, cli.json).await?;
            }
            PoeAction::List => {
                commands::poe_cmd::list(&server_url, cli.json).await?;
            }
            PoeAction::Prepare { instruction } => {
                commands::poe_cmd::prepare(&server_url, &instruction, cli.json).await?;
            }
            PoeAction::Sign { contract_id } => {
                commands::poe_cmd::sign(&server_url, &contract_id, cli.json).await?;
            }
            PoeAction::Reject { contract_id } => {
                commands::poe_cmd::reject(&server_url, &contract_id, cli.json).await?;
            }
            PoeAction::Pending => {
                commands::poe_cmd::pending(&server_url, cli.json).await?;
            }
        },
```

**Step 4: Build and test**

Run: `cargo test -p aleph-cli`

**Step 5: Commit**
```bash
git add apps/cli/src/commands/poe_cmd.rs apps/cli/src/commands/mod.rs apps/cli/src/main.rs
git commit -m "cli: add poe command for POE execution engine management"
```

---

### Task 5: Add services command

**Files:**
- Create: `apps/cli/src/commands/services_cmd.rs`
- Modify: `apps/cli/src/commands/mod.rs`
- Modify: `apps/cli/src/main.rs`

**Context:** `services.*` RPC methods are already registered as stateless handlers. They manage background plugin services.

RPC signatures:
- `services.list`: params `{plugin_id?, state?}` → `{services[], total, running}`
- `services.status`: params `{plugin_id, service_id}` → `{service: ServiceInfo}`
- `services.start`: params `{plugin_id, service_id}` → `{service: ServiceInfo}`
- `services.stop`: params `{plugin_id, service_id}` → `{service: ServiceInfo}`

ServiceInfo: `{id, plugin_id, name, state, started_at?, error?}`

**Step 1: Create `services_cmd.rs`**

```rust
//! Background service management commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// List background services
pub async fn list(
    server_url: &str,
    plugin: Option<&str>,
    state: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let mut params = serde_json::json!({});
    if let Some(p) = plugin {
        params["plugin_id"] = Value::String(p.to_string());
    }
    if let Some(s) = state {
        params["state"] = Value::String(s.to_string());
    }

    let result: Value = client.call("services.list", Some(params)).await?;

    let mut rows = Vec::new();
    if let Some(services) = result.get("services").and_then(|v| v.as_array()) {
        for s in services {
            rows.push(vec![
                s.get("id").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
                s.get("plugin_id").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
                s.get("name").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
                s.get("state").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
            ]);
        }
    }

    output::print_table(&["ID", "Plugin", "Name", "State"], &rows, json, &result);

    client.close().await?;
    Ok(())
}

/// Get service status
pub async fn status(server_url: &str, plugin_id: &str, service_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "plugin_id": plugin_id, "service_id": service_id });
    let result: Value = client.call("services.status", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else if let Some(svc) = result.get("service") {
        let pairs = vec![
            ("ID", svc.get("id").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
            ("Plugin", svc.get("plugin_id").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
            ("Name", svc.get("name").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
            ("State", svc.get("state").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
            ("Started", svc.get("started_at").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
            ("Error", svc.get("error").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
        ];
        output::print_detail(&pairs, false, &result);
    } else {
        println!("Service not found.");
    }

    client.close().await?;
    Ok(())
}

/// Start a service
pub async fn start(server_url: &str, plugin_id: &str, service_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "plugin_id": plugin_id, "service_id": service_id });
    let result: Value = client.call("services.start", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Service '{}' started.", service_id);
    }

    client.close().await?;
    Ok(())
}

/// Stop a service
pub async fn stop(server_url: &str, plugin_id: &str, service_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "plugin_id": plugin_id, "service_id": service_id });
    let result: Value = client.call("services.stop", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Service '{}' stopped.", service_id);
    }

    client.close().await?;
    Ok(())
}
```

**Step 2: Add module + enum + dispatch to mod.rs and main.rs**

Add to `mod.rs`:
```rust
pub mod services_cmd;
```

Add `ServicesAction` enum to `main.rs`:
```rust
#[derive(Subcommand)]
enum ServicesAction {
    /// List background services
    List {
        /// Filter by plugin ID
        #[arg(long)]
        plugin: Option<String>,
        /// Filter by state (running, stopped, error)
        #[arg(long)]
        state: Option<String>,
    },
    /// Get service status
    Status {
        /// Plugin ID
        plugin_id: String,
        /// Service ID
        service_id: String,
    },
    /// Start a service
    Start {
        /// Plugin ID
        plugin_id: String,
        /// Service ID
        service_id: String,
    },
    /// Stop a service
    Stop {
        /// Plugin ID
        plugin_id: String,
        /// Service ID
        service_id: String,
    },
}
```

Add `Services` variant to `Commands`:
```rust
    /// Background service management
    Services {
        #[command(subcommand)]
        action: ServicesAction,
    },
```

Add dispatch:
```rust
        Some(Commands::Services { action }) => match action {
            ServicesAction::List { plugin, state } => {
                commands::services_cmd::list(&server_url, plugin.as_deref(), state.as_deref(), cli.json).await?;
            }
            ServicesAction::Status { plugin_id, service_id } => {
                commands::services_cmd::status(&server_url, &plugin_id, &service_id, cli.json).await?;
            }
            ServicesAction::Start { plugin_id, service_id } => {
                commands::services_cmd::start(&server_url, &plugin_id, &service_id, cli.json).await?;
            }
            ServicesAction::Stop { plugin_id, service_id } => {
                commands::services_cmd::stop(&server_url, &plugin_id, &service_id, cli.json).await?;
            }
        },
```

**Step 3: Build and test**

Run: `cargo test -p aleph-cli`

**Step 4: Commit**
```bash
git add apps/cli/src/commands/services_cmd.rs apps/cli/src/commands/mod.rs apps/cli/src/main.rs
git commit -m "cli: add services command for background service management"
```

---

### Task 6: Add identity command

**Files:**
- Create: `apps/cli/src/commands/identity_cmd.rs`
- Modify: `apps/cli/src/commands/mod.rs`
- Modify: `apps/cli/src/main.rs`

**Context:** Identity handlers were wired in Task 1. `identity.get` returns `{soul, has_session_override}`. `identity.set` takes `{soul: SoulManifest}` as JSON. `identity.list` returns `{sources[]}`.

**Step 1: Create `identity_cmd.rs`**

```rust
//! Identity/soul management commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// Get current identity/soul
pub async fn get(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("identity.get", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let has_override = result.get("has_session_override")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        println!("=== Current Identity ===");
        println!();

        if let Some(soul) = result.get("soul") {
            let identity = soul.get("identity").and_then(|v| v.as_str()).unwrap_or("(empty)");
            println!("Identity: {}", identity);

            if let Some(directives) = soul.get("directives").and_then(|v| v.as_array()) {
                if !directives.is_empty() {
                    println!("Directives:");
                    for d in directives {
                        if let Some(s) = d.as_str() {
                            println!("  - {}", s);
                        }
                    }
                }
            }
        }

        println!();
        println!("Session override: {}", if has_override { "active" } else { "none" });
    }

    client.close().await?;
    Ok(())
}

/// Set identity via JSON soul manifest
pub async fn set(server_url: &str, manifest_json: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let soul: Value = serde_json::from_str(manifest_json)
        .map_err(|e| crate::error::CliError::Other(format!("Invalid soul manifest JSON: {}", e)))?;
    let params = serde_json::json!({ "soul": soul });
    let result: Value = client.call("identity.set", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Identity updated.");
    }

    client.close().await?;
    Ok(())
}

/// Clear session identity override
pub async fn clear(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("identity.clear", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let had = result.get("had_override").and_then(|v| v.as_bool()).unwrap_or(false);
        if had {
            println!("Session identity override cleared.");
        } else {
            println!("No session override was active.");
        }
    }

    client.close().await?;
    Ok(())
}

/// List identity sources
pub async fn list(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("identity.list", None::<()>).await?;

    let mut rows = Vec::new();
    if let Some(sources) = result.get("sources").and_then(|v| v.as_array()) {
        for s in sources {
            rows.push(vec![
                s.get("source_type").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
                s.get("path").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
                s.get("loaded").and_then(|v| v.as_bool()).map(|b| if b { "yes" } else { "no" }).unwrap_or("-").to_string(),
            ]);
        }
    }

    output::print_table(&["Type", "Path", "Loaded"], &rows, json, &result);

    client.close().await?;
    Ok(())
}
```

**Step 2: Add module + enum + dispatch**

Add to `mod.rs`:
```rust
pub mod identity_cmd;
```

Add `IdentityAction` enum to `main.rs`:
```rust
#[derive(Subcommand)]
enum IdentityAction {
    /// Get current identity/soul
    Get,
    /// Set identity from JSON manifest
    Set {
        /// Soul manifest as JSON string
        manifest: String,
    },
    /// Clear session identity override
    Clear,
    /// List identity sources
    List,
}
```

Add `Identity` to `Commands`:
```rust
    /// Identity/soul management
    Identity {
        #[command(subcommand)]
        action: IdentityAction,
    },
```

Add dispatch:
```rust
        Some(Commands::Identity { action }) => match action {
            IdentityAction::Get => {
                commands::identity_cmd::get(&server_url, cli.json).await?;
            }
            IdentityAction::Set { manifest } => {
                commands::identity_cmd::set(&server_url, &manifest, cli.json).await?;
            }
            IdentityAction::Clear => {
                commands::identity_cmd::clear(&server_url, cli.json).await?;
            }
            IdentityAction::List => {
                commands::identity_cmd::list(&server_url, cli.json).await?;
            }
        },
```

**Step 3: Build and test**

Run: `cargo test -p aleph-cli`

**Step 4: Commit**
```bash
git add apps/cli/src/commands/identity_cmd.rs apps/cli/src/commands/mod.rs apps/cli/src/main.rs
git commit -m "cli: add identity command for soul/identity management"
```

---

### Task 7: Add vault command

**Files:**
- Create: `apps/cli/src/commands/vault_cmd.rs`
- Modify: `apps/cli/src/commands/mod.rs`
- Modify: `apps/cli/src/main.rs`
- Modify: `apps/cli/Cargo.toml` (add rpassword dependency)

**Context:** `vault.*` RPC methods are already registered as stateless. Security: `vault store` must NOT accept master key as a CLI argument. Use `rpassword` for interactive input.

RPC signatures:
- `vault.status`: no params → vault status object
- `vault.storeKey`: params `{master_key}` → `{success}`
- `vault.deleteKey`: no params → `{success, was_present}`
- `vault.verify`: no params → `{verified, message}`

**Step 1: Add rpassword to CLI Cargo.toml**

In `apps/cli/Cargo.toml`, add to `[dependencies]`:
```toml
rpassword = "7"
```

**Step 2: Create `vault_cmd.rs`**

```rust
//! Vault (key management) commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// Show vault status
pub async fn status(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("vault.status", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("=== Vault Status ===");
        println!();
        // Print all top-level fields
        if let Some(obj) = result.as_object() {
            for (k, v) in obj {
                let display = match v {
                    Value::String(s) => s.clone(),
                    Value::Bool(b) => b.to_string(),
                    Value::Number(n) => n.to_string(),
                    Value::Null => "-".to_string(),
                    other => other.to_string(),
                };
                println!("  {}: {}", k, display);
            }
        }
    }

    client.close().await?;
    Ok(())
}

/// Store a master key (reads interactively, never from CLI args)
pub async fn store(server_url: &str, json: bool) -> CliResult<()> {
    let master_key = if json {
        // In JSON mode, read from stdin
        let mut key = String::new();
        std::io::stdin().read_line(&mut key)
            .map_err(|e| crate::error::CliError::Other(format!("Failed to read from stdin: {}", e)))?;
        key.trim().to_string()
    } else {
        // Interactive: prompt with hidden input
        rpassword::prompt_password("Enter master key: ")
            .map_err(|e| crate::error::CliError::Other(format!("Failed to read password: {}", e)))?
    };

    if master_key.is_empty() {
        if json {
            output::print_json(&serde_json::json!({"error": "Empty key provided"}));
        } else {
            eprintln!("Error: Empty key provided.");
        }
        return Ok(());
    }

    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "master_key": master_key });
    let result: Value = client.call("vault.storeKey", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let success = result.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
        if success {
            println!("Master key stored in vault.");
        } else {
            println!("Failed to store key.");
        }
    }

    client.close().await?;
    Ok(())
}

/// Delete master key from vault
pub async fn delete(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("vault.deleteKey", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let was_present = result.get("was_present").and_then(|v| v.as_bool()).unwrap_or(false);
        if was_present {
            println!("Master key deleted from vault.");
        } else {
            println!("No key was present in vault.");
        }
    }

    client.close().await?;
    Ok(())
}

/// Verify vault integrity
pub async fn verify(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("vault.verify", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let verified = result.get("verified").and_then(|v| v.as_bool()).unwrap_or(false);
        let message = result.get("message").and_then(|v| v.as_str()).unwrap_or("");
        if verified {
            println!("Vault verified: {}", message);
        } else {
            println!("Vault verification failed: {}", message);
        }
    }

    client.close().await?;
    Ok(())
}
```

**Step 3: Add module + enum + dispatch**

Add to `mod.rs`:
```rust
pub mod vault_cmd;
```

Add `VaultAction` enum to `main.rs`:
```rust
#[derive(Subcommand)]
enum VaultAction {
    /// Show vault status
    Status,
    /// Store master key (interactive input)
    Store,
    /// Delete master key
    Delete,
    /// Verify vault integrity
    Verify,
}
```

Add `Vault` to `Commands`:
```rust
    /// Vault key management
    Vault {
        #[command(subcommand)]
        action: VaultAction,
    },
```

Add dispatch:
```rust
        Some(Commands::Vault { action }) => match action {
            VaultAction::Status => {
                commands::vault_cmd::status(&server_url, cli.json).await?;
            }
            VaultAction::Store => {
                commands::vault_cmd::store(&server_url, cli.json).await?;
            }
            VaultAction::Delete => {
                commands::vault_cmd::delete(&server_url, cli.json).await?;
            }
            VaultAction::Verify => {
                commands::vault_cmd::verify(&server_url, cli.json).await?;
            }
        },
```

**Step 4: Build and test**

Run: `cargo test -p aleph-cli`

**Step 5: Commit**
```bash
git add apps/cli/src/commands/vault_cmd.rs apps/cli/src/commands/mod.rs apps/cli/src/main.rs apps/cli/Cargo.toml
git commit -m "cli: add vault command for key management"
```

---

### Task 8: Add MCP command

**Files:**
- Create: `apps/cli/src/commands/mcp_cmd.rs`
- Modify: `apps/cli/src/commands/mod.rs`
- Modify: `apps/cli/src/main.rs`

**Context:** MCP approval handlers are already registered as stateless handlers. `mcp.respond_approval` takes `{request_id, approved, reason?}` — the CLI splits this into `approve` and `reject` subcommands.

RPC signatures:
- `mcp.list_pending_approvals`: no params → array of pending approvals
- `mcp.respond_approval`: params `{request_id, approved, reason?}` → `{success}`
- `mcp.cancel_approval`: params `{request_id}` → `{success}`

**Step 1: Create `mcp_cmd.rs`**

```rust
//! MCP (Model Context Protocol) approval workflow commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// List pending tool approval requests
pub async fn pending(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("mcp.list_pending_approvals", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let approvals = result.as_array();
        if let Some(items) = approvals {
            if items.is_empty() {
                println!("No pending approval requests.");
            } else {
                let mut rows = Vec::new();
                for item in items {
                    rows.push(vec![
                        item.get("request_id").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
                        item.get("tool").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
                        item.get("plugin").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
                    ]);
                }
                output::print_table(&["Request ID", "Tool", "Plugin"], &rows, false, &result);
            }
        } else {
            println!("No pending approval requests.");
        }
    }

    client.close().await?;
    Ok(())
}

/// Approve a tool execution request
pub async fn approve(server_url: &str, request_id: &str, reason: Option<&str>, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let mut params = serde_json::json!({
        "request_id": request_id,
        "approved": true,
    });
    if let Some(r) = reason {
        params["reason"] = Value::String(r.to_string());
    }

    let result: Value = client.call("mcp.respond_approval", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Approval '{}' approved.", request_id);
    }

    client.close().await?;
    Ok(())
}

/// Reject a tool execution request
pub async fn reject(server_url: &str, request_id: &str, reason: Option<&str>, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let mut params = serde_json::json!({
        "request_id": request_id,
        "approved": false,
    });
    if let Some(r) = reason {
        params["reason"] = Value::String(r.to_string());
    }

    let result: Value = client.call("mcp.respond_approval", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Approval '{}' rejected.", request_id);
    }

    client.close().await?;
    Ok(())
}

/// Cancel a pending approval request
pub async fn cancel(server_url: &str, request_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "request_id": request_id });
    let result: Value = client.call("mcp.cancel_approval", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Approval '{}' cancelled.", request_id);
    }

    client.close().await?;
    Ok(())
}
```

**Step 2: Add module + enum + dispatch**

Add to `mod.rs`:
```rust
pub mod mcp_cmd;
```

Add `McpAction` enum to `main.rs`:
```rust
#[derive(Subcommand)]
enum McpAction {
    /// List pending tool approval requests
    Pending,
    /// Approve a tool execution request
    Approve {
        /// Request ID
        request_id: String,
        /// Reason for approval
        #[arg(long)]
        reason: Option<String>,
    },
    /// Reject a tool execution request
    Reject {
        /// Request ID
        request_id: String,
        /// Reason for rejection
        #[arg(long)]
        reason: Option<String>,
    },
    /// Cancel a pending approval
    Cancel {
        /// Request ID
        request_id: String,
    },
}
```

Add `Mcp` to `Commands`:
```rust
    /// MCP tool approval workflow
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
```

Add dispatch:
```rust
        Some(Commands::Mcp { action }) => match action {
            McpAction::Pending => {
                commands::mcp_cmd::pending(&server_url, cli.json).await?;
            }
            McpAction::Approve { request_id, reason } => {
                commands::mcp_cmd::approve(&server_url, &request_id, reason.as_deref(), cli.json).await?;
            }
            McpAction::Reject { request_id, reason } => {
                commands::mcp_cmd::reject(&server_url, &request_id, reason.as_deref(), cli.json).await?;
            }
            McpAction::Cancel { request_id } => {
                commands::mcp_cmd::cancel(&server_url, &request_id, cli.json).await?;
            }
        },
```

**Step 3: Build and test**

Run: `cargo test -p aleph-cli`

**Step 4: Commit**
```bash
git add apps/cli/src/commands/mcp_cmd.rs apps/cli/src/commands/mod.rs apps/cli/src/main.rs
git commit -m "cli: add mcp command for tool approval workflow"
```
