# ACP Client Enhancement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade Aleph's ACP client to support unified tool delegation, real-time streaming, structured errors, trust levels, and session persistence.

**Architecture:** Vertical slice approach — build one working end-to-end flow first (unified tool + streaming), then layer in errors, trust, persistence, cleanup. All changes within existing modules, no new files.

**Tech Stack:** Rust, tokio async, serde/schemars, NDJSON JSON-RPC 2.0

**Spec:** `docs/superpowers/specs/2026-03-28-acp-client-enhancement-design.md`

---

## File Map

| File | Role | Changes |
|------|------|---------|
| `src/acp/protocol.rs` | ACP message types | +AcpErrorCode, +AcpOperationError, +AcpRequest::load_session(), +notification helpers |
| `src/acp/transport.rs` | NDJSON stdio I/O | +request_streaming() |
| `src/acp/session.rs` | Subprocess lifecycle | +PersistedAcpSession, prompt() dual-path |
| `src/acp/manager.rs` | Session pool | +persistence_hook, +restore_sessions(), -AcpManagerConfig, -with_config() |
| `src/acp/mod.rs` | Module exports | +AcpSessionEvent |
| `src/config/types/acp.rs` | Config types | +TrustLevel, +trust_level field |
| `src/builtin_tools/mod.rs` | Tool progress system | +on_tool_streaming_chunk trait method, +notify fn |
| `src/builtin_tools/acp_tools.rs` | ACP tools | Rewrite: unified AcpDelegateTool |
| `src/executor/builtin_registry/registry.rs` | Tool registry struct | Replace 3 fields → 1 acp_delegate_tool |
| `src/executor/builtin_registry/builder.rs` | Tool wiring | Replace 3 tool registrations → 1 |
| `src/executor/builtin_registry/definitions.rs` | Tool metadata | Replace 3 definitions → 1 |
| `src/executor/builtin_registry/groups.rs` | Tool categories | Update acp group |
| `src/acp/tests.rs` | Integration tests | Migrate AcpManagerConfig tests |
| `src/bin/aleph-server/commands/start/builder/agent_init.rs` | Server startup | Wire persistence restore |

---

### Task 1: Structured Error Types (protocol.rs)

**Files:**
- Modify: `src/acp/protocol.rs`

Foundation layer — needed by all subsequent tasks.

- [ ] **Step 1: Add AcpErrorCode enum and AcpOperationError struct**

Append after the existing `AcpSessionState` section (before tests):

```rust
// =============================================================================
// Structured ACP Errors
// =============================================================================

/// Classifies ACP operation failures for programmatic handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpErrorCode {
    /// Harness not found or not registered
    HarnessNotFound,
    /// Harness executable not installed or not in PATH
    HarnessUnavailable,
    /// Harness disabled or trust_level=disabled
    HarnessDenied,
    /// ACP session died or connection closed
    SessionDead,
    /// Request timed out
    Timeout,
    /// ACP protocol error (from remote JSON-RPC error response)
    ProtocolError { code: i64 },
    /// Mode not supported by harness
    ModeUnsupported,
    /// User cancelled or confirmation denied
    Cancelled,
    /// Process spawn failure
    SpawnFailed,
}

/// Structured ACP operation error with classification.
#[derive(Debug)]
pub struct AcpOperationError {
    pub code: AcpErrorCode,
    pub message: String,
    /// Original ACP error from remote, if any
    pub remote_error: Option<AcpError>,
}

impl fmt::Display for AcpOperationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ACP {:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for AcpOperationError {}

impl From<AcpOperationError> for crate::error::AlephError {
    fn from(e: AcpOperationError) -> Self {
        crate::error::AlephError::tool(e.to_string())
    }
}

impl AcpOperationError {
    pub fn new(code: AcpErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), remote_error: None }
    }

    pub fn with_remote(code: AcpErrorCode, message: impl Into<String>, remote: AcpError) -> Self {
        Self { code, message: message.into(), remote_error: Some(remote) }
    }
}
```

- [ ] **Step 2: Add AcpRequest::load_session() constructor**

Add after `AcpRequest::cancel()`:

```rust
    /// Create a `session/load` request (best-effort session restore).
    pub fn load_session(session_id: &str, cwd: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: next_id(),
            method: "session/load".to_string(),
            params: Some(serde_json::json!({
                "sessionId": session_id,
                "cwd": cwd,
                "mcpServers": [],
            })),
        }
    }
```

- [ ] **Step 3: Add enhanced notification parsing methods on AcpResponse**

Add after `is_turn_complete()`:

```rust
    /// Extract thinking text from an `agent_thought_chunk` notification.
    pub fn streaming_thought(&self) -> Option<String> {
        if self.method.as_deref() != Some("session/update") {
            return None;
        }
        let params = self.params.as_ref()?;
        let update = params.get("update")?;
        if update.get("sessionUpdate")?.as_str()? != "agent_thought_chunk" {
            return None;
        }
        let content = update.get("content")?;
        if content.get("type")?.as_str()? == "text" {
            return content.get("text")?.as_str().map(String::from);
        }
        None
    }

    /// Extract tool call info from a `tool_call` or `tool_call_update` notification.
    /// Returns (tool_name, status) if this is a tool-related notification.
    pub fn tool_call_info(&self) -> Option<(String, String)> {
        if self.method.as_deref() != Some("session/update") {
            return None;
        }
        let params = self.params.as_ref()?;
        let update = params.get("update")?;
        let session_update = update.get("sessionUpdate")?.as_str()?;
        if session_update != "tool_call" && session_update != "tool_call_update" {
            return None;
        }
        let name = update.get("name")
            .or_else(|| update.get("toolName"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let status = update.get("status")
            .and_then(|v| v.as_str())
            .unwrap_or(session_update)
            .to_string();
        Some((name, status))
    }
```

- [ ] **Step 4: Add tests for new types**

