# ACP Probe Tests — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build 65 probe tests across 7 layers to verify the ACP harness management system at production quality.

**Architecture:** Two-layer testing — P1-P6 use in-process mocks (trait mock + mock scripts) for fast deterministic tests; P7 spawns a real Aleph server for E2E RPC verification via WebSocket. Follows existing probe patterns (provider_rpc_probe, session_probe).

**Tech Stack:** Rust, tokio, serial_test, serde_json, tempfile. Mock scripts in bash/python.

---

### Task 1: Scaffold Probe Structure + Mock Scripts

**Files:**
- Create: `tests/acp_probe.rs`
- Create: `tests/acp_probe/mock_scripts/mock_claude.sh`
- Create: `tests/acp_probe/mock_scripts/mock_codex.sh`
- Create: `tests/acp_probe/mock_scripts/mock_crash.sh`
- Create: `tests/acp_probe/mock_scripts/mock_timeout.sh`
- Create: `tests/acp_probe/mock_scripts/mock_env_echo.sh`
- Create: `tests/acp_probe/mock_scripts/mock_cwd_echo.sh`

**Step 1: Create entry point**

```rust
// tests/acp_probe.rs
#[allow(dead_code)]
mod acp_probe {
    pub mod mock_harness;
    pub mod p1_config_and_presets;
    pub mod p2_manager_lifecycle;
    pub mod p3_custom_harness;
    pub mod p4_rpc_handlers;
    pub mod p5_tool_execution;
    pub mod p6_error_paths;
    // P7 is separate — see Task 8
}
```

**Step 2: Create mock scripts**

`mock_claude.sh`:
```bash
#!/bin/bash
# Simulates Claude Code CLI oneshot: JSON output with "result" field
prompt="$*"
echo "{\"type\":\"result\",\"result\":\"echo: ${prompt}\"}"
```

`mock_codex.sh`:
```bash
#!/bin/bash
# Simulates Codex CLI oneshot: plain text output
echo "codex response: $*"
```

`mock_crash.sh`:
```bash
#!/bin/bash
echo "fatal error" >&2
exit 1
```

`mock_timeout.sh`:
```bash
#!/bin/bash
sleep 300
```

`mock_env_echo.sh`:
```bash
#!/bin/bash
echo "TEST_VAR=${TEST_VAR}"
```

`mock_cwd_echo.sh`:
```bash
#!/bin/bash
pwd
```

**Step 3: chmod +x all scripts**

```bash
chmod +x tests/acp_probe/mock_scripts/*.sh
```

**Step 4: Create placeholder mock_harness.rs**

```rust
// tests/acp_probe/mock_harness.rs
// Will be filled in Task 2
```

Create placeholder files for p1 through p6 (empty modules) so entry point compiles.

**Step 5: Verify structure compiles**

Run: `cargo test -p alephcore --test acp_probe --no-run 2>&1 | tail -5`
Expected: Compiles (0 tests found)

**Step 6: Commit**

```
git add tests/acp_probe.rs tests/acp_probe/
git commit -m "test(acp): scaffold probe test structure with mock scripts"
```

---

### Task 2: MockAcpHarness Implementation

**Files:**
- Create: `tests/acp_probe/mock_harness.rs`

**Step 1: Implement MockAcpHarness**

