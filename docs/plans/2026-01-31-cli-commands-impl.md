# CLI Commands Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 实现核心 CLI 命令：gateway call, config, channels, cron

**Architecture:** 创建 `core/src/cli/` 模块，包含 RPC 客户端和命令实现，扩展 `aleph_gateway.rs` 添加新 subcommands

**Tech Stack:** Rust, Clap 4, tokio-tungstenite, serde_json

---

## Task 1: Create CLI Module Structure

**Files:**
- Create: `core/src/cli/mod.rs`
- Create: `core/src/cli/error.rs`
- Modify: `core/src/lib.rs`

**Step 1: Create cli/mod.rs**

```rust
//! CLI utilities for Aleph Gateway commands.

pub mod error;
pub mod client;
pub mod output;

pub use error::CliError;
pub use client::GatewayClient;
pub use output::{OutputFormat, print_json, print_table};
```

**Step 2: Create cli/error.rs**

```rust
//! CLI error types.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum CliError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("RPC error: {0}")]
    RpcError(String),

    #[error("Timeout after {0}ms")]
    Timeout(u64),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}
```

**Step 3: Export from lib.rs**

Add to `core/src/lib.rs`:

```rust
pub mod cli;
```

**Step 4: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo check`

**Step 5: Commit**

```bash
git add core/src/cli/ core/src/lib.rs
git commit -m "feat(cli): create cli module structure"
```

---

## Task 2: Implement Gateway RPC Client

**Files:**
- Create: `core/src/cli/client.rs`

**Step 1: Create client.rs**

```rust
//! Gateway RPC client for CLI commands.

use crate::cli::CliError;
use futures_util::{SinkExt, StreamExt};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// Default Gateway URL
pub const DEFAULT_GATEWAY_URL: &str = "ws://127.0.0.1:18789";

/// Default timeout in milliseconds
pub const DEFAULT_TIMEOUT_MS: u64 = 30000;

/// Gateway RPC client
pub struct GatewayClient {
    url: String,
    timeout_ms: u64,
}

impl GatewayClient {
    /// Create a new client with default settings
    pub fn new() -> Self {
        Self {
            url: DEFAULT_GATEWAY_URL.to_string(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }

    /// Set the Gateway URL
    pub fn with_url(mut self, url: &str) -> Self {
        self.url = url.to_string();
        self
    }

    /// Set the timeout in milliseconds
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Call an RPC method and return the result
    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<T, CliError> {
        let result = self.call_raw(method, params).await?;
        serde_json::from_value(result).map_err(|e| CliError::InvalidResponse(e.to_string()))
    }

    /// Call an RPC method and return raw JSON value
    pub async fn call_raw(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, CliError> {
        // Connect to Gateway
        let (ws_stream, _) = timeout(
            Duration::from_millis(5000),
            connect_async(&self.url),
        )
        .await
        .map_err(|_| CliError::Timeout(5000))?
        .map_err(|e| CliError::ConnectionFailed(e.to_string()))?;

        let (mut write, mut read) = ws_stream.split();

        // Build JSON-RPC request
        let request = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or(json!({})),
            "id": 1
        });

        // Send request
        write
            .send(Message::Text(request.to_string()))
            .await
            .map_err(|e| CliError::ConnectionFailed(e.to_string()))?;

        // Wait for response with timeout
        let response = timeout(Duration::from_millis(self.timeout_ms), read.next())
            .await
            .map_err(|_| CliError::Timeout(self.timeout_ms))?
            .ok_or_else(|| CliError::InvalidResponse("Connection closed".to_string()))?
            .map_err(|e| CliError::ConnectionFailed(e.to_string()))?;

        // Parse response
        let text = response
            .to_text()
            .map_err(|e| CliError::InvalidResponse(e.to_string()))?;

        let json: Value = serde_json::from_str(text)?;

        // Check for RPC error
        if let Some(error) = json.get("error") {
            let message = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            return Err(CliError::RpcError(message.to_string()));
        }

        // Extract result
        json.get("result")
            .cloned()
            .or_else(|| json.get("payload").cloned())
            .ok_or_else(|| CliError::InvalidResponse("No result in response".to_string()))
    }
}

impl Default for GatewayClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_builder() {
        let client = GatewayClient::new()
            .with_url("ws://localhost:9999")
            .with_timeout(5000);