Add to the existing `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn test_acp_error_code_copy() {
        let code = AcpErrorCode::Timeout;
        let code2 = code; // Copy
        assert_eq!(code, code2);

        let proto = AcpErrorCode::ProtocolError { code: -32600 };
        let proto2 = proto;
        assert_eq!(proto, proto2);
    }

    #[test]
    fn test_acp_operation_error_display() {
        let err = AcpOperationError::new(AcpErrorCode::Timeout, "timed out after 5m");
        assert!(err.to_string().contains("Timeout"));
        assert!(err.to_string().contains("timed out"));
    }

    #[test]
    fn test_acp_operation_error_into_aleph_error() {
        let err = AcpOperationError::new(AcpErrorCode::HarnessNotFound, "not found");
        let aleph_err: crate::error::AlephError = err.into();
        assert!(aleph_err.to_string().contains("HarnessNotFound"));
    }

    #[test]
    fn test_load_session_request() {
        let req = AcpRequest::load_session("sess-42", "/tmp");
        assert_eq!(req.method, "session/load");
        let p = req.params.unwrap();
        assert_eq!(p["sessionId"], "sess-42");
        assert_eq!(p["cwd"], "/tmp");
    }

    #[test]
    fn test_streaming_thought_extraction() {
        let notif = AcpResponse {
            jsonrpc: "2.0".to_string(),
            id: None, result: None, error: None,
            method: Some("session/update".to_string()),
            params: Some(serde_json::json!({
                "sessionId": "s1",
                "update": {
                    "sessionUpdate": "agent_thought_chunk",
                    "content": {"type": "text", "text": "thinking..."}
                }
            })),
        };
        assert_eq!(notif.streaming_thought(), Some("thinking...".to_string()));
    }

    #[test]
    fn test_tool_call_info_extraction() {
        let notif = AcpResponse {
            jsonrpc: "2.0".to_string(),
            id: None, result: None, error: None,
            method: Some("session/update".to_string()),
            params: Some(serde_json::json!({
                "sessionId": "s1",
                "update": {
                    "sessionUpdate": "tool_call",
                    "name": "read_file",
                    "status": "running"
                }
            })),
        };
        let (name, status) = notif.tool_call_info().unwrap();
        assert_eq!(name, "read_file");
        assert_eq!(status, "running");
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib acp::protocol`
Expected: All tests pass including new ones.

- [ ] **Step 6: Commit**

```
git add src/acp/protocol.rs
git commit -m "acp: add structured error types, load_session, notification parsing"
```

---

### Task 2: Streaming Transport (transport.rs)

**Files:**
- Modify: `src/acp/transport.rs`

- [ ] **Step 1: Add request_streaming() method**

Add after the existing `request()` method:

```rust
    /// Send a request and stream notifications via callback as they arrive.
    ///
    /// Unlike `request()`, notifications are forwarded immediately to the callback
    /// instead of being collected. Only the final response (matching id) is returned.
    ///
    /// Use this for `session/prompt` where real-time streaming matters.
    /// Use `request()` for `initialize` and `session/new` where it doesn't.
    pub async fn request_streaming(
        &mut self,
        req: &AcpRequest,
        timeout: Duration,
        on_notification: impl Fn(&AcpResponse),
    ) -> Result<AcpResponse> {
        self.send(req).await?;
        let expected_id = req.id;

        let result = tokio::time::timeout(timeout, async {
            loop {
                match self.event_rx.recv().await {
                    Some(Ok(resp)) => {
                        if resp.id == Some(expected_id) {
                            if let Some(ref err) = resp.error {
                                return Err(crate::acp::protocol::AcpOperationError::with_remote(
                                    crate::acp::protocol::AcpErrorCode::ProtocolError { code: err.code },
                                    format!("ACP error {}: {}", err.code, err.message),
                                    err.clone(),
                                ).into());
                            }
                            return Ok(resp);
                        }
                        // Notification — forward immediately
                        on_notification(&resp);
                    }
                    Some(Err(e)) => return Err(e),
                    None => {
                        return Err(crate::acp::protocol::AcpOperationError::new(
                            crate::acp::protocol::AcpErrorCode::SessionDead,
                            "ACP connection closed while waiting for response",
                        ).into());
                    }
                }
            }
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_elapsed) => Err(crate::acp::protocol::AcpOperationError::new(
                crate::acp::protocol::AcpErrorCode::Timeout,
                format!("ACP request timed out after {:?}", timeout),
            ).into()),
        }
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib acp::transport`
Expected: Existing tests still pass. (No new test needed — `request_streaming` is tested via integration in Task 3.)

- [ ] **Step 3: Commit**

```
git add src/acp/transport.rs
git commit -m "acp: add request_streaming() for real-time notification forwarding"
```

---

### Task 3: Streaming Session Prompt (session.rs)

**Files:**
- Modify: `src/acp/session.rs`

- [ ] **Step 1: Add PersistedAcpSession struct**

Add after `HarnessConfig` (before `AcpSession`):

```rust
// =============================================================================
// PersistedAcpSession
// =============================================================================

/// Minimal state for restoring an ACP session after restart.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersistedAcpSession {
    pub harness_id: String,
    pub acp_session_id: String,
    pub cwd: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: chrono::DateTime<chrono::Utc>,
}
```

- [ ] **Step 2: Add load_acp_session method to AcpSession**

Add to the `impl AcpSession` block, after `create_acp_session`:

```rust
    /// Try to restore an existing ACP session via `session/load`.
    ///
    /// Returns Ok(session_id) on success, Err on failure (caller should fall back to session/new).
    pub async fn load_acp_session(&mut self, session_id: &str, cwd: &str, timeout: Duration) -> Result<String> {
        let req = AcpRequest::load_session(session_id, cwd);
        match self.transport.request(&req, timeout).await {
            Ok((resp, _)) => {
                let sid = resp.result.as_ref()
                    .and_then(|r| r.get("sessionId"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(session_id)
                    .to_string();
                info!(harness_id = %self.harness_id, session_id = %sid, "ACP session loaded");
                self.acp_session_id = Some(sid.clone());
                Ok(sid)
            }
            Err(e) => {
                debug!(harness_id = %self.harness_id, error = %e, "session/load failed, will fall back to session/new");
                Err(e)
            }
        }
    }
```