```rust
// tests/acp_probe/mock_harness.rs
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;

use alephcore::acp::harness::{AcpHarness, HarnessMode};
use alephcore::acp::session::HarnessConfig;
use alephcore::error::{AlephError, Result};

pub struct MockAcpHarness {
    id: String,
    name: String,
    mode: HarnessMode,
    available: AtomicBool,
    failing: AtomicBool,
    responses: Mutex<VecDeque<String>>,
    default_response: String,
    pub call_count: AtomicU64,
    pub last_prompt: Mutex<Option<String>>,
}

impl MockAcpHarness {
    pub fn new(id: &str, name: &str, mode: HarnessMode) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            mode,
            available: AtomicBool::new(true),
            failing: AtomicBool::new(false),
            responses: Mutex::new(VecDeque::new()),
            default_response: format!("mock response from {}", id),
            call_count: AtomicU64::new(0),
            last_prompt: Mutex::new(None),
        }
    }

    pub fn oneshot(id: &str, name: &str) -> Self {
        Self::new(id, name, HarnessMode::Oneshot)
    }

    pub fn native_acp(id: &str, name: &str) -> Self {
        Self::new(id, name, HarnessMode::NativeAcp)
    }

    pub fn set_available(&self, available: bool) {
        self.available.store(available, Ordering::SeqCst);
    }

    pub fn set_failing(&self) {
        self.failing.store(true, Ordering::SeqCst);
    }

    pub fn enqueue_response(&self, response: &str) {
        self.responses.lock().unwrap().push_back(response.to_string());
    }

    pub fn calls(&self) -> u64 {
        self.call_count.load(Ordering::SeqCst)
    }

    pub fn was_called(&self) -> bool {
        self.calls() > 0
    }

    pub fn last_prompt_text(&self) -> Option<String> {
        self.last_prompt.lock().unwrap().clone()
    }
}

#[async_trait]
impl AcpHarness for MockAcpHarness {
    fn id(&self) -> &str { &self.id }
    fn display_name(&self) -> &str { &self.name }
    fn mode(&self) -> HarnessMode { self.mode }

    fn build_config(&self, cwd: Option<&str>) -> HarnessConfig {
        HarnessConfig {
            executable: format!("mock-{}", self.id),
            args: vec![],
            cwd: cwd.map(String::from),
            ..Default::default()
        }
    }

    async fn is_available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    async fn execute_oneshot(&self, prompt: &str, _cwd: &str) -> Result<String> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        *self.last_prompt.lock().unwrap() = Some(prompt.to_string());

        if self.failing.load(Ordering::SeqCst) {
            return Err(AlephError::tool("Mock harness failing".into()));
        }

        let response = self.responses.lock().unwrap().pop_front()
            .unwrap_or_else(|| self.default_response.clone());
        Ok(response)
    }
}
```

**Step 2: Add helper to resolve mock script paths**

```rust
/// Resolve path to a mock script in the test fixtures directory.
pub fn mock_script_path(name: &str) -> String {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .unwrap_or_else(|_| ".".to_string());
    format!("{}/tests/acp_probe/mock_scripts/{}", manifest, name)
}
```

**Step 3: Verify compiles**

Run: `cargo test -p alephcore --test acp_probe --no-run 2>&1 | tail -5`

**Step 4: Commit**

```
git add tests/acp_probe/mock_harness.rs
git commit -m "test(acp): add MockAcpHarness with response queue and call tracking"
```

---

### Task 3: P1 — Config & Presets Tests

**Files:**
- Create: `tests/acp_probe/p1_config_and_presets.rs`

**Step 1: Write all 7 tests**

