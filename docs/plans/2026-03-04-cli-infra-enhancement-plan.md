# CLI Infrastructure Enhancement Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Complete the Aleph CLI foundation — config management, missing RPC handlers, daemon control, and shell completion (~820 lines across 6 new files and 5 modified files).

**Architecture:** Layered extension (Gateway RPC → CLI command → TUI slash command). CLI remains a pure protocol client (R4). Each module is end-to-end complete.

**Tech Stack:** Rust (alephcore + aleph-cli), JSON-RPC 2.0, clap + clap_complete, aleph-protocol

**Important**: The TUI slash commands (`/usage`, `/memory`, `/compact`) already make real RPC calls with error fallbacks. The TUI code calls `session.usage`, `memory.search` (already exists in Gateway), and `session.compact`. We just need the Gateway handlers to match these method names.

---

## Task 1: Shell Completion

**Files:**
- Modify: `apps/cli/Cargo.toml`
- Create: `apps/cli/src/commands/completion.rs`
- Modify: `apps/cli/src/commands/mod.rs`
- Modify: `apps/cli/src/main.rs`

**Step 1: Add clap_complete dependency**

In `apps/cli/Cargo.toml`, add to `[dependencies]`:

```toml
clap_complete = "4"
```

**Step 2: Create completion command module**

Create `apps/cli/src/commands/completion.rs`:

```rust
//! Shell completion generation

use clap::CommandFactory;
use clap_complete::{generate, Shell};
use std::io;

use crate::Cli;

/// Generate shell completion script and print to stdout
pub fn run(shell: Shell) {
    let mut cmd = Cli::command();
    generate(shell, &mut cmd, "aleph", &mut io::stdout());
}
```

**Step 3: Add module export**

In `apps/cli/src/commands/mod.rs`, add:

```rust
pub mod completion;
```

**Step 4: Add Completion command variant**

In `apps/cli/src/main.rs`, add to the `Commands` enum (after `Info`):

```rust
    /// Generate shell completion script
    Completion {
        /// Shell type
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
```

Add to the match block (after `Commands::Info`):

```rust
        Some(Commands::Completion { shell }) => {
            commands::completion::run(shell);
        }
```

Add import at top of file:

```rust
// No new imports needed — clap_complete::Shell is referenced via full path in the enum
```

**Step 5: Build and verify**

Run: `cargo build -p aleph-cli`
Expected: Compiles without errors

Run: `cargo run -p aleph-cli -- completion bash | head -5`
Expected: Outputs bash completion script starting with `_aleph()`

**Step 6: Commit**

```bash
git add apps/cli/Cargo.toml apps/cli/src/commands/completion.rs apps/cli/src/commands/mod.rs apps/cli/src/main.rs
git commit -m "cli: add shell completion generation (bash/zsh/fish)"
```

---

## Task 2: Config Management — Gateway Handlers

**Files:**
- Create: `core/src/gateway/handlers/config_mgmt.rs`
- Modify: `core/src/gateway/handlers/mod.rs`

**Context:** The Gateway already has a config system at `core/src/config/` with TOML loading, validation, and schema generation. The existing `config.schema` handler is in `core/src/gateway/handlers/config.rs`. We add `config.get`, `config.set`, `config.validate` in a new file to avoid name conflicts.

**Step 1: Write tests for config handlers**

Create `core/src/gateway/handlers/config_mgmt.rs`:

```rust
//! Configuration management RPC handlers
//!
//! Provides config.get, config.set, and config.validate methods
//! for reading and writing Aleph configuration via JSON-RPC.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::AlephConfig;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

use super::parse_params;

/// Shared config reference for handlers
pub type SharedConfig = Arc<RwLock<AlephConfig>>;

#[derive(Debug, Deserialize)]
struct GetParams {
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SetParams {
    path: String,
    value: Value,
}

/// Handle config.get — retrieve config value by dot-path
pub async fn handle_get(request: JsonRpcRequest, config: SharedConfig) -> JsonRpcResponse {
    let path = request
        .params
        .as_ref()
        .and_then(|p| p.get("path"))
        .and_then(|v| v.as_str());

    let config_guard = config.read().await;
    let full_value = match serde_json::to_value(&*config_guard) {
        Ok(v) => v,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                -32603,
                format!("Failed to serialize config: {}", e),
            );
        }
    };

    match path {
        None => JsonRpcResponse::success(request.id, json!({ "value": full_value })),
        Some(p) => {
            let value = resolve_dot_path(&full_value, p);
            match value {
                Some(v) => JsonRpcResponse::success(request.id, json!({ "value": v })),
                None => JsonRpcResponse::error(
                    request.id,
                    -32001,
                    format!("Config path '{}' not found", p),
                ),
            }
        }
    }
}

/// Handle config.set — set a config value by dot-path
pub async fn handle_set(request: JsonRpcRequest, config: SharedConfig) -> JsonRpcResponse {
    let params: SetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let mut config_guard = config.write().await;
    let mut full_value = match serde_json::to_value(&*config_guard) {
        Ok(v) => v,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                -32603,
                format!("Failed to serialize config: {}", e),
            );
        }
    };

    // Get previous value
    let previous = resolve_dot_path(&full_value, &params.path).cloned();

    // Set new value
    if !set_dot_path(&mut full_value, &params.path, params.value.clone()) {
        return JsonRpcResponse::error(
            request.id,
            -32602,
            format!("Cannot set path '{}'", params.path),
        );
    }

    // Deserialize back to config
    match serde_json::from_value::<AlephConfig>(full_value) {
        Ok(new_config) => {
            *config_guard = new_config;
            JsonRpcResponse::success(
                request.id,
                json!({
                    "success": true,
                    "path": params.path,
                    "previous": previous,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            -32602,
            format!("Invalid config value: {}", e),
        ),
    }
}

/// Handle config.validate — check if current config is valid
pub async fn handle_validate(request: JsonRpcRequest, config: SharedConfig) -> JsonRpcResponse {
    let config_guard = config.read().await;

    // Serialize and deserialize to validate
    match serde_json::to_value(&*config_guard) {
        Ok(value) => match serde_json::from_value::<AlephConfig>(value) {
            Ok(_) => JsonRpcResponse::success(
                request.id,
                json!({ "valid": true, "errors": [] }),
            ),
            Err(e) => JsonRpcResponse::success(
                request.id,
                json!({ "valid": false, "errors": [e.to_string()] }),
            ),
        },
        Err(e) => JsonRpcResponse::error(
            request.id,
            -32603,
            format!("Failed to serialize config: {}", e),
        ),
    }
}

/// Resolve a dot-separated path in a JSON value
fn resolve_dot_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for key in path.split('.') {
        current = current.get(key)?;
    }
    Some(current)
}

/// Set a value at a dot-separated path in a JSON value
fn set_dot_path(value: &mut Value, path: &str, new_value: Value) -> bool {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.is_empty() {
        return false;
    }

    let mut current = value;
    for part in &parts[..parts.len() - 1] {
        current = match current.get_mut(*part) {
            Some(v) => v,
            None => return false,
        };
    }

    if let Some(last) = parts.last() {
        if let Value::Object(map) = current {
            map.insert(last.to_string(), new_value);
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_dot_path_simple() {
        let v = json!({ "a": { "b": { "c": 42 } } });
        assert_eq!(resolve_dot_path(&v, "a.b.c"), Some(&json!(42)));
    }

    #[test]
    fn resolve_dot_path_missing() {
        let v = json!({ "a": 1 });
        assert_eq!(resolve_dot_path(&v, "a.b"), None);
    }

    #[test]
    fn set_dot_path_creates_value() {
        let mut v = json!({ "a": { "b": 1 } });
        assert!(set_dot_path(&mut v, "a.b", json!(2)));
        assert_eq!(v["a"]["b"], json!(2));
    }

    #[test]
    fn set_dot_path_invalid_returns_false() {
        let mut v = json!({ "a": 1 });
        assert!(!set_dot_path(&mut v, "a.b.c", json!(2)));
    }
}
```

**Step 2: Register handlers in mod.rs**

In `core/src/gateway/handlers/mod.rs`:
- Add module declaration: `pub mod config_mgmt;`
- In `HandlerRegistry::new()`, add placeholder registrations (to be wired with real config in Gateway startup):

```rust
// Config management (requires SharedConfig — placeholder until wired)
registry.register("config.get", |req| async move {
    JsonRpcResponse::error(req.id, -32603, "config.get requires SharedConfig — wire in Gateway startup".to_string())
});
registry.register("config.set", |req| async move {
    JsonRpcResponse::error(req.id, -32603, "config.set requires SharedConfig — wire in Gateway startup".to_string())
});
registry.register("config.validate", |req| async move {
    JsonRpcResponse::error(req.id, -32603, "config.validate requires SharedConfig — wire in Gateway startup".to_string())
});
```

**Step 3: Build and test**

Run: `cargo test -p alephcore --lib config_mgmt`
Expected: 4 tests pass (resolve_dot_path_simple, resolve_dot_path_missing, set_dot_path_creates_value, set_dot_path_invalid_returns_false)

Run: `cargo check -p alephcore`
Expected: Compiles

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/config_mgmt.rs core/src/gateway/handlers/mod.rs
git commit -m "gateway: add config.get/set/validate RPC handlers"
```

**Note:** The AlephConfig type import path may need adjustment based on the actual config module structure. Check `core/src/config/mod.rs` for the exact public type name. It might be `Config`, `AlephConfig`, or `ServerConfig`. Use whatever is publicly exported.

---

## Task 3: Config Management — CLI Command

**Files:**
- Create: `apps/cli/src/commands/config_cmd.rs`
- Modify: `apps/cli/src/commands/mod.rs`
- Modify: `apps/cli/src/main.rs`

**Step 1: Create config command module**

Create `apps/cli/src/commands/config_cmd.rs`:

```rust
//! Configuration management commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;