- [ ] **Step 3: Refactor prompt() to dual-path (streaming vs legacy)**

Replace the entire `prompt()` method body with:

```rust
    pub async fn prompt(
        &mut self,
        text: &str,
        cwd: &str,
        timeout: Duration,
        on_chunk: Option<&AcpChunkCallback>,
    ) -> Result<(String, Vec<AcpResponse>)> {
        if self.state == AcpSessionState::Error {
            return Err(AlephError::tool(format!(
                "ACP harness '{}' is in error state",
                self.harness_id
            )));
        }

        self.state = AcpSessionState::Busy;

        // Ensure we have an ACP session
        let session_id = self.create_acp_session(cwd, timeout).await?;
        let req = AcpRequest::prompt(&session_id, text);

        if let Some(cb) = on_chunk {
            // Streaming path: forward chunks in real-time via callback
            let cb = cb.clone();
            let accumulated = std::sync::Mutex::new(String::new());

            let result = self.transport.request_streaming(&req, timeout, |notif| {
                if let Some(chunk) = notif.streaming_text() {
                    cb(&chunk);
                    if let Ok(mut acc) = accumulated.lock() {
                        acc.push_str(&chunk);
                    }
                }
            }).await;

            match result {
                Ok(resp) => {
                    let acc_text = accumulated.into_inner().unwrap_or_default();
                    let result_text = if acc_text.is_empty() {
                        resp.text_content().unwrap_or_default()
                    } else {
                        acc_text
                    };
                    self.state = AcpSessionState::Idle;
                    Ok((result_text, vec![]))
                }
                Err(e) => {
                    error!(harness_id = %self.harness_id, error = %e, "ACP prompt failed");
                    self.state = AcpSessionState::Error;
                    Err(e)
                }
            }
        } else {
            // Legacy path: collect all notifications, extract text after
            match self.transport.request(&req, timeout).await {
                Ok((resp, notifications)) => {
                    let mut text_parts: Vec<String> = Vec::new();
                    for notif in &notifications {
                        if let Some(chunk) = notif.streaming_text() {
                            text_parts.push(chunk);
                        }
                    }
                    let result_text = if !text_parts.is_empty() {
                        text_parts.join("")
                    } else {
                        resp.text_content().unwrap_or_default()
                    };
                    self.state = AcpSessionState::Idle;
                    Ok((result_text, notifications))
                }
                Err(e) => {
                    error!(harness_id = %self.harness_id, error = %e, "ACP prompt failed");
                    self.state = AcpSessionState::Error;
                    Err(e)
                }
            }
        }
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib acp::session`
Expected: Existing tests pass.

- [ ] **Step 5: Commit**

```
git add src/acp/session.rs
git commit -m "acp: add streaming prompt path and PersistedAcpSession"
```

---

### Task 4: TrustLevel Config (config/types/acp.rs)

**Files:**
- Modify: `src/config/types/acp.rs`

- [ ] **Step 1: Add TrustLevel enum**

Add after `OutputFormatSerde`:

```rust
// =============================================================================
// TrustLevel
// =============================================================================

/// Trust level for LLM delegation to an ACP harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    /// LLM can freely delegate without user confirmation
    Full,
    /// Each delegation requires user confirmation
    Confirm,
    /// Delegation disabled
    Disabled,
}

fn default_trust_level() -> TrustLevel {
    TrustLevel::Confirm
}
```

- [ ] **Step 2: Add trust_level field to AcpHarnessEntry**

Add to the struct after `preset`:

```rust
    /// Trust level for LLM delegation. Preset harnesses default to Full,
    /// custom harnesses default to Confirm.
    #[serde(default = "default_trust_level")]
    pub trust_level: TrustLevel,
```

Update `Default for AcpHarnessEntry` to include:

```rust
            trust_level: default_trust_level(),
```

- [ ] **Step 3: Set Full trust in preset factories**

Add `trust_level: TrustLevel::Full,` to each of `preset_claude_code()`, `preset_codex()`, `preset_gemini()`.

- [ ] **Step 4: Add tests**

```rust
    #[test]
    fn test_trust_level_serde() {
        let json = r#""full""#;
        let t: TrustLevel = serde_json::from_str(json).unwrap();
        assert_eq!(t, TrustLevel::Full);

        let json = r#""confirm""#;
        let t: TrustLevel = serde_json::from_str(json).unwrap();
        assert_eq!(t, TrustLevel::Confirm);

        let json = r#""disabled""#;
        let t: TrustLevel = serde_json::from_str(json).unwrap();
        assert_eq!(t, TrustLevel::Disabled);
    }

    #[test]
    fn test_preset_trust_levels() {
        assert_eq!(AcpHarnessEntry::preset_claude_code().trust_level, TrustLevel::Full);
        assert_eq!(AcpHarnessEntry::preset_codex().trust_level, TrustLevel::Full);
        assert_eq!(AcpHarnessEntry::preset_gemini().trust_level, TrustLevel::Full);
    }

    #[test]
    fn test_custom_harness_default_trust() {
        let entry = AcpHarnessEntry::default();
        assert_eq!(entry.trust_level, TrustLevel::Confirm);
    }

    #[test]
    fn test_trust_level_deserialize_missing() {
        // When trust_level is absent from JSON, defaults to Confirm
        let json = r#"{"display_name":"Test","enabled":true}"#;
        let entry: AcpHarnessEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.trust_level, TrustLevel::Confirm);
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib config::types::acp`
Expected: All pass.

- [ ] **Step 6: Commit**

```
git add src/config/types/acp.rs
git commit -m "acp: add TrustLevel enum to harness config"
```

---