```rust
// tests/acp_probe/p1_config_and_presets.rs
use alephcore::config::types::acp::*;
use std::collections::HashMap;

#[test]
fn p1_01_preset_defaults_complete() {
    let claude = AcpHarnessEntry::preset_claude_code();
    assert_eq!(claude.executable.as_deref(), Some("claude"));
    assert_eq!(claude.mode, HarnessModeSerde::Oneshot);
    assert!(matches!(claude.output_format, OutputFormatSerde::Json { .. }));

    let codex = AcpHarnessEntry::preset_codex();
    assert_eq!(codex.executable.as_deref(), Some("codex"));
    assert_eq!(codex.mode, HarnessModeSerde::Oneshot);
    assert!(matches!(codex.output_format, OutputFormatSerde::PlainText));

    let gemini = AcpHarnessEntry::preset_gemini();
    assert_eq!(gemini.executable.as_deref(), Some("gemini"));
    assert_eq!(gemini.mode, HarnessModeSerde::NativeAcp);
}

#[test]
fn p1_02_all_presets_returns_three() {
    let presets = AcpHarnessEntry::all_presets();
    assert_eq!(presets.len(), 3);
    // Verify hyphenated keys
    let keys: Vec<String> = presets.iter().map(|(k, _)| k.clone()).collect();
    assert!(keys.contains(&"claude-code".to_string()));
    assert!(keys.contains(&"codex".to_string()));
    assert!(keys.contains(&"gemini".to_string()));
}

#[test]
fn p1_03_is_preset_id() {
    assert!(AcpHarnessEntry::is_preset_id("claude-code"));
    assert!(AcpHarnessEntry::is_preset_id("codex"));
    assert!(AcpHarnessEntry::is_preset_id("gemini"));
    assert!(!AcpHarnessEntry::is_preset_id("my-custom-cli"));
    assert!(!AcpHarnessEntry::is_preset_id(""));
}

#[test]
fn p1_04_harness_mode_serde_roundtrip() {
    // JSON roundtrip
    for mode in [HarnessModeSerde::NativeAcp, HarnessModeSerde::Oneshot] {
        let json = serde_json::to_string(&mode).unwrap();
        let back: HarnessModeSerde = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mode);
    }
}

#[test]
fn p1_05_output_format_serde_roundtrip() {
    let plain = OutputFormatSerde::PlainText;
    let json_fmt = OutputFormatSerde::Json { field: "result".into() };

    for fmt in [&plain, &json_fmt] {
        let json = serde_json::to_string(fmt).unwrap();
        let back: OutputFormatSerde = serde_json::from_str(&json).unwrap();
        // Verify roundtrip (compare serialized forms since no PartialEq)
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }
}

#[test]
fn p1_06_config_merge_user_override() {
    let mut presets = AcpHarnessEntry::all_presets()
        .into_iter().collect::<HashMap<_, _>>();

    // User overrides Claude executable and timeout
    let mut user_override = AcpHarnessEntry::preset_claude_code();
    user_override.executable = Some("/custom/path/claude".to_string());
    user_override.timeout_seconds = 600;
    presets.insert("claude-code".to_string(), user_override);

    let entry = presets.get("claude-code").unwrap();
    assert_eq!(entry.executable.as_deref(), Some("/custom/path/claude"));
    assert_eq!(entry.timeout_seconds, 600);
    // Other presets unchanged
    assert_eq!(presets.get("codex").unwrap().executable.as_deref(), Some("codex"));
}

#[test]
fn p1_07_default_values_sensible() {
    let entry = AcpHarnessEntry::default();
    assert_eq!(entry.timeout_seconds, 300);
    assert!(entry.enabled);
    assert!(entry.args.is_empty());
    assert!(entry.env.is_empty());
    assert!(entry.preset.is_none());
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --test acp_probe p1_ -- --nocapture 2>&1 | tail -15`
Expected: 7 passed

**Step 3: Commit**

```
git add tests/acp_probe/p1_config_and_presets.rs
git commit -m "test(acp): P1 config and presets probe tests (7 tests)"
```

---

### Task 4: P2 — Manager Lifecycle Tests

**Files:**
- Create: `tests/acp_probe/p2_manager_lifecycle.rs`

**Step 1: Write all 10 tests**

Tests use `AcpHarnessManager::from_entries()` with real `AcpHarnessEntry` configs. For mode-routing tests (p2_08), need to use mock scripts as executables since the manager calls `execute_oneshot` on real harness impls.

Key patterns:
- All tests are `#[tokio::test]` (manager methods are async)
- Use `AcpHarnessEntry::all_presets()` as base
- Custom entries use mock script paths from `mock_harness::mock_script_path()`
- Test dynamic register/unregister/update via manager methods

```rust
use alephcore::acp::manager::AcpHarnessManager;
use alephcore::acp::harness::HarnessMode;
use alephcore::config::types::acp::*;
use std::collections::HashMap;

fn make_presets() -> HashMap<String, AcpHarnessEntry> {
    AcpHarnessEntry::all_presets().into_iter().collect()
}

#[tokio::test]
async fn p2_01_from_entries_registers_all() { ... }
// ... etc for all 10 tests
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --test acp_probe p2_ -- --nocapture 2>&1 | tail -15`
Expected: 10 passed

**Step 3: Commit**

```
git add tests/acp_probe/p2_manager_lifecycle.rs
git commit -m "test(acp): P2 manager lifecycle probe tests (10 tests)"
```

---

### Task 5: P3 — CustomHarness Tests

**Files:**
- Create: `tests/acp_probe/p3_custom_harness.rs`

**Step 1: Write all 10 tests**

These tests create `CustomHarness` instances pointing to mock scripts and exercise real process spawn + output parsing.

