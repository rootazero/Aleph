# ACP Agents & CLI/TUI Split Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add ACP Agent system to Aleph for spawning/managing external CLI tools (Claude Code, Codex, Gemini) and split apps/cli into aleph-cli + aleph-tui.

**Architecture:** New `src/acp/` module with AcpHarness trait, StdioTransport (NDJSON over stdio), and AcpHarnessManager. Three harness adapters + four builtin tools (claude_code, codex, gemini_cli, acp_switch). CLI/TUI split moves TUI code to `apps/tui/` as separate workspace member.

**Tech Stack:** Rust, tokio (process/io), serde_json (NDJSON), schemars (JsonSchema), ratatui (TUI)

---

### Task 1: ACP Protocol Types

**Files:**
- Create: `src/acp/mod.rs`
- Create: `src/acp/protocol.rs`
- Modify: `src/lib.rs` — add `pub mod acp;`

**Step 1: Create module entry**

```rust
// src/acp/mod.rs
//! ACP (Agent Client Protocol) module
//!
//! Manages external CLI tools (Claude Code, Codex, Gemini) as ACP harnesses.
//! Supports Tool mode (LLM-dispatched) and Agent mode (direct conversation).

pub mod protocol;
pub mod harness;
pub mod session;
pub mod manager;
pub mod harnesses;
```

**Step 2: Create protocol types**

```rust
// src/acp/protocol.rs
//! ACP protocol message types (NDJSON over stdio)

use serde::{Deserialize, Serialize};

/// Request sent to ACP CLI subprocess
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRequest {
    pub jsonrpc: String,
    pub id: String,
    pub method: String,
    pub params: serde_json::Value,
}

impl AcpRequest {
    pub fn new(id: impl Into<String>, method: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }

    pub fn initialize(id: impl Into<String>) -> Self {
        Self::new(id, "initialize", serde_json::json!({
            "clientInfo": {
                "name": "aleph",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "protocolVersion": "0.1",
            "capabilities": {}
        }))
    }

    pub fn new_session(id: impl Into<String>) -> Self {
        Self::new(id, "newSession", serde_json::json!({}))
    }

    pub fn prompt(id: impl Into<String>, text: &str, cwd: Option<&str>) -> Self {
        let mut params = serde_json::json!({ "prompt": text });
        if let Some(dir) = cwd {
            params["cwd"] = serde_json::Value::String(dir.to_string());
        }
        Self::new(id, "prompt", params)
    }

    pub fn cancel(id: impl Into<String>) -> Self {
        Self::new(id, "cancel", serde_json::json!({}))
    }
}

/// Event received from ACP CLI subprocess
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpResponse {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<AcpError>,
    /// For streaming events (notifications without id)
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

impl AcpResponse {
    /// Whether this is a final result (has id + result/error)
    pub fn is_result(&self) -> bool {
        self.id.is_some() && (self.result.is_some() || self.error.is_some())
    }

    /// Whether this is a streaming notification (method + params, no id)
    pub fn is_notification(&self) -> bool {
        self.method.is_some() && self.id.is_none()
    }

    /// Extract text content from result, if any
    pub fn text_content(&self) -> Option<String> {
        self.result.as_ref().and_then(|r| {
            // Try common patterns: { "content": "..." } or { "message": "..." }
            r.get("content").or_else(|| r.get("message"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    // If result is a string directly
                    r.as_str().map(|s| s.to_string())
                })
        })
    }
}

/// ACP error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

impl std::fmt::Display for AcpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ACP error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for AcpError {}

/// State of an ACP session
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpSessionState {
    /// Ready to accept prompts
    Idle,
    /// Currently processing a prompt
    Busy,
    /// Process crashed or protocol error
    Error,
}
```

**Step 3: Register module in lib.rs**

Add `pub mod acp;` to `src/lib.rs` (alongside existing modules).

**Step 4: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS (no other modules reference acp yet)

**Step 5: Commit**

```bash
git add src/acp/mod.rs src/acp/protocol.rs src/lib.rs
git commit -m "acp: add protocol types for ACP Agent system"
```

---

### Task 2: StdioTransport

**Files:**
- Create: `src/acp/transport.rs`
- Modify: `src/acp/mod.rs` — add `pub mod transport;`

**Step 1: Write transport tests**

```rust
// At bottom of src/acp/transport.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::protocol::AcpRequest;

    #[test]
    fn test_serialize_ndjson_line() {
        let req = AcpRequest::initialize("1");
        let line = serde_json::to_string(&req).unwrap();
        assert!(!line.contains('\n'));
        assert!(line.contains("initialize"));
    }

    #[test]
    fn test_deserialize_response() {
        let json = r#"{"jsonrpc":"2.0","id":"1","result":{"content":"hello"}}"#;
        let resp: crate::acp::protocol::AcpResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.text_content(), Some("hello".to_string()));
    }

    #[test]
    fn test_deserialize_error_response() {
        let json = r#"{"jsonrpc":"2.0","id":"1","error":{"code":-1,"message":"fail"}}"#;
        let resp: crate::acp::protocol::AcpResponse = serde_json::from_str(json).unwrap();
        assert!(resp.error.is_some());
        assert!(resp.result.is_none());
    }
}
```

**Step 2: Write StdioTransport implementation**

