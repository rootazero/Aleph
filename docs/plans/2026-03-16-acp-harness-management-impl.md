# ACP Harness Management — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add visual configuration and management for ACP CLI harnesses (Claude Code, Codex, Gemini, custom) in the Panel UI, with Gateway RPC handlers and dynamic harness registration.

**Architecture:** Extend existing `AcpHarnessManager` with dynamic registration, add Gateway RPC handlers following `rerank_config` pattern, add Panel settings page under Extensions tab following split-pane layout.

**Tech Stack:** Rust (alephcore), Leptos 0.8 WASM (panel), JSON-RPC 2.0, TOML config

---

### Task 1: Extend AcpConfig Data Model

**Files:**
- Modify: `src/config/types/acp.rs`

**Step 1: Expand AcpHarnessEntry to AcpHarnessConfig**

Replace the current minimal `AcpHarnessEntry` with the full configuration struct:

```rust
// src/config/types/acp.rs

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// AcpConfig
// =============================================================================

/// ACP harness management configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AcpConfig {
    /// Enable/disable ACP functionality
    #[serde(default)]
    pub enabled: bool,

    /// Registered ACP harnesses keyed by ID
    #[serde(default)]
    pub harnesses: HashMap<String, AcpHarnessEntry>,
}

// =============================================================================
// AcpHarnessEntry
// =============================================================================

/// Configuration for a single ACP harness
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AcpHarnessEntry {
    /// Human-readable display name
    #[serde(default)]
    pub display_name: String,

    /// Path to the harness executable (resolved from PATH if absent)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,

    /// CLI arguments for launching the harness
    #[serde(default)]
    pub args: Vec<String>,

    /// Execution mode
    #[serde(default)]
    pub mode: HarnessModeSerde,

    /// Output parsing format (Oneshot mode only)
    #[serde(default)]
    pub output_format: OutputFormatSerde,

    /// Additional environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Default working directory
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,

    /// Request timeout in seconds (default 300)
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,

    /// Whether this harness is enabled
    #[serde(default = "super::search::default_true")]
    pub enabled: bool,

    /// Preset identifier: "claude-code", "codex", "gemini", or None for custom
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
}

fn default_timeout() -> u64 { 300 }

impl Default for AcpHarnessEntry {
    fn default() -> Self {
        Self {
            display_name: String::new(),
            executable: None,
            args: Vec::new(),
            mode: HarnessModeSerde::default(),
            output_format: OutputFormatSerde::default(),
            env: HashMap::new(),
            cwd: None,
            timeout_seconds: 300,
            enabled: true,
            preset: None,
        }
    }
}

// =============================================================================
// HarnessModeSerde — serializable version of HarnessMode
// =============================================================================

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessModeSerde {
    /// Full ACP protocol over persistent stdio
    NativeAcp,
    /// Fresh process per prompt
    #[default]
    Oneshot,
}

// =============================================================================
// OutputFormatSerde — how to parse oneshot CLI output
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputFormatSerde {
    /// Return stdout as-is
    PlainText,
    /// Parse stdout as JSON, extract specified field
    Json {
        #[serde(default = "default_json_field")]
        field: String,
    },
}

fn default_json_field() -> String { "result".to_string() }

impl Default for OutputFormatSerde {
    fn default() -> Self {
        Self::PlainText
    }
}

// =============================================================================
// Preset defaults
// =============================================================================

impl AcpHarnessEntry {
    /// Create a preset entry for Claude Code
    pub fn preset_claude_code() -> Self {
        Self {
            display_name: "Claude Code".to_string(),
            executable: Some("claude".to_string()),
            args: vec!["--print".into(), "--output-format".into(), "json".into()],
            mode: HarnessModeSerde::Oneshot,
            output_format: OutputFormatSerde::Json { field: "result".to_string() },
            timeout_seconds: 300,
            enabled: true,
            preset: Some("claude-code".to_string()),
            ..Default::default()
        }
    }

    /// Create a preset entry for Codex
    pub fn preset_codex() -> Self {
        Self {
            display_name: "Codex".to_string(),
            executable: Some("codex".to_string()),
            args: vec!["exec".into()],
            mode: HarnessModeSerde::Oneshot,
            output_format: OutputFormatSerde::PlainText,
            timeout_seconds: 300,
            enabled: true,
            preset: Some("codex".to_string()),
            ..Default::default()
        }
    }

    /// Create a preset entry for Gemini
    pub fn preset_gemini() -> Self {
        Self {
            display_name: "Gemini".to_string(),
            executable: Some("gemini".to_string()),
            args: vec!["--acp".into()],
            mode: HarnessModeSerde::NativeAcp,
            output_format: OutputFormatSerde::PlainText,
            timeout_seconds: 300,
            enabled: true,
            preset: Some("gemini".to_string()),
            ..Default::default()
        }
    }

    /// Return all preset defaults keyed by ID
    pub fn all_presets() -> HashMap<String, AcpHarnessEntry> {
        let mut presets = HashMap::new();
        presets.insert("claude-code".to_string(), Self::preset_claude_code());
        presets.insert("codex".to_string(), Self::preset_codex());
        presets.insert("gemini".to_string(), Self::preset_gemini());
        presets
    }

    /// Preset IDs
    pub fn preset_ids() -> &'static [&'static str] {
        &["claude-code", "codex", "gemini"]
    }

    /// Check if an ID is a preset
    pub fn is_preset_id(id: &str) -> bool {
        Self::preset_ids().contains(&id)
    }
}
```