Key patterns:
- Use `mock_script_path("mock_claude.sh")` as executable
- Create `AcpHarnessEntry` with appropriate mode/output_format
- Call `harness.execute_oneshot(prompt, cwd).await`
- Verify parsed output matches expected format
- Use `tempfile::TempDir` for cwd tests

```rust
use alephcore::acp::harnesses::CustomHarness;
use alephcore::acp::harness::AcpHarness;
use alephcore::config::types::acp::*;
use super::mock_harness::mock_script_path;
use std::collections::HashMap;

fn make_oneshot_entry(script: &str, format: OutputFormatSerde) -> AcpHarnessEntry {
    AcpHarnessEntry {
        display_name: "Test CLI".to_string(),
        executable: Some(mock_script_path(script)),
        args: vec![],  // scripts take prompt as positional args
        mode: HarnessModeSerde::Oneshot,
        output_format: format,
        enabled: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn p3_01_oneshot_plaintext() {
    let entry = make_oneshot_entry("mock_codex.sh", OutputFormatSerde::PlainText);
    let harness = CustomHarness::new("test-codex".into(), entry);
    let result = harness.execute_oneshot("hello world", "/tmp").await.unwrap();
    assert!(result.contains("codex response:"));
    assert!(result.contains("hello world"));
}

#[tokio::test]
async fn p3_02_oneshot_json_extract_field() {
    let entry = make_oneshot_entry("mock_claude.sh",
        OutputFormatSerde::Json { field: "result".into() });
    let harness = CustomHarness::new("test-claude".into(), entry);
    let result = harness.execute_oneshot("test prompt", "/tmp").await.unwrap();
    assert!(result.contains("echo:"));
    assert!(result.contains("test prompt"));
}

// ... p3_03 through p3_10 following same pattern
```

For `p3_05_oneshot_with_env_vars`:
```rust
#[tokio::test]
async fn p3_05_oneshot_with_env_vars() {
    let mut entry = make_oneshot_entry("mock_env_echo.sh", OutputFormatSerde::PlainText);
    entry.env.insert("TEST_VAR".to_string(), "probe_value".to_string());
    let harness = CustomHarness::new("env-test".into(), entry);
    let result = harness.execute_oneshot("ignored", "/tmp").await.unwrap();
    assert!(result.contains("TEST_VAR=probe_value"));
}
```

For `p3_07_oneshot_process_crash`:
```rust
#[tokio::test]
async fn p3_07_oneshot_process_crash() {
    let entry = make_oneshot_entry("mock_crash.sh", OutputFormatSerde::PlainText);
    let harness = CustomHarness::new("crash-test".into(), entry);
    let result = harness.execute_oneshot("test", "/tmp").await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("exited with"));
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --test acp_probe p3_ -- --nocapture 2>&1 | tail -15`
Expected: 10 passed

**Step 3: Commit**

```
git add tests/acp_probe/p3_custom_harness.rs
git commit -m "test(acp): P3 custom harness probe tests with mock scripts (10 tests)"
```

---

### Task 6: P4 — RPC Handler Integration Tests

**Files:**
- Create: `tests/acp_probe/p4_rpc_handlers.rs`

**Step 1: Write all 13 tests**

These test the Gateway RPC handlers with a real `AcpHarnessManager` and real `Config`. They verify config persistence and event broadcasting.

Key patterns:
- Follow `acp_config.rs` inline test setup but with more thorough assertions
- Use `tempfile::TempDir` for config persistence
- Verify `Config.acp.harnesses` state after mutations
- Verify EventBus receives "config.acp.changed" events

Setup helpers:
```rust
use alephcore::acp::manager::AcpHarnessManager;
use alephcore::config::Config;
use alephcore::config::types::acp::AcpHarnessEntry;
use alephcore::gateway::event_bus::GatewayEventBus;
use alephcore::gateway::handlers::acp_config;
use alephcore::gateway::protocol::JsonRpcRequest;
use serde_json::json;
use serial_test::serial;
use std::sync::Arc;
use tokio::sync::RwLock;

fn setup() -> (Arc<AcpHarnessManager>, Arc<RwLock<Config>>, Arc<GatewayEventBus>) {
    let entries = AcpHarnessEntry::all_presets().into_iter().collect();
    let manager = Arc::new(AcpHarnessManager::from_entries(entries));
    let config = Arc::new(RwLock::new(Config::default()));
    let event_bus = Arc::new(GatewayEventBus::new());
    (manager, config, event_bus)
}
```