```rust
// src/acp/transport.rs
//! NDJSON stdio transport for ACP communication

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use crate::acp::protocol::{AcpRequest, AcpResponse};
use crate::error::{AlephError, Result};

/// NDJSON transport over child process stdio
pub struct StdioTransport {
    stdin: ChildStdin,
    event_rx: mpsc::Receiver<Result<AcpResponse>>,
    /// Handle to the reader task so we can abort on drop
    _reader_handle: tokio::task::JoinHandle<()>,
}

impl StdioTransport {
    /// Create a new transport from child process stdin/stdout
    pub fn new(stdin: ChildStdin, stdout: ChildStdout) -> Self {
        let (event_tx, event_rx) = mpsc::channel(256);

        let reader_handle = tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        let parsed = serde_json::from_str::<AcpResponse>(&line)
                            .map_err(|e| {
                                AlephError::tool(format!("ACP parse error: {} — line: {}", e,
                                    if line.len() > 200 { &line[..200] } else { &line }))
                            });
                        if event_tx.send(parsed).await.is_err() {
                            debug!("ACP event receiver dropped, stopping reader");
                            break;
                        }
                    }
                    Ok(None) => {
                        debug!("ACP stdout closed (EOF)");
                        break;
                    }
                    Err(e) => {
                        warn!(error = %e, "ACP stdout read error");
                        let _ = event_tx.send(Err(AlephError::tool(format!(
                            "ACP read error: {}", e
                        )))).await;
                        break;
                    }
                }
            }
        });

        Self {
            stdin,
            event_rx,
            _reader_handle: reader_handle,
        }
    }

    /// Send an ACP request as NDJSON line
    pub async fn send(&mut self, req: &AcpRequest) -> Result<()> {
        let mut line = serde_json::to_string(req)
            .map_err(|e| AlephError::tool(format!("ACP serialize error: {}", e)))?;
        line.push('\n');

        self.stdin.write_all(line.as_bytes()).await
            .map_err(|e| AlephError::tool(format!("ACP write error: {}", e)))?;
        self.stdin.flush().await
            .map_err(|e| AlephError::tool(format!("ACP flush error: {}", e)))?;

        debug!(method = %req.method, id = %req.id, "Sent ACP request");
        Ok(())
    }

    /// Receive next event from the CLI
    pub async fn recv(&mut self) -> Option<Result<AcpResponse>> {
        self.event_rx.recv().await
    }

    /// Send a request and wait for the response with matching id
    ///
    /// Notifications received while waiting are collected and returned separately.
    pub async fn request(
        &mut self,
        req: &AcpRequest,
        timeout: std::time::Duration,
    ) -> Result<(AcpResponse, Vec<AcpResponse>)> {
        self.send(req).await?;
        let request_id = req.id.clone();
        let mut notifications = Vec::new();

        let result = tokio::time::timeout(timeout, async {
            loop {
                match self.recv().await {
                    Some(Ok(resp)) => {
                        if resp.id.as_deref() == Some(&request_id) {
                            return Ok(resp);
                        }
                        // Collect notifications while waiting
                        if resp.is_notification() {
                            notifications.push(resp);
                        }
                    }
                    Some(Err(e)) => return Err(e),
                    None => return Err(AlephError::tool("ACP connection closed")),
                }
            }
        })
        .await
        .map_err(|_| AlephError::tool(format!(
            "ACP request '{}' timed out after {:?}", req.method, timeout
        )))??;

        if let Some(ref err) = result.error {
            return Err(AlephError::tool(format!("ACP error: {}", err)));
        }

        Ok((result, notifications))
    }
}

// Tests at bottom...
```

**Step 3: Add module to mod.rs**

Add `pub mod transport;` to `src/acp/mod.rs`.

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib acp::transport`
Expected: PASS (3 tests)

**Step 5: Commit**

```bash
git add src/acp/transport.rs src/acp/mod.rs
git commit -m "acp: add StdioTransport for NDJSON over stdio"
```

---

### Task 3: AcpSession

**Files:**
- Create: `src/acp/session.rs`

**Step 1: Write AcpSession**

```rust
// src/acp/session.rs
//! ACP session — manages a single CLI subprocess lifecycle

use tokio::process::{Child, Command};
use tracing::{debug, info, warn, error};

use crate::acp::protocol::{AcpRequest, AcpResponse, AcpSessionState};
use crate::acp::transport::StdioTransport;
use crate::error::{AlephError, Result};

/// Configuration for spawning an ACP harness
#[derive(Debug, Clone)]
pub struct HarnessConfig {
    /// Executable path (e.g. "claude", "codex", "gemini")
    pub executable: String,
    /// Additional CLI arguments for ACP mode
    pub args: Vec<String>,
    /// Working directory
    pub cwd: Option<String>,
    /// Environment variables to set
    pub env: Vec<(String, String)>,
    /// Request timeout
    pub timeout: std::time::Duration,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            executable: String::new(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            timeout: std::time::Duration::from_secs(300), // 5 minutes
        }
    }
}

/// A live ACP session with a CLI subprocess
pub struct AcpSession {
    /// Harness identifier
    harness_id: String,
    /// Child process handle
    child: Child,
    /// NDJSON transport
    transport: StdioTransport,
    /// Current state
    state: AcpSessionState,
    /// Whether initialize handshake completed
    initialized: bool,
}

impl AcpSession {
    /// Spawn a new CLI subprocess and create session
    pub async fn spawn(harness_id: &str, config: &HarnessConfig) -> Result<Self> {
        info!(harness = harness_id, executable = %config.executable, "Spawning ACP session");

        let mut cmd = Command::new(&config.executable);
        cmd.args(&config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        if let Some(ref cwd) = config.cwd {
            cmd.current_dir(cwd);
        }

        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(|e| {
            AlephError::tool(format!(
                "Failed to spawn ACP harness '{}' ({}): {}",
                harness_id, config.executable, e
            ))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            AlephError::tool(format!("ACP harness '{}': failed to capture stdin", harness_id))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AlephError::tool(format!("ACP harness '{}': failed to capture stdout", harness_id))
        })?;

        let transport = StdioTransport::new(stdin, stdout);

        Ok(Self {
            harness_id: harness_id.to_string(),
            child,
            transport,
            state: AcpSessionState::Idle,
            initialized: false,
        })
    }

    /// Perform ACP initialization handshake
    pub async fn initialize(&mut self, timeout: std::time::Duration) -> Result<()> {
        if self.initialized {
            return Ok(());
        }

        let req = AcpRequest::initialize("init-1");
        let (resp, _) = self.transport.request(&req, timeout).await?;

        debug!(
            harness = %self.harness_id,
            result = ?resp.result,
            "ACP initialize handshake complete"
        );
        self.initialized = true;
        Ok(())
    }

    /// Send a prompt and collect the full response
    ///
    /// Returns the final result text and any streaming notifications received.
    pub async fn prompt(
        &mut self,
        id: &str,
        text: &str,
        cwd: Option<&str>,
        timeout: std::time::Duration,
    ) -> Result<(String, Vec<AcpResponse>)> {
        if self.state == AcpSessionState::Error {
            return Err(AlephError::tool(format!(
                "ACP session '{}' is in error state — call restart() first",
                self.harness_id
            )));
        }

        self.state = AcpSessionState::Busy;

        let req = AcpRequest::prompt(id, text, cwd);
        let result = self.transport.request(&req, timeout).await;

        match result {
            Ok((resp, notifications)) => {
                self.state = AcpSessionState::Idle;
                let text = resp.text_content().unwrap_or_else(|| {
                    // Fallback: serialize the entire result
                    resp.result.as_ref()
                        .map(|v| serde_json::to_string_pretty(v).unwrap_or_default())
                        .unwrap_or_default()
                });
                Ok((text, notifications))
            }
            Err(e) => {
                self.state = AcpSessionState::Error;
                Err(e)
            }
        }
    }

    /// Cancel the current prompt
    pub async fn cancel(&mut self) -> Result<()> {
        let req = AcpRequest::cancel("cancel-1");
        self.transport.send(&req).await?;
        self.state = AcpSessionState::Idle;
        Ok(())
    }

    /// Get current session state
    pub fn state(&self) -> AcpSessionState {
        self.state
    }

    /// Check if the subprocess is still running
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Kill the subprocess
    pub async fn kill(&mut self) {
        if let Err(e) = self.child.kill().await {
            warn!(harness = %self.harness_id, error = %e, "Failed to kill ACP subprocess");
        }
        self.state = AcpSessionState::Error;
    }
}

impl Drop for AcpSession {
    fn drop(&mut self) {
        // Best-effort kill on drop — can't await in Drop
        let _ = self.child.start_kill();
    }
}
```

**Step 2: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 3: Commit**

```bash
git add src/acp/session.rs
git commit -m "acp: add AcpSession for CLI subprocess lifecycle"
```

---

### Task 4: AcpHarness Trait & Harness Adapters

**Files:**
- Create: `src/acp/harness.rs`
- Create: `src/acp/harnesses/mod.rs`
- Create: `src/acp/harnesses/claude_code.rs`
- Create: `src/acp/harnesses/codex.rs`
- Create: `src/acp/harnesses/gemini.rs`

**Step 1: Write AcpHarness trait**

```rust
// src/acp/harness.rs
//! AcpHarness trait — abstraction for different CLI tool adapters

use async_trait::async_trait;

use crate::acp::session::{AcpSession, HarnessConfig};
use crate::error::Result;

/// Adapter for a specific CLI tool that supports ACP
#[async_trait]
pub trait AcpHarness: Send + Sync {
    /// Unique harness identifier (e.g. "claude-code", "codex", "gemini")
    fn id(&self) -> &str;

