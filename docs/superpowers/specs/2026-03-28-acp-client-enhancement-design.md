# ACP Client Enhancement Design

**Date**: 2026-03-28
**Scope**: ACP Client layer upgrade — tool unification, streaming, errors, trust, persistence
**Approach**: Vertical slice (incremental enhancement on existing architecture)

## Background

Aleph's ACP module manages external CLI agents (Claude Code, Codex, Gemini) via a well-structured harness trait system. Comparing with OpenClaw's ACP bridge and acpx CLI client reveals significant gaps in the client layer: no real-time streaming to UI, fragmented tool registration (one tool per harness), no structured errors, no trust level control, and no session persistence.

This design addresses all six gaps while preserving Aleph's existing architecture (harness trait, session pool, NDJSON transport).

## Reference Implementations

- **OpenClaw** (`~/GitHub/openclaw`): ACP bridge between IDE and Gateway. Key patterns: event translation layer, session LRU with TTL, tool location extraction, rate limiting, session config options.
- **acpx** (`~/GitHub/acpx`): Headless CLI client for ACP sessions. Key patterns: session persistence to JSON files, directory-walk session lookup, queue IPC, flows runtime, permission policy system, structured error codes.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Implementation strategy | Vertical slice (A) | Existing abstractions are sound; gaps are "wiring" not "architecture" |
| Permission model | Simplified trust_level per harness | External agents have their own safety; Aleph only gates delegation |
| Persistence location | File-based (`~/.aleph/data/acp_sessions.json`) | Gateway session store is in-memory only; ACP state needs disk persistence |
| Tool structure | Single unified acp_delegate | Dynamic harness support without code changes |

---

## 1. Unified acp_delegate Tool

### Problem
Three macro-generated tools (`claude_code`, `codex`, `gemini_cli`) plus `acp_switch`. Not extensible — each new harness requires a new tool struct.

### Design

Replace with a single `AcpDelegateTool` struct. The `harness` field selects which agent to delegate to:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AcpDelegateArgs {
    /// Which harness to delegate to (e.g. "claude-code", "gemini", "codex", or any custom)
    pub harness: String,
    /// The prompt/task description
    pub prompt: String,
    /// Working directory (optional, defaults to home)
    pub cwd: Option<String>,
    /// Mode override: "oneshot" or "native_acp" (optional)
    pub mode: Option<String>,
    /// Reuse existing session for continuity (default true, native_acp only)
    pub reuse_session: Option<bool>,
}
```

### Dynamic description

`AlephTool::DESCRIPTION` is `&'static str` and cannot be dynamically generated. Instead, the tool uses a fixed description that instructs the LLM to check available harnesses:

```rust
const DESCRIPTION: &'static str = "Delegate a task to an external CLI agent via ACP. \
    Use 'claude-code', 'codex', or 'gemini' as the harness parameter, \
    or any custom harness registered via acp.create.";
```

The LLM discovers custom harness IDs through the existing `acp.list` RPC (which powers the settings panel) or via the `list_tools` meta-tool which can enumerate capabilities.

### Deletions
- `acp_delegate_tool!` macro — entire definition
- `ClaudeCodeTool`, `CodexTool`, `GeminiCliTool` — three macro-generated structs
- `AcpDelegateOutput` — replaced by new output type with streaming metadata

### Preserved
- `AcpSwitchTool` — agent mode switching is a separate concern
- Helper functions (`resolve_cwd`, `truncate`, `parse_mode`)

### Registration
`executor/builtin_registry/builder.rs` registers one `AcpDelegateTool` instead of three separate tools. The re-export in `builtin_tools/mod.rs` changes accordingly: remove `ClaudeCodeTool, CodexTool, GeminiCliTool`, add `AcpDelegateTool`.

---

## 2. Real-time Streaming

### Problem
`transport.request()` collects all notifications into a Vec, only returning when the final response arrives. `session.prompt()` processes chunks after the fact. Users see no output until the external agent finishes.

### Design

New method on `StdioTransport`:

```rust
/// Send a request and stream notifications via callback as they arrive.
/// Returns the final response when it arrives. Does not collect notifications.
pub async fn request_streaming(
    &mut self,
    req: &AcpRequest,
    timeout: Duration,
    on_notification: impl Fn(&AcpResponse) + Send,
) -> Result<AcpResponse>
```