**Step 2: Run compile check**

Run: `cargo check -p alephcore 2>&1 | head -30`
Expected: Compilation errors in files that import the old `AcpHarnessEntry` — fix in next task.

**Step 3: Commit**

```
git add src/config/types/acp.rs
git commit -m "acp: expand AcpHarnessEntry with full harness configuration"
```

---

### Task 2: Add CustomHarness Implementation

**Files:**
- Create: `src/acp/harnesses/custom.rs`
- Modify: `src/acp/harnesses/mod.rs` (add `pub use custom::CustomHarness;`)
- Modify: `src/acp/harness.rs` (add conversion helpers between HarnessMode ↔ HarnessModeSerde)

**Step 1: Create custom.rs**

```rust
// src/acp/harnesses/custom.rs

//! Custom ACP harness adapter — user-defined CLI tools.

use async_trait::async_trait;
use tokio::process::Command;
use tracing::{debug, error};

use crate::acp::harness::{AcpHarness, HarnessMode};
use crate::acp::session::HarnessConfig;
use crate::config::types::acp::{AcpHarnessEntry, HarnessModeSerde, OutputFormatSerde};
use crate::error::{AlephError, Result};

use std::time::Duration;

/// ACP harness built from user configuration (AcpHarnessEntry).
///
/// Supports both NativeAcp and Oneshot modes with configurable
/// output parsing (PlainText or JSON field extraction).
pub struct CustomHarness {
    harness_id: String,
    config: AcpHarnessEntry,
}

impl CustomHarness {
    pub fn new(id: String, config: AcpHarnessEntry) -> Self {
        Self { harness_id: id, config }
    }

    fn executable(&self) -> &str {
        self.config.executable.as_deref()
            .unwrap_or(&self.harness_id)
    }

    fn parse_output(&self, stdout: &str) -> Result<String> {
        match &self.config.output_format {
            OutputFormatSerde::PlainText => Ok(stdout.trim().to_string()),
            OutputFormatSerde::Json { field } => {
                match serde_json::from_str::<serde_json::Value>(stdout) {
                    Ok(json) => {
                        if let Some(value) = json.get(field).and_then(|v| v.as_str()) {
                            Ok(value.to_string())
                        } else {
                            // Field not found, return full JSON
                            Ok(json.to_string())
                        }
                    }
                    Err(_) => {
                        // Not valid JSON, return raw text
                        Ok(stdout.trim().to_string())
                    }
                }
            }
        }
    }
}

#[async_trait]
impl AcpHarness for CustomHarness {
    fn id(&self) -> &str {
        &self.harness_id
    }

    fn display_name(&self) -> &str {
        &self.config.display_name
    }

    fn mode(&self) -> HarnessMode {
        match self.config.mode {
            HarnessModeSerde::NativeAcp => HarnessMode::NativeAcp,
            HarnessModeSerde::Oneshot => HarnessMode::Oneshot,
        }
    }

    fn build_config(&self, cwd: Option<&str>) -> HarnessConfig {
        HarnessConfig {
            executable: self.executable().to_string(),
            args: self.config.args.clone(),
            cwd: cwd.map(String::from).or_else(|| self.config.cwd.clone()),
            env: self.config.env.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            timeout: Duration::from_secs(self.config.timeout_seconds),
        }
    }

    async fn execute_oneshot(&self, prompt: &str, cwd: &str) -> Result<String> {
        let exe = self.executable();
        let mut cmd = Command::new(exe);

        // Add configured args, then the prompt
        for arg in &self.config.args {
            cmd.arg(arg);
        }
        cmd.arg(prompt);

        cmd.current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Set extra env vars
        for (k, v) in &self.config.env {
            cmd.env(k, v);
        }

        debug!(harness = %self.harness_id, "Spawning oneshot custom harness");

        let output = cmd.output().await.map_err(|e| {
            AlephError::tool(format!(
                "Failed to execute '{}': {}. Is it installed and in PATH?",
                exe, e
            ))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(harness = %self.harness_id, stderr = %stderr, "Custom harness CLI failed");
            return Err(AlephError::tool(format!(
                "'{}' exited with {}: {}",
                exe,
                output.status,
                stderr.chars().take(500).collect::<String>()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        self.parse_output(&stdout)
    }
}
```