        assert_eq!(client.url, "ws://localhost:9999");
        assert_eq!(client.timeout_ms, 5000);
    }
}
```

**Step 2: Update mod.rs**

Ensure `pub mod client;` is in mod.rs.

**Step 3: Verify compilation**

Run: `cargo check`

**Step 4: Commit**

```bash
git add core/src/cli/client.rs core/src/cli/mod.rs
git commit -m "feat(cli): implement Gateway RPC client"
```

---

## Task 3: Implement Output Formatting

**Files:**
- Create: `core/src/cli/output.rs`

**Step 1: Create output.rs**

```rust
//! Output formatting utilities for CLI commands.

use serde::Serialize;
use std::io::{self, Write};

/// Output format for CLI commands
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable table format
    Table,
    /// Machine-readable JSON format
    Json,
}

impl OutputFormat {
    /// Determine format from --json flag
    pub fn from_json_flag(json: bool) -> Self {
        if json {
            OutputFormat::Json
        } else {
            OutputFormat::Table
        }
    }
}

/// Print data as JSON
pub fn print_json<T: Serialize>(data: &T) -> io::Result<()> {
    let json = serde_json::to_string_pretty(data)?;
    println!("{}", json);
    Ok(())
}

/// Print a simple key-value table
pub fn print_table(rows: &[(&str, &str)]) {
    let max_key_len = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);

    for (key, value) in rows {
        println!("{:width$}  {}", key, value, width = max_key_len);
    }
}

/// Print a list table with headers
pub fn print_list_table(headers: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        println!("(empty)");
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
    for (i, header) in headers.iter().enumerate() {
        if i > 0 {
            print!("  ");
        }
        print!("{:width$}", header.to_uppercase(), width = widths[i]);
    }
    println!();

    // Print separator
    for (i, width) in widths.iter().enumerate() {
        if i > 0 {
            print!("  ");
        }
        print!("{}", "-".repeat(*width));
    }
    println!();

    // Print rows
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                print!("  ");
            }
            if i < widths.len() {
                print!("{:width$}", cell, width = widths[i]);
            }
        }
        println!();
    }
}

/// Print success message
pub fn print_success(message: &str) {
    println!("✓ {}", message);
}

/// Print error message
pub fn print_error(message: &str) {
    eprintln!("✗ {}", message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_from_flag() {
        assert_eq!(OutputFormat::from_json_flag(true), OutputFormat::Json);
        assert_eq!(OutputFormat::from_json_flag(false), OutputFormat::Table);
    }
}
```

**Step 2: Verify and commit**

```bash
cargo check
git add core/src/cli/output.rs
git commit -m "feat(cli): add output formatting utilities"
```

---

## Task 4: Add Gateway Subcommand to Binary

**Files:**
- Modify: `core/src/bin/aleph_gateway.rs`

**Step 1: Add Gateway command enum variant**

Find the `Command` enum and add after `Plugins`:

```rust
    /// Gateway RPC tools
    Gateway {
        #[command(subcommand)]
        action: GatewayAction,
    },
```

**Step 2: Add GatewayAction enum**

Add after `PluginsAction`:

```rust
/// Gateway subcommands
#[derive(Subcommand, Debug)]
enum GatewayAction {
    /// Call an RPC method on the Gateway
    Call {
        /// RPC method name (e.g., "health", "config.get")
        method: String,

        /// JSON parameters
        #[arg(long, short = 'p')]
        params: Option<String>,

        /// Gateway WebSocket URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,

        /// Timeout in milliseconds
        #[arg(long, default_value = "30000")]
        timeout: u64,
    },
}
```

**Step 3: Add handler in main match**

Find the command match block and add:

```rust
Some(Command::Gateway { action }) => {
    handle_gateway_command(action).await?;
}
```

**Step 4: Implement handle_gateway_command**

Add the function:

```rust
#[cfg(feature = "gateway")]
async fn handle_gateway_command(action: GatewayAction) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::cli::{GatewayClient, print_json};

    match action {
        GatewayAction::Call { method, params, url, timeout } => {
            let client = GatewayClient::new()
                .with_url(&url)
                .with_timeout(timeout);

            let params_value: Option<serde_json::Value> = params
                .map(|p| serde_json::from_str(&p))
                .transpose()?;

            let result: serde_json::Value = client.call_raw(&method, params_value).await?;
            print_json(&result)?;
        }
    }

    Ok(())
}
```

**Step 5: Verify and commit**

```bash
cargo check
git add core/src/bin/aleph_gateway.rs
git commit -m "feat(cli): add gateway call command"
```

---

## Task 5: Add Config Subcommand

**Files:**
- Modify: `core/src/bin/aleph_gateway.rs`
- Create: `core/src/cli/config.rs`

**Step 1: Create cli/config.rs**

```rust
//! Config CLI command implementations.