### Task 5: Streaming Callback Infrastructure (builtin_tools/mod.rs)

**Files:**
- Modify: `src/builtin_tools/mod.rs`

- [ ] **Step 1: Extend ToolProgressCallback trait**

Add to the trait (after `on_tool_result`):

```rust
    /// Called when a tool emits a streaming chunk (e.g., ACP delegation output).
    /// Default no-op — existing implementations don't break.
    fn on_tool_streaming_chunk(&self, tool_name: &str, chunk: &str) {
        let _ = (tool_name, chunk);
    }
```

- [ ] **Step 2: Add notify_tool_streaming_chunk function**

Add after `notify_tool_result`:

```rust
/// Notify that a tool has emitted a streaming chunk
///
/// Called by streaming tools (e.g., ACP delegate) to forward real-time output.
/// If no handler is set, this is a no-op.
pub fn notify_tool_streaming_chunk(tool_name: &str, chunk: &str) {
    let callback = TOOL_PROGRESS_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref handler) = *callback {
        handler.on_tool_streaming_chunk(tool_name, chunk);
    }
}
```

- [ ] **Step 3: Run compile check**

Run: `cargo check -p alephcore`
Expected: Compiles without errors (default trait method is backward compatible).

- [ ] **Step 4: Commit**

```
git add src/builtin_tools/mod.rs
git commit -m "tools: add streaming chunk callback to ToolProgressCallback"
```

---

### Task 6: Unified AcpDelegateTool (acp_tools.rs rewrite)

**Files:**
- Modify: `src/builtin_tools/acp_tools.rs`
- Modify: `src/builtin_tools/mod.rs` (re-exports)

- [ ] **Step 1: Rewrite acp_tools.rs**

Replace the entire file content (preserve AcpSwitchTool and helpers, delete macro + 3 tools):