**Step 2: Update harnesses/mod.rs**

Add line after existing exports:
```rust
mod custom;
pub use custom::CustomHarness;
```

**Step 3: Add HarnessMode ↔ HarnessModeSerde conversions in harness.rs**

Add after `HarnessMode` enum definition:

```rust
impl HarnessMode {
    pub fn to_serde(&self) -> crate::config::types::acp::HarnessModeSerde {
        match self {
            HarnessMode::NativeAcp => crate::config::types::acp::HarnessModeSerde::NativeAcp,
            HarnessMode::Oneshot => crate::config::types::acp::HarnessModeSerde::Oneshot,
        }
    }

    pub fn from_serde(s: &crate::config::types::acp::HarnessModeSerde) -> Self {
        match s {
            crate::config::types::acp::HarnessModeSerde::NativeAcp => HarnessMode::NativeAcp,
            crate::config::types::acp::HarnessModeSerde::Oneshot => HarnessMode::Oneshot,
        }
    }
}
```

**Step 4: Run compile check**

Run: `cargo check -p alephcore 2>&1 | head -30`
Expected: PASS (or errors from start/mod.rs AcpHarnessEntry usage — fix in Task 3)

**Step 5: Commit**

```
git add src/acp/harnesses/custom.rs src/acp/harnesses/mod.rs src/acp/harness.rs
git commit -m "acp: add CustomHarness for user-defined CLI tools"
```

---

### Task 3: Extend AcpHarnessManager with Dynamic Registration

**Files:**
- Modify: `src/acp/manager.rs`

**Step 1: Rewrite manager to support dynamic registration**

The manager needs:
1. `RwLock` around `harnesses` (was plain HashMap)
2. Hold `configs` map for introspection by RPC handlers
3. `register_harness` / `unregister_harness` / `update_harness` methods
4. Factory method to build harness from `AcpHarnessEntry`
5. Backward-compat: `with_config` still works, but now reads `AcpHarnessEntry` instead of `AcpManagerConfig`

Key changes to `AcpHarnessManager`:

```rust
use crate::acp::harnesses::CustomHarness;
use crate::config::types::acp::{AcpHarnessEntry, HarnessModeSerde};

pub struct AcpHarnessManager {
    harnesses: RwLock<HashMap<String, Box<dyn AcpHarness>>>,
    configs: RwLock<HashMap<String, AcpHarnessEntry>>,
    sessions: RwLock<HashMap<String, AcpSession>>,
}
```

New constructor from config entries:

```rust
impl AcpHarnessManager {
    /// Create manager from AcpHarnessEntry configs.
    /// For preset IDs, uses dedicated harness impls.
    /// For custom IDs, uses CustomHarness.
    pub fn from_entries(entries: HashMap<String, AcpHarnessEntry>) -> Self {
        let mut harnesses: HashMap<String, Box<dyn AcpHarness>> = HashMap::new();
        let mut configs: HashMap<String, AcpHarnessEntry> = HashMap::new();

        for (id, entry) in entries {
            if !entry.enabled { continue; }
            let harness = Self::build_harness(&id, &entry);
            harnesses.insert(id.clone(), harness);
            configs.insert(id, entry);
        }

        Self {
            harnesses: RwLock::new(harnesses),
            configs: RwLock::new(configs),
            sessions: RwLock::new(HashMap::new()),
        }
    }

    fn build_harness(id: &str, entry: &AcpHarnessEntry) -> Box<dyn AcpHarness> {
        let exe = entry.executable.clone();
        match entry.preset.as_deref() {
            Some("claude-code") => Box::new(ClaudeCodeHarness::new(exe)),
            Some("codex") => Box::new(CodexHarness::new(exe)),
            Some("gemini") => Box::new(GeminiHarness::new(exe)),
            _ => Box::new(CustomHarness::new(id.to_string(), entry.clone())),
        }
    }

    pub async fn register_harness(&self, id: String, entry: AcpHarnessEntry) -> Result<()> {
        let harness = Self::build_harness(&id, &entry);
        self.harnesses.write().await.insert(id.clone(), harness);
        self.configs.write().await.insert(id, entry);
        Ok(())
    }

    pub async fn unregister_harness(&self, id: &str) -> Result<()> {
        if AcpHarnessEntry::is_preset_id(id) {
            return Err(AlephError::tool("Cannot delete preset harness".into()));
        }
        // Kill active session if any
        if let Some(mut session) = self.sessions.write().await.remove(id) {
            session.kill().await;
        }
        self.harnesses.write().await.remove(id);
        self.configs.write().await.remove(id);
        Ok(())
    }

    pub async fn update_harness(&self, id: &str, entry: AcpHarnessEntry) -> Result<()> {
        // Kill active session (will be rebuilt on next use)
        if let Some(mut session) = self.sessions.write().await.remove(id) {
            session.kill().await;
        }
        let harness = Self::build_harness(id, &entry);
        self.harnesses.write().await.insert(id.to_string(), harness);
        self.configs.write().await.insert(id.to_string(), entry);
        Ok(())
    }

    pub async fn get_config(&self, id: &str) -> Option<AcpHarnessEntry> {
        self.configs.read().await.get(id).cloned()
    }

    pub async fn list_configs(&self) -> Vec<(String, AcpHarnessEntry)> {
        self.configs.read().await.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}
```

All existing methods (`harness_ids`, `has_harness`, `display_name`, `harness_mode`, `available_harnesses`, `ensure_session`, `prompt`, `cancel`, `shutdown_all`) must be updated to use `self.harnesses.read().await` / `self.harnesses.write().await` instead of direct HashMap access.

**Step 2: Keep backward compat with `with_config(AcpManagerConfig)`**

Convert the old `AcpManagerConfig` path to build `AcpHarnessEntry` entries and delegate to `from_entries`. Or deprecate `with_config` and update the caller in `start/mod.rs` to use `from_entries` directly.

**Step 3: Update start/mod.rs (lines 396-416)**

Replace the old `AcpManagerConfig`-based init with:

```rust
let acp_manager = {
    let app_cfg = app_config.read().await;
    if app_cfg.acp.enabled {
        use alephcore::acp::manager::AcpHarnessManager;
        use alephcore::config::types::acp::AcpHarnessEntry;

        // Merge preset defaults with user overrides
        let mut entries = AcpHarnessEntry::all_presets();
        for (id, user_entry) in &app_cfg.acp.harnesses {
            entries.insert(id.clone(), user_entry.clone());
        }

        let manager = Arc::new(AcpHarnessManager::from_entries(entries));
        if !args.daemon {
            println!("ACP harness manager initialized");
        }
        Some(manager)
    } else {
        None
    }
};
```

**Step 4: Run compile check and fix all errors**

Run: `cargo check -p alephcore 2>&1 | head -50`
Then: `cargo check --bin aleph 2>&1 | head -50`