use crate::cli::{CliError, GatewayClient, OutputFormat, print_json, print_table, print_success};
use serde_json::{json, Value};

/// Handle config get command
pub async fn handle_get(
    client: &GatewayClient,
    path: Option<String>,
    format: OutputFormat,
) -> Result<(), CliError> {
    let params = path.map(|p| json!({ "path": p }));
    let result: Value = client.call_raw("config.get", params).await?;

    match format {
        OutputFormat::Json => {
            print_json(&result)?;
        }
        OutputFormat::Table => {
            // Pretty print the config
            let json_str = serde_json::to_string_pretty(&result)?;
            println!("{}", json_str);
        }
    }

    Ok(())
}

/// Handle config set command
pub async fn handle_set(
    client: &GatewayClient,
    path: String,
    value: String,
) -> Result<(), CliError> {
    // Parse value as JSON, or treat as string
    let value_json: Value = serde_json::from_str(&value)
        .unwrap_or_else(|_| Value::String(value.clone()));

    // Build patch object from path
    let patch = build_patch_from_path(&path, value_json);

    client.call_raw("config.patch", Some(json!({ "patch": patch }))).await?;
    print_success(&format!("Set {} = {}", path, value));

    Ok(())
}

/// Handle config validate command
pub async fn handle_validate(client: &GatewayClient) -> Result<(), CliError> {
    let result: Value = client.call_raw("config.validate", None).await?;

    if let Some(valid) = result.get("valid").and_then(|v| v.as_bool()) {
        if valid {
            print_success("Configuration is valid");
        } else {
            eprintln!("Configuration has errors:");
            if let Some(errors) = result.get("errors").and_then(|e| e.as_array()) {
                for error in errors {
                    eprintln!("  - {}", error);
                }
            }
        }
    }

    Ok(())
}

/// Handle config reload command
pub async fn handle_reload(client: &GatewayClient) -> Result<(), CliError> {
    client.call_raw("config.reload", None).await?;
    print_success("Configuration reloaded");
    Ok(())
}

/// Handle config schema command
pub async fn handle_schema(
    client: &GatewayClient,
    output: Option<String>,
) -> Result<(), CliError> {
    let result: Value = client.call_raw("config.schema", None).await?;

    let schema = result.get("schema").cloned().unwrap_or(result);

    if let Some(path) = output {
        let content = serde_json::to_string_pretty(&schema)?;
        std::fs::write(&path, content)?;
        print_success(&format!("Schema written to {}", path));
    } else {
        print_json(&schema)?;
    }

    Ok(())
}

/// Handle config edit command
pub async fn handle_edit() -> Result<(), CliError> {
    let config_path = dirs::home_dir()
        .map(|h| h.join(".aether").join("config.toml"))
        .ok_or_else(|| CliError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Cannot find home directory",
        )))?;

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());

    let status = std::process::Command::new(&editor)
        .arg(&config_path)
        .status()?;

    if status.success() {
        print_success("Config file saved");
    }

    Ok(())
}