Each test calls the handler function directly (no server needed):
```rust
#[tokio::test]
#[serial]
async fn p4_05_create_custom_persists_to_config() {
    let (manager, config, event_bus) = setup();
    let request = JsonRpcRequest::with_id("acp.create", Some(json!({
        "id": "my-tool",
        "config": {
            "display_name": "My Tool",
            "executable": "/usr/bin/my-tool",
            "mode": "oneshot"
        }
    })), json!(1));
    let response = acp_config::handle_create(request, manager.clone(), config.clone(), event_bus).await;
    assert!(response.is_success());

    // Verify persisted in config
    let cfg = config.read().await;
    assert!(cfg.acp.harnesses.contains_key("my-tool"));

    // Verify registered in manager
    assert!(manager.has_harness("my-tool").await);
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --test acp_probe p4_ -- --nocapture --test-threads=1 2>&1 | tail -15`
Expected: 13 passed

**Step 3: Commit**

```
git add tests/acp_probe/p4_rpc_handlers.rs
git commit -m "test(acp): P4 RPC handler integration probe tests (13 tests)"
```

---

### Task 7: P5 — Tool Execution + P6 — Error Paths

**Files:**
- Create: `tests/acp_probe/p5_tool_execution.rs`
- Create: `tests/acp_probe/p6_error_paths.rs`

**Step 1: Write P5 (7 tests)**

Tests verify that ACP delegate tools correctly route to the manager. Since tools call `manager.prompt()`, we test with a real manager pointing at mock scripts.

```rust
use alephcore::builtin_tools::acp_tools::*;
use alephcore::acp::manager::AcpHarnessManager;
use alephcore::config::types::acp::AcpHarnessEntry;
use super::mock_harness::mock_script_path;
use std::collections::HashMap;
use std::sync::Arc;

fn make_manager_with_mock_scripts() -> Arc<AcpHarnessManager> {
    let mut entries: HashMap<String, AcpHarnessEntry> = AcpHarnessEntry::all_presets()
        .into_iter().collect();
    // Override executables with mock scripts
    entries.get_mut("claude-code").unwrap().executable = Some(mock_script_path("mock_claude.sh"));
    entries.get_mut("codex").unwrap().executable = Some(mock_script_path("mock_codex.sh"));
    // gemini needs NativeAcp mock — skip for tool tests
    Arc::new(AcpHarnessManager::from_entries(entries))
}
```

**Step 2: Write P6 (8 tests)**

Error path tests exercise failure scenarios:
- `p6_01_oneshot_timeout`: Use mock_timeout.sh with 2-second timeout on CustomHarness
- `p6_06_concurrent_register_unregister`: Spawn multiple tokio tasks doing concurrent register/unregister
- `p6_07_shutdown_all_kills_sessions`: Create manager, spawn NativeAcp session (if possible), call shutdown_all
- `p6_08_manager_prompt_unknown_harness`: Call manager.prompt("nonexistent", ...) → error

```rust
#[tokio::test]
async fn p6_01_oneshot_timeout() {
    let mut entry = AcpHarnessEntry {
        executable: Some(mock_script_path("mock_timeout.sh")),
        mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        timeout_seconds: 2,  // 2 second timeout
        enabled: true,
        ..Default::default()
    };
    let harness = CustomHarness::new("timeout-test".into(), entry);
    let result = harness.execute_oneshot("test", "/tmp").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().to_lowercase().contains("timeout")
        || result.unwrap_err().to_string().contains("timed out"));
}
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --test acp_probe p5_ p6_ -- --nocapture 2>&1 | tail -15`
Expected: 15 passed

**Step 4: Commit**

```
git add tests/acp_probe/p5_tool_execution.rs tests/acp_probe/p6_error_paths.rs
git commit -m "test(acp): P5 tool execution + P6 error paths probe tests (15 tests)"
```

---

### Task 8: P7 — Real Server RPC Probe

**Files:**
- Modify: `tests/acp_probe.rs` (add p7 module conditionally)
- Create: `tests/acp_probe/p7_rpc_server_probe.rs`

**Step 1: Implement server harness**

Reuse the `provider_rpc_probe` harness pattern. Either import it or create a lightweight copy. The server needs `[acp] enabled = true` in its config.