**Step 5: Run existing tests**

Run: `cargo test -p alephcore --lib acp 2>&1 | tail -20`
Expected: Some tests may fail due to the RwLock change (sync → async). Update tests in `manager.rs` to be `#[tokio::test]`.

**Step 6: Commit**

```
git add src/acp/manager.rs src/bin/aleph/commands/start/mod.rs
git commit -m "acp: extend AcpHarnessManager with dynamic registration"
```

---

### Task 4: Add Gateway RPC Handlers

**Files:**
- Create: `src/gateway/handlers/acp_config.rs`
- Modify: `src/gateway/handlers/mod.rs` (add `pub mod acp_config;`)
- Modify: `src/bin/aleph/commands/start/builder/handlers.rs` (register new handlers)
- Modify: `src/bin/aleph/commands/start/mod.rs` (pass acp_manager to register_config_handlers)

**Step 1: Create acp_config.rs**

Follow `rerank_config.rs` as reference pattern. Handlers receive `Arc<AcpHarnessManager>` + `Arc<RwLock<Config>>` + `Arc<GatewayEventBus>`.

```rust
// src/gateway/handlers/acp_config.rs

//! ACP Harness Configuration RPC handlers
//!
//! | Method | Description |
//! |--------|-------------|
//! | acp.list | List all harnesses with availability status |
//! | acp.get | Get single harness config |
//! | acp.create | Add custom harness |
//! | acp.update | Update harness config |
//! | acp.delete | Delete custom harness |
//! | acp.test | Test harness availability |
//! | acp.set_enabled | Toggle harness enabled state |
//! | acp.presets | Return preset defaults |

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::acp::manager::AcpHarnessManager;
use crate::config::Config;
use crate::config::types::acp::AcpHarnessEntry;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Serialize)]
struct AcpHarnessInfo {
    id: String,
    display_name: String,
    executable: String,
    mode: String,
    enabled: bool,
    available: bool,
    preset: Option<String>,
    config: AcpHarnessEntry,
}

#[derive(Debug, Serialize)]
struct AcpTestResult {
    success: bool,
    message: String,
    duration_ms: u64,
}
```

Implement handlers:

- `handle_list`: Read configs from manager via `list_configs()`, check `is_available()` for each, merge preset defaults for unregistered presets, return `Vec<AcpHarnessInfo>`
- `handle_get`: Get single config via `get_config(id)`, check availability
- `handle_create`: Validate ID (`[a-z0-9-]`, not preset), register harness, save to config TOML, broadcast event
- `handle_update`: Update harness, save to config, broadcast event
- `handle_delete`: Validate not preset, unregister, remove from config, broadcast event
- `handle_test`: Run `is_available()` check + optional simple prompt test, return `AcpTestResult`
- `handle_set_enabled`: Toggle enabled, save, broadcast
- `handle_presets`: Return `AcpHarnessEntry::all_presets()`

Config persistence pattern (from rerank_config reference):
```rust
// Save to TOML
let mut cfg = config.write().await;
cfg.acp.harnesses.insert(id, entry);
if let Err(e) = cfg.save() {
    return JsonRpcResponse::error(request.id, INTERNAL_ERROR, &format!("Failed to save: {}", e));
}
// Broadcast
event_bus.publish(GatewayEvent::ConfigChanged(ConfigChangedEvent {
    topic: "config.acp.changed".to_string(),
    ..Default::default()
}));
```

**Step 2: Register in handlers/mod.rs**

Add `pub mod acp_config;` to the module declarations.

**Step 3: Wire handlers in handlers.rs**

In `register_config_handlers()`, add the `acp_manager` parameter and register handlers:

```rust
// ACP harness config
if let Some(ref acp) = acp_manager {
    register_handler!(server, "acp.list", acp_config::handle_list, acp, config);
    register_handler!(server, "acp.get", acp_config::handle_get, acp, config);
    register_handler!(server, "acp.create", acp_config::handle_create, acp, config, event_bus);
    register_handler!(server, "acp.update", acp_config::handle_update, acp, config, event_bus);
    register_handler!(server, "acp.delete", acp_config::handle_delete, acp, config, event_bus);
    register_handler!(server, "acp.test", acp_config::handle_test, acp);
    register_handler!(server, "acp.set_enabled", acp_config::handle_set_enabled, acp, config, event_bus);
    register_handler!(server, "acp.presets", acp_config::handle_presets);
}
```