    /// Human-readable display name
    fn display_name(&self) -> &str;

    /// Build the HarnessConfig for spawning
    fn build_config(&self, cwd: Option<&str>) -> HarnessConfig;

    /// Check if the CLI executable is installed and available
    async fn is_available(&self) -> bool {
        let config = self.build_config(None);
        tokio::process::Command::new(&config.executable)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Spawn and initialize a new session
    async fn spawn_session(&self, cwd: Option<&str>) -> Result<AcpSession> {
        let config = self.build_config(cwd);
        let mut session = AcpSession::spawn(self.id(), &config).await?;
        session.initialize(config.timeout).await?;
        Ok(session)
    }
}
```

**Step 2: Write Claude Code harness**

```rust
// src/acp/harnesses/claude_code.rs
//! Claude Code CLI harness adapter

use async_trait::async_trait;

use crate::acp::harness::AcpHarness;
use crate::acp::session::HarnessConfig;

pub struct ClaudeCodeHarness {
    executable: String,
}

impl ClaudeCodeHarness {
    pub fn new(executable: Option<String>) -> Self {
        Self {
            executable: executable.unwrap_or_else(|| "claude".to_string()),
        }
    }
}

#[async_trait]
impl AcpHarness for ClaudeCodeHarness {
    fn id(&self) -> &str { "claude-code" }
    fn display_name(&self) -> &str { "Claude Code" }

    fn build_config(&self, cwd: Option<&str>) -> HarnessConfig {
        HarnessConfig {
            executable: self.executable.clone(),
            args: vec!["--acp".to_string()],
            cwd: cwd.map(|s| s.to_string()),
            ..Default::default()
        }
    }
}
```

**Step 3: Write Codex harness**

```rust
// src/acp/harnesses/codex.rs
//! Codex CLI harness adapter

use async_trait::async_trait;

use crate::acp::harness::AcpHarness;
use crate::acp::session::HarnessConfig;

pub struct CodexHarness {
    executable: String,
}

impl CodexHarness {
    pub fn new(executable: Option<String>) -> Self {
        Self {
            executable: executable.unwrap_or_else(|| "codex".to_string()),
        }
    }
}

#[async_trait]
impl AcpHarness for CodexHarness {
    fn id(&self) -> &str { "codex" }
    fn display_name(&self) -> &str { "Codex" }

    fn build_config(&self, cwd: Option<&str>) -> HarnessConfig {
        HarnessConfig {
            executable: self.executable.clone(),
            args: vec!["--acp".to_string()],
            cwd: cwd.map(|s| s.to_string()),
            ..Default::default()
        }
    }
}
```

**Step 4: Write Gemini harness**

```rust
// src/acp/harnesses/gemini.rs
//! Gemini CLI harness adapter

use async_trait::async_trait;

use crate::acp::harness::AcpHarness;
use crate::acp::session::HarnessConfig;

pub struct GeminiHarness {
    executable: String,
}

impl GeminiHarness {
    pub fn new(executable: Option<String>) -> Self {
        Self {
            executable: executable.unwrap_or_else(|| "gemini".to_string()),
        }
    }
}

#[async_trait]
impl AcpHarness for GeminiHarness {
    fn id(&self) -> &str { "gemini" }
    fn display_name(&self) -> &str { "Gemini" }