```rust
// tests/acp_probe/p7_rpc_server_probe.rs

use serde_json::{json, Value};
use serial_test::serial;
use tokio::sync::OnceCell;
// Reuse or adapt AlephTestServer from provider_rpc_probe
```

If the `AlephTestServer` type from provider_rpc_probe isn't directly importable (different test binary), create a minimal version following the same pattern:
1. Find free port
2. Write config.toml with `[acp]\nenabled = true` to TempDir
3. Spawn aleph binary
4. Poll TCP until ready
5. Provide `rpc_call`/`rpc_ok`/`rpc_err` helpers

**Step 2: Write all 10 tests**

All tests marked `#[serial]`. Key test: `p7_04_create_update_delete_cycle` does full CRUD in one function.

```rust
#[tokio::test]
#[serial]
async fn p7_01_list_returns_presets() {
    let server = get_server().await;
    let result = server.rpc_ok("acp.list", json!({})).await;
    let list = result.as_array().unwrap();
    assert!(list.len() >= 3);
    // Verify schema: each entry has id, display_name, mode, enabled, available
    for entry in list {
        assert!(entry.get("id").is_some());
        assert!(entry.get("display_name").is_some());
        assert!(entry.get("mode").is_some());
        assert!(entry.get("enabled").is_some());
        assert!(entry.get("available").is_some());
    }
}

#[tokio::test]
#[serial]
async fn p7_04_create_update_delete_cycle() {
    let server = get_server().await;
    let test_id = "probe-test-cli";

    // Create
    let created = server.rpc_ok("acp.create", json!({
        "id": test_id,
        "config": {
            "display_name": "Probe Test CLI",
            "executable": "/usr/bin/echo",
            "mode": "oneshot",
            "timeout_seconds": 60
        }
    })).await;
    assert_eq!(created["id"].as_str(), Some(test_id));

    // Get — verify exists
    let got = server.rpc_ok("acp.get", json!({"id": test_id})).await;
    assert_eq!(got["display_name"].as_str(), Some("Probe Test CLI"));

    // Update
    let updated = server.rpc_ok("acp.update", json!({
        "id": test_id,
        "config": {
            "display_name": "Updated CLI",
            "executable": "/usr/bin/echo",
            "mode": "oneshot",
            "timeout_seconds": 120
        }
    })).await;
    assert_eq!(updated["display_name"].as_str(), Some("Updated CLI"));

    // Delete
    server.rpc_ok("acp.delete", json!({"id": test_id})).await;

    // Verify gone
    let err = server.rpc_err("acp.get", json!({"id": test_id})).await;
    assert!(err["message"].as_str().unwrap().contains("not found")
        || err["message"].as_str().unwrap().contains("Unknown"));
}

#[tokio::test]
#[serial]
async fn p7_09_test_harness_returns_result() {
    let server = get_server().await;
    let result = server.rpc_ok("acp.test", json!({"id": "codex"})).await;
    // Structure check only — success may be false if CLI not installed
    assert!(result.get("success").is_some());
    assert!(result.get("message").is_some());
    assert!(result.get("duration_ms").is_some());
    assert!(result["duration_ms"].as_u64().unwrap() > 0);
}
```

**Step 3: Run P7 tests**

Run: `cargo test -p alephcore --test acp_probe p7_ -- --nocapture --test-threads=1 2>&1 | tail -15`
Expected: 10 passed (server spawns once, reused)

**Step 4: Commit**

```
git add tests/acp_probe/p7_rpc_server_probe.rs tests/acp_probe.rs
git commit -m "test(acp): P7 real server RPC probe tests (10 tests)"
```

---

### Task 9: Full Suite Verification

**Step 1: Run all ACP probe tests**

```bash
cargo test -p alephcore --test acp_probe -- --nocapture 2>&1 | tail -30
```

Expected: 65 tests passed

**Step 2: Run existing ACP unit tests to confirm no regression**

```bash
cargo test -p alephcore --lib acp 2>&1 | tail -10
```

Expected: All existing tests still pass

**Step 3: Run full core test suite**

```bash
cargo test -p alephcore --lib 2>&1 | tail -10
```

Expected: No new failures

**Step 4: Final commit if any fixes needed**