**Step 4: Update register_config_handlers signature**

Add parameter: `acp_manager: Option<Arc<AcpHarnessManager>>`

Update the call site in `start/mod.rs` (line 447) to pass `acp_manager.clone()`.

**Step 5: Run compile check**

Run: `cargo check --bin aleph 2>&1 | head -30`

**Step 6: Commit**

```
git add src/gateway/handlers/acp_config.rs src/gateway/handlers/mod.rs \
  src/bin/aleph/commands/start/builder/handlers.rs \
  src/bin/aleph/commands/start/mod.rs
git commit -m "gateway: add ACP harness config RPC handlers"
```

---

### Task 5: Add Panel API Layer

**Files:**
- Modify: `apps/panel/src/api.rs` (add AcpApi struct and types)

**Step 1: Add ACP API types and methods**

Add at the end of api.rs, following the `RerankConfigApi` pattern:

```rust
// =============================================================================
// ACP Harness Config API
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpHarnessInfo {
    pub id: String,
    pub display_name: String,
    pub executable: String,
    pub mode: String,
    pub enabled: bool,
    pub available: bool,
    pub preset: Option<String>,
    pub config: AcpHarnessConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpHarnessConfig {
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub output_format: serde_json::Value,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpTestResult {
    pub success: bool,
    pub message: String,
    pub duration_ms: u64,
}

pub struct AcpApi;

impl AcpApi {
    pub async fn list(state: &DashboardState) -> Result<Vec<AcpHarnessInfo>, String> {
        let result = state.rpc_call("acp.list", Value::Null).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse ACP harness list: {}", e))
    }

    pub async fn get(state: &DashboardState, id: &str) -> Result<AcpHarnessInfo, String> {
        let result = state.rpc_call("acp.get", json!({ "id": id })).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse ACP harness: {}", e))
    }

    pub async fn create(state: &DashboardState, id: &str, config: AcpHarnessConfig) -> Result<AcpHarnessInfo, String> {
        let result = state.rpc_call("acp.create", json!({ "id": id, "config": config })).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to create ACP harness: {}", e))
    }

    pub async fn update(state: &DashboardState, id: &str, config: AcpHarnessConfig) -> Result<AcpHarnessInfo, String> {
        let result = state.rpc_call("acp.update", json!({ "id": id, "config": config })).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to update ACP harness: {}", e))
    }

    pub async fn delete(state: &DashboardState, id: &str) -> Result<(), String> {
        state.rpc_call("acp.delete", json!({ "id": id })).await?;
        Ok(())
    }

    pub async fn test(state: &DashboardState, id: &str) -> Result<AcpTestResult, String> {
        let result = state.rpc_call("acp.test", json!({ "id": id })).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse test result: {}", e))
    }

    pub async fn set_enabled(state: &DashboardState, id: &str, enabled: bool) -> Result<(), String> {
        state.rpc_call("acp.set_enabled", json!({ "id": id, "enabled": enabled })).await?;
        Ok(())
    }

    pub async fn presets(state: &DashboardState) -> Result<std::collections::HashMap<String, AcpHarnessConfig>, String> {
        let result = state.rpc_call("acp.presets", Value::Null).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse presets: {}", e))
    }
}
```

**Step 2: Compile check**

Run: `cd apps/panel && cargo check 2>&1 | head -20`

**Step 3: Commit**

```
git add apps/panel/src/api.rs
git commit -m "panel: add ACP harness config API layer"
```

---

### Task 6: Add Panel Settings Tab Registration

**Files:**
- Modify: `apps/panel/src/components/settings_sidebar.rs` (add Acp tab to enum + Extensions group)
- Modify: `apps/panel/src/views/settings/mod.rs` (add module + route)

**Step 1: Add SettingsTab::Acp variant**