/// Print the config file path
pub fn file() {
    let path = CliConfig::config_path(None);
    println!("{}", path.display());
}

/// Get a config value by dot-path
pub async fn get(server_url: &str, path: Option<&str>, config: &CliConfig) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let params = match path {
        Some(p) => serde_json::json!({ "path": p }),
        None => serde_json::json!({}),
    };

    let result: Value = client.call("config.get", Some(params)).await?;

    if let Some(value) = result.get("value") {
        println!("{}", serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()));
    }

    client.close().await?;
    Ok(())
}

/// Set a config value by dot-path
pub async fn set(server_url: &str, path: &str, value: &str, config: &CliConfig) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    // Parse value as JSON, fall back to string
    let json_value: Value = serde_json::from_str(value).unwrap_or(Value::String(value.to_string()));

    let params = serde_json::json!({
        "path": path,
        "value": json_value,
    });

    let result: Value = client.call("config.set", Some(params)).await?;

    if result.get("success").and_then(|v| v.as_bool()) == Some(true) {
        println!("Set {} = {}", path, value);
        if let Some(prev) = result.get("previous") {
            println!("Previous: {}", prev);
        }
    }

    client.close().await?;
    Ok(())
}

/// Validate the current configuration
pub async fn validate(server_url: &str, config: &CliConfig) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let result: Value = client.call("config.validate", None::<()>).await?;

    let valid = result.get("valid").and_then(|v| v.as_bool()).unwrap_or(false);
    if valid {
        println!("Configuration is valid.");
    } else {
        println!("Configuration has errors:");
        if let Some(errors) = result.get("errors").and_then(|v| v.as_array()) {
            for err in errors {
                println!("  - {}", err.as_str().unwrap_or("unknown error"));
            }
        }
    }

    client.close().await?;
    Ok(())
}
```

**Step 2: Add module export**

In `apps/cli/src/commands/mod.rs`, add:

```rust
pub mod config_cmd;
```

**Step 3: Add Config command to main.rs**

In `apps/cli/src/main.rs`, add `ConfigAction` enum after `SessionAction`:

```rust
#[derive(Subcommand)]
enum ConfigAction {
    /// Print config file path
    File,
    /// Get a config value (no path = show all)
    Get {
        /// Dot-separated config path
        path: Option<String>,
    },
    /// Set a config value
    Set {
        /// Dot-separated config path
        path: String,
        /// Value to set (JSON or plain string)
        value: String,
    },
    /// Validate current configuration
    Validate,
}
```

Add to `Commands` enum:

```rust
    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
```

Add to match block:

```rust
        Some(Commands::Config { action }) => match action {
            ConfigAction::File => {
                commands::config_cmd::file();
            }
            ConfigAction::Get { path } => {
                commands::config_cmd::get(&server_url, path.as_deref(), &config).await?;
            }
            ConfigAction::Set { path, value } => {
                commands::config_cmd::set(&server_url, &path, &value, &config).await?;
            }
            ConfigAction::Validate => {
                commands::config_cmd::validate(&server_url, &config).await?;
            }
        },
```

**Step 4: Handle config_path method**

Check `apps/cli/src/config.rs` for how to get the config file path. If there's no `config_path()` method, add one or use the known default path `~/.aleph/config.toml`. Adjust `config_cmd::file()` accordingly:

```rust
pub fn file() {
    let home = dirs::home_dir().unwrap_or_default();
    let path = home.join(".aleph").join("config.toml");
    println!("{}", path.display());
}
```

**Step 5: Build and verify**

Run: `cargo build -p aleph-cli`
Expected: Compiles without errors

Run: `cargo run -p aleph-cli -- config file`
Expected: Prints path like `/Users/xxx/.aleph/config.toml`

**Step 6: Commit**

```bash
git add apps/cli/src/commands/config_cmd.rs apps/cli/src/commands/mod.rs apps/cli/src/main.rs
git commit -m "cli: add config get/set/file/validate commands"
```

---

## Task 4: Session Usage — Gateway Handler

**Files:**
- Create: `core/src/gateway/handlers/session_usage.rs`
- Modify: `core/src/gateway/handlers/mod.rs`

**Context:** The TUI already calls `client.call("session.usage", ...)` with params `{ "session_key": "..." }`. We need a Gateway handler registered as `session.usage`.

**Step 1: Create session usage handler**

Create `core/src/gateway/handlers/session_usage.rs`:

```rust
//! Session usage statistics RPC handler
//!
//! Returns token counts and message statistics for a session.

use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

use super::session::SessionStore;