The callback is synchronous (`Fn`, not `async Fn`). This is fine because:
- The existing `ToolProgressCallback` trait (`on_tool_start`, `on_tool_result`) is also synchronous
- The callback only needs to push data to a channel or call `notify_tool_start`/`notify_tool_result` — both are non-blocking
- If async bridging is needed, the callback can send to an `mpsc::UnboundedSender` (zero-cost when empty)

Differences from `request()`:
- Each notification is immediately forwarded via callback (no Vec accumulation)
- Only the final response (matching id) is returned
- `request()` is preserved for non-streaming operations (initialize, session/new)

### Session integration

`session.prompt()` changes its control flow based on whether `on_chunk` is provided:

```rust
pub async fn prompt(
    &mut self,
    text: &str,
    cwd: &str,
    timeout: Duration,
    on_chunk: Option<&AcpChunkCallback>,
) -> Result<(String, Option<AcpResponse>)> {
    // ... state checks, session creation ...

    let session_id = self.create_acp_session(cwd, timeout).await?;
    let req = AcpRequest::prompt(&session_id, text);

    if let Some(cb) = on_chunk {
        // Streaming path: forward chunks in real-time
        let mut accumulated_text = String::new();
        let cb_ref = cb.clone();
        let text_ref = &mut accumulated_text;

        let on_notif = |notif: &AcpResponse| {
            if let Some(chunk) = notif.streaming_text() {
                cb_ref(&chunk);
                // Note: accumulated_text is built inside the closure via side channel
            }
        };

        let resp = self.transport.request_streaming(&req, timeout, on_notif).await?;
        self.state = AcpSessionState::Idle;

        // Final text from streaming accumulation or response fallback
        let result_text = if accumulated_text.is_empty() {
            resp.text_content().unwrap_or_default()
        } else {
            accumulated_text
        };

        Ok((result_text, Some(resp)))
    } else {
        // Legacy path: collect all notifications, extract text after
        let (resp, notifications) = self.transport.request(&req, timeout).await?;
        let text_parts: Vec<String> = notifications
            .iter()
            .filter_map(|n| n.streaming_text())
            .collect();
        let result_text = if !text_parts.is_empty() {
            text_parts.join("")
        } else {
            resp.text_content().unwrap_or_default()
        };
        self.state = AcpSessionState::Idle;
        Ok((result_text, None))
    }
}
```

### Tool integration — streaming callback

The existing `ToolProgressCallback` trait has `on_tool_start` and `on_tool_result`. We extend it with a new method for streaming chunks:

```rust
// In builtin_tools/mod.rs — extend the existing trait
pub trait ToolProgressCallback: Send + Sync {
    fn on_tool_start(&self, tool_name: &str, args_summary: &str);
    fn on_tool_result(&self, tool_name: &str, result_summary: &str, success: bool);

    /// Called when a tool emits a streaming chunk (e.g., ACP delegation output)
    fn on_tool_streaming_chunk(&self, tool_name: &str, chunk: &str) {
        // Default no-op — existing implementations don't break
        let _ = (tool_name, chunk);
    }
}
```

Add a corresponding global notification function:

```rust
pub fn notify_tool_streaming_chunk(tool_name: &str, chunk: &str) {
    let callback = TOOL_PROGRESS_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref handler) = *callback {
        handler.on_tool_streaming_chunk(tool_name, chunk);
    }
}
```

`AcpDelegateTool::call()` constructs the callback using this:

```rust
let on_chunk: AcpChunkCallback = Arc::new(move |chunk: &str| {
    notify_tool_streaming_chunk("acp_delegate", chunk);
});
let result = self.manager.prompt(harness_id, prompt, &cwd, mode, reuse, Some(on_chunk)).await;
```

### Enhanced notification parsing

In addition to `agent_message_chunk` (existing), add parsing for:
- `agent_thought_chunk` — thinking output (optional forwarding)
- `tool_call` / `tool_call_update` — external agent's tool invocation status
- `turn_complete` — already implemented, no change

---

## 3. Structured Error System

### Problem
All ACP errors are `AlephError::tool(format!(...))` strings. Callers cannot distinguish error types programmatically.

### Design

New types in `protocol.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpErrorCode {
    HarnessNotFound,
    HarnessUnavailable,
    HarnessDenied,
    SessionDead,
    Timeout,
    ProtocolError { code: i64 },
    ModeUnsupported,
    Cancelled,
    SpawnFailed,
}

#[derive(Debug)]
pub struct AcpOperationError {
    pub code: AcpErrorCode,
    pub message: String,
    pub remote_error: Option<AcpError>,
}

impl From<AcpOperationError> for AlephError {
    fn from(e: AcpOperationError) -> Self {
        AlephError::tool(e.to_string())
    }
}
```