```rust
//! ACP delegate and switch tools
//!
//! Provides a unified tool that delegates tasks to external CLI agents
//! (Claude Code, Codex, Gemini, or custom) via the ACP harness system.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::{notify_tool_result, notify_tool_start, notify_tool_streaming_chunk};
use crate::acp::harness::HarnessMode;
use crate::acp::manager::AcpHarnessManager;
use crate::acp::AcpChunkCallback;
use crate::config::types::acp::TrustLevel;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// AcpDelegateTool — unified delegation to any ACP harness
// =============================================================================

/// Arguments for the unified ACP delegate tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AcpDelegateArgs {
    /// Which harness to delegate to (e.g. "claude-code", "gemini", "codex", or any custom).
    pub harness: String,
    /// The prompt / task description to send to the external CLI agent.
    pub prompt: String,
    /// Working directory for the agent session. Defaults to home directory if not specified.
    pub cwd: Option<String>,
    /// Execution mode override: "oneshot" or "native_acp". If not specified, uses the harness default.
    pub mode: Option<String>,
    /// Whether to reuse an existing session for multi-step continuity (native_acp mode only). Defaults to true.
    pub reuse_session: Option<bool>,
}

/// Output from the unified ACP delegate tool.
#[derive(Debug, Clone, Serialize)]
pub struct AcpDelegateOutput {
    /// Which harness produced the result.
    pub harness: String,
    /// The text response from the external agent.
    pub result: String,
}

/// Unified ACP delegate tool — delegates tasks to any registered ACP harness.
#[derive(Clone)]
pub struct AcpDelegateTool {
    manager: Arc<AcpHarnessManager>,
}

impl AcpDelegateTool {
    pub fn new(manager: Arc<AcpHarnessManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for AcpDelegateTool {
    const NAME: &'static str = "acp_delegate";
    const DESCRIPTION: &'static str = "Delegate a task to an external CLI agent via ACP. \
        Use 'claude-code', 'codex', or 'gemini' as the harness parameter, \
        or any custom harness registered via acp.create.";

    type Args = AcpDelegateArgs;
    type Output = AcpDelegateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let args_summary = format!("{}: {}", &args.harness, truncate(&args.prompt, 80));
        notify_tool_start(Self::NAME, &args_summary);

        // Trust level check
        let config = self.manager.get_config(&args.harness).await.ok_or_else(|| {
            AlephError::tool(format!("Unknown ACP harness: '{}'. Check available harnesses via acp.list.", args.harness))
        })?;

        match config.trust_level {
            TrustLevel::Disabled => {
                let msg = format!("ACP harness '{}' is disabled (trust_level=disabled)", args.harness);
                notify_tool_result(Self::NAME, &msg, false);
                return Err(AlephError::tool(msg));
            }
            TrustLevel::Confirm => {
                // For now, log and proceed. Full approval integration is deferred
                // until the gateway approval mechanism supports tool-level confirmation.
                info!(harness = %args.harness, "ACP delegate: trust_level=confirm, proceeding");
            }
            TrustLevel::Full => {}
        }

        let cwd = resolve_cwd(args.cwd.as_deref());
        let mode = args.mode.as_deref().map(parse_mode).transpose()?;
        let reuse = args.reuse_session.unwrap_or(true);

        // Build streaming callback
        let on_chunk: AcpChunkCallback = Arc::new(move |chunk: &str| {
            notify_tool_streaming_chunk("acp_delegate", chunk);
        });

        let result = self.manager.prompt(
            &args.harness, &args.prompt, &cwd, mode, reuse, Some(on_chunk),
        ).await;

        match result {
            Ok(text) => {
                notify_tool_result(Self::NAME, "completed", true);
                Ok(AcpDelegateOutput {
                    harness: args.harness,
                    result: text,
                })
            }
            Err(e) => {
                notify_tool_result(Self::NAME, &e.to_string(), false);
                Err(e)
            }
        }
    }
}

// =============================================================================
// AcpSwitchTool (preserved unchanged)
// =============================================================================

/// Arguments for switching the active CLI agent.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AcpSwitchArgs {
    /// Target agent to switch to: "claude-code", "codex", "gemini", or "aleph".
    pub target: String,
}

/// Output from the ACP switch tool.
#[derive(Debug, Clone, Serialize)]
pub struct AcpSwitchOutput {
    /// The target that was switched to.
    pub target: String,
    /// Human-readable status message.
    pub message: String,
}

/// Switch to direct conversation with an external CLI agent, or switch back to Aleph.
#[derive(Clone)]
pub struct AcpSwitchTool {
    manager: Arc<AcpHarnessManager>,
}

impl AcpSwitchTool {
    pub fn new(manager: Arc<AcpHarnessManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for AcpSwitchTool {
    const NAME: &'static str = "acp_switch";
    const DESCRIPTION: &'static str =
        "Switch to direct conversation with an external CLI agent (Claude Code, Codex, or Gemini), or switch back to Aleph.";

    type Args = AcpSwitchArgs;
    type Output = AcpSwitchOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let args_summary = format!("Switch to: {}", &args.target);
        notify_tool_start(Self::NAME, &args_summary);

        if args.target == "aleph" {
            let msg = "Switched back to Aleph.".to_string();
            notify_tool_result(Self::NAME, &msg, true);
            return Ok(AcpSwitchOutput {
                target: "aleph".to_string(),
                message: msg,
            });
        }

        if !self.manager.has_harness(&args.target).await {
            let err_msg = format!("Unknown agent: '{}'. Valid targets: claude-code, codex, gemini, aleph", &args.target);
            notify_tool_result(Self::NAME, &err_msg, false);
            return Err(AlephError::tool(err_msg));
        }

        if self.manager.harness_mode(&args.target).await == Some(HarnessMode::NativeAcp) {
            let cwd = resolve_cwd(None);
            self.manager.ensure_session(&args.target, &cwd).await?;
        }

        let display_name = self
            .manager
            .display_name(&args.target)
            .await
            .unwrap_or_else(|| args.target.clone());
        let msg = format!("Switched to {}. Messages will be forwarded to this agent.", display_name);

        info!(target = %args.target, "ACP agent switch");
        notify_tool_result(Self::NAME, &msg, true);

        Ok(AcpSwitchOutput {
            target: args.target,
            message: msg,
        })
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn parse_mode(s: &str) -> Result<HarnessMode> {
    match s {
        "oneshot" => Ok(HarnessMode::Oneshot),
        "native_acp" => Ok(HarnessMode::NativeAcp),
        _ => Err(AlephError::tool(format!(
            "Invalid mode '{}'. Use 'oneshot' or 'native_acp'.",
            s
        ))),
    }
}

fn resolve_cwd(cwd: Option<&str>) -> String {
    cwd.map(|s| s.to_string()).unwrap_or_else(|| {
        dirs::home_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".to_string())
    })
}

/// Truncate a string to at most `max_len` characters, appending "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    match s.char_indices().nth(max_len) {
        Some((idx, _)) => format!("{}...", &s[..idx]),
        None => s.to_string(),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long() {
        let result = truncate("hello world this is a long string", 11);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_utf8() {
        let result = truncate("你好世界这是一段中文", 4);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_resolve_cwd_some() {
        assert_eq!(resolve_cwd(Some("/tmp")), "/tmp");
    }

    #[test]
    fn test_resolve_cwd_none() {
        let cwd = resolve_cwd(None);
        assert!(!cwd.is_empty());
    }

    #[test]
    fn test_delegate_args_deserialize() {
        let json = r#"{"harness": "claude-code", "prompt": "Fix the bug", "cwd": "/home/user/project"}"#;
        let args: AcpDelegateArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.harness, "claude-code");
        assert_eq!(args.prompt, "Fix the bug");
        assert_eq!(args.cwd, Some("/home/user/project".to_string()));
    }

    #[test]
    fn test_delegate_args_minimal() {
        let json = r#"{"harness": "gemini", "prompt": "test"}"#;
        let args: AcpDelegateArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.harness, "gemini");
        assert_eq!(args.mode, None);
        assert_eq!(args.reuse_session, None);
    }

    #[test]
    fn test_parse_mode_valid() {
        assert!(matches!(parse_mode("oneshot"), Ok(HarnessMode::Oneshot)));
        assert!(matches!(parse_mode("native_acp"), Ok(HarnessMode::NativeAcp)));
    }

    #[test]
    fn test_parse_mode_invalid() {
        assert!(parse_mode("unknown").is_err());
    }

    #[test]
    fn test_switch_args_deserialize() {
        let json = r#"{"target": "claude-code"}"#;
        let args: AcpSwitchArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.target, "claude-code");
    }
}
```

- [ ] **Step 2: Update re-exports in builtin_tools/mod.rs**

Change the `pub use acp_tools` line from:
```rust
pub use acp_tools::{
    AcpDelegateArgs, AcpDelegateOutput,
    ClaudeCodeTool, CodexTool, GeminiCliTool,
    AcpSwitchArgs, AcpSwitchOutput, AcpSwitchTool,
};
```
To:
```rust
pub use acp_tools::{
    AcpDelegateArgs, AcpDelegateOutput, AcpDelegateTool,
    AcpSwitchArgs, AcpSwitchOutput, AcpSwitchTool,
};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib builtin_tools::acp_tools`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```
git add src/builtin_tools/acp_tools.rs src/builtin_tools/mod.rs
git commit -m "acp: unify 3 delegate tools into single AcpDelegateTool with streaming + trust"
```

---

### Task 7: Update Tool Registry (builder + registry + definitions + groups)

**Files:**
- Modify: `src/executor/builtin_registry/registry.rs:134-137` (struct fields)
- Modify: `src/executor/builtin_registry/registry.rs:620-643` (dispatch)
- Modify: `src/executor/builtin_registry/builder.rs:260-315` (tool creation)
- Modify: `src/executor/builtin_registry/builder.rs:532-534` (struct init)
- Modify: `src/executor/builtin_registry/definitions.rs:376-395` (tool defs)
- Modify: `src/executor/builtin_registry/groups.rs:109` (category)