#[derive(Debug, Deserialize)]
struct UsageParams {
    session_key: String,
}

/// Handle session.usage — return token/message stats for a session
pub async fn handle(request: JsonRpcRequest, store: Arc<SessionStore>) -> JsonRpcResponse {
    let params: UsageParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let sessions = store.list(None).await;
    let session_info = sessions.iter().find(|s| s.key == params.session_key);

    match session_info {
        Some(info) => {
            // Estimate tokens from message history
            let history = store.get_history(&params.session_key, None).await;
            let (input_tokens, output_tokens) = match &history {
                Some(messages) => estimate_tokens(messages),
                None => (0u64, 0u64),
            };

            let total = input_tokens + output_tokens;
            let message_count = history.as_ref().map(|h| h.len()).unwrap_or(0);

            JsonRpcResponse::success(
                request.id,
                json!({
                    "session_key": params.session_key,
                    "tokens": total,
                    "input_tokens": input_tokens,
                    "output_tokens": output_tokens,
                    "messages": message_count,
                    "created_at": info.created_at,
                    "last_active_at": info.last_active_at,
                }),
            )
        }
        None => JsonRpcResponse::error(
            request.id,
            -32001,
            format!("Session '{}' not found", params.session_key),
        ),
    }
}

/// Estimate token counts from message history.
/// Uses rough approximation: ~4 chars per token for English, ~2 chars per token for CJK.
fn estimate_tokens(messages: &[super::session::HistoryMessage]) -> (u64, u64) {
    let mut input = 0u64;
    let mut output = 0u64;
    for msg in messages {
        let chars = msg.content.len() as u64;
        let tokens = chars / 3; // rough average
        match msg.role.as_str() {
            "user" => input += tokens,
            "assistant" => output += tokens,
            _ => input += tokens, // system messages count as input
        }
    }
    (input, output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_basic() {
        let messages = vec![
            super::super::session::HistoryMessage {
                role: "user".to_string(),
                content: "Hello, how are you?".to_string(), // 19 chars
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                metadata: None,
            },
            super::super::session::HistoryMessage {
                role: "assistant".to_string(),
                content: "I am doing well, thank you for asking!".to_string(), // 38 chars
                timestamp: "2026-01-01T00:00:01Z".to_string(),
                metadata: None,
            },
        ];
        let (input, output) = estimate_tokens(&messages);
        assert!(input > 0);
        assert!(output > input); // assistant message is longer
    }
}
```

**Step 2: Register in mod.rs**

In `core/src/gateway/handlers/mod.rs`:
- Add module: `pub mod session_usage;`
- Add placeholder registration:

```rust
// Session usage (requires SessionStore — placeholder)
registry.register("session.usage", |req| async move {
    JsonRpcResponse::error(req.id, -32603, "session.usage requires SessionStore".to_string())
});
```

**Step 3: Build and test**

Run: `cargo test -p alephcore --lib session_usage`
Expected: 1 test passes

Run: `cargo check -p alephcore`
Expected: Compiles

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/session_usage.rs core/src/gateway/handlers/mod.rs
git commit -m "gateway: add session.usage RPC handler for token statistics"
```

**Note:** The import paths for `SessionStore` and `HistoryMessage` need to match the actual types in `session.rs`. Check `core/src/gateway/handlers/session.rs` for the exact public types. Adjust imports accordingly.

---

## Task 5: Session Compact — Gateway Handler

**Files:**
- Modify: `core/src/gateway/handlers/session.rs` (add handle_compact)
- Modify: `core/src/gateway/handlers/mod.rs`

**Context:** The TUI calls `client.call("session.compact", { "session_key": "..." })`. Initial implementation uses simple truncation: keep last N messages, summarize older ones into a system message prefix.

**Step 1: Add compact handler to session.rs**

In `core/src/gateway/handlers/session.rs`, add at the end (before tests if any):

```rust
/// Handle session.compact — compress session history by summarizing older messages
pub async fn handle_compact(
    request: JsonRpcRequest,
    store: Arc<SessionStore>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct CompactParams {
        session_key: String,
    }

    let params: CompactParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let history = store.get_history(&params.session_key, None).await;
    let messages = match history {
        Some(msgs) => msgs,
        None => {
            return JsonRpcResponse::error(
                request.id,
                -32001,
                format!("Session '{}' not found", params.session_key),
            );
        }
    };

    let before_count = messages.len();

    // Keep last 20 messages, compact anything before that
    let keep_count = 20;
    if before_count <= keep_count {
        return JsonRpcResponse::success(
            request.id,
            json!({
                "message": "Session is already compact.",
                "before_messages": before_count,
                "after_messages": before_count,
                "tokens_saved": 0,
            }),
        );
    }

    // Build summary of compacted messages
    let compacted = &messages[..before_count - keep_count];
    let summary = build_summary(compacted);

    // Reset session and re-add summary + recent messages
    store.reset(&params.session_key).await;
    store.add_message(&params.session_key, "system", &summary).await;
    for msg in &messages[before_count - keep_count..] {
        store.add_message(&params.session_key, &msg.role, &msg.content).await;
    }

    let after_count = keep_count + 1; // kept messages + summary
    let chars_removed: usize = compacted.iter().map(|m| m.content.len()).sum();
    let tokens_saved = chars_removed / 3; // rough estimate

    JsonRpcResponse::success(
        request.id,
        json!({
            "message": format!("Compacted {} messages into summary.", compacted.len()),
            "before_messages": before_count,
            "after_messages": after_count,
            "tokens_saved": tokens_saved,
        }),
    )
}

/// Build a brief summary of compacted messages
fn build_summary(messages: &[HistoryMessage]) -> String {
    let user_msgs: Vec<&str> = messages
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .collect();
    let topics: Vec<&str> = user_msgs.iter().take(5).copied().collect();

    format!(
        "[Session summary: {} earlier messages compacted. Topics discussed: {}]",
        messages.len(),
        if topics.is_empty() {
            "general conversation".to_string()
        } else {
            topics.join("; ")
        }
    )
}
```

**Step 2: Register in mod.rs**

Add placeholder:

```rust
// Session compact (requires SessionStore — placeholder)
registry.register("session.compact", |req| async move {
    JsonRpcResponse::error(req.id, -32603, "session.compact requires SessionStore".to_string())
});
```

**Step 3: Build and test**

Run: `cargo check -p alephcore`
Expected: Compiles

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/session.rs core/src/gateway/handlers/mod.rs
git commit -m "gateway: add session.compact RPC handler for context compression"
```

---

## Task 6: Daemon Control — Gateway Handlers

**Files:**
- Create: `core/src/gateway/handlers/daemon_control.rs`
- Modify: `core/src/gateway/handlers/mod.rs`

**Step 1: Create daemon control handler**

Create `core/src/gateway/handlers/daemon_control.rs`:

```rust
//! Daemon control RPC handlers
//!
//! Provides daemon.status, daemon.shutdown, and daemon.logs methods.

use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::oneshot;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Shared state for daemon status tracking
pub struct DaemonState {
    pub start_time: Instant,
    pub shutdown_tx: Option<oneshot::Sender<()>>,
}

/// Handle daemon.status — return server runtime info
pub async fn handle_status(
    request: JsonRpcRequest,
    start_time: Instant,
    connection_count: usize,
) -> JsonRpcResponse {
    let uptime = start_time.elapsed().as_secs();

    JsonRpcResponse::success(
        request.id,
        json!({
            "running": true,
            "uptime_secs": uptime,
            "version": env!("CARGO_PKG_VERSION"),
            "connections": connection_count,
            "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        }),
    )
}

/// Handle daemon.shutdown — initiate graceful shutdown
pub async fn handle_shutdown(
    request: JsonRpcRequest,
    shutdown_tx: Arc<tokio::sync::Mutex<Option<oneshot::Sender<()>>>>,
) -> JsonRpcResponse {
    let mut guard = shutdown_tx.lock().await;
    if let Some(tx) = guard.take() {
        let _ = tx.send(());
        JsonRpcResponse::success(request.id, json!({ "status": "shutting_down" }))
    } else {
        JsonRpcResponse::error(
            request.id,
            -32603,
            "Shutdown already in progress".to_string(),
        )
    }
}

#[derive(Debug, Deserialize)]
struct LogsParams {
    #[serde(default = "default_lines")]
    lines: usize,
    #[serde(default)]
    level: Option<String>,
}

fn default_lines() -> usize {
    50
}

/// Handle daemon.logs — return recent log lines
pub async fn handle_logs(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: LogsParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or(LogsParams {
            lines: 50,
            level: None,
        });

    // Find log directory
    let log_dir = log_directory();
    let log_file = find_latest_log(&log_dir);

    match log_file {
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(content) => {
                let mut lines: Vec<&str> = content.lines().collect();

                // Filter by level if specified
                if let Some(ref level) = params.level {
                    let level_upper = level.to_uppercase();
                    lines.retain(|line| line.contains(&level_upper));
                }

                // Take last N lines
                let start = lines.len().saturating_sub(params.lines);
                let result: Vec<String> = lines[start..].iter().map(|s| s.to_string()).collect();

                JsonRpcResponse::success(
                    request.id,
                    json!({
                        "logs": result,
                        "file": path.display().to_string(),
                        "total_lines": result.len(),
                    }),
                )
            }
            Err(e) => JsonRpcResponse::error(
                request.id,
                -32603,
                format!("Failed to read log file: {}", e),
            ),
        },
        None => JsonRpcResponse::success(
            request.id,
            json!({
                "logs": [],
                "file": null,
                "total_lines": 0,
            }),
        ),
    }
}

/// Get the log directory path
fn log_directory() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".aleph")
        .join("logs")
}

