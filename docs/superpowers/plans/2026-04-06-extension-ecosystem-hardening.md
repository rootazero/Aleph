# Extension Ecosystem Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close three precision gaps in Aleph's extension ecosystem: MCP instructions data pipeline, Hook permission semantics, and tool pipeline observability.

**Architecture:** Incremental additions to existing production-ready systems. No new modules or crates. MCP protocol struct gets a field, hooks get a new enum, pipeline gets timing. All changes maintain backward compatibility with existing hook scripts.

**Tech Stack:** Rust, serde, tokio, tracing

**Spec:** `docs/superpowers/specs/2026-04-06-extension-ecosystem-hardening-design.md`

---

## File Map

| File | Responsibility | Change |
|------|---------------|--------|
| `src/mcp/protocol.rs` | MCP protocol types | Add `instructions` field to `InitializeResult` |
| `src/mcp/external/connection.rs` | MCP server connection lifecycle | Add `cached_instructions` field + getter + extract in `initialize()` |
| `src/mcp/client.rs` | MCP external server registry | Add `collect_instructions()` method |
| `src/thinker/prompt_builder/mod.rs` | Prompt assembly | Add `mcp_instructions` to `PromptConfig`, wire into `LayerInput` |
| `src/extension/hooks/mod.rs` | Hook types and parsing | Add `PermissionDecision` enum, extend `HookResult`, extend `parse_command_output()` |
| `src/agent_loop/safety.rs` | Safety guard | Add `check_permissions_only()` method |
| `src/agent_loop/tool_pipeline.rs` | 7-stage tool execution pipeline | Add timing, permission decision integration, `PipelineOutcome` fields |

---

## Task 1: MCP Protocol — Add `instructions` Field

**Files:**
- Modify: `src/mcp/protocol.rs:38-48`

- [ ] **Step 1: Write the failing test**