### Error source mapping

| Location | Error | Code |
|----------|-------|------|
| `transport.rs` — timeout | Request exceeded deadline | `Timeout` |
| `transport.rs` — channel closed | Child process died | `SessionDead` |
| `transport.rs` — JSON-RPC error | Remote error response | `ProtocolError { code }` |
| `session.rs` — spawn failure | Executable not found | `SpawnFailed` |
| `session.rs` — error state | Session in error state | `SessionDead` |
| `manager.rs` — harness lookup | Unknown harness ID | `HarnessNotFound` |
| `manager.rs` — mode validation | Unsupported mode | `ModeUnsupported` |
| `acp_tools.rs` — trust check | trust_level=disabled | `HarnessDenied` |
| `acp_tools.rs` — user denied | Confirmation rejected | `Cancelled` |

### Backward compatibility
`From<AcpOperationError> for AlephError` conversion means existing `Result<T>` signatures don't change. Callers that want fine-grained handling can match on `AcpOperationError`; others just `?` propagate.

---

## 4. Trust Level

### Config change

New enum in `config/types/acp.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    Full,
    Confirm,
    Disabled,
}
```

Note: `TrustLevel` does NOT derive `Default`. Instead, `AcpHarnessEntry` controls the default contextually.

New field on `AcpHarnessEntry`:

```rust
/// Trust level for LLM delegation. Preset harnesses default to Full,
/// custom harnesses default to Confirm (set explicitly in preset factories and Default impl).
#[serde(default = "default_trust_level")]
pub trust_level: TrustLevel,

fn default_trust_level() -> TrustLevel {
    TrustLevel::Confirm  // Safe default for unknown/custom harnesses
}
```

### Default values
- `AcpHarnessEntry::Default` impl uses `TrustLevel::Confirm` (safe default for custom harnesses)
- Built-in preset factories (`preset_claude_code()`, `preset_codex()`, `preset_gemini()`) explicitly set `trust_level: TrustLevel::Full` — locally installed CLI tools are user-chosen

### Enforcement

In `AcpDelegateTool::call()`, before calling `manager.prompt()`:

1. Read harness config via `manager.get_config(&args.harness).await`
2. If `None` → return `AcpOperationError { code: HarnessNotFound, .. }` (harness registered but config missing is not possible — registration always inserts both)
3. If `trust_level == Disabled` → return `AcpOperationError { code: HarnessDenied, .. }`
4. If `trust_level == Confirm` → return approval-required response (gateway's existing approval mechanism)
5. If `trust_level == Full` → proceed directly

### Configurability via tools (R9)
Trust level is part of `AcpHarnessEntry`, which is already manageable via `acp.update` RPC handler. LLM can change trust levels via natural language → tool call.

---

## 5. Session Persistence

### Purpose
Restore ACP session context after Aleph restart, if the external agent supports `session/load`.

### Data model

New struct in `session.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedAcpSession {
    pub harness_id: String,
    pub acp_session_id: String,
    pub cwd: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: chrono::DateTime<chrono::Utc>,
}
```

### Storage
File-based persistence at `~/.aleph/data/acp_sessions.json`. The gateway session store is currently in-memory only ("In a real implementation, this would use SQLite persistence" — comment in `session/store.rs`), so we cannot piggyback on it.

The file is a simple JSON array of `PersistedAcpSession` entries. Written atomically (write to temp file, rename). Read on startup.

### Persistence hook

New field on `AcpHarnessManager`:

```rust
pub struct AcpHarnessManager {
    harnesses: RwLock<HashMap<String, Box<dyn AcpHarness>>>,
    configs: RwLock<HashMap<String, AcpHarnessEntry>>,
    sessions: RwLock<HashMap<SessionKey, AcpSession>>,
    /// Persistence hook, set once at startup. Behind RwLock for thread safety.
    persistence_hook: RwLock<Option<Arc<dyn Fn(AcpSessionEvent) + Send + Sync>>>,
}

#[derive(Debug, Clone)]
pub enum AcpSessionEvent {
    Created { harness_id: String, acp_session_id: String, cwd: String },
    Updated { harness_id: String, acp_session_id: String },
    Removed { harness_id: String, cwd: String },
}

impl AcpHarnessManager {
    pub async fn set_persistence_hook(&self, hook: Arc<dyn Fn(AcpSessionEvent) + Send + Sync>) {
        let mut h = self.persistence_hook.write().await;
        *h = Some(hook);
    }

    pub async fn restore_sessions(&self, persisted: Vec<PersistedAcpSession>) -> Vec<String>;
}
```

### Persistence timing
- After `session.create_acp_session()` → emit `Created`
- After `session.prompt()` success → emit `Updated`
- After cancel or session death → emit `Removed`

### Restore flow
1. On startup, read `~/.aleph/data/acp_sessions.json`
2. Call `manager.restore_sessions(persisted)`
3. For each entry: spawn subprocess → try `session/load` with saved session_id
4. If `session/load` fails → fallback to `session/new` (context lost but session alive)
5. Failures are logged and silently skipped (best-effort, no panic)

### Protocol: session/load

`session/load` is a best-effort extension. Not all ACP agents support it (OpenClaw does via `loadSession`, acpx does via saved session files). Detection strategy:

- Send `session/load` request
- If the agent returns a JSON-RPC error with code `-32601` (method not found) or `-32001` / `-32002` (resource not found), fall back to `session/new`
- Any other error is also treated as "unsupported" and falls back to `session/new`
- The `ProtocolError { code }` variant in `AcpErrorCode` provides the error code for matching

New request constructor in `protocol.rs`:

```rust
impl AcpRequest {
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
}
```

---

## 6. Code Cleanup

### Deleted code
| Item | Location | Reason |
|------|----------|--------|
| `acp_delegate_tool!` macro | `acp_tools.rs` | Replaced by unified struct |
| `ClaudeCodeTool` struct | `acp_tools.rs` | Macro artifact |
| `CodexTool` struct | `acp_tools.rs` | Macro artifact |
| `GeminiCliTool` struct | `acp_tools.rs` | Macro artifact |
| `AcpDelegateOutput` | `acp_tools.rs` | Replaced by new output |
| `AcpManagerConfig` struct | `manager.rs` | Legacy, replaced by `from_entries()` |
| `with_config()` method | `manager.rs` | Uses legacy config |

### Test migration
Tests in `manager.rs` that use `AcpManagerConfig` and `with_config()` must be rewritten to use `from_entries()` with equivalent `AcpHarnessEntry` maps. Specifically:
- `test_manager_disable_harness` → construct entries with `enabled: false`
- `test_manager_executable_override` → construct entries with `executable: Some("/custom/claude")`

### Preserved unchanged
| Item | Reason |
|------|--------|
| `AcpSwitchTool` | Independent concern (agent mode switching) |
| `AcpHarness` trait | Core abstraction, sound |
| `HarnessMode` enum | Used everywhere |
| All harness impls (claude_code.rs, gemini.rs, codex.rs, custom.rs) | No changes needed |
| `StdioTransport::request()` | Still needed for non-streaming ops |
| `mock_server.rs` | Test infrastructure |

---

## File Change Summary

| File | Change | Description |
|------|--------|-------------|
| `acp/protocol.rs` | Modify | +AcpErrorCode, +AcpOperationError, +AcpRequest::load_session(), +notification parsing |
| `acp/transport.rs` | Modify | +request_streaming() method |
| `acp/session.rs` | Modify | prompt() dual-path (streaming vs legacy), +PersistedAcpSession |
| `acp/manager.rs` | Modify | +persistence_hook (RwLock<Option<...>>), +restore_sessions(), -AcpManagerConfig, -with_config() |
| `acp/mod.rs` | Modify | +AcpSessionEvent export |
| `config/types/acp.rs` | Modify | +TrustLevel enum (no Default derive), +trust_level field, preset factories set Full |
| `builtin_tools/mod.rs` | Modify | +on_tool_streaming_chunk() to ToolProgressCallback trait, +notify_tool_streaming_chunk() |
| `builtin_tools/acp_tools.rs` | Rewrite | Unified AcpDelegateTool, +trust check, +streaming, -macro, -3 structs |
| `executor/builtin_registry/builder.rs` | Modify | Register unified tool, remove 3 old registrations |

No new files created. All changes in existing modules.

---

## Implementation Order (Vertical Slice)

1. **acp_delegate tool + streaming** — immediate user-visible value
2. **Cancel** — already implemented, just verify integration
3. **Structured errors** — replace string errors across the module
4. **Trust level** — config + enforcement in tool
5. **Session persistence** — hook + restore flow + file I/O
6. **Code cleanup** — remove legacy code, migrate tests