/// Find the most recent log file in the directory
fn find_latest_log(dir: &PathBuf) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "log")
                .unwrap_or(false)
        })
        .max_by_key(|e| e.metadata().ok().and_then(|m| m.modified().ok()))
        .map(|e| e.path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_directory_is_under_home() {
        let dir = log_directory();
        assert!(dir.to_string_lossy().contains(".aleph"));
    }
}
```

**Step 2: Register in mod.rs**

- Add module: `pub mod daemon_control;`
- Add registrations:

```rust
// Daemon logs (no dependencies)
registry.register("daemon.logs", daemon_control::handle_logs);

// daemon.status and daemon.shutdown need runtime state — registered as placeholders
registry.register("daemon.status", |req| async move {
    JsonRpcResponse::error(req.id, -32603, "daemon.status requires runtime state".to_string())
});
registry.register("daemon.shutdown", |req| async move {
    JsonRpcResponse::error(req.id, -32603, "daemon.shutdown requires runtime state".to_string())
});
```

**Step 3: Build and test**

Run: `cargo test -p alephcore --lib daemon_control`
Expected: 1 test passes

Run: `cargo check -p alephcore`
Expected: Compiles

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/daemon_control.rs core/src/gateway/handlers/mod.rs
git commit -m "gateway: add daemon.status/shutdown/logs RPC handlers"
```