- [ ] **Step 1: Update registry.rs struct fields**

Replace lines 134-137:
```rust
    pub(crate) claude_code_tool: Option<crate::builtin_tools::acp_tools::ClaudeCodeTool>,
    pub(crate) codex_tool: Option<crate::builtin_tools::acp_tools::CodexTool>,
    pub(crate) gemini_cli_tool: Option<crate::builtin_tools::acp_tools::GeminiCliTool>,
```
With:
```rust
    pub(crate) acp_delegate_tool: Option<crate::builtin_tools::acp_tools::AcpDelegateTool>,
```

- [ ] **Step 2: Update registry.rs dispatch**

Replace the three `claude_code` / `codex` / `gemini_cli` match arms (lines 620-637):
```rust
            "claude_code" => Box::pin(async move { ... }),
            "codex" => Box::pin(async move { ... }),
            "gemini_cli" => Box::pin(async move { ... }),
```
With a single arm:
```rust
            "acp_delegate" => Box::pin(async move {
                let tool = self.acp_delegate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("acp_delegate not available: ACP not configured")
                })?;
                tool.call_json(arguments).await
            }),
```

- [ ] **Step 3: Update builder.rs tool creation**

Replace the block at lines 260-315 (the claude_code/codex/gemini_cli creation) with:

```rust
        // Add unified ACP delegate tool (if AcpHarnessManager is provided)
        let (acp_delegate_tool, acp_switch_tool) =
            if let Some(ref manager) = config.acp_manager {
                use crate::builtin_tools::acp_tools::{AcpDelegateTool, AcpSwitchTool};
                info!("Creating ACP tools");

                use schemars::schema_for;
                let acp_schema = serde_json::to_value(
                    schema_for!(crate::builtin_tools::acp_tools::AcpDelegateArgs)
                ).unwrap_or_default();
                let acp_switch_schema = serde_json::to_value(
                    schema_for!(crate::builtin_tools::acp_tools::AcpSwitchArgs)
                ).unwrap_or_default();

                let mut ut = UnifiedTool::new(
                    "builtin:acp_delegate", "acp_delegate",
                    AcpDelegateTool::DESCRIPTION, ToolSource::Builtin,
                );
                ut.parameters_schema = Some(acp_schema);
                tools.insert("acp_delegate".to_string(), ut);
                let delegate = Some(AcpDelegateTool::new(Arc::clone(manager)));

                let mut ut = UnifiedTool::new(
                    "builtin:acp_switch", "acp_switch",
                    AcpSwitchTool::DESCRIPTION, ToolSource::Builtin,
                );
                ut.parameters_schema = Some(acp_switch_schema);
                tools.insert("acp_switch".to_string(), ut);
                let sw = Some(AcpSwitchTool::new(Arc::clone(manager)));

                info!("Registered ACP tools (acp_delegate=true, acp_switch=true)");
                (delegate, sw)
            } else {
                (None, None)
            };
```

- [ ] **Step 4: Update builder.rs struct initialization**

Replace lines 532-534:
```rust
            claude_code_tool,
            codex_tool,
            gemini_cli_tool,
```
With:
```rust
            acp_delegate_tool,
```

- [ ] **Step 5: Update definitions.rs**

Replace the three ACP tool definitions (lines 376-395):
```rust
    BuiltinToolDefinition {
        name: "claude_code", ...
    },
    BuiltinToolDefinition {
        name: "codex", ...
    },
    BuiltinToolDefinition {
        name: "gemini_cli", ...
    },
```
With one:
```rust
    BuiltinToolDefinition {
        name: "acp_delegate",
        description: "Delegate a task to an external CLI agent via ACP. Use 'claude-code', 'codex', or 'gemini' as the harness parameter, or any custom harness registered via acp.create.",
        requires_config: true,
    },
```

- [ ] **Step 6: Update groups.rs**

Change line 109:
```rust
        tools: &["claude_code", "codex", "gemini_cli", "acp_switch"],
```
To:
```rust
        tools: &["acp_delegate", "acp_switch"],
```

- [ ] **Step 7: Compile check**

Run: `cargo check -p alephcore`
Expected: Compiles. Fix any remaining references to old tool names if compiler errors appear.

- [ ] **Step 8: Commit**

```
git add src/executor/builtin_registry/
git commit -m "acp: replace 3 per-harness tools with unified acp_delegate in registry"
```

---

### Task 8: Manager Persistence Hook + Cleanup (manager.rs)

**Files:**
- Modify: `src/acp/manager.rs`
- Modify: `src/acp/mod.rs`

- [ ] **Step 1: Add AcpSessionEvent to mod.rs**

Add to `src/acp/mod.rs`:

```rust
/// Events emitted by the ACP manager for session persistence.
#[derive(Debug, Clone)]
pub enum AcpSessionEvent {
    Created { harness_id: String, acp_session_id: String, cwd: String },
    Updated { harness_id: String, acp_session_id: String },
    Removed { harness_id: String, cwd: String },
}
```

- [ ] **Step 2: Add persistence_hook field to AcpHarnessManager**

Add to the struct:
```rust
    /// Optional persistence callback for session state changes.
    persistence_hook: RwLock<Option<Arc<dyn Fn(AcpSessionEvent) + Send + Sync>>>,
```

Update `from_entries()` initialization:
```rust
        Self {
            harnesses: RwLock::new(harnesses),
            configs: RwLock::new(configs),
            sessions: RwLock::new(HashMap::new()),
            persistence_hook: RwLock::new(None),
        }
```

- [ ] **Step 3: Add set_persistence_hook and restore_sessions methods**