    fn build_config(&self, cwd: Option<&str>) -> HarnessConfig {
        HarnessConfig {
            executable: self.executable.clone(),
            args: vec!["--acp".to_string()],
            cwd: cwd.map(|s| s.to_string()),
            ..Default::default()
        }
    }
}
```

**Step 5: Write harnesses/mod.rs**

```rust
// src/acp/harnesses/mod.rs
//! Concrete ACP harness adapters for CLI tools

pub mod claude_code;
pub mod codex;
pub mod gemini;

pub use claude_code::ClaudeCodeHarness;
pub use codex::CodexHarness;
pub use gemini::GeminiHarness;
```

**Step 6: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 7: Commit**

```bash
git add src/acp/harness.rs src/acp/harnesses/
git commit -m "acp: add AcpHarness trait and adapters for Claude Code, Codex, Gemini"
```

---

### Task 5: AcpHarnessManager

**Files:**
- Create: `src/acp/manager.rs`

**Step 1: Write manager tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manager_registers_harnesses() {
        let manager = AcpHarnessManager::new();
        // Should have 3 default harnesses
        let ids = manager.harness_ids();
        assert!(ids.contains(&"claude-code".to_string()));
        assert!(ids.contains(&"codex".to_string()));
        assert!(ids.contains(&"gemini".to_string()));
    }

    #[test]
    fn test_manager_has_harness() {
        let manager = AcpHarnessManager::new();
        assert!(manager.has_harness("claude-code"));
        assert!(!manager.has_harness("unknown"));
    }
}
```

**Step 2: Write AcpHarnessManager**

```rust
// src/acp/manager.rs
//! AcpHarnessManager — lifecycle management for ACP harness sessions

use std::collections::HashMap;

use tracing::{info, warn, error};

use crate::acp::harness::AcpHarness;
use crate::acp::harnesses::{ClaudeCodeHarness, CodexHarness, GeminiHarness};
use crate::acp::session::AcpSession;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

/// Configuration for ACP harness manager
#[derive(Debug, Clone)]
pub struct AcpManagerConfig {
    /// Per-harness executable overrides
    pub executables: HashMap<String, String>,
    /// Per-harness enabled flags
    pub enabled: HashMap<String, bool>,
}

impl Default for AcpManagerConfig {
    fn default() -> Self {
        Self {
            executables: HashMap::new(),
            enabled: HashMap::new(),
        }
    }
}

/// Manages ACP harness instances and their sessions
///
/// Lazy-start pattern: harnesses are registered at creation,
/// but CLI subprocesses are only spawned on first use.
pub struct AcpHarnessManager {
    harnesses: HashMap<String, Box<dyn AcpHarness>>,
    sessions: RwLock<HashMap<String, AcpSession>>,
}

impl AcpHarnessManager {
    /// Create manager with default harnesses (all three CLIs)
    pub fn new() -> Self {
        Self::with_config(AcpManagerConfig::default())
    }

    /// Create manager with custom configuration
    pub fn with_config(config: AcpManagerConfig) -> Self {
        let mut harnesses: HashMap<String, Box<dyn AcpHarness>> = HashMap::new();

        // Register Claude Code if enabled (default: true)
        if *config.enabled.get("claude-code").unwrap_or(&true) {
            let exec = config.executables.get("claude-code").cloned();
            harnesses.insert("claude-code".to_string(), Box::new(ClaudeCodeHarness::new(exec)));
        }

        // Register Codex if enabled (default: true)
        if *config.enabled.get("codex").unwrap_or(&true) {
            let exec = config.executables.get("codex").cloned();
            harnesses.insert("codex".to_string(), Box::new(CodexHarness::new(exec)));
        }

        // Register Gemini if enabled (default: true)
        if *config.enabled.get("gemini").unwrap_or(&true) {
            let exec = config.executables.get("gemini").cloned();
            harnesses.insert("gemini".to_string(), Box::new(GeminiHarness::new(exec)));
        }

        info!(
            count = harnesses.len(),
            harnesses = ?harnesses.keys().collect::<Vec<_>>(),
            "ACP harness manager initialized"
        );

        Self {
            harnesses,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// List registered harness IDs
    pub fn harness_ids(&self) -> Vec<String> {
        self.harnesses.keys().cloned().collect()
    }

    /// Check if a harness is registered
    pub fn has_harness(&self, id: &str) -> bool {
        self.harnesses.contains_key(id)
    }

    /// Get harness display name
    pub fn display_name(&self, id: &str) -> Option<&str> {
        self.harnesses.get(id).map(|h| h.display_name())
    }

    /// Check which harnesses have their CLI installed
    pub async fn available_harnesses(&self) -> Vec<String> {
        let mut available = Vec::new();
        for (id, harness) in &self.harnesses {
            if harness.is_available().await {
                available.push(id.clone());
            }
        }
        available
    }

    /// Get or spawn a session for a harness (lazy-start)
    ///
    /// If session exists and is alive, returns it.
    /// If session doesn't exist or is dead, spawns a new one.
    pub async fn get_or_spawn(&self, harness_id: &str, cwd: Option<&str>) -> Result<()> {
        let harness = self.harnesses.get(harness_id).ok_or_else(|| {
            AlephError::tool(format!("Unknown ACP harness: '{}'. Available: {:?}",
                harness_id, self.harness_ids()))
        })?;

        let mut sessions = self.sessions.write().await;

        // Check if existing session is alive
        if let Some(session) = sessions.get_mut(harness_id) {
            if session.is_alive() {
                return Ok(());
            }
            warn!(harness = harness_id, "ACP session dead, respawning");
            sessions.remove(harness_id);
        }

        // Spawn new session
        let session = harness.spawn_session(cwd).await?;
        sessions.insert(harness_id.to_string(), session);
        info!(harness = harness_id, "ACP session spawned");
        Ok(())
    }

    /// Send a prompt to a harness and get the response
    pub async fn prompt(
        &self,
        harness_id: &str,
        prompt_text: &str,
        cwd: Option<&str>,
    ) -> Result<String> {
        // Ensure session exists
        self.get_or_spawn(harness_id, cwd).await?;

        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(harness_id).ok_or_else(|| {
            AlephError::tool(format!("ACP session '{}' not found after spawn", harness_id))
        })?;

        let timeout = std::time::Duration::from_secs(300);
        let id = format!("prompt-{}", uuid::Uuid::new_v4());
        let (result, _notifications) = session.prompt(&id, prompt_text, cwd, timeout).await?;
        Ok(result)
    }

    /// Cancel current operation on a harness
    pub async fn cancel(&self, harness_id: &str) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(harness_id) {
            session.cancel().await?;
        }
        Ok(())
    }

    /// Shutdown all active sessions
    pub async fn shutdown_all(&self) {
        let mut sessions = self.sessions.write().await;
        for (id, mut session) in sessions.drain() {
            info!(harness = %id, "Shutting down ACP session");
            session.kill().await;
        }
    }
}

// Tests at bottom...
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib acp::manager`
Expected: PASS (2 tests)

**Step 4: Commit**

```bash
git add src/acp/manager.rs
git commit -m "acp: add AcpHarnessManager with lazy-start lifecycle"
```

---

### Task 6: ACP Configuration Type

**Files:**
- Create: `src/config/types/acp.rs`
- Modify: `src/config/types/mod.rs` — add `pub mod acp;` and re-export

**Step 1: Write AcpConfig**

```rust
// src/config/types/acp.rs
//! ACP (Agent Client Protocol) configuration types

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// ACP module configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AcpConfig {
    /// Enable/disable ACP functionality
    #[serde(default)]
    pub enabled: bool,

    /// Per-harness configurations
    #[serde(default)]
    pub harnesses: HashMap<String, AcpHarnessEntry>,
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            harnesses: HashMap::new(),
        }
    }
}

/// Configuration for a single ACP harness
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AcpHarnessEntry {
    /// Path to CLI executable (uses default if omitted)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,

    /// Enable/disable this harness
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AcpHarnessEntry {
    fn default() -> Self {
        Self {
            executable: None,
            enabled: true,
        }
    }
}
```

**Step 2: Register in config/types/mod.rs**

Add after the last `pub mod` line:
```rust
pub mod acp;
```

Add after the last `pub use` line:
```rust
pub use acp::*;
```

**Step 3: Add `acp` field to the main Config struct**

Find the main Config struct in `src/config/mod.rs` (or wherever it lives). Add:
```rust
/// ACP (Agent Client Protocol) configuration
#[serde(default)]
pub acp: AcpConfig,
```

**Step 4: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 5: Commit**

```bash
git add src/config/types/acp.rs src/config/types/mod.rs src/config/mod.rs
git commit -m "config: add AcpConfig for ACP harness configuration"
```

---

### Task 7: ACP Builtin Tools

**Files:**
- Create: `src/builtin_tools/acp_tools.rs`
- Modify: `src/builtin_tools/mod.rs` — add module + re-exports

**Step 1: Write ACP tools**

```rust
// src/builtin_tools/acp_tools.rs
//! ACP builtin tools for delegating tasks to external CLI harnesses
//!
//! Four tools:
//! - claude_code: Delegate task to Claude Code CLI
//! - codex: Delegate task to Codex CLI
//! - gemini_cli: Delegate task to Gemini CLI
//! - acp_switch: Switch to/from direct ACP agent conversation

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::acp::manager::AcpHarnessManager;
use crate::builtin_tools::{notify_tool_start, notify_tool_result};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Shared Args/Output
// =============================================================================

/// Input for ACP delegate tools (claude_code, codex, gemini_cli)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AcpDelegateArgs {
    /// The task or prompt to send to the CLI tool
    pub prompt: String,
    /// Working directory for the CLI tool (optional, defaults to current workspace)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// Output from ACP delegate tools
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpDelegateOutput {
    /// The harness that handled the request
    pub harness: String,
    /// The result text from the CLI tool
    pub result: String,
}

// =============================================================================
// Claude Code Tool
// =============================================================================

/// Delegate a coding task to Claude Code CLI
#[derive(Clone)]
pub struct ClaudeCodeTool {
    manager: Arc<AcpHarnessManager>,
}

impl ClaudeCodeTool {
    pub fn new(manager: Arc<AcpHarnessManager>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl AlephTool for ClaudeCodeTool {
    const NAME: &'static str = "claude_code";
    const DESCRIPTION: &'static str = "Delegate a coding task to Claude Code CLI. Use this when you need Claude Code's specialized coding capabilities (code generation, refactoring, debugging) with direct file system access.";
    type Args = AcpDelegateArgs;
    type Output = AcpDelegateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, &args.prompt);
        let result = self.manager.prompt("claude-code", &args.prompt, args.cwd.as_deref()).await?;
        notify_tool_result(Self::NAME, &result, true);
        Ok(AcpDelegateOutput {
            harness: "claude-code".to_string(),
            result,
        })
    }
}

// =============================================================================
// Codex Tool
// =============================================================================

/// Delegate a coding task to Codex CLI
#[derive(Clone)]
pub struct CodexTool {
    manager: Arc<AcpHarnessManager>,
}

impl CodexTool {
    pub fn new(manager: Arc<AcpHarnessManager>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl AlephTool for CodexTool {
    const NAME: &'static str = "codex";
    const DESCRIPTION: &'static str = "Delegate a coding task to OpenAI Codex CLI. Use this when you need Codex's code generation and editing capabilities with direct file system access.";
    type Args = AcpDelegateArgs;
    type Output = AcpDelegateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, &args.prompt);
        let result = self.manager.prompt("codex", &args.prompt, args.cwd.as_deref()).await?;
        notify_tool_result(Self::NAME, &result, true);
        Ok(AcpDelegateOutput {
            harness: "codex".to_string(),
            result,
        })
    }
}

// =============================================================================
// Gemini CLI Tool
// =============================================================================

/// Delegate a task to Gemini CLI
#[derive(Clone)]
pub struct GeminiCliTool {
    manager: Arc<AcpHarnessManager>,
}

impl GeminiCliTool {
    pub fn new(manager: Arc<AcpHarnessManager>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl AlephTool for GeminiCliTool {
    const NAME: &'static str = "gemini_cli";
    const DESCRIPTION: &'static str = "Delegate a task to Google Gemini CLI. Use this when you need Gemini's capabilities with direct file system access.";
    type Args = AcpDelegateArgs;
    type Output = AcpDelegateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, &args.prompt);
        let result = self.manager.prompt("gemini", &args.prompt, args.cwd.as_deref()).await?;
        notify_tool_result(Self::NAME, &result, true);
        Ok(AcpDelegateOutput {
            harness: "gemini".to_string(),
            result,
        })
    }
}

// =============================================================================
// ACP Switch Tool
// =============================================================================

/// Input for switching ACP agent mode
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AcpSwitchArgs {
    /// Target harness ID to switch to ("claude-code", "codex", "gemini"),
    /// or "aleph" to switch back to normal Aleph mode
    pub target: String,
}

/// Output from ACP switch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpSwitchOutput {
    /// The target that was switched to
    pub target: String,
    /// Human-readable status message
    pub message: String,
}

/// Switch between Aleph main loop and direct ACP agent conversation
#[derive(Clone)]
pub struct AcpSwitchTool {
    manager: Arc<AcpHarnessManager>,
}

impl AcpSwitchTool {
    pub fn new(manager: Arc<AcpHarnessManager>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl AlephTool for AcpSwitchTool {
    const NAME: &'static str = "acp_switch";
    const DESCRIPTION: &'static str = "Switch to direct conversation with an external CLI agent (Claude Code, Codex, or Gemini), or switch back to Aleph. When switched, user messages are forwarded directly to the CLI tool instead of going through Aleph's LLM.";
    type Args = AcpSwitchArgs;
    type Output = AcpSwitchOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, &args.target);

        if args.target == "aleph" {
            notify_tool_result(Self::NAME, "Switched back to Aleph", true);
            return Ok(AcpSwitchOutput {
                target: "aleph".to_string(),
                message: "Switched back to Aleph main loop.".to_string(),
            });
        }

        // Validate harness exists
        if !self.manager.has_harness(&args.target) {
            let available = self.manager.harness_ids();
            return Err(crate::error::AlephError::tool(format!(
                "Unknown ACP target: '{}'. Available: {:?}", args.target, available
            )));
        }

        // Pre-spawn session so it's ready for direct mode
        self.manager.get_or_spawn(&args.target, None).await?;

        let display = self.manager.display_name(&args.target).unwrap_or(&args.target);
        let msg = format!("Switched to {}. Messages will be forwarded directly. Say 'switch back to Aleph' to return.", display);
        notify_tool_result(Self::NAME, &msg, true);

        Ok(AcpSwitchOutput {
            target: args.target,
            message: msg,
        })
    }
}
```

**Step 2: Register module in builtin_tools/mod.rs**

Add after the last `pub mod` line:
```rust
pub mod acp_tools;
```

Add re-exports:
```rust
pub use acp_tools::{
    AcpDelegateArgs, AcpDelegateOutput,
    ClaudeCodeTool, CodexTool, GeminiCliTool,
    AcpSwitchArgs, AcpSwitchOutput, AcpSwitchTool,
};
```

**Step 3: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 4: Commit**

```bash
git add src/builtin_tools/acp_tools.rs src/builtin_tools/mod.rs
git commit -m "tools: add ACP delegate tools (claude_code, codex, gemini_cli, acp_switch)"
```

---

### Task 8: Register ACP Tools in BuiltinToolRegistry

**Files:**
- Modify: `src/executor/builtin_registry/registry.rs`
- Modify: `src/executor/builtin_registry/config.rs` (BuiltinToolConfig)

**Step 1: Add AcpHarnessManager to BuiltinToolConfig**

In `src/executor/builtin_registry/config.rs`, add field:
```rust
/// ACP harness manager for delegate tools
pub acp_manager: Option<Arc<AcpHarnessManager>>,
```

And import at top:
```rust
use crate::acp::manager::AcpHarnessManager;
```

**Step 2: Add ACP tool fields to BuiltinToolRegistry struct**

In `registry.rs`, add to the struct:
```rust
/// ACP delegate tools (optional - requires AcpHarnessManager)
pub(crate) claude_code_tool: Option<crate::builtin_tools::acp_tools::ClaudeCodeTool>,
pub(crate) codex_tool: Option<crate::builtin_tools::acp_tools::CodexTool>,
pub(crate) gemini_cli_tool: Option<crate::builtin_tools::acp_tools::GeminiCliTool>,
pub(crate) acp_switch_tool: Option<crate::builtin_tools::acp_tools::AcpSwitchTool>,
```

**Step 3: Initialize ACP tools in `with_config()`**

Add after other optional tool initialization:
```rust
// Create ACP tools if manager is provided
let (claude_code_tool, codex_tool, gemini_cli_tool, acp_switch_tool) =
    if let Some(ref manager) = config.acp_manager {
        info!("Creating ACP delegate tools");
        (
            if manager.has_harness("claude-code") { Some(crate::builtin_tools::acp_tools::ClaudeCodeTool::new(Arc::clone(manager))) } else { None },
            if manager.has_harness("codex") { Some(crate::builtin_tools::acp_tools::CodexTool::new(Arc::clone(manager))) } else { None },
            if manager.has_harness("gemini") { Some(crate::builtin_tools::acp_tools::GeminiCliTool::new(Arc::clone(manager))) } else { None },
            Some(crate::builtin_tools::acp_tools::AcpSwitchTool::new(Arc::clone(manager))),
        )
    } else {
        (None, None, None, None)
    };
```

**Step 4: Register tool metadata**

Add to the `tools` HashMap construction:
```rust
if claude_code_tool.is_some() {
    tools.insert("claude_code".to_string(), UnifiedTool::new(
        "builtin:claude_code", "claude_code",
        crate::builtin_tools::acp_tools::ClaudeCodeTool::DESCRIPTION,
        ToolSource::Builtin,
    ));
}
if codex_tool.is_some() {
    tools.insert("codex".to_string(), UnifiedTool::new(
        "builtin:codex", "codex",
        crate::builtin_tools::acp_tools::CodexTool::DESCRIPTION,
        ToolSource::Builtin,
    ));
}
if gemini_cli_tool.is_some() {
    tools.insert("gemini_cli".to_string(), UnifiedTool::new(
        "builtin:gemini_cli", "gemini_cli",
        crate::builtin_tools::acp_tools::GeminiCliTool::DESCRIPTION,
        ToolSource::Builtin,
    ));
}
if acp_switch_tool.is_some() {
    tools.insert("acp_switch".to_string(), UnifiedTool::new(
        "builtin:acp_switch", "acp_switch",
        crate::builtin_tools::acp_tools::AcpSwitchTool::DESCRIPTION,
        ToolSource::Builtin,
    ));
}
```

**Step 5: Add execution match cases in `execute_tool()`**

Add to the match block in `execute_tool()`:
```rust
// ACP delegate tools
"claude_code" => Box::pin(async move {
    let tool = self.claude_code_tool.as_ref().ok_or_else(|| {
        AlephError::tool("claude_code not available: ACP not configured or Claude Code not enabled")
    })?;
    tool.call_json(arguments).await
}),
"codex" => Box::pin(async move {
    let tool = self.codex_tool.as_ref().ok_or_else(|| {
        AlephError::tool("codex not available: ACP not configured or Codex not enabled")
    })?;
    tool.call_json(arguments).await
}),
"gemini_cli" => Box::pin(async move {
    let tool = self.gemini_cli_tool.as_ref().ok_or_else(|| {
        AlephError::tool("gemini_cli not available: ACP not configured or Gemini not enabled")
    })?;
    tool.call_json(arguments).await
}),
"acp_switch" => Box::pin(async move {
    let tool = self.acp_switch_tool.as_ref().ok_or_else(|| {
        AlephError::tool("acp_switch not available: ACP not configured")
    })?;
    tool.call_json(arguments).await
}),
```

**Step 6: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 7: Commit**

```bash
git add src/executor/builtin_registry/registry.rs src/executor/builtin_registry/config.rs
git commit -m "registry: wire ACP delegate tools into BuiltinToolRegistry"
```

---

### Task 9: Startup Initialization

**Files:**
- Modify: `src/bin/aleph/commands/start/builder/subsystems.rs`
- Modify: `src/bin/aleph/commands/start/mod.rs` (or wherever BuiltinToolConfig is assembled)

**Step 1: Find where BuiltinToolConfig is built and add ACP manager**

In the startup code (likely `subsystems.rs` or the builder), find where `BuiltinToolConfig` is constructed. Add:

```rust
// Initialize ACP manager if enabled
let acp_manager = if config.acp.enabled {
    use crate::acp::manager::{AcpHarnessManager, AcpManagerConfig};

    let mut mgr_config = AcpManagerConfig::default();
    for (id, entry) in &config.acp.harnesses {
        mgr_config.enabled.insert(id.clone(), entry.enabled);
        if let Some(ref exec) = entry.executable {
            mgr_config.executables.insert(id.clone(), exec.clone());
        }
    }
    Some(Arc::new(AcpHarnessManager::with_config(mgr_config)))
} else {
    None
};
```

Then set it on `BuiltinToolConfig`:
```rust
builtin_config.acp_manager = acp_manager.clone();
```

**Step 2: Add shutdown hook**

In the server shutdown sequence, add:
```rust
// Shutdown ACP sessions
if let Some(ref manager) = acp_manager {
    manager.shutdown_all().await;
}
```

**Step 3: Compile check and test**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 4: Commit**

```bash
git add src/bin/aleph/commands/start/
git commit -m "start: initialize AcpHarnessManager on startup when ACP enabled"
```

---

### Task 10: CLI/TUI Split — Create apps/tui crate

**Files:**
- Create: `apps/tui/Cargo.toml`
- Create: `apps/tui/src/main.rs`
- Move: `apps/cli/src/tui/` → `apps/tui/src/tui/` (contents integrated into tui crate)
- Move: `apps/cli/src/client.rs` → `apps/shared/src/client.rs` (shared WebSocket client)
- Modify: `Cargo.toml` (workspace) — add `apps/tui` member
- Modify: `apps/cli/Cargo.toml` — remove ratatui/crossterm deps
- Modify: `apps/cli/src/main.rs` — remove Chat subcommand

**Step 1: Create apps/tui/Cargo.toml**

```toml
[package]
name = "aleph-tui"
version = "0.1.0"
edition = "2021"

# IMPORTANT: This crate MUST NOT depend on alephcore
# It uses only aleph-protocol for wire types + aleph-shared for WebSocket client

[[bin]]
name = "aleph-tui"
path = "src/main.rs"

[dependencies]
# Aleph shared
aleph-protocol = { path = "../../shared/protocol" }
aleph-logging = { path = "../../crates/logging" }
aleph-shared = { path = "../shared" }

# Async
tokio = { version = "1", features = ["rt-multi-thread", "sync", "time", "macros", "io-util", "signal", "net"] }
futures-util = "0.3"

# Terminal UI
crossterm = "0.28"
ratatui = "0.29"
tui-textarea = "0.7"
unicode-width = "0.2"

# CLI
clap = { version = "4.4", features = ["derive"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1.0"
toml = "0.8"

# Misc
tracing = "0.1"
```

**Step 2: Create apps/tui/src/main.rs**

```rust
//! Aleph TUI — Interactive terminal interface for Aleph AI assistant

use clap::Parser;

#[derive(Parser)]
#[command(name = "aleph-tui", about = "Interactive terminal interface for Aleph")]
struct Cli {
    /// Aleph server WebSocket URL
    #[arg(long, default_value = "ws://127.0.0.1:18789/ws")]
    server: String,

    /// Session key to resume
    #[arg(long)]
    session: Option<String>,

    /// Enable verbose output
    #[arg(long, short)]
    verbose: bool,
}

mod tui;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    aleph_logging::init_subscriber(if cli.verbose { "debug" } else { "info" });

    // Launch TUI
    tui::run(&cli.server, cli.session.as_deref()).await
}
```

**Step 3: Move TUI modules from apps/cli/src/tui/ to apps/tui/src/tui/**

Copy all files from `apps/cli/src/tui/` to `apps/tui/src/tui/`:
- `mod.rs`, `app.rs`, `render.rs`, `event.rs`, `slash.rs`, `markdown.rs`, `theme.rs`, `widgets/`

Update imports in each file: change `crate::client::AlephClient` to `aleph_shared::client::AlephClient` (or however the shared client is structured).

**Step 4: Move shared WebSocket client**

If `apps/shared/` already has a client module, the TUI can use it. Otherwise, move `apps/cli/src/client.rs` to `apps/shared/src/client.rs` and have both cli and tui depend on it.

**Step 5: Update apps/cli**

- Remove `chat` subcommand from `apps/cli/src/main.rs`
- Remove `mod tui;` declaration
- Remove ratatui/crossterm/tui-textarea from `apps/cli/Cargo.toml`
- Remove the `apps/cli/src/tui/` directory

**Step 6: Update workspace Cargo.toml**

Add `"apps/tui"` to workspace members list.

**Step 7: Compile check**

Run: `cargo check -p aleph-tui && cargo check -p aleph-cli`
Expected: PASS for both

**Step 8: Commit**

```bash
git add apps/tui/ apps/cli/ apps/shared/ Cargo.toml
git commit -m "apps: split CLI into aleph-cli (management) + aleph-tui (interactive terminal)"
```

---

### Task 11: Protocol & Transport Tests

**Files:**
- Create: `src/acp/tests.rs`

**Step 1: Write comprehensive tests**

```rust
// src/acp/tests.rs
//! Integration tests for ACP protocol and transport

#[cfg(test)]
mod tests {
    use crate::acp::protocol::*;

    #[test]
    fn test_request_initialize_serialization() {
        let req = AcpRequest::initialize("test-1");
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("initialize"));
        assert!(json.contains("aleph"));
        assert!(json.contains("test-1"));
        // Verify NDJSON: no embedded newlines
        assert!(!json.contains('\n'));
    }

    #[test]
    fn test_request_prompt_with_cwd() {
        let req = AcpRequest::prompt("p-1", "hello", Some("/tmp"));
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("hello"));
        assert!(json.contains("/tmp"));
    }

    #[test]
    fn test_request_prompt_without_cwd() {
        let req = AcpRequest::prompt("p-2", "hello", None);
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("hello"));
        assert!(!json.contains("cwd"));
    }

    #[test]
    fn test_response_result_text_extraction() {
        let json = r#"{"jsonrpc":"2.0","id":"1","result":{"content":"the answer"}}"#;
        let resp: AcpResponse = serde_json::from_str(json).unwrap();
        assert!(resp.is_result());
        assert!(!resp.is_notification());
        assert_eq!(resp.text_content(), Some("the answer".to_string()));
    }

    #[test]
    fn test_response_notification() {
        let json = r#"{"jsonrpc":"2.0","method":"progress","params":{"status":"working"}}"#;
        let resp: AcpResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.is_result());
        assert!(resp.is_notification());
        assert_eq!(resp.method, Some("progress".to_string()));
    }

    #[test]
    fn test_response_error() {
        let json = r#"{"jsonrpc":"2.0","id":"1","error":{"code":-32600,"message":"Invalid request"}}"#;
        let resp: AcpResponse = serde_json::from_str(json).unwrap();
        assert!(resp.is_result());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "Invalid request");
    }

    #[test]
    fn test_session_state_transitions() {
        assert_ne!(AcpSessionState::Idle, AcpSessionState::Busy);
        assert_ne!(AcpSessionState::Busy, AcpSessionState::Error);
    }

    #[test]
    fn test_manager_config_defaults() {
        use crate::acp::manager::{AcpHarnessManager, AcpManagerConfig};

        let manager = AcpHarnessManager::new();
        assert!(manager.has_harness("claude-code"));
        assert!(manager.has_harness("codex"));
        assert!(manager.has_harness("gemini"));
        assert!(!manager.has_harness("nonexistent"));
    }

    #[test]
    fn test_manager_config_disable_harness() {
        use crate::acp::manager::{AcpHarnessManager, AcpManagerConfig};

        let mut config = AcpManagerConfig::default();
        config.enabled.insert("codex".to_string(), false);

        let manager = AcpHarnessManager::with_config(config);
        assert!(manager.has_harness("claude-code"));
        assert!(!manager.has_harness("codex")); // disabled
        assert!(manager.has_harness("gemini"));
    }

    #[test]
    fn test_manager_custom_executable() {
        use crate::acp::manager::{AcpHarnessManager, AcpManagerConfig};

        let mut config = AcpManagerConfig::default();
        config.executables.insert("claude-code".to_string(), "/usr/local/bin/claude".to_string());

        let manager = AcpHarnessManager::with_config(config);
        assert!(manager.has_harness("claude-code"));
        assert_eq!(manager.display_name("claude-code"), Some("Claude Code"));
    }
}
```

**Step 2: Add test module to mod.rs**

Add to `src/acp/mod.rs`:
```rust
#[cfg(test)]
mod tests;
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib acp`
Expected: PASS (all tests)

**Step 4: Commit**

```bash
git add src/acp/tests.rs src/acp/mod.rs
git commit -m "test: add ACP protocol, transport, and manager unit tests"
```

---

### Task 12: Mock ACP Server for Integration Tests

**Files:**
- Create: `src/acp/mock_server.rs` (test-only)

**Step 1: Write mock ACP server**

```rust
// src/acp/mock_server.rs
//! Mock ACP server for integration testing
//!
//! Spawns as a subprocess, reads NDJSON from stdin, writes responses to stdout.
//! Supports: initialize → success, prompt → echo back with prefix.

#[cfg(test)]
pub mod mock {
    use std::io::{BufRead, Write};

    /// Run the mock server inline (for tests that spawn it as a process,
    /// use the standalone binary approach instead)
    pub fn run_mock_inline(stdin: impl BufRead, mut stdout: impl Write) {
        for line in stdin.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };

            if line.trim().is_empty() {
                continue;
            }

            let req: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let id = req.get("id").and_then(|v| v.as_str()).unwrap_or("0");
            let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");

            let response = match method {
                "initialize" => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "serverInfo": { "name": "mock-acp", "version": "0.1" },
                        "protocolVersion": "0.1",
                        "capabilities": {}
                    }
                }),
                "prompt" => {
                    let prompt = req.get("params")
                        .and_then(|p| p.get("prompt"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": format!("[mock] Processed: {}", prompt)
                        }
                    })
                },
                "cancel" => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "cancelled": true }
                }),
                _ => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": "Method not found" }
                }),
            };

            let mut resp_line = serde_json::to_string(&response).unwrap();
            resp_line.push('\n');
            let _ = stdout.write_all(resp_line.as_bytes());
            let _ = stdout.flush();
        }
    }
}
```

**Step 2: Add to mod.rs**

```rust
#[cfg(test)]
pub mod mock_server;
```

**Step 3: Commit**

```bash
git add src/acp/mock_server.rs src/acp/mod.rs
git commit -m "test: add mock ACP server for integration testing"
```

---

## Implementation Notes

### File Modification Summary

**Create (new files):**
- `src/acp/mod.rs`
- `src/acp/protocol.rs`
- `src/acp/transport.rs`
- `src/acp/session.rs`
- `src/acp/harness.rs`
- `src/acp/manager.rs`
- `src/acp/harnesses/mod.rs`
- `src/acp/harnesses/claude_code.rs`
- `src/acp/harnesses/codex.rs`
- `src/acp/harnesses/gemini.rs`
- `src/acp/tests.rs`
- `src/acp/mock_server.rs`
- `src/config/types/acp.rs`
- `src/builtin_tools/acp_tools.rs`
- `apps/tui/Cargo.toml`
- `apps/tui/src/main.rs`
- `apps/tui/src/tui/` (moved from apps/cli)

**Modify (existing files):**
- `src/lib.rs` — add `pub mod acp;`
- `src/config/types/mod.rs` — add `pub mod acp;` + re-export
- `src/config/mod.rs` — add `acp: AcpConfig` field
- `src/builtin_tools/mod.rs` — add `pub mod acp_tools;` + re-exports
- `src/executor/builtin_registry/config.rs` — add `acp_manager` field
- `src/executor/builtin_registry/registry.rs` — add ACP tool fields, init, metadata, execution
- `src/bin/aleph/commands/start/` — initialize AcpHarnessManager
- `apps/cli/Cargo.toml` — remove TUI deps
- `apps/cli/src/main.rs` — remove Chat subcommand
- `Cargo.toml` — add `apps/tui` workspace member

### Dependency Order

Tasks 1-5 (ACP core) → Task 6 (config) → Task 7 (tools) → Task 8 (registry) → Task 9 (startup) → Task 10 (CLI/TUI split) → Tasks 11-12 (tests)

Tasks 1-5 can be done sequentially without external dependencies.
Task 10 (CLI/TUI split) is independent of Tasks 1-9 and can be parallelized.