**Note:** `dirs` crate may need to be added to `core/Cargo.toml` if not already present. Check with `cargo check`. The `sysinfo` crate is already used in `system_info.rs` if needed for memory stats.

---

## Task 7: Daemon Control — CLI Command

**Files:**
- Create: `apps/cli/src/commands/daemon.rs`
- Modify: `apps/cli/src/commands/mod.rs`
- Modify: `apps/cli/src/main.rs`

**Step 1: Create daemon command module**

Create `apps/cli/src/commands/daemon.rs`:

```rust
//! Daemon (Gateway server) management commands

use serde_json::Value;
use std::process::Command;
use std::time::Duration;

use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::{CliError, CliResult};

/// Show Gateway server status
pub async fn status(server_url: &str) -> CliResult<()> {
    match AlephClient::connect(server_url).await {
        Ok((client, _events)) => {
            let result: Value = client.call("daemon.status", None::<()>).await?;

            println!("Gateway Status");
            println!("──────────────");
            println!(
                "Status:      {}",
                if result.get("running").and_then(|v| v.as_bool()) == Some(true) {
                    "Running"
                } else {
                    "Unknown"
                }
            );
            if let Some(uptime) = result.get("uptime_secs").and_then(|v| v.as_u64()) {
                println!("Uptime:      {}", format_uptime(uptime));
            }
            if let Some(version) = result.get("version").and_then(|v| v.as_str()) {
                println!("Version:     {}", version);
            }
            if let Some(conns) = result.get("connections").and_then(|v| v.as_u64()) {
                println!("Connections: {}", conns);
            }
            if let Some(platform) = result.get("platform").and_then(|v| v.as_str()) {
                println!("Platform:    {}", platform);
            }

            client.close().await?;
            Ok(())
        }
        Err(_) => {
            println!("Gateway Status");
            println!("──────────────");
            println!("Status:      Not running");
            println!("URL:         {}", server_url);

            // Check for PID file
            let pid_file = dirs::home_dir()
                .unwrap_or_default()
                .join(".aleph")
                .join("aleph.pid");
            if pid_file.exists() {
                if let Ok(pid_str) = std::fs::read_to_string(&pid_file) {
                    println!("Stale PID:   {} (process not responding)", pid_str.trim());
                }
            }

            Ok(())
        }
    }
}

/// Start the Gateway server
pub async fn start(server_url: &str) -> CliResult<()> {
    // Check if already running
    if AlephClient::connect(server_url).await.is_ok() {
        println!("Gateway is already running at {}", server_url);
        return Ok(());
    }

    println!("Starting Gateway...");

    // Find the aleph binary
    let exe = std::env::current_exe().unwrap_or_else(|_| "aleph".into());

    match Command::new(&exe).arg("serve").spawn() {
        Ok(child) => {
            // Write PID file
            let pid_file = dirs::home_dir()
                .unwrap_or_default()
                .join(".aleph")
                .join("aleph.pid");
            let _ = std::fs::write(&pid_file, child.id().to_string());

            // Wait briefly and check if it started
            tokio::time::sleep(Duration::from_secs(2)).await;

            if AlephClient::connect(server_url).await.is_ok() {
                println!("Gateway started successfully at {}", server_url);
            } else {
                println!("Gateway process started (PID: {}), but not yet responding.", child.id());
                println!("Check logs: aleph daemon logs");
            }
        }
        Err(e) => {
            return Err(CliError::Other(format!("Failed to start Gateway: {}", e)));
        }
    }

    Ok(())
}

/// Stop the Gateway server
pub async fn stop(server_url: &str) -> CliResult<()> {
    match AlephClient::connect(server_url).await {
        Ok((client, _events)) => {
            println!("Sending shutdown signal...");
            match client.call::<_, Value>("daemon.shutdown", None::<()>).await {
                Ok(_) => println!("Gateway is shutting down."),
                Err(e) => println!("Shutdown request sent, but got error: {}", e),
            }
            Ok(())
        }
        Err(_) => {
            // Try PID file fallback
            let pid_file = dirs::home_dir()
                .unwrap_or_default()
                .join(".aleph")
                .join("aleph.pid");

            if pid_file.exists() {
                if let Ok(pid_str) = std::fs::read_to_string(&pid_file) {
                    if let Ok(pid) = pid_str.trim().parse::<i32>() {
                        println!("Sending SIGTERM to PID {}...", pid);
                        unsafe {
                            libc::kill(pid, libc::SIGTERM);
                        }
                        let _ = std::fs::remove_file(&pid_file);
                        println!("Signal sent. Gateway should stop shortly.");
                        return Ok(());
                    }
                }
            }

            println!("Gateway is not running at {}", server_url);
            Ok(())
        }
    }
}

/// Restart the Gateway server
pub async fn restart(server_url: &str) -> CliResult<()> {
    stop(server_url).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    start(server_url).await
}

/// View Gateway logs
pub async fn logs(server_url: &str, lines: usize, level: Option<&str>) -> CliResult<()> {
    // Try via RPC first
    if let Ok((client, _events)) = AlephClient::connect(server_url).await {
        let params = serde_json::json!({
            "lines": lines,
            "level": level,
        });

        match client.call::<_, Value>("daemon.logs", Some(params)).await {
            Ok(result) => {
                if let Some(logs) = result.get("logs").and_then(|v| v.as_array()) {
                    for line in logs {
                        println!("{}", line.as_str().unwrap_or(""));
                    }
                    if logs.is_empty() {
                        println!("No log entries found.");
                    }
                }
                client.close().await?;
                return Ok(());
            }
            Err(_) => {
                client.close().await?;
                // Fall through to local file reading
            }
        }
    }

    // Fallback: read log file directly
    let log_dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".aleph")
        .join("logs");

    if !log_dir.exists() {
        println!("No log directory found at {}", log_dir.display());
        return Ok(());
    }

    // Find latest log file
    let latest = std::fs::read_dir(&log_dir)
        .ok()
        .and_then(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map(|ext| ext == "log").unwrap_or(false))
                .max_by_key(|e| e.metadata().ok().and_then(|m| m.modified().ok()))
                .map(|e| e.path())
        });

    match latest {
        Some(path) => {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| CliError::Other(format!("Failed to read {}: {}", path.display(), e)))?;

            let mut log_lines: Vec<&str> = content.lines().collect();

            if let Some(lvl) = level {
                let lvl_upper = lvl.to_uppercase();
                log_lines.retain(|line| line.contains(&lvl_upper));
            }

            let start = log_lines.len().saturating_sub(lines);
            for line in &log_lines[start..] {
                println!("{}", line);
            }
        }
        None => {
            println!("No log files found in {}", log_dir.display());
        }
    }

    Ok(())
}

fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m {}s", mins, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_uptime_minutes() {
        assert_eq!(format_uptime(125), "2m 5s");
    }

    #[test]
    fn format_uptime_hours() {
        assert_eq!(format_uptime(3725), "1h 2m");
    }

    #[test]
    fn format_uptime_days() {
        assert_eq!(format_uptime(90061), "1d 1h 1m");
    }
}
```