/// Build a nested JSON object from a dot-separated path
fn build_patch_from_path(path: &str, value: Value) -> Value {
    let parts: Vec<&str> = path.split('.').collect();
    let mut result = value;

    for part in parts.into_iter().rev() {
        result = json!({ part: result });
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_patch_from_path() {
        let patch = build_patch_from_path("general.language", json!("zh-Hans"));
        assert_eq!(patch, json!({ "general": { "language": "zh-Hans" } }));
    }

    #[test]
    fn test_build_patch_nested() {
        let patch = build_patch_from_path("providers.openai.model", json!("gpt-4o"));
        assert_eq!(patch, json!({ "providers": { "openai": { "model": "gpt-4o" } } }));
    }
}
```

**Step 2: Add Config command to binary**

Add to Command enum:

```rust
    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
```

Add ConfigAction enum:

```rust
/// Config subcommands
#[derive(Subcommand, Debug)]
enum ConfigAction {
    /// Get configuration (all or specific path)
    Get {
        /// Config path (e.g., "general.language")
        path: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
    /// Set a configuration value
    Set {
        /// Config path (e.g., "general.language")
        path: String,

        /// Value to set (JSON or string)
        value: String,

        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
    /// Edit configuration in editor
    Edit,
    /// Validate configuration
    Validate {
        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
    /// Reload configuration
    Reload {
        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
    /// Output JSON Schema
    Schema {
        /// Output file path
        #[arg(long, short = 'o')]
        output: Option<String>,

        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
}
```

**Step 3: Add handler**

```rust
Some(Command::Config { action }) => {
    handle_config_command(action).await?;
}
```

```rust
#[cfg(feature = "gateway")]
async fn handle_config_command(action: ConfigAction) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::cli::{GatewayClient, OutputFormat, config};

    match action {
        ConfigAction::Get { path, json, url } => {
            let client = GatewayClient::new().with_url(&url);
            let format = OutputFormat::from_json_flag(json);
            config::handle_get(&client, path, format).await?;
        }
        ConfigAction::Set { path, value, url } => {
            let client = GatewayClient::new().with_url(&url);
            config::handle_set(&client, path, value).await?;
        }
        ConfigAction::Edit => {
            config::handle_edit().await?;
        }
        ConfigAction::Validate { url } => {
            let client = GatewayClient::new().with_url(&url);
            config::handle_validate(&client).await?;
        }
        ConfigAction::Reload { url } => {
            let client = GatewayClient::new().with_url(&url);
            config::handle_reload(&client).await?;
        }
        ConfigAction::Schema { output, url } => {
            let client = GatewayClient::new().with_url(&url);
            config::handle_schema(&client, output).await?;
        }
    }

    Ok(())
}
```

**Step 4: Update cli/mod.rs**

```rust
pub mod config;
```

**Step 5: Verify and commit**

```bash
cargo check
git add core/src/cli/config.rs core/src/cli/mod.rs core/src/bin/aleph_gateway.rs
git commit -m "feat(cli): add config commands"
```

---

## Task 6: Add Channels Subcommand

**Files:**
- Create: `core/src/cli/channels.rs`
- Modify: `core/src/bin/aleph_gateway.rs`

**Step 1: Create cli/channels.rs**

```rust
//! Channels CLI command implementations.

use crate::cli::{CliError, GatewayClient, OutputFormat, print_json, print_list_table};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct ChannelInfo {
    name: String,
    #[serde(rename = "type")]
    channel_type: String,
    status: String,
    #[serde(default)]
    connected_at: Option<String>,
}

/// Handle channels list command
pub async fn handle_list(
    client: &GatewayClient,
    format: OutputFormat,
) -> Result<(), CliError> {
    let result: Value = client.call_raw("channels.list", None).await?;

    match format {
        OutputFormat::Json => {
            print_json(&result)?;
        }
        OutputFormat::Table => {
            let channels: Vec<ChannelInfo> = serde_json::from_value(
                result.get("channels").cloned().unwrap_or(result.clone())
            ).unwrap_or_default();

            if channels.is_empty() {
                println!("No channels configured");
                return Ok(());
            }

            let headers = &["Name", "Type", "Status", "Connected"];
            let rows: Vec<Vec<String>> = channels
                .iter()
                .map(|c| vec![
                    c.name.clone(),
                    c.channel_type.clone(),
                    c.status.clone(),
                    c.connected_at.clone().unwrap_or_else(|| "-".to_string()),
                ])
                .collect();

            print_list_table(headers, &rows);
        }
    }

    Ok(())
}

/// Handle channels status command
pub async fn handle_status(
    client: &GatewayClient,
    name: Option<String>,
    format: OutputFormat,
) -> Result<(), CliError> {
    let params = name.map(|n| json!({ "name": n }));
    let result: Value = client.call_raw("channels.status", params).await?;

    match format {
        OutputFormat::Json => {
            print_json(&result)?;
        }
        OutputFormat::Table => {
            let json_str = serde_json::to_string_pretty(&result)?;
            println!("{}", json_str);
        }
    }

    Ok(())
}
```

**Step 2: Add Channels command to binary**

Add to Command enum:

```rust
    /// Manage channels
    Channels {
        #[command(subcommand)]
        action: ChannelsAction,
    },
```

Add ChannelsAction enum:

```rust
/// Channels subcommands
#[derive(Subcommand, Debug)]
enum ChannelsAction {
    /// List all channels
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
    /// Get channel status
    Status {
        /// Channel name (optional, all if not specified)
        name: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
}
```

**Step 3: Add handler**

```rust
Some(Command::Channels { action }) => {
    handle_channels_command(action).await?;
}
```

```rust
#[cfg(feature = "gateway")]
async fn handle_channels_command(action: ChannelsAction) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::cli::{GatewayClient, OutputFormat, channels};

    match action {
        ChannelsAction::List { json, url } => {
            let client = GatewayClient::new().with_url(&url);
            let format = OutputFormat::from_json_flag(json);
            channels::handle_list(&client, format).await?;
        }
        ChannelsAction::Status { name, json, url } => {
            let client = GatewayClient::new().with_url(&url);
            let format = OutputFormat::from_json_flag(json);
            channels::handle_status(&client, name, format).await?;
        }
    }

    Ok(())
}
```

**Step 4: Update cli/mod.rs**

```rust
pub mod channels;
```

**Step 5: Verify and commit**

```bash
cargo check
git add core/src/cli/channels.rs core/src/cli/mod.rs core/src/bin/aleph_gateway.rs
git commit -m "feat(cli): add channels commands"
```

---

## Task 7: Add Cron Subcommand

**Files:**
- Create: `core/src/cli/cron.rs`
- Modify: `core/src/bin/aleph_gateway.rs`

**Step 1: Create cli/cron.rs**

```rust
//! Cron CLI command implementations.

use crate::cli::{CliError, GatewayClient, OutputFormat, print_json, print_list_table, print_success};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct CronJob {
    id: String,
    schedule: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    last_run: Option<String>,
    #[serde(default)]
    next_run: Option<String>,
    #[serde(default)]
    enabled: bool,
}

/// Handle cron list command
pub async fn handle_list(
    client: &GatewayClient,
    format: OutputFormat,
) -> Result<(), CliError> {
    let result: Value = client.call_raw("cron.list", None).await?;

    match format {
        OutputFormat::Json => {
            print_json(&result)?;
        }
        OutputFormat::Table => {
            let jobs: Vec<CronJob> = serde_json::from_value(
                result.get("jobs").cloned().unwrap_or(result.clone())
            ).unwrap_or_default();

            if jobs.is_empty() {
                println!("No cron jobs configured");
                return Ok(());
            }

            let headers = &["ID", "Schedule", "Description", "Last Run", "Next Run"];
            let rows: Vec<Vec<String>> = jobs
                .iter()
                .map(|j| vec![
                    j.id.clone(),
                    j.schedule.clone(),
                    j.description.clone().unwrap_or_else(|| "-".to_string()),
                    j.last_run.clone().unwrap_or_else(|| "-".to_string()),
                    j.next_run.clone().unwrap_or_else(|| "-".to_string()),
                ])
                .collect();

            print_list_table(headers, &rows);
        }
    }

    Ok(())
}

/// Handle cron status command
pub async fn handle_status(
    client: &GatewayClient,
    format: OutputFormat,
) -> Result<(), CliError> {
    let result: Value = client.call_raw("cron.status", None).await?;

    match format {
        OutputFormat::Json => {
            print_json(&result)?;
        }
        OutputFormat::Table => {
            let json_str = serde_json::to_string_pretty(&result)?;
            println!("{}", json_str);
        }
    }

    Ok(())
}

/// Handle cron run command
pub async fn handle_run(
    client: &GatewayClient,
    job_id: String,
) -> Result<(), CliError> {
    let params = json!({ "job_id": job_id });
    client.call_raw("cron.run", Some(params)).await?;
    print_success(&format!("Triggered job: {}", job_id));
    Ok(())
}
```

**Step 2: Add Cron command to binary**

Add to Command enum:

```rust
    /// Manage cron jobs
    Cron {
        #[command(subcommand)]
        action: CronAction,
    },
```

Add CronAction enum:

```rust
/// Cron subcommands
#[derive(Subcommand, Debug)]
enum CronAction {
    /// List cron jobs
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
    /// Get cron service status
    Status {
        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
    /// Trigger a cron job manually
    Run {
        /// Job ID to run
        job_id: String,

        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
}
```

**Step 3: Add handler**

```rust
Some(Command::Cron { action }) => {
    handle_cron_command(action).await?;
}
```

```rust
#[cfg(feature = "gateway")]
async fn handle_cron_command(action: CronAction) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::cli::{GatewayClient, OutputFormat, cron};

    match action {
        CronAction::List { json, url } => {
            let client = GatewayClient::new().with_url(&url);
            let format = OutputFormat::from_json_flag(json);
            cron::handle_list(&client, format).await?;
        }
        CronAction::Status { json, url } => {
            let client = GatewayClient::new().with_url(&url);
            let format = OutputFormat::from_json_flag(json);
            cron::handle_status(&client, format).await?;
        }
        CronAction::Run { job_id, url } => {
            let client = GatewayClient::new().with_url(&url);
            cron::handle_run(&client, job_id).await?;
        }
    }

    Ok(())
}
```

**Step 4: Update cli/mod.rs**

```rust
pub mod cron;
```

**Step 5: Verify and commit**

```bash
cargo check
git add core/src/cli/cron.rs core/src/cli/mod.rs core/src/bin/aleph_gateway.rs
git commit -m "feat(cli): add cron commands"
```

---

## Task 8: Add Missing RPC Handlers

**Files:**
- Modify: `core/src/gateway/handlers/channel.rs`
- Create or modify: `core/src/gateway/handlers/cron.rs`
- Modify: `core/src/gateway/handlers/mod.rs`

**Step 1: Check and add channels.list/status handlers**

Read `channel.rs` to see what exists, add if missing:

```rust
/// Handle channels.list
pub async fn handle_list(
    _params: Value,
    ctx: &HandlerContext,
) -> Result<Value, RpcError> {
    let channels = ctx.channel_registry.list_channels().await;
    Ok(json!({ "channels": channels }))
}

/// Handle channels.status
pub async fn handle_status(
    params: Value,
    ctx: &HandlerContext,
) -> Result<Value, RpcError> {
    let name = params.get("name").and_then(|n| n.as_str());
    let status = ctx.channel_registry.get_status(name).await;
    Ok(json!(status))
}
```

**Step 2: Create cron handlers if needed**

Check if `cron.rs` exists in handlers, if not create with basic handlers:

```rust
//! Cron job RPC handlers.

use serde_json::{json, Value};
use crate::gateway::RpcError;

/// Handle cron.list - list all cron jobs
pub async fn handle_list(_params: Value) -> Result<Value, RpcError> {
    // TODO: Integrate with actual cron service
    Ok(json!({
        "jobs": []
    }))
}

/// Handle cron.status - get cron service status
pub async fn handle_status(_params: Value) -> Result<Value, RpcError> {
    Ok(json!({
        "running": true,
        "job_count": 0
    }))
}

/// Handle cron.run - trigger a job manually
pub async fn handle_run(params: Value) -> Result<Value, RpcError> {
    let job_id = params.get("job_id")
        .and_then(|j| j.as_str())
        .ok_or_else(|| RpcError::invalid_params("job_id is required"))?;

    // TODO: Integrate with actual cron service
    Ok(json!({
        "triggered": job_id
    }))
}
```

**Step 3: Register handlers in mod.rs**

Add to handler registration:

```rust
registry.register("channels.list", handle_channels_list);
registry.register("channels.status", handle_channels_status);
registry.register("cron.list", handle_cron_list);
registry.register("cron.status", handle_cron_status);
registry.register("cron.run", handle_cron_run);
```

**Step 4: Verify and commit**

```bash
cargo check
git add core/src/gateway/handlers/
git commit -m "feat(gateway): add channels and cron RPC handlers"
```

---

## Task 9: Final Verification and Tests

**Step 1: Build the binary**

```bash
cd /Volumes/TBU4/Workspace/Aether/core
cargo build --features gateway --bin aleph-gateway
```

**Step 2: Test help output**

```bash
./target/debug/aleph-gateway --help
./target/debug/aleph-gateway gateway --help
./target/debug/aleph-gateway config --help
./target/debug/aleph-gateway channels --help
./target/debug/aleph-gateway cron --help
```

**Step 3: Run unit tests**

```bash
cargo test cli::
```

**Step 4: Final commit**

```bash
git add -A
git commit -m "feat(cli): complete CLI commands implementation

- gateway call: generic RPC invocation
- config: get/set/edit/validate/reload/schema
- channels: list/status
- cron: list/status/run

All commands support --json flag for machine-readable output."
```

---

## Success Criteria

- [ ] `aleph-gateway gateway call health` works
- [ ] `aleph-gateway config get` shows configuration
- [ ] `aleph-gateway config set general.language zh-Hans` updates config
- [ ] `aleph-gateway config schema` outputs JSON Schema
- [ ] `aleph-gateway channels list` shows channels
- [ ] `aleph-gateway cron list` shows jobs
- [ ] All commands support `--json` flag
- [ ] All commands support `--url` for custom Gateway
- [ ] Unit tests pass
- [ ] Help text is clear and complete