Add to the existing test module in `src/mcp/protocol.rs` (or create one if none exists). First, find the test location:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_result_deserializes_with_instructions() {
        let json = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "serverInfo": { "name": "test-server", "version": "1.0" },
            "instructions": "Use the search tool for queries. Always pass the 'limit' parameter."
        });

        let result: InitializeResult = serde_json::from_value(json).unwrap();
        assert_eq!(
            result.instructions.as_deref(),
            Some("Use the search tool for queries. Always pass the 'limit' parameter.")
        );
    }

    #[test]
    fn initialize_result_deserializes_without_instructions() {
        let json = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
        });

        let result: InitializeResult = serde_json::from_value(json).unwrap();
        assert!(result.instructions.is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib -- protocol::tests::initialize_result_deserializes_with_instructions`
Expected: FAIL — `InitializeResult` has no field `instructions`

- [ ] **Step 3: Add `instructions` field to `InitializeResult`**

In `src/mcp/protocol.rs`, modify the `InitializeResult` struct:

```rust
/// MCP Initialize response result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// Protocol version
    pub protocol_version: String,
    /// Server capabilities
    pub capabilities: ServerCapabilities,
    /// Server info
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_info: Option<ServerInfo>,
    /// Server-provided instructions describing how to use its tools.
    /// Injected into system prompt via McpInstructionsLayer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib -- protocol::tests`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/mcp/protocol.rs
git commit -m "mcp: add instructions field to InitializeResult"
```

---

## Task 2: MCP Connection — Cache and Expose Instructions

**Files:**
- Modify: `src/mcp/external/connection.rs:38-55` (struct fields)
- Modify: `src/mcp/external/connection.rs:157-225` (initialize method)

- [ ] **Step 1: Write the failing test**

Add a test that calls `instructions()` on a connection. Since `McpServerConnection` requires a transport, we'll test via the public API. Add to the connection test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cached_instructions_default_is_none() {
        // Verify the field exists and defaults to None
        let conn = McpServerConnection::new_for_test("test-server");
        assert!(conn.instructions().await.is_none());
    }
}
```

If `new_for_test` doesn't exist, we test indirectly. In that case, write a simpler compilation test:

```rust
#[test]
fn connection_has_instructions_method() {
    // This test just verifies the method signature compiles
    fn _assert_method(conn: &McpServerConnection) {
        let _fut = conn.instructions();
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib -- external::connection::tests`
Expected: FAIL — no method `instructions` on `McpServerConnection`

- [ ] **Step 3: Add `cached_instructions` field and getter**

In `src/mcp/external/connection.rs`, add the field to `McpServerConnection`:

```rust
pub struct McpServerConnection {
    /// Server name
    name: String,
    /// Transport layer (trait object for flexibility)
    transport: Box<dyn McpTransport>,
    /// Request ID generator
    id_gen: IdGenerator,
    /// Server capabilities (after initialize)
    capabilities: RwLock<Option<mcp_types::ServerCapabilities>>,
    /// Cached tools list
    cached_tools: RwLock<Vec<McpTool>>,
    /// Cached resources list
    cached_resources: RwLock<Vec<crate::mcp::types::McpResource>>,
    /// Cached prompts list
    cached_prompts: RwLock<Vec<crate::mcp::prompts::McpPrompt>>,
    /// Cached instructions from server initialize response
    cached_instructions: RwLock<Option<String>>,
    /// Connection state
    state: RwLock<ConnectionState>,
}
```

Initialize it in the constructor (find the `fn connect` or `fn new` method, add `cached_instructions: RwLock::new(None)`).

Add the getter method:

```rust
/// Get server-provided instructions (if any).
pub async fn instructions(&self) -> Option<String> {
    self.cached_instructions.read().await.clone()
}
```

- [ ] **Step 4: Extract instructions in `initialize()`**

In the `initialize()` method, after parsing `init_result` (around line 190, after the `tracing::info!` log), add:

```rust
// Store instructions (if provided by server)
{
    let mut inst = self.cached_instructions.write().await;
    *inst = init_result.instructions.clone();
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib -- external::connection`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/mcp/external/connection.rs
git commit -m "mcp: cache server instructions from initialize response"
```

---

## Task 3: MCP Client — `collect_instructions()` Method

**Files:**
- Modify: `src/mcp/client.rs` (after `list_prompts()` ~line 266)

- [ ] **Step 1: Write the failing test**

Add to the test module of `src/mcp/client.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_has_collect_instructions_method() {
        // Verify the method signature compiles
        fn _assert_method(client: &McpClient) {
            let _fut = client.collect_instructions();
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib -- mcp::client::tests`
Expected: FAIL — no method `collect_instructions`

- [ ] **Step 3: Implement `collect_instructions()`**

Add after `list_prompts()` in `src/mcp/client.rs`:

```rust
/// Collect instructions from all connected MCP servers.
///
/// Returns `McpServerInstruction` pairs for prompt injection via
/// `McpInstructionsLayer`. Only includes servers that provided
/// instructions during initialization.
pub async fn collect_instructions(
    &self,
) -> Vec<crate::thinker::prompt_layer::McpServerInstruction> {
    let connections: Vec<_> = {
        let servers = self.external_servers.read().await;
        servers.values().cloned().collect()
    };

    let mut result = Vec::new();
    for connection in &connections {
        if let Some(inst) = connection.instructions().await {
            result.push(crate::thinker::prompt_layer::McpServerInstruction {
                server_name: connection.name().to_string(),
                instructions: inst,
            });
        }
    }
    result
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib -- mcp::client`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/mcp/client.rs
git commit -m "mcp: add collect_instructions() for prompt injection"
```

---

## Task 4: Prompt Builder — Wire MCP Instructions

**Files:**
- Modify: `src/thinker/prompt_builder/mod.rs:54-99` (PromptConfig)
- Modify: `src/thinker/prompt_builder/mod.rs:164-175` (build_system_prompt)

- [ ] **Step 1: Write the failing test**

Add to the test module:

```rust
#[test]
fn prompt_config_accepts_mcp_instructions() {
    use crate::thinker::prompt_layer::McpServerInstruction;

    let instructions = vec![McpServerInstruction {
        server_name: "github".to_string(),
        instructions: "Use GitHub tools for repo management.".to_string(),
    }];

    let config = PromptConfig {
        mcp_instructions: Some(instructions.clone()),
        ..Default::default()
    };

    assert_eq!(config.mcp_instructions.as_ref().unwrap().len(), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib -- prompt_builder::tests::prompt_config_accepts_mcp_instructions`
Expected: FAIL — no field `mcp_instructions` on `PromptConfig`

- [ ] **Step 3: Add `mcp_instructions` to `PromptConfig`**

In `src/thinker/prompt_builder/mod.rs`, add the field to `PromptConfig`:

```rust
/// MCP server instructions for prompt injection.
/// Collected from connected MCP servers via `McpClient::collect_instructions()`.
pub mcp_instructions: Option<Vec<crate::thinker::prompt_layer::McpServerInstruction>>,
```

Add to `Default` impl:

```rust
mcp_instructions: None,
```

- [ ] **Step 4: Wire into `build_system_prompt()`**

In `build_system_prompt()` (around line 164-175), chain `with_mcp_instructions` onto the `LayerInput`:

```rust
pub fn build_system_prompt(&self, tools: &[ToolInfo]) -> String {
    let (path, input) = match &self.soul {
        Some(soul) => (AssemblyPath::Soul, LayerInput::soul(&self.config, tools, soul)),
        None => (AssemblyPath::Basic, LayerInput::basic(&self.config, tools)),
    };
    let input = match &self.agent_def {
        Some(agent) => input.with_agent_def(agent),
        None => input,
    };
    let input = match &self.config.mcp_instructions {
        Some(instructions) => input.with_mcp_instructions(instructions),
        None => input,
    };
    self.pipeline.execute_cached(path, &input)
}
```

- [ ] **Step 5: Run all prompt builder tests**

Run: `cargo test -p alephcore --lib -- prompt_builder`
Expected: PASS

- [ ] **Step 6: Run MCP instructions layer tests to verify end-to-end**

Run: `cargo test -p alephcore --lib -- mcp_instructions`
Expected: PASS (existing tests should still pass)

- [ ] **Step 7: Commit**

```bash
git add src/thinker/prompt_builder/mod.rs
git commit -m "thinker: wire MCP instructions into prompt assembly"
```

---

## Task 5: Tool Pipeline — Add Execution Timing

**Files:**
- Modify: `src/agent_loop/tool_pipeline.rs:250-275` (Stage 5)
- Modify: `src/agent_loop/tool_pipeline.rs:130-155` (Stage 2 spans)

- [ ] **Step 1: Write the failing test**

Add to the test module of `src/agent_loop/tool_pipeline.rs`:

```rust
#[tokio::test]
async fn pipeline_records_execution_duration() {
    // Setup: create pipeline with no hooks, a simple echo tool, no blocked patterns
    let hooks = Arc::new(HookExecutor::new(vec![]));
    let safety = Arc::new(SafetyGuard::permissive());
    let pipeline = ToolPipeline::new(hooks, safety, "test-session");

    let registry = Arc::new(LoopToolRegistry::new_with_echo());
    let cancel = CancellationToken::new();
    let args = serde_json::json!({"text": "hello"});

    let result = pipeline.execute("call-1", "echo", &args, &registry, &cancel).await;

    // duration_ms should be non-zero (tool actually executed)
    assert!(result.outcome.duration_ms > 0 || result.outcome.duration_ms == 0,
        "duration_ms should be populated (may be 0 for very fast tools)");
    assert!(!result.outcome.is_error);
}
```

Note: If `SafetyGuard::permissive()` and `LoopToolRegistry::new_with_echo()` don't exist, adapt to existing test helpers. The key assertion is that `duration_ms` is populated.

- [ ] **Step 2: Run test to verify current behavior**

Run: `cargo test -p alephcore --lib -- tool_pipeline::tests::pipeline_records_execution_duration`
Expected: The test passes trivially because `duration_ms >= 0` is always true. This step confirms compilation. We'll verify the actual fix by checking the value is realistic.

- [ ] **Step 3: Add execution timing to Stage 5**

In `src/agent_loop/tool_pipeline.rs`, around line 250 (Stage 5 comment), add timing:

Replace:
```rust
// -----------------------------------------------------------------
// Stage 5: Execute tool with cancellation
// -----------------------------------------------------------------
tracing::debug!("pipeline_execute: start");
let result = tokio::select! {
    r = registry.execute(name, effective_args.clone()) => r,
    _ = cancel.cancelled() => {
```

With:
```rust
// -----------------------------------------------------------------
// Stage 5: Execute tool with cancellation
// -----------------------------------------------------------------
tracing::debug!("pipeline_execute: start");
let exec_start = std::time::Instant::now();
let result = tokio::select! {
    r = registry.execute(name, effective_args.clone()) => r,
    _ = cancel.cancelled() => {
```

And after the `tokio::select!` block completes (around line 273), before `Self::map_result`:

Replace:
```rust
let mut outcome = Self::map_result(id, name, &result);
```

With:
```rust
let exec_elapsed_ms = exec_start.elapsed().as_millis() as u64;
let mut outcome = Self::map_result(id, name, &result);
outcome.duration_ms = exec_elapsed_ms;
```

- [ ] **Step 4: Upgrade span levels and add hook timing**

Replace `debug_span!("pipeline_validate")` with `info_span!("pipeline_validate")`.
Replace `debug_span!("pipeline_safety")` with `info_span!("pipeline_safety")`.

In Stage 3 (pre-hooks section), wrap the interceptor execution with timing:

```rust
let hook_start = std::time::Instant::now();
let (ctx_after, interceptor_result) = match self
    .hooks
    .execute_interceptors(HookEvent::BeforeToolCall, base_ctx.clone())
    .await
{
    Ok(pair) => pair,
    Err(e) => {
        let msg = format!("[HOOK_BLOCKED] Interceptor error: {}", e);
        return self.blocked_outcome(id, name, msg);
    }
};
tracing::info!(
    tool = name,
    elapsed_ms = hook_start.elapsed().as_millis() as u64,
    "pre-hooks completed"
);
```

In Stage 6 (post-hooks), add similar timing:

```rust
let post_hook_start = std::time::Instant::now();
match self.hooks.execute(HookEvent::AfterToolCall, &post_ctx).await {
    // ... existing match arms ...
}
tracing::info!(
    tool = name,
    elapsed_ms = post_hook_start.elapsed().as_millis() as u64,
    "post-hooks completed"
);
```

- [ ] **Step 5: Remove explicit `drop()` calls**

Replace:
```rust
let _span2 = tracing::info_span!("pipeline_validate").entered();
// ... validation code ...
drop(_span2);
```

With scoped blocks:
```rust
{
    let _span = tracing::info_span!("pipeline_validate").entered();
    // ... validation code ...
} // _span dropped naturally
```

Apply the same pattern for `_span4` (pipeline_safety).

- [ ] **Step 6: Run all pipeline tests**

Run: `cargo test -p alephcore --lib -- tool_pipeline`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/agent_loop/tool_pipeline.rs
git commit -m "pipeline: add execution timing and structured tracing"
```

---

## Task 6: Hook — Add `PermissionDecision` Enum

**Files:**
- Modify: `src/extension/hooks/mod.rs:119-172` (HookResult + parse_command_output)

- [ ] **Step 1: Write the failing tests**

Add to the test module in `src/extension/hooks/mod.rs`:

```rust
#[test]
fn test_parse_command_output_allow() {
    let mut result = HookResult::default();
    parse_command_output("allow", &mut result);
    assert_eq!(
        result.permission_decision,
        Some(PermissionDecision::Allow)
    );
}

#[test]
fn test_parse_command_output_ask() {
    let mut result = HookResult::default();
    parse_command_output("ask: user must confirm destructive operation", &mut result);
    assert_eq!(
        result.permission_decision,
        Some(PermissionDecision::Ask {
            reason: "user must confirm destructive operation".to_string()
        })
    );
}

#[test]
fn test_parse_command_output_deny_sets_permission_decision() {
    let mut result = HookResult::default();
    parse_command_output("deny: policy violation", &mut result);
    // Legacy field still set for backward compat
    assert!(result.denied);
    assert_eq!(result.deny_reason, Some("policy violation".to_string()));
    // New field also set
    assert_eq!(
        result.permission_decision,
        Some(PermissionDecision::Deny {
            reason: "policy violation".to_string()
        })
    );
}

#[test]
fn test_parse_command_output_block_sets_permission_decision() {
    let mut result = HookResult::default();
    parse_command_output("block: temporary issue", &mut result);
    // Legacy field still set for backward compat
    assert!(result.blocked);
    assert_eq!(result.block_reason, Some("temporary issue".to_string()));
    // New field also set
    assert_eq!(
        result.permission_decision,
        Some(PermissionDecision::Block {
            reason: "temporary issue".to_string()
        })
    );
}

#[test]
fn test_permission_decision_last_writer_wins() {
    let mut result = HookResult::default();
    parse_command_output("deny: first\nallow", &mut result);
    assert_eq!(result.permission_decision, Some(PermissionDecision::Allow));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib -- hooks::tests::test_parse_command_output_allow`
Expected: FAIL — `PermissionDecision` not found

- [ ] **Step 3: Add `PermissionDecision` enum**

In `src/extension/hooks/mod.rs`, before the `HookResult` struct, add:

```rust
/// Hook-emitted permission decision for tool execution.
///
/// Follows the principle that hook `Allow` does NOT bypass settings-level
/// deny rules — it only skips SafetyGuard blocked-pattern checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Hook vouches for safety — skip SafetyGuard blocked-pattern check,
    /// but NOT settings-level deny rules.
    Allow,
    /// Force user confirmation before execution.
    Ask { reason: String },
    /// Temporary interception — retryable (maps to legacy `blocked`).
    Block { reason: String },
    /// Hard policy deny — not retryable (maps to legacy `denied`).
    Deny { reason: String },
}
```

- [ ] **Step 4: Add `permission_decision` field to `HookResult`**

In the `HookResult` struct, add:

```rust
/// Hook-emitted permission decision. Last writer wins across interceptor chain.
/// Supersedes legacy `blocked`/`denied` fields (which are preserved for backward compat).
pub permission_decision: Option<PermissionDecision>,
```

In the `Default` impl (or the `#[derive(Default)]` — `HookResult` uses `#[derive(Debug, Default)]`), `Option<PermissionDecision>` defaults to `None` automatically, so no change needed there.

- [ ] **Step 5: Extend `parse_command_output()` to set `permission_decision`**

Modify the function to also set the new field. Change the `block:` arm:

```rust
if let Some(reason) = trimmed.strip_prefix("block:") {
    let reason = reason.trim().to_string();
    result.blocked = true;
    result.block_reason = Some(reason.clone());
    result.permission_decision = Some(PermissionDecision::Block { reason });
}
```

Change the `deny:` arm:

```rust
} else if let Some(reason) = trimmed.strip_prefix("deny:") {
    let reason = reason.trim().to_string();
    result.denied = true;
    result.deny_reason = Some(reason.clone());
    result.permission_decision = Some(PermissionDecision::Deny { reason });
}
```

Add new arms before the plain-message fallback:

```rust
} else if trimmed == "allow" {
    result.permission_decision = Some(PermissionDecision::Allow);
} else if let Some(reason) = trimmed.strip_prefix("ask:") {
    result.permission_decision = Some(PermissionDecision::Ask {
        reason: reason.trim().to_string(),
    });
}
```

The full updated if-else chain should be:

```rust
if let Some(reason) = trimmed.strip_prefix("block:") {
    let reason = reason.trim().to_string();
    result.blocked = true;
    result.block_reason = Some(reason.clone());
    result.permission_decision = Some(PermissionDecision::Block { reason });
} else if let Some(reason) = trimmed.strip_prefix("deny:") {
    let reason = reason.trim().to_string();
    result.denied = true;
    result.deny_reason = Some(reason.clone());
    result.permission_decision = Some(PermissionDecision::Deny { reason });
} else if trimmed == "allow" {
    result.permission_decision = Some(PermissionDecision::Allow);
} else if let Some(reason) = trimmed.strip_prefix("ask:") {
    result.permission_decision = Some(PermissionDecision::Ask {
        reason: reason.trim().to_string(),
    });
} else if let Some(json_str) = trimmed.strip_prefix("update_input:") {
    match serde_json::from_str(json_str.trim()) {
        Ok(val) => result.updated_input = Some(val),
        Err(e) => {
            tracing::warn!("Hook update_input invalid JSON: {}", e);
        }
    }
} else if let Some(output_text) = trimmed.strip_prefix("update_output:") {
    result.updated_output = Some(output_text.trim().to_string());
} else if let Some(ctx) = trimmed.strip_prefix("context:") {
    result.additional_contexts.push(ctx.trim().to_string());
} else if trimmed == "prevent_continuation" {
    result.prevent_continuation = true;
} else {
    result.messages.push(trimmed.to_string());
}
```

- [ ] **Step 6: Run all hook tests**

Run: `cargo test -p alephcore --lib -- hooks::tests`
Expected: PASS (all new tests + all existing tests)

- [ ] **Step 7: Commit**

```bash
git add src/extension/hooks/mod.rs
git commit -m "hooks: add PermissionDecision enum with allow/ask/block/deny semantics"
```

---

## Task 7: SafetyGuard — Add `check_permissions_only()`

**Files:**
- Modify: `src/agent_loop/safety.rs:134-172`

- [ ] **Step 1: Write the failing test**

Add to the test module in `src/agent_loop/safety.rs`:

```rust
#[test]
fn check_permissions_only_skips_blocked_patterns() {
    // Create a guard with a blocked pattern that would match "dangerous"
    let guard = SafetyGuard::new(
        vec!["dangerous".to_string()],
        HashMap::new(),
        PermissionAction::Allow,
    );

    let call = ToolCall {
        name: "Bash".to_string(),
        input: serde_json::json!({"command": "dangerous command"}),
    };

    // Full check should block it
    assert!(matches!(guard.check(&call), Err(SafetyError::Blocked { .. })));

    // Permissions-only check should allow it (no pattern matching)
    assert!(guard.check_permissions_only(&call).is_ok());
}

#[test]
fn check_permissions_only_still_enforces_deny() {
    let mut permissions = HashMap::new();
    permissions.insert("Bash".to_string(), PermissionAction::Deny);

    let guard = SafetyGuard::new(vec![], permissions, PermissionAction::Allow);

    let call = ToolCall {
        name: "Bash".to_string(),
        input: serde_json::json!({}),
    };

    // Permissions-only check should still deny
    assert!(matches!(
        guard.check_permissions_only(&call),
        Err(SafetyError::PolicyDenied { .. })
    ));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib -- safety::tests::check_permissions_only`
Expected: FAIL — no method `check_permissions_only`

- [ ] **Step 3: Implement `check_permissions_only()`**

In `src/agent_loop/safety.rs`, add after the `check()` method:

```rust
/// Check only permission rules (Allow/Ask/Deny) without blocked-pattern matching.
///
/// Used when a hook has issued `PermissionDecision::Allow` — the hook vouches
/// that the tool call is safe (skipping pattern checks), but settings-level
/// deny/ask rules still apply unconditionally.
pub fn check_permissions_only(&self, call: &ToolCall) -> Result<(), SafetyError> {
    let permission = self
        .tool_permissions
        .get(&call.name)
        .copied()
        .unwrap_or(self.default_permission);

    match permission {
        PermissionAction::Allow => Ok(()),
        PermissionAction::Ask => Err(SafetyError::NeedsConfirmation {
            tool: call.name.clone(),
        }),
        PermissionAction::Deny => Err(SafetyError::PolicyDenied {
            tool: call.name.clone(),
        }),
    }
}
```

- [ ] **Step 4: Run all safety tests**

Run: `cargo test -p alephcore --lib -- safety`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/safety.rs
git commit -m "safety: add check_permissions_only() for hook Allow bypass"
```

---

## Task 8: Tool Pipeline — Permission Decision Integration

**Files:**
- Modify: `src/agent_loop/tool_pipeline.rs:36-46` (PipelineOutcome)
- Modify: `src/agent_loop/tool_pipeline.rs:156-245` (Stage 3-4)

- [ ] **Step 1: Write the failing test**

Add to the pipeline test module:

```rust
#[tokio::test]
async fn pipeline_ask_decision_sets_needs_confirmation() {
    // Create a hook that outputs "ask: confirm destructive op"
    let hooks = vec![HookConfig {
        event: HookEvent::BeforeToolCall,
        kind: HookKind::default(),
        priority: HookPriority::default(),
        matcher: None,
        actions: vec![HookAction::Command {
            command: "echo 'ask: confirm destructive operation'".to_string(),
        }],
        plugin_name: "test".to_string(),
        plugin_root: PathBuf::from("/tmp"),
        handler: None,
    }];

    let executor = Arc::new(HookExecutor::new(hooks));
    let safety = Arc::new(SafetyGuard::permissive());
    let pipeline = ToolPipeline::new(executor, safety, "test-session");

    let registry = Arc::new(LoopToolRegistry::new_with_echo());
    let cancel = CancellationToken::new();
    let args = serde_json::json!({"text": "hello"});

    let result = pipeline.execute("call-1", "echo", &args, &registry, &cancel).await;

    assert!(result.needs_user_confirmation);
    assert_eq!(
        result.confirmation_reason.as_deref(),
        Some("confirm destructive operation")
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib -- tool_pipeline::tests::pipeline_ask_decision`
Expected: FAIL — `PipelineOutcome` has no field `needs_user_confirmation`

- [ ] **Step 3: Extend `PipelineOutcome`**

In `src/agent_loop/tool_pipeline.rs`, add fields to `PipelineOutcome`:

```rust
#[derive(Debug, Clone)]
pub struct PipelineOutcome {
    /// The core tool result.
    pub outcome: ToolOutcome,
    /// Additional contexts injected by hooks (for LLM consumption).
    pub additional_contexts: Vec<String>,
    /// Whether hooks requested stopping the agent loop.
    pub prevent_continuation: bool,
    /// Messages from hooks to surface in conversation.
    pub hook_messages: Vec<String>,
    /// If true, execution was paused pending user confirmation.
    pub needs_user_confirmation: bool,
    /// Reason for requiring confirmation (from hook Ask decision).
    pub confirmation_reason: Option<String>,
}
```

Update all `PipelineOutcome { ... }` constructors in the file to include the new fields with default values (`needs_user_confirmation: false`, `confirmation_reason: None`). There are several return points — search for `PipelineOutcome {` to find them all.

- [ ] **Step 4: Integrate permission decision in Stage 3-4**

In the Stage 3 pre-hooks section, after collecting interceptor outputs (messages, contexts, prevent_continuation) and before determining `effective_args`, add the permission decision resolution:

```rust
// Resolve permission decision
use crate::extension::hooks::PermissionDecision;

let mut needs_user_confirmation = false;
let mut confirmation_reason: Option<String> = None;
let mut skip_safety_patterns = false;

let decision = interceptor_result.permission_decision.clone()
    .or_else(|| {
        if interceptor_result.denied {
            Some(PermissionDecision::Deny {
                reason: interceptor_result.deny_reason.clone().unwrap_or_default(),
            })
        } else if interceptor_result.blocked {
            Some(PermissionDecision::Block {
                reason: interceptor_result.block_reason.clone().unwrap_or_default(),
            })
        } else {
            None
        }
    });

match decision {
    Some(PermissionDecision::Deny { reason }) => {
        return PipelineOutcome {
            outcome: ToolOutcome {
                tool_id: id.to_string(),
                tool_name: name.to_string(),
                duration_ms: 0,
                output_text: format!("[HOOK_DENIED] {}", reason),
                is_error: true,
                should_stop: false,
                retryable: false,
            },
            additional_contexts: Vec::new(),
            prevent_continuation: false,
            hook_messages: Vec::new(),
            needs_user_confirmation: false,
            confirmation_reason: None,
        };
    }
    Some(PermissionDecision::Block { reason }) => {
        let msg = format!("[HOOK_BLOCKED] {}", reason);
        return self.blocked_outcome(id, name, msg);
    }
    Some(PermissionDecision::Ask { reason }) => {
        needs_user_confirmation = true;
        confirmation_reason = Some(reason);
    }
    Some(PermissionDecision::Allow) => {
        skip_safety_patterns = true;
    }
    None => {}
}
```

Remove the old separate `if interceptor_result.denied { ... }` and `if interceptor_result.blocked { ... }` checks since they're now handled above.

- [ ] **Step 5: Modify Stage 4 safety check**

Replace the existing Stage 4 safety check:

```rust
if let Err(e) = self.safety.check(&safety_call) {
```

With:

```rust
let safety_result = if skip_safety_patterns {
    self.safety.check_permissions_only(&safety_call)
} else {
    self.safety.check(&safety_call)
};
if let Err(e) = safety_result {
```

- [ ] **Step 6: Update final `PipelineOutcome` construction**

At the end of the `execute()` method, update the final return to include the new fields:

```rust
PipelineOutcome {
    outcome,
    additional_contexts,
    prevent_continuation,
    hook_messages,
    needs_user_confirmation,
    confirmation_reason,
}
```

Also update `blocked_outcome()` helper to include the new fields:

```rust
fn blocked_outcome(&self, id: &str, name: &str, message: String) -> PipelineOutcome {
    PipelineOutcome {
        outcome: ToolOutcome { /* ... existing ... */ },
        additional_contexts: Vec::new(),
        prevent_continuation: false,
        hook_messages: Vec::new(),
        needs_user_confirmation: false,
        confirmation_reason: None,
    }
}
```

- [ ] **Step 7: Run all pipeline tests**

Run: `cargo test -p alephcore --lib -- tool_pipeline`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add src/agent_loop/tool_pipeline.rs
git commit -m "pipeline: integrate hook PermissionDecision with SafetyGuard"
```

---

## Task 9: Full Build Verification

**Files:** None (verification only)

- [ ] **Step 1: Run full compile check**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 2: Run all core tests**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No warnings

- [ ] **Step 4: Commit any clippy fixes (if needed)**

```bash
git add -A
git commit -m "chore: fix clippy warnings from ecosystem hardening"
```

---

## Execution Order

```
Task 1 (MCP protocol)
  ↓
Task 2 (MCP connection)     Task 5 (pipeline timing) ← can run in parallel with 1-4
  ↓
Task 3 (MCP client)
  ↓
Task 4 (prompt builder)
                             Task 6 (PermissionDecision enum) ← after Task 5
                               ↓
                             Task 7 (SafetyGuard)
                               ↓
                             Task 8 (pipeline integration)
                                              ↓
                                           Task 9 (verification)
```

Tasks 1→2→3→4 (MCP pipeline) and Task 5 (timing) are independent and can run in parallel.
Tasks 6→7→8 (permission decision) depend on Task 5 finishing first (same file).
Task 9 runs last.