**Step 2: Add module export and main.rs integration**

In `apps/cli/src/commands/mod.rs`, add:

```rust
pub mod daemon;
```

In `apps/cli/src/main.rs`, add `DaemonAction` enum:

```rust
#[derive(Subcommand)]
enum DaemonAction {
    /// Show Gateway server status
    Status,
    /// Start Gateway server
    Start,
    /// Stop Gateway server
    Stop,
    /// Restart Gateway server
    Restart,
    /// View Gateway logs
    Logs {
        /// Number of lines to show
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,
        /// Filter by log level
        #[arg(short, long)]
        level: Option<String>,
    },
}
```

Add to `Commands` enum:

```rust
    /// Manage Gateway daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
```

Add to match block:

```rust
        Some(Commands::Daemon { action }) => match action {
            DaemonAction::Status => {
                commands::daemon::status(&server_url).await?;
            }
            DaemonAction::Start => {
                commands::daemon::start(&server_url).await?;
            }
            DaemonAction::Stop => {
                commands::daemon::stop(&server_url).await?;
            }
            DaemonAction::Restart => {
                commands::daemon::restart(&server_url).await?;
            }
            DaemonAction::Logs { lines, level } => {
                commands::daemon::logs(&server_url, lines, level.as_deref()).await?;
            }
        },
```