In `settings_sidebar.rs`, add `Acp` to the `SettingsTab` enum. Then implement:
- `.path()` → `"/settings/acp"`
- `.label()` → `"ACP"`
- `.icon_svg()` → terminal/CLI icon SVG

**Step 2: Add to SETTINGS_GROUPS Extensions group**

```rust
SettingsGroup { label: "Extensions", tabs: &[Mcp, Plugins, Skills, Acp] },
```

**Step 3: Add module to settings/mod.rs**

```rust
pub mod acp_harnesses;
pub use acp_harnesses::AcpHarnessesView;
```

Add route in the settings router match.

**Step 4: Compile check**

Run: `cd apps/panel && cargo check 2>&1 | head -20`
Expected: Error — `acp_harnesses` module doesn't exist yet. That's OK, created in next task.

**Step 5: Commit**

```
git add apps/panel/src/components/settings_sidebar.rs apps/panel/src/views/settings/mod.rs
git commit -m "panel: register ACP tab in settings sidebar"
```

---

### Task 7: Create Panel ACP Harnesses Settings Page

**Files:**
- Create: `apps/panel/src/views/settings/acp_harnesses.rs`

**Step 1: Build the main component**

Follow `reranking_providers.rs` as reference. Structure:

```rust
#[component]
pub fn AcpHarnessesView() -> impl IntoView {
    // State signals
    let harnesses = RwSignal::new(Vec::<AcpHarnessInfo>::new());
    let selected_id = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(true);
    let show_add_form = RwSignal::new(false);

    // Load harness list on mount
    Effect::new(move || {
        spawn_local(async move {
            if let Ok(state) = use_context::<DashboardState>() {
                match AcpApi::list(&state).await {
                    Ok(list) => { harnesses.set(list); loading.set(false); }
                    Err(_) => { loading.set(false); }
                }
            }
        });
    });

    view! {
        <div class="flex h-full">
            // Left panel: preset cards + custom list + add button
            <div class="flex flex-col w-5/12 min-w-[400px] border-r border-border">
                <HarnessListPanel harnesses selected_id show_add_form />
            </div>
            // Right panel: detail or add form
            <div class="flex-1 overflow-y-auto">
                <Show when=move || show_add_form.get()>
                    <AddHarnessPanel harnesses show_add_form />
                </Show>
                <Show when=move || selected_id.get().is_some() && !show_add_form.get()>
                    <HarnessDetailPanel harnesses selected_id />
                </Show>
            </div>
        </div>
    }
}
```

**Step 2: Build HarnessListPanel**

Left panel with:
- Header: "ACP Agent CLI" + global description
- Section: "Preset CLI" — 3 cards in grid (Claude Code, Codex, Gemini)
  - Each card: icon (first letter colored circle), name, availability badge, enabled indicator
  - Click → set selected_id
- Section: "Custom CLI" — list of non-preset harnesses
- Button: "+ Add Custom CLI" → show_add_form = true

Card component:
```rust
#[component]
fn HarnessCard(info: AcpHarnessInfo, selected_id: RwSignal<Option<String>>, show_add_form: RwSignal<bool>) -> impl IntoView {
    let is_selected = move || selected_id.get().as_deref() == Some(&info.id);
    let id = info.id.clone();
    view! {
        <button
            class=move || format!("p-4 rounded-xl border {} text-left transition-all",
                if is_selected() { "border-primary bg-primary/5" } else { "border-border hover:border-primary/30" })
            on:click=move |_| { selected_id.set(Some(id.clone())); show_add_form.set(false); }
        >
            // Icon circle + name
            // Availability badge: green "Installed" or gray "Not Installed"
            // Enabled indicator dot
        </button>
    }
}
```

**Step 3: Build HarnessDetailPanel**

Right panel showing selected harness config:
- Header: display name + availability status badge
- Card "CONFIGURATION":
  - Executable path (text input)
  - Mode (dropdown: Oneshot / NativeAcp) — disabled for presets
  - Timeout (number input, seconds)
- Card "ADVANCED" (collapsible, default collapsed):
  - Args (tag list or textarea, comma-separated)
  - Output Format (dropdown: PlainText / Json) — shown only for Oneshot
    - If Json: field name input
  - Environment Variables (key-value pairs with add/remove)
  - Working Directory (text input)