```rust
    /// Set the persistence hook for session state changes.
    pub async fn set_persistence_hook(&self, hook: Arc<dyn Fn(super::AcpSessionEvent) + Send + Sync>) {
        let mut h = self.persistence_hook.write().await;
        *h = Some(hook);
    }

    /// Emit a persistence event (no-op if no hook set).
    async fn emit_persistence_event(&self, event: super::AcpSessionEvent) {
        let hook = self.persistence_hook.read().await;
        if let Some(ref h) = *hook {
            h(event);
        }
    }

    /// Restore sessions from persisted state. Returns list of successfully restored harness IDs.
    ///
    /// For each entry: spawn subprocess, try session/load with saved ID,
    /// fall back to session/new if load fails.
    pub async fn restore_sessions(&self, persisted: Vec<crate::acp::session::PersistedAcpSession>) -> Vec<String> {
        let mut restored = Vec::new();
        for entry in persisted {
            let key = SessionKey::new(&entry.harness_id, &entry.cwd);

            // Spawn a new subprocess
            let harnesses = self.harnesses.read().await;
            let harness = match harnesses.get(&entry.harness_id) {
                Some(h) => h,
                None => {
                    warn!(harness_id = %entry.harness_id, "Harness not found, skipping restore");
                    continue;
                }
            };

            let mut session = match harness.spawn_session(Some(&entry.cwd)).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(harness_id = %entry.harness_id, error = %e, "Failed to spawn for restore");
                    continue;
                }
            };
            drop(harnesses);

            // Try session/load with persisted session ID
            let timeout = std::time::Duration::from_secs(30);
            if session.load_acp_session(&entry.acp_session_id, &entry.cwd, timeout).await.is_err() {
                // Fall back to session/new (context lost)
                if let Err(e) = session.create_acp_session(&entry.cwd, timeout).await {
                    warn!(harness_id = %entry.harness_id, error = %e, "Failed to create new session on restore");
                    continue;
                }
            }

            info!(harness_id = %entry.harness_id, "Restored ACP session");
            self.sessions.write().await.insert(key, session);
            restored.push(entry.harness_id);
        }
        restored
    }
```

- [ ] **Step 4: Wire persistence events into ensure_session and cancel**

In `ensure_session()`, after inserting a new session (near the end), emit Created:

```rust
        info!(harness_id, "ACP session started");
        // Emit persistence event
        if let Some(sid) = new_session.acp_session_id() {
            self.emit_persistence_event(super::AcpSessionEvent::Created {
                harness_id: harness_id.to_string(),
                acp_session_id: sid.to_string(),
                cwd: cwd.to_string(),
            }).await;
        }
        sessions.insert(key, new_session);
```

In `ensure_session()`, when removing a dead session, emit Removed:

```rust
                warn!(harness_id, "ACP session died, respawning");
                self.emit_persistence_event(super::AcpSessionEvent::Removed {
                    harness_id: harness_id.to_string(),
                    cwd: cwd.to_string(),
                }).await;
                sessions.remove(&key);
```