**Step 3: Add libc dependency**

In `apps/cli/Cargo.toml`:

```toml
libc = "0.2"
dirs = "5"
```

Check if `dirs` is already a dependency (it may be).

**Step 4: Build and test**

Run: `cargo test -p aleph-cli`
Expected: All existing tests + 3 new format_uptime tests pass

Run: `cargo build -p aleph-cli`
Expected: Compiles

**Step 5: Commit**

```bash
git add apps/cli/src/commands/daemon.rs apps/cli/src/commands/mod.rs apps/cli/src/main.rs apps/cli/Cargo.toml
git commit -m "cli: add daemon status/start/stop/restart/logs commands"
```

---

## Task 8: Wire Handlers in Gateway Startup & Final Verification

**Files:**
- Modify: Gateway startup code (find the file that builds `HandlerRegistry` with real dependencies)
- Potentially: `core/src/bin/aleph/` or `core/src/gateway/server.rs`

**Context:** Tasks 2-6 registered placeholder handlers. This task wires them with real dependencies (SessionStore, SharedConfig, DaemonState) during Gateway startup.

**Step 1: Find the handler wiring location**

Search for where handlers are wired with real dependencies. Look for patterns like:
- `handlers_mut()` calls on `GatewayServer`
- `register()` calls outside `HandlerRegistry::new()`
- Builder pattern in `core/src/bin/aleph/`

The key file is likely `core/src/bin/aleph/commands/start/builder/handlers.rs` or similar.

**Step 2: Wire config handlers**

```rust
// Wire config.get/set/validate with shared config
let shared_config: Arc<RwLock<AlephConfig>> = /* from server initialization */;

let cfg = shared_config.clone();
handlers.register("config.get", move |req| {
    let config = cfg.clone();
    async move { config_mgmt::handle_get(req, config).await }
});

let cfg = shared_config.clone();
handlers.register("config.set", move |req| {
    let config = cfg.clone();
    async move { config_mgmt::handle_set(req, config).await }
});

let cfg = shared_config.clone();
handlers.register("config.validate", move |req| {
    let config = cfg.clone();
    async move { config_mgmt::handle_validate(req, config).await }
});
```

**Step 3: Wire session handlers**

```rust
// Wire session.usage and session.compact with SessionStore
let store = session_store.clone();
handlers.register("session.usage", move |req| {
    let s = store.clone();
    async move { session_usage::handle(req, s).await }
});

let store = session_store.clone();
handlers.register("session.compact", move |req| {
    let s = store.clone();
    async move { session::handle_compact(req, s).await }
});
```

**Step 4: Wire daemon handlers**

```rust
// Wire daemon.status with start time and connection count
let start_time = Instant::now();
let connections = gateway_shared_state.connections.clone();
handlers.register("daemon.status", move |req| {
    let conns = connections.clone();
    async move {
        let count = conns.read().await.len();
        daemon_control::handle_status(req, start_time, count).await
    }
});

// Wire daemon.shutdown with shutdown sender
let shutdown_tx = Arc::new(tokio::sync::Mutex::new(Some(shutdown_sender)));
let tx = shutdown_tx.clone();
handlers.register("daemon.shutdown", move |req| {
    let sender = tx.clone();
    async move { daemon_control::handle_shutdown(req, sender).await }
});
```

**Step 5: Build full workspace**

Run: `cargo build -p alephcore`
Expected: Compiles

Run: `cargo test -p alephcore --lib`
Expected: All tests pass (including new handler tests)

Run: `cargo test -p aleph-cli`
Expected: All 101+ tests pass

Run: `cargo clippy -p aleph-cli -- -D warnings`
Expected: Zero warnings

**Step 6: Commit**

```bash
git add -A
git commit -m "gateway: wire config/usage/compact/daemon handlers with runtime dependencies"
```

---

## Summary

| Task | Module | New Lines | Description |
|------|--------|-----------|-------------|
| 1 | Shell Completion | ~50 | `aleph completion bash/zsh/fish` |
| 2 | Config (Gateway) | ~150 | `config.get/set/validate` RPC handlers |
| 3 | Config (CLI) | ~120 | `aleph config get/set/file/validate` commands |
| 4 | Usage (Gateway) | ~80 | `session.usage` RPC handler |
| 5 | Compact (Gateway) | ~80 | `session.compact` RPC handler |
| 6 | Daemon (Gateway) | ~120 | `daemon.status/shutdown/logs` RPC handlers |
| 7 | Daemon (CLI) | ~180 | `aleph daemon status/start/stop/restart/logs` |
| 8 | Wiring | ~40 | Wire all handlers with runtime dependencies |
| **Total** | | **~820** | |