- Action buttons:
  - "Test Connection" — calls `AcpApi::test(id)`, shows toast
  - "Enable/Disable" toggle — calls `AcpApi::set_enabled(id, bool)`
  - "Delete" (custom only) — calls `AcpApi::delete(id)`, confirmation dialog

**Step 4: Build AddHarnessPanel**

Similar to detail panel but with:
- ID field (text input, validated `[a-z0-9-]`)
- Display Name field
- All config fields
- Mode selector determines which fields are shown
- "Save" button → `AcpApi::create(id, config)`

**Step 5: Compile check**

Run: `cd apps/panel && cargo check 2>&1 | head -30`

**Step 6: Build WASM**

Run: `just build-panel` or equivalent WASM build command

**Step 7: Commit**

```
git add apps/panel/src/views/settings/acp_harnesses.rs
git commit -m "panel: add ACP harnesses settings page with split-pane layout"
```

---

### Task 8: Integration Testing

**Files:**
- Modify: `src/acp/manager.rs` (update existing tests)

**Step 1: Update manager tests for async API**

Existing tests used sync HashMap access. Convert to `#[tokio::test]` and use `.await`:

```rust
#[tokio::test]
async fn test_manager_registers_harnesses() {
    let entries = AcpHarnessEntry::all_presets();
    let manager = AcpHarnessManager::from_entries(entries);
    let ids = manager.harness_ids().await;
    assert!(ids.contains(&"claude-code".to_string()));
    assert!(ids.contains(&"codex".to_string()));
    assert!(ids.contains(&"gemini".to_string()));
}

#[tokio::test]
async fn test_dynamic_register_unregister() {
    let manager = AcpHarnessManager::from_entries(AcpHarnessEntry::all_presets());

    // Register custom harness
    let custom = AcpHarnessEntry {
        display_name: "My CLI".to_string(),
        executable: Some("my-cli".to_string()),
        mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        enabled: true,
        ..Default::default()
    };
    manager.register_harness("my-cli".to_string(), custom).await.unwrap();
    assert!(manager.has_harness("my-cli").await);

    // Unregister
    manager.unregister_harness("my-cli").await.unwrap();
    assert!(!manager.has_harness("my-cli").await);
}

#[tokio::test]
async fn test_cannot_delete_preset() {
    let manager = AcpHarnessManager::from_entries(AcpHarnessEntry::all_presets());
    let result = manager.unregister_harness("claude-code").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_update_harness() {
    let manager = AcpHarnessManager::from_entries(AcpHarnessEntry::all_presets());
    let mut entry = manager.get_config("claude-code").await.unwrap();
    entry.timeout_seconds = 600;
    manager.update_harness("claude-code", entry).await.unwrap();
    let updated = manager.get_config("claude-code").await.unwrap();
    assert_eq!(updated.timeout_seconds, 600);
}
```

**Step 2: Run all ACP tests**

Run: `cargo test -p alephcore --lib acp 2>&1 | tail -30`
Expected: All pass

**Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -10`
Expected: Pre-existing failures only (markdown_skill tests)

**Step 4: Commit**

```
git add src/acp/manager.rs
git commit -m "acp: update manager tests for async dynamic registration"
```

---

### Task 9: End-to-End Verification

**Step 1: Full build**

Run: `cargo build --bin aleph 2>&1 | tail -10`

**Step 2: WASM build**

Run: `just build-panel` or equivalent

**Step 3: Start server and verify**

Run: `cargo run --bin aleph` — verify ACP manager initializes, no panics.

**Step 4: Test RPC endpoints manually (if server is running)**

```bash
# List harnesses
curl -X POST http://localhost:3000/rpc -d '{"jsonrpc":"2.0","id":1,"method":"acp.list"}'

# Get presets
curl -X POST http://localhost:3000/rpc -d '{"jsonrpc":"2.0","id":2,"method":"acp.presets"}'

# Test a harness
curl -X POST http://localhost:3000/rpc -d '{"jsonrpc":"2.0","id":3,"method":"acp.test","params":{"id":"claude-code"}}'
```

**Step 5: Final commit**

If any fixes were needed, commit them.