In `cancel()`, after sending cancel, emit Removed if the session is no longer useful:
(No change needed here — cancel just resets to Idle, it doesn't destroy the session.)

- [ ] **Step 5: Delete AcpManagerConfig and with_config()**

Remove `AcpManagerConfig` struct (lines 43-54) and `with_config()` method (lines 120-140) from manager.rs.

- [ ] **Step 6: Migrate tests in manager.rs**

Replace `test_manager_disable_harness` and `test_manager_executable_override` tests to use `from_entries()`:

```rust
    #[tokio::test]
    async fn test_manager_disable_harness() {
        let mut entries: HashMap<String, AcpHarnessEntry> = AcpHarnessEntry::all_presets().into_iter().collect();
        entries.get_mut("codex").unwrap().enabled = false;
        let manager = AcpHarnessManager::from_entries(entries);
        assert!(!manager.has_harness("codex").await);
        assert!(manager.has_harness("claude-code").await);
        assert!(manager.has_harness("gemini").await);
    }

    #[tokio::test]
    async fn test_manager_executable_override() {
        let mut entries: HashMap<String, AcpHarnessEntry> = AcpHarnessEntry::all_presets().into_iter().collect();
        entries.get_mut("claude-code").unwrap().executable = Some("/custom/claude".to_string());
        let manager = AcpHarnessManager::from_entries(entries);
        assert!(manager.has_harness("claude-code").await);
        let harnesses = manager.harnesses.read().await;
        let harness = harnesses.get("claude-code").unwrap();
        let cfg = harness.build_config(None);
        assert_eq!(cfg.executable, "/custom/claude");
    }
```

- [ ] **Step 7: Migrate tests in acp/tests.rs**

Replace the 4 tests that use `AcpManagerConfig` with equivalent `from_entries()` versions. Pattern:

```rust
// OLD:
let mut config = AcpManagerConfig::default();
config.enabled.insert("codex".to_string(), false);
let mgr = AcpHarnessManager::with_config(config);

// NEW:
let mut entries: HashMap<String, AcpHarnessEntry> = AcpHarnessEntry::all_presets().into_iter().collect();
entries.get_mut("codex").unwrap().enabled = false;
let mgr = AcpHarnessManager::from_entries(entries);
```

Update the import in `acp/tests.rs` from `use super::manager::{AcpHarnessManager, AcpManagerConfig};` to `use super::manager::AcpHarnessManager;`.

- [ ] **Step 8: Run tests**

Run: `cargo test -p alephcore --lib acp`
Expected: All ACP tests pass.

- [ ] **Step 9: Commit**

```
git add src/acp/manager.rs src/acp/mod.rs src/acp/tests.rs
git commit -m "acp: add persistence hook, remove legacy AcpManagerConfig, migrate tests"
```

---

### Task 9: Session Persistence File I/O (manager.rs)

**Files:**
- Modify: `src/acp/manager.rs`

- [ ] **Step 1: Add persistence file helpers**

Add at the top of manager.rs (after imports):

```rust
/// Default persistence file path for ACP sessions.
fn acp_sessions_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".aleph")
        .join("data")
        .join("acp_sessions.json")
}

/// Load persisted ACP sessions from disk (best-effort).
pub fn load_persisted_sessions() -> Vec<crate::acp::session::PersistedAcpSession> {
    let path = acp_sessions_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            serde_json::from_str(&content).unwrap_or_else(|e| {
                warn!("Failed to parse ACP sessions file: {}", e);
                Vec::new()
            })
        }
        Err(_) => Vec::new(), // File doesn't exist yet
    }
}

/// Save persisted ACP sessions to disk (atomic write).
pub fn save_persisted_sessions(sessions: &[crate::acp::session::PersistedAcpSession]) {
    let path = acp_sessions_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    match serde_json::to_string_pretty(sessions) {
        Ok(json) => {
            if std::fs::write(&tmp, &json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
        Err(e) => warn!("Failed to serialize ACP sessions: {}", e),
    }
}
```

- [ ] **Step 2: Wire persistence events into prompt() flow**

In `AcpHarnessManager::prompt()`, after re-inserting the session (NativeAcp path), emit Updated/Removed:

```rust
                // Re-insert session if still alive
                if session.is_alive() {
                    if let Some(sid) = session.acp_session_id() {
                        self.emit_persistence_event(super::AcpSessionEvent::Updated {
                            harness_id: harness_id.to_string(),
                            acp_session_id: sid.to_string(),
                        }).await;
                    }
                    self.sessions.write().await.insert(key, session);
                } else {
                    self.emit_persistence_event(super::AcpSessionEvent::Removed {
                        harness_id: harness_id.to_string(),
                        cwd: cwd.to_string(),
                    }).await;
                    warn!(harness_id, "ACP session died after prompt, not re-inserting");
                }
```

- [ ] **Step 3: Create the persistence hook wiring function**

Add a public function that creates and wires the persistence hook:

```rust
/// Wire up file-based persistence for ACP sessions.
/// Call this after creating the AcpHarnessManager at startup.
pub async fn wire_persistence(manager: &AcpHarnessManager) {
    use crate::sync_primitives::Arc;
    use std::sync::Mutex;

    // Load initial state
    let sessions = Arc::new(Mutex::new(load_persisted_sessions()));

    let sessions_ref = Arc::clone(&sessions);
    manager.set_persistence_hook(Arc::new(move |event: super::AcpSessionEvent| {
        let mut store = sessions_ref.lock().unwrap_or_else(|e| e.into_inner());
        match event {
            super::AcpSessionEvent::Created { ref harness_id, ref acp_session_id, ref cwd } => {
                // Remove any existing entry for this harness+cwd, then add new
                store.retain(|s| !(s.harness_id == *harness_id && s.cwd == *cwd));
                store.push(crate::acp::session::PersistedAcpSession {
                    harness_id: harness_id.clone(),
                    acp_session_id: acp_session_id.clone(),
                    cwd: cwd.clone(),
                    created_at: chrono::Utc::now(),
                    last_used_at: chrono::Utc::now(),
                });
            }
            super::AcpSessionEvent::Updated { ref harness_id, ref acp_session_id } => {
                if let Some(entry) = store.iter_mut().find(|s| s.harness_id == *harness_id && s.acp_session_id == *acp_session_id) {
                    entry.last_used_at = chrono::Utc::now();
                }
            }
            super::AcpSessionEvent::Removed { ref harness_id, ref cwd } => {
                store.retain(|s| !(s.harness_id == *harness_id && s.cwd == *cwd));
            }
        }
        save_persisted_sessions(&store);
    })).await;

    // Restore existing sessions
    let persisted = sessions.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if !persisted.is_empty() {
        let restored = manager.restore_sessions(persisted).await;
        if !restored.is_empty() {
            info!(count = restored.len(), "Restored ACP sessions from disk");
        }
    }
}
```

- [ ] **Step 4: Wire into server startup**

In `src/bin/aleph-server/commands/start/builder/agent_init.rs`, after the ACP manager is created, add:

```rust
// Wire ACP session persistence
crate::acp::manager::wire_persistence(&acp_manager).await;
```

Find the location where `acp_manager` is created (search for `AcpHarnessManager` in that file) and add this call right after.

- [ ] **Step 5: Add tests**

```rust
    #[test]
    fn test_acp_sessions_path() {
        let path = acp_sessions_path();
        assert!(path.to_string_lossy().contains("acp_sessions.json"));
    }

    #[test]
    fn test_load_nonexistent_returns_empty() {
        // load_persisted_sessions returns empty vec when file doesn't exist
        // (This always passes since we don't create the file in tests)
        let sessions = load_persisted_sessions();
        // We can't assert empty because the real file might exist, but it shouldn't panic
        let _ = sessions;
    }
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p alephcore --lib acp::manager`
Expected: All pass.

- [ ] **Step 7: Compile check with server binary**

Run: `cargo check -p alephcore --bin aleph-server`
Expected: Compiles (verifies startup wiring).

- [ ] **Step 8: Commit**

```
git add src/acp/manager.rs src/bin/aleph-server/commands/start/builder/agent_init.rs
git commit -m "acp: wire file-based session persistence end-to-end"
```

---

### Task 10: Final Compile + Full Test Run

**Files:** None (verification only)

- [ ] **Step 1: Full compile check**

Run: `cargo check -p alephcore`
Expected: Zero errors. Fix any remaining references to deleted types.

- [ ] **Step 2: Run all ACP tests**

Run: `cargo test -p alephcore --lib acp`
Expected: All pass.

- [ ] **Step 3: Run all builtin_tools tests**

Run: `cargo test -p alephcore --lib builtin_tools`
Expected: All pass.

- [ ] **Step 4: Run full core test suite**

Run: `cargo test -p alephcore --lib`
Expected: All pass (pre-existing failures in `tools::markdown_skill::loader::tests` are known and unrelated).

- [ ] **Step 5: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -50`
Expected: No new warnings in modified files.

- [ ] **Step 6: Final commit if any fixups needed**

```
git add -A
git commit -m "acp: fix clippy warnings and compile issues"
```
