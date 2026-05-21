# Hook Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire HookExecutor into the tool execution pipeline and session lifecycle, enabling plugins/extensions to intercept, modify, and observe tool calls at runtime.

**Architecture:** A `ToolPipeline` struct encapsulates the 6-stage execution flow (pre-hooks -> safety -> execute -> post-hooks -> failure-hooks). It replaces the raw `execute_single_tool` function in `streaming_bridge.rs` (production path) and `execute_one` in `tool_orchestrator.rs` (test path). Session-level hooks are called directly in `loop_core.rs`.

**Tech Stack:** Rust, tokio, serde_json, tracing

**Spec:** `docs/superpowers/specs/2026-04-01-hook-pipeline-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `src/extension/hooks/mod.rs` | Modify | HookResult + HookContext field additions, parse_command_output fn |
| `src/extension/types/hooks.rs` | Modify | Add AfterToolCallFailure variant to HookEvent |
| `src/agent_loop/tool_pipeline.rs` | Create | ToolPipeline struct + PipelineOutcome + 6-stage execute |
| `src/agent_loop/mod.rs` | Modify | Export tool_pipeline module |
| `src/agent_loop/streaming_bridge.rs` | Modify | Use ToolPipeline in execute_single_tool, StreamingToolExecutor holds Arc<ToolPipeline> |
| `src/agent_loop/tool_orchestrator.rs` | Modify | Use ToolPipeline in execute_one and execute_tool_batch |
| `src/agent_loop/loop_core.rs` | Modify | AgentLoop holds HookExecutor, session-level hook callsites |

---

### Task 1: Enhance HookResult with new fields

**Files:**
- Modify: `src/extension/hooks/mod.rs`

- [ ] **Step 1: Write test for new HookResult fields**

In `src/extension/hooks/mod.rs`, add to the existing `mod tests` block:

```rust
#[test]
fn test_hook_result_new_fields_default() {
    let result = HookResult::default();
    assert!(result.updated_input.is_none());
    assert!(result.additional_contexts.is_empty());
    assert!(!result.prevent_continuation);
}

#[test]
fn test_hook_result_additional_contexts() {
    let mut result = HookResult::default();
    result.additional_contexts.push("lint: 2 warnings".to_string());
    result.additional_contexts.push("security: clean".to_string());
    assert_eq!(result.additional_contexts.len(), 2);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib hook_result_new_fields`
Expected: FAIL — `updated_input` field does not exist

- [ ] **Step 3: Add new fields to HookResult**

In `src/extension/hooks/mod.rs`, add three fields to `HookResult`:

```rust
/// Hook execution result (aggregated from all matching hooks)
#[derive(Debug, Default)]
pub struct HookResult {
    /// Whether the action was blocked (for BeforeToolCall)
    pub blocked: bool,
    /// Block reason (if blocked)
    pub block_reason: Option<String>,
    /// Modified arguments (if any hook modified them)
    pub modified_arguments: Option<String>,
    /// Messages to inject into the conversation
    pub messages: Vec<String>,
    /// Agents to invoke
    pub agents_to_invoke: Vec<String>,
    /// Individual action results
    pub action_results: Vec<ActionResult>,
    /// Number of hooks executed
    pub hooks_executed: usize,

    // ── New fields ──

    /// Hook-modified tool input (JSON). Last writer wins across interceptor chain.
    pub updated_input: Option<serde_json::Value>,
    /// Additional context strings to inject into next LLM turn (as system-reminders).
    pub additional_contexts: Vec<String>,
    /// If true, agent loop should stop even if the tool succeeded.
    pub prevent_continuation: bool,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib hook_result_new_fields`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/extension/hooks/mod.rs
git commit -m "hooks: add updated_input, additional_contexts, prevent_continuation to HookResult"
```

---

### Task 2: Extend HookContext with tool output fields

**Files:**
- Modify: `src/extension/hooks/mod.rs`

- [ ] **Step 1: Write test for new HookContext fields**

```rust
#[test]
fn test_hook_context_with_tool_output() {
    let ctx = HookContext::new("session-1")
        .with_tool_name("Write")
        .with_tool_output("File written successfully")
        .with_tool_error(false);
    assert_eq!(ctx.tool_output, Some("File written successfully".to_string()));
    assert_eq!(ctx.tool_error, Some(false));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib hook_context_with_tool_output`
Expected: FAIL — `with_tool_output` method does not exist

- [ ] **Step 3: Add fields and builder methods to HookContext**

In `src/extension/hooks/mod.rs`, add to `HookContext` struct:

```rust
pub struct HookContext {
    // ... existing fields ...

    /// Tool execution output (only set for AfterToolCall/AfterToolCallFailure hooks)
    pub tool_output: Option<String>,
    /// Whether the tool execution resulted in an error
    pub tool_error: Option<bool>,
}
```

Update `Default` (add `tool_output: None, tool_error: None` to the default) and add builder methods:

```rust
/// Set the tool output
pub fn with_tool_output(mut self, output: impl Into<String>) -> Self {
    self.tool_output = Some(output.into());
    self
}

/// Set whether the tool errored
pub fn with_tool_error(mut self, is_error: bool) -> Self {
    self.tool_error = Some(is_error);
    self
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib hook_context_with_tool_output`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/extension/hooks/mod.rs
git commit -m "hooks: add tool_output and tool_error fields to HookContext"
```

---

### Task 3: Add AfterToolCallFailure event variant

**Files:**
- Modify: `src/extension/types/hooks.rs`

- [ ] **Step 1: Write test for new event variant**

In `src/extension/registry/types.rs`, add to the `test_all_hook_events_serialize` test — append `HookEvent::AfterToolCallFailure` to the `events` array.

Also add a dedicated test:

```rust
#[test]
fn test_after_tool_call_failure_event() {
    use crate::extension::types::HookEvent;

    let event = HookEvent::AfterToolCallFailure;
    let json = serde_json::to_string(&event).unwrap();
    let roundtrip: HookEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(roundtrip, event);

    // Test alias
    let aliased: HookEvent = serde_json::from_str("\"PostToolUseFailure\"").unwrap();
    assert_eq!(aliased, HookEvent::AfterToolCallFailure);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib after_tool_call_failure`
Expected: FAIL — variant does not exist

- [ ] **Step 3: Add AfterToolCallFailure to HookEvent enum**

In `src/extension/types/hooks.rs`, add after `AfterToolCall`:

```rust
/// After a tool call fails
#[serde(alias = "PostToolUseFailure", alias = "AfterToolCallFailure")]
AfterToolCallFailure,
```

- [ ] **Step 4: Run all hook event tests to verify**

Run: `cargo test -p alephcore --lib hook_event`
Expected: ALL PASS

- [ ] **Step 5: Commit**

```bash
git add src/extension/types/hooks.rs src/extension/registry/types.rs
git commit -m "hooks: add AfterToolCallFailure event variant"
```

---

### Task 4: Implement parse_command_output

**Files:**
- Modify: `src/extension/hooks/mod.rs`

- [ ] **Step 1: Write tests for parse_command_output**

```rust
#[test]
fn test_parse_command_output_block() {
    let mut result = HookResult::default();
    parse_command_output("block: unauthorized access", &mut result);
    assert!(result.blocked);
    assert_eq!(result.block_reason, Some("unauthorized access".to_string()));
}

#[test]
fn test_parse_command_output_update_input() {
    let mut result = HookResult::default();
    parse_command_output(r#"update_input: {"path": "/safe"}"#, &mut result);
    assert_eq!(result.updated_input, Some(serde_json::json!({"path": "/safe"})));
}

#[test]
fn test_parse_command_output_invalid_json_ignored() {
    let mut result = HookResult::default();
    parse_command_output("update_input: not json", &mut result);
    assert!(result.updated_input.is_none());
}

#[test]
fn test_parse_command_output_context() {
    let mut result = HookResult::default();
    parse_command_output("context: File auto-formatted\ncontext: Lint passed", &mut result);
    assert_eq!(result.additional_contexts, vec!["File auto-formatted", "Lint passed"]);
}

#[test]
fn test_parse_command_output_prevent_continuation() {
    let mut result = HookResult::default();
    parse_command_output("prevent_continuation", &mut result);
    assert!(result.prevent_continuation);
}

#[test]
fn test_parse_command_output_plain_message() {
    let mut result = HookResult::default();
    parse_command_output("Hello from hook", &mut result);
    assert_eq!(result.messages, vec!["Hello from hook"]);
}

#[test]
fn test_parse_command_output_mixed() {
    let mut result = HookResult::default();
    parse_command_output(
        "context: formatted\nHello\nblock: danger\n\nprevent_continuation",
        &mut result,
    );
    assert_eq!(result.additional_contexts, vec!["formatted"]);
    assert_eq!(result.messages, vec!["Hello"]);
    assert!(result.blocked);
    assert_eq!(result.block_reason, Some("danger".to_string()));
    assert!(result.prevent_continuation);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib parse_command_output`
Expected: FAIL — function does not exist

- [ ] **Step 3: Implement parse_command_output**

In `src/extension/hooks/mod.rs`, add before the `tests` module:

```rust
/// Parse structured output from a command hook.
///
/// Each line is parsed independently using a prefix protocol:
/// - `block: <reason>` — block the tool call
/// - `update_input: <json>` — replace tool input arguments
/// - `context: <text>` — inject additional context for LLM
/// - `prevent_continuation` — stop the agent loop
/// - (no prefix) — treat as a message
pub fn parse_command_output(output: &str, result: &mut HookResult) {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(reason) = trimmed.strip_prefix("block:") {
            result.blocked = true;
            result.block_reason = Some(reason.trim().to_string());
        } else if let Some(json_str) = trimmed.strip_prefix("update_input:") {
            match serde_json::from_str(json_str.trim()) {
                Ok(val) => result.updated_input = Some(val),
                Err(e) => {
                    tracing::warn!("Hook update_input invalid JSON: {}", e);
                }
            }
        } else if let Some(ctx) = trimmed.strip_prefix("context:") {
            result.additional_contexts.push(ctx.trim().to_string());
        } else if trimmed == "prevent_continuation" {
            result.prevent_continuation = true;
        } else {
            result.messages.push(trimmed.to_string());
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib parse_command_output`
Expected: ALL PASS

- [ ] **Step 5: Commit**

```bash
git add src/extension/hooks/mod.rs
git commit -m "hooks: implement parse_command_output line-prefix protocol"
```

---

### Task 5: Refactor HookExecutor to use parse_command_output

**Files:**
- Modify: `src/extension/hooks/executor.rs`

- [ ] **Step 1: Run existing executor tests as baseline**

Run: `cargo test -p alephcore --lib hook_executor`
Expected: ALL PASS (baseline)

- [ ] **Step 2: Replace inline block: parsing with parse_command_output**

In `executor.rs`, method `execute`, replace the `HookAction::Command` arm (lines ~138-148):

**Old:**
```rust
HookAction::Command { .. } => {
    // Check for block signal in command output
    if let Some(ref output) = ar.output {
        if output.trim().to_lowercase().starts_with("block:") {
            result.blocked = true;
            result.block_reason = Some(
                output.trim().get(6..).unwrap_or("").trim().to_string(),
            );
        }
    }
}
```

**New:**
```rust
HookAction::Command { .. } => {
    if let Some(ref output) = ar.output {
        super::parse_command_output(output, &mut result);
    }
}
```

Do the same replacement in `execute_interceptors` (lines ~388-396), replacing the inline `block:` check:

**Old:**
```rust
if let HookAction::Command { .. } = action {
    if let Some(ref output) = ar.output {
        if output.trim().to_lowercase().starts_with("block:") {
            let reason =
                output.trim().get(6..).unwrap_or("").trim().to_string();
            return Ok((current_context, Some(reason)));
        }
    }
}
```

**New:**
```rust
if let HookAction::Command { .. } = action {
    if let Some(ref output) = ar.output {
        let mut probe = super::HookResult::default();
        super::parse_command_output(output, &mut probe);
        if probe.blocked {
            return Ok((current_context, probe.block_reason));
        }
    }
}
```

- [ ] **Step 3: Run existing tests to verify no regression**

Run: `cargo test -p alephcore --lib hook_executor`
Expected: ALL PASS (same as baseline)

- [ ] **Step 4: Add test verifying new protocol works through executor**

```rust
#[tokio::test]
async fn test_hook_executor_command_with_context() {
    let hooks = vec![HookConfig {
        event: HookEvent::AfterToolCall,
        kind: HookKind::default(),
        priority: HookPriority::default(),
        matcher: None,
        actions: vec![HookAction::Command {
            command: "echo 'context: File formatted'".to_string(),
        }],
        plugin_name: "test-plugin".to_string(),
        plugin_root: PathBuf::from("/tmp"),
        handler: None,
    }];

    let executor = HookExecutor::new(hooks);
    let context = HookContext::new("session").with_tool_name("Write");

    let result = executor
        .execute(HookEvent::AfterToolCall, &context)
        .await
        .unwrap();

    assert_eq!(result.hooks_executed, 1);
    assert_eq!(result.additional_contexts, vec!["File formatted"]);
}
```

- [ ] **Step 5: Run all hook tests**

Run: `cargo test -p alephcore --lib hooks`
Expected: ALL PASS

- [ ] **Step 6: Commit**

```bash
git add src/extension/hooks/executor.rs
git commit -m "hooks: refactor executor to use parse_command_output protocol"
```

---

### Task 6: Create ToolPipeline

**Files:**
- Create: `src/agent_loop/tool_pipeline.rs`
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Write tests for ToolPipeline**

Create `src/agent_loop/tool_pipeline.rs` with the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::tool::{LoopTool, LoopToolRegistry, ToolResult};
    use crate::extension::hooks::{HookExecutor, HookContext};
    use crate::extension::types::{HookAction, HookConfig, HookEvent, HookKind};
    use crate::extension::{HookPriority, PermissionAction};
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::path::PathBuf;

    struct EchoTool;

    #[async_trait]
    impl LoopTool for EchoTool {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "Echoes input" }
        fn schema(&self) -> Value { json!({"type": "object"}) }
        async fn execute(&self, input: Value) -> ToolResult {
            ToolResult::Success { output: input }
        }
    }

    fn permissive_guard() -> SafetyGuard {
        SafetyGuard::new(vec![], HashMap::new(), PermissionAction::Allow)
    }

    fn empty_pipeline() -> ToolPipeline {
        ToolPipeline::new(
            Arc::new(HookExecutor::empty()),
            Arc::new(permissive_guard()),
            "test-session",
        )
    }

    #[tokio::test]
    async fn pipeline_executes_tool_without_hooks() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();
        let pipeline = empty_pipeline();

        let outcome = pipeline.execute("call1", "echo", &json!({"msg": "hi"}), &registry, &cancel).await;

        assert!(!outcome.outcome.is_error);
        assert!(outcome.outcome.output_text.contains("hi"));
        assert!(outcome.additional_contexts.is_empty());
        assert!(!outcome.prevent_continuation);
    }

    #[tokio::test]
    async fn pipeline_pre_hook_blocks_execution() {
        let hooks = vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::Interceptor,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: "echo 'block: forbidden'".to_string(),
            }],
            plugin_name: "test".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
        }];

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();

        let pipeline = ToolPipeline::new(
            Arc::new(HookExecutor::new(hooks)),
            Arc::new(permissive_guard()),
            "test-session",
        );

        let outcome = pipeline.execute("call1", "echo", &json!({}), &registry, &cancel).await;
        assert!(outcome.outcome.is_error);
        assert!(outcome.outcome.output_text.contains("forbidden"));
    }

    #[tokio::test]
    async fn pipeline_post_hook_injects_context() {
        let hooks = vec![HookConfig {
            event: HookEvent::AfterToolCall,
            kind: HookKind::Observer,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: "echo 'context: auto-formatted'".to_string(),
            }],
            plugin_name: "test".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
        }];

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();

        let pipeline = ToolPipeline::new(
            Arc::new(HookExecutor::new(hooks)),
            Arc::new(permissive_guard()),
            "test-session",
        );

        let outcome = pipeline.execute("call1", "echo", &json!({"x": 1}), &registry, &cancel).await;
        assert!(!outcome.outcome.is_error);
        assert_eq!(outcome.additional_contexts, vec!["auto-formatted"]);
    }

    #[tokio::test]
    async fn pipeline_empty_hooks_zero_overhead() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();
        let pipeline = empty_pipeline();

        // Should behave identically to raw tool execution
        let outcome = pipeline.execute("call1", "echo", &json!({"a": "b"}), &registry, &cancel).await;
        assert!(!outcome.outcome.is_error);
        assert!(outcome.hook_messages.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib tool_pipeline`
Expected: FAIL — module does not exist

- [ ] **Step 3: Implement ToolPipeline struct and PipelineOutcome**

Write `src/agent_loop/tool_pipeline.rs`:

```rust
//! Tool execution pipeline with hook integration.
//!
//! Wraps the raw tool execution flow with a 6-stage pipeline:
//! 1. Build HookContext
//! 2. Pre-hooks (interceptors) — can block or modify input
//! 3. Safety check
//! 4. Execute tool
//! 5. Post-hooks (observers) — inject additional contexts
//! 6. Failure hooks (if error)

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace};

use crate::agent_loop::safety::{SafetyError, SafetyGuard, ToolCall as SafetyToolCall};
use crate::agent_loop::tool::{LoopToolRegistry, ToolResult};
use crate::agent_loop::tool_orchestrator::ToolOutcome;
use crate::extension::hooks::{HookContext, HookExecutor};
use crate::extension::types::HookEvent;
use crate::tool_output::compressor::compress_tool_output;

/// Extended outcome carrying hook-injected metadata alongside the core ToolOutcome.
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
}

/// Tool execution pipeline with hook integration.
///
/// Holds shared references to the hook executor and safety guard.
/// Created once per session and passed to execution sites.
pub struct ToolPipeline {
    hooks: Arc<HookExecutor>,
    safety: Arc<SafetyGuard>,
    session_id: String,
    working_dir: Option<PathBuf>,
}

impl ToolPipeline {
    pub fn new(
        hooks: Arc<HookExecutor>,
        safety: Arc<SafetyGuard>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            hooks,
            safety,
            session_id: session_id.into(),
            working_dir: None,
        }
    }

    pub fn with_working_dir(mut self, dir: PathBuf) -> Self {
        self.working_dir = Some(dir);
        self
    }

    /// Reference to the safety guard (for callers that need direct access).
    pub fn safety(&self) -> &SafetyGuard {
        &self.safety
    }

    /// Reference to the hook executor.
    pub fn hooks(&self) -> &HookExecutor {
        &self.hooks
    }

    /// True if no hooks are registered (all hook stages become no-ops).
    pub fn has_hooks(&self) -> bool {
        self.hooks.hook_count() > 0
    }

    /// Execute a single tool call through the full pipeline.
    pub async fn execute(
        &self,
        id: &str,
        name: &str,
        arguments: &Value,
        registry: &Arc<LoopToolRegistry>,
        cancel: &CancellationToken,
    ) -> PipelineOutcome {
        let mut additional_contexts = Vec::new();
        let mut hook_messages = Vec::new();
        let mut prevent_continuation = false;
        let mut effective_args = arguments.clone();

        // ── Stage 1: Build HookContext ──
        let ctx = self.build_context(name, arguments);

        // ── Stage 2: Pre-hooks (interceptors) ──
        if self.has_hooks() {
            let pre_result = self
                .hooks
                .execute_interceptors(HookEvent::BeforeToolCall, ctx.clone())
                .await;

            match pre_result {
                Ok((_, Some(block_reason))) => {
                    debug!(tool = name, reason = %block_reason, "Tool blocked by pre-hook");
                    return PipelineOutcome {
                        outcome: ToolOutcome {
                            tool_id: id.to_string(),
                            tool_name: name.to_string(),
                            output_text: format!("[HOOK_BLOCKED] {}", block_reason),
                            is_error: true,
                            should_stop: false,
                            retryable: false,
                        },
                        additional_contexts,
                        prevent_continuation,
                        hook_messages,
                    };
                }
                Ok((_, None)) => {
                    // Also run the general execute to collect messages/contexts/updated_input
                    let gen_result = self
                        .hooks
                        .execute(HookEvent::BeforeToolCall, &ctx)
                        .await;
                    if let Ok(hr) = gen_result {
                        hook_messages.extend(hr.messages);
                        additional_contexts.extend(hr.additional_contexts);
                        if hr.prevent_continuation {
                            prevent_continuation = true;
                        }
                        if let Some(updated) = hr.updated_input {
                            effective_args = updated;
                        }
                    }
                }
                Err(e) => {
                    debug!(tool = name, error = %e, "Pre-hook execution error, proceeding");
                }
            }
        }

        // ── Stage 3: Safety check ──
        let safety_call = SafetyToolCall {
            name: name.to_string(),
            input: effective_args.clone(),
        };
        if let Err(e) = self.safety.check(&safety_call) {
            let msg = match &e {
                SafetyError::Blocked { tool, pattern } => {
                    format!("[BLOCKED] Tool '{}' blocked by safety pattern '{}'", tool, pattern)
                }
                SafetyError::NeedsConfirmation { tool } => {
                    format!("[NEEDS_CONFIRMATION] Tool '{}' requires user confirmation", tool)
                }
                SafetyError::PolicyDenied { tool } => {
                    format!("[DENIED] Tool '{}' denied by policy", tool)
                }
            };
            return PipelineOutcome {
                outcome: ToolOutcome {
                    tool_id: id.to_string(),
                    tool_name: name.to_string(),
                    output_text: msg,
                    is_error: true,
                    should_stop: false,
                    retryable: false,
                },
                additional_contexts,
                prevent_continuation,
                hook_messages,
            };
        }

        // ── Stage 4: Execute tool ──
        let result = tokio::select! {
            r = registry.execute(name, effective_args) => r,
            _ = cancel.cancelled() => {
                return PipelineOutcome {
                    outcome: ToolOutcome {
                        tool_id: id.to_string(),
                        tool_name: name.to_string(),
                        output_text: "[CANCELLED] Tool execution was cancelled".to_string(),
                        is_error: true,
                        should_stop: false,
                        retryable: false,
                    },
                    additional_contexts,
                    prevent_continuation,
                    hook_messages,
                };
            }
        };

        let outcome = Self::map_result(id, name, &result);

        // ── Stage 5: Post-hooks (observers) ──
        if self.has_hooks() {
            let post_ctx = ctx
                .clone()
                .with_tool_output(&outcome.output_text)
                .with_tool_error(outcome.is_error);

            let post_result = self
                .hooks
                .execute(HookEvent::AfterToolCall, &post_ctx)
                .await;
            if let Ok(hr) = post_result {
                hook_messages.extend(hr.messages);
                additional_contexts.extend(hr.additional_contexts);
                if hr.prevent_continuation {
                    prevent_continuation = true;
                }
            }

            // ── Stage 6: Failure hooks ──
            if outcome.is_error {
                let fail_result = self
                    .hooks
                    .execute(HookEvent::AfterToolCallFailure, &post_ctx)
                    .await;
                if let Ok(hr) = fail_result {
                    hook_messages.extend(hr.messages);
                    additional_contexts.extend(hr.additional_contexts);
                }
            }
        }

        trace!(
            tool = name,
            contexts = additional_contexts.len(),
            messages = hook_messages.len(),
            "Pipeline execution complete"
        );

        PipelineOutcome {
            outcome,
            additional_contexts,
            prevent_continuation,
            hook_messages,
        }
    }

    fn build_context(&self, name: &str, arguments: &Value) -> HookContext {
        let mut ctx = HookContext::new(&self.session_id)
            .with_tool_name(name)
            .with_arguments(&arguments.to_string());

        if let Some(ref dir) = self.working_dir {
            ctx = ctx.with_working_dir(dir.clone());
        }

        // Extract file_path from arguments if present (common for Write/Edit/Read tools)
        if let Some(path) = arguments.get("file_path").and_then(|v| v.as_str()) {
            ctx = ctx.with_file_path(path);
        }

        ctx
    }

    fn map_result(id: &str, name: &str, result: &ToolResult) -> ToolOutcome {
        match result {
            ToolResult::Success { output } => {
                let raw = value_to_text(output);
                let compressed = compress_tool_output(name, &raw);
                ToolOutcome {
                    tool_id: id.to_string(),
                    tool_name: name.to_string(),
                    output_text: compressed,
                    is_error: false,
                    should_stop: false,
                    retryable: false,
                }
            }
            ToolResult::Error { error, retryable } => ToolOutcome {
                tool_id: id.to_string(),
                tool_name: name.to_string(),
                output_text: error.clone(),
                is_error: true,
                should_stop: false,
                retryable: *retryable,
            },
            ToolResult::SuccessAndStopLoop { output } => {
                let raw = value_to_text(output);
                let compressed = compress_tool_output(name, &raw);
                ToolOutcome {
                    tool_id: id.to_string(),
                    tool_name: name.to_string(),
                    output_text: compressed,
                    is_error: false,
                    should_stop: true,
                    retryable: false,
                }
            }
        }
    }
}

fn value_to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
```

- [ ] **Step 4: Export from mod.rs**

In `src/agent_loop/mod.rs`, add:

```rust
pub mod tool_pipeline;
```

And add to the pub use section:

```rust
pub use tool_pipeline::{PipelineOutcome, ToolPipeline};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib tool_pipeline`
Expected: ALL PASS

- [ ] **Step 6: Run full build check**

Run: `cargo check -p alephcore`
Expected: PASS (no compile errors)

- [ ] **Step 7: Commit**

```bash
git add src/agent_loop/tool_pipeline.rs src/agent_loop/mod.rs
git commit -m "feat: add ToolPipeline with 6-stage hook-integrated execution"
```

---

### Task 7: Wire ToolPipeline into StreamingToolExecutor (production path)

**Files:**
- Modify: `src/agent_loop/streaming_bridge.rs`

- [ ] **Step 1: Run existing streaming_bridge tests as baseline**

Run: `cargo test -p alephcore --lib streaming_bridge`
Expected: ALL PASS

- [ ] **Step 2: Add ToolPipeline to StreamingToolExecutor**

In `streaming_bridge.rs`, modify `StreamingToolExecutor`:

```rust
pub struct StreamingToolExecutor {
    ready_rx: mpsc::Receiver<ReadyToolCall>,
    registry: Arc<LoopToolRegistry>,
    pipeline: Arc<ToolPipeline>,
    cancel: CancellationToken,
}
```

Update `StreamingToolBridge::new` to accept `Arc<ToolPipeline>` instead of `Arc<SafetyGuard>`:

```rust
pub fn new(
    registry: Arc<LoopToolRegistry>,
    pipeline: Arc<ToolPipeline>,
    cancel: CancellationToken,
) -> (Self, StreamingToolExecutor) {
    let (tx, rx) = mpsc::channel(32);
    let bridge = Self {
        pending: HashMap::new(),
        ready_tx: tx,
        tool_index: 0,
    };
    let executor = StreamingToolExecutor {
        ready_rx: rx,
        registry,
        pipeline,
        cancel,
    };
    (bridge, executor)
}
```

- [ ] **Step 3: Change run() to return Vec<PipelineOutcome>**

Update `StreamingToolExecutor::run`:

```rust
pub async fn run(mut self) -> Vec<PipelineOutcome> {
    let mut results: Vec<(usize, PipelineOutcome)> = Vec::new();
    let mut in_flight: Vec<(usize, JoinHandle<PipelineOutcome>)> = Vec::new();
    // ... rest uses pipeline.execute instead of execute_single_tool
}
```

- [ ] **Step 4: Replace execute_single_tool calls with pipeline.execute**

In `spawn_tool_execution`:

```rust
fn spawn_tool_execution(
    &self,
    id: String,
    name: String,
    arguments: Value,
) -> JoinHandle<PipelineOutcome> {
    let registry = Arc::clone(&self.registry);
    let pipeline = Arc::clone(&self.pipeline);
    let cancel = self.cancel.clone();

    tokio::spawn(async move {
        pipeline.execute(&id, &name, &arguments, &registry, &cancel).await
    })
}
```

In `execute_one`:

```rust
async fn execute_one(&self, id: &str, name: &str, arguments: &Value) -> PipelineOutcome {
    self.pipeline.execute(id, name, arguments, &self.registry, &self.cancel).await
}
```

- [ ] **Step 5: Remove execute_single_tool function**

Delete the standalone `execute_single_tool` function and the duplicate `value_to_text` function (now in `tool_pipeline.rs`).

Remove the now-unused imports: `SafetyError`, `SafetyGuard`, `SafetyToolCall`, `compress_tool_output`.

- [ ] **Step 6: Update tests to use ToolPipeline**

Update all test functions in `streaming_bridge.rs::tests`. Replace:

```rust
let (mut bridge, executor) = StreamingToolBridge::new(
    Arc::new(registry),
    Arc::new(permissive_guard()),
    cancel,
);
```

With:

```rust
let pipeline = Arc::new(ToolPipeline::new(
    Arc::new(HookExecutor::empty()),
    Arc::new(permissive_guard()),
    "test-session",
));
let (mut bridge, executor) = StreamingToolBridge::new(
    Arc::new(registry),
    pipeline,
    cancel,
);
```

Update result assertions from `results[0].tool_name` to `results[0].outcome.tool_name` (since return type is now `PipelineOutcome`).

- [ ] **Step 7: Run tests**

Run: `cargo test -p alephcore --lib streaming_bridge`
Expected: ALL PASS

- [ ] **Step 8: Run full build check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 9: Commit**

```bash
git add src/agent_loop/streaming_bridge.rs
git commit -m "feat: wire ToolPipeline into StreamingToolExecutor (production path)"
```

---

### Task 8: Wire ToolPipeline into tool_orchestrator (test path)

**Files:**
- Modify: `src/agent_loop/tool_orchestrator.rs`

- [ ] **Step 1: Run existing tests as baseline**

Run: `cargo test -p alephcore --lib tool_orchestrator`
Expected: ALL PASS

- [ ] **Step 2: Replace execute_one with pipeline.execute**

Change `execute_tool_batch` signature:

```rust
pub async fn execute_tool_batch(
    tool_calls: &[NativeToolCall],
    registry: &LoopToolRegistry,
    pipeline: &ToolPipeline,
    cancel: &CancellationToken,
    callback: &mut dyn LoopCallback,
) -> Vec<PipelineOutcome>
```

Replace all `execute_one(tc, registry, safety, cancel)` calls with `pipeline.execute(&tc.id, &tc.name, &tc.arguments, &Arc::new(registry_clone), cancel)`.

Note: since `execute_tool_batch` takes `&LoopToolRegistry` not `Arc`, wrap in a temporary Arc or change parameter. The simplest approach: change to `registry: &Arc<LoopToolRegistry>`.

- [ ] **Step 3: Delete execute_one function**

Remove the `execute_one` function and the duplicate `value_to_text` function.

- [ ] **Step 4: Update safety checks in batch layer**

The batch layer currently does pre-safety checks for `callback.on_tool_start`. Since safety is now inside pipeline, use `pipeline.safety().check()` for the pre-check:

```rust
let safety_call = crate::agent_loop::SafetyToolCall {
    name: tc.name.clone(),
    input: tc.arguments.clone(),
};
if pipeline.safety().check(&safety_call).is_ok() {
    callback.on_tool_start(&tc.name, &tc.arguments);
    safe_indices.push(idx);
} else {
    let outcome = pipeline.execute(&tc.id, &tc.name, &tc.arguments, registry, cancel).await;
    results.push((idx, outcome));
}
```

- [ ] **Step 5: Update callback.on_tool_done to extract ToolOutcome**

```rust
let tool_result = outcome_to_tool_result(&po.outcome);
callback.on_tool_done(&tool_calls[idx].name, &tool_result);
```

- [ ] **Step 6: Update tests**

Replace `SafetyGuard` with `ToolPipeline` in all test calls. Use `HookExecutor::empty()` for the hook executor.

- [ ] **Step 7: Run tests**

Run: `cargo test -p alephcore --lib tool_orchestrator`
Expected: ALL PASS

- [ ] **Step 8: Commit**

```bash
git add src/agent_loop/tool_orchestrator.rs
git commit -m "refactor: wire ToolPipeline into tool_orchestrator (test path)"
```

---

### Task 9: Wire session-level hooks into AgentLoop

**Files:**
- Modify: `src/agent_loop/loop_core.rs`

- [ ] **Step 1: Add HookExecutor to AgentLoop struct**

Add field to `AgentLoop`:

```rust
pub struct AgentLoop<P: LoopProvider> {
    // ... existing fields ...
    /// Hook executor for session-level events.
    hook_executor: Arc<HookExecutor>,
}
```

Update the constructor/builder to accept and store it. If using a builder pattern, add:

```rust
pub fn with_hook_executor(mut self, hooks: Arc<HookExecutor>) -> Self {
    self.hook_executor = hooks;
    self
}
```

Default to `Arc::new(HookExecutor::empty())` if not provided.

- [ ] **Step 2: Add SessionStart hook at loop entry**

In `run_with_history_messages` (or equivalent entry method), before the first LLM call:

```rust
// Session-level hook: SessionStart (observers only)
if self.hook_executor.hook_count() > 0 {
    let ctx = HookContext::new(&session_id);
    self.hook_executor.execute_observers(HookEvent::SessionStart, &ctx).await;
}
```

- [ ] **Step 3: Add SessionEnd hook at loop exit**

Before returning from `run_with_history_messages`:

```rust
// Session-level hook: SessionEnd (observers only)
if self.hook_executor.hook_count() > 0 {
    let ctx = HookContext::new(&session_id);
    self.hook_executor.execute_observers(HookEvent::SessionEnd, &ctx).await;
}
```

- [ ] **Step 4: Construct ToolPipeline from AgentLoop's components**

Where the loop currently creates `StreamingToolBridge::new(registry, safety, cancel)`, construct a `ToolPipeline` and pass it:

```rust
let pipeline = Arc::new(ToolPipeline::new(
    Arc::clone(&self.hook_executor),
    Arc::clone(&self.safety_guard),
    &session_id,
));
let (bridge, executor) = StreamingToolBridge::new(
    Arc::clone(&registry),
    pipeline,
    cancel.clone(),
);
```

- [ ] **Step 5: Consume PipelineOutcome in the tool result processing**

Where the loop processes `Vec<ToolOutcome>` from the executor, update to handle `Vec<PipelineOutcome>`:

```rust
for po in outcomes {
    // Existing ToolOutcome logic applied to po.outcome
    // ...

    // NEW: collect hook-injected contexts for next prompt turn
    if !po.additional_contexts.is_empty() {
        // Inject into session guidance or prompt sections
        for ctx in &po.additional_contexts {
            // Implementation depends on existing prompt injection mechanism
            // Likely: append to a Vec<String> that gets added to next system prompt
        }
    }

    // NEW: hook messages become conversation entries
    for msg in &po.hook_messages {
        // Inject as system-reminder style content
    }

    // NEW: prevent_continuation check
    if po.prevent_continuation {
        // Break the loop
    }
}
```

- [ ] **Step 6: Run full build check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 7: Run all tests**

Run: `cargo test -p alephcore --lib`
Expected: ALL PASS

- [ ] **Step 8: Commit**

```bash
git add src/agent_loop/loop_core.rs
git commit -m "feat: wire session-level hooks and ToolPipeline into AgentLoop"
```

---

### Task 10: Integration test — full pipeline round trip

**Files:**
- Modify: `src/agent_loop/tool_pipeline.rs` (add integration test)

- [ ] **Step 1: Write integration test with pre+post hooks**

```rust
#[tokio::test]
async fn pipeline_full_round_trip_with_hooks() {
    // Setup: pre-hook injects context, post-hook injects context
    let hooks = vec![
        HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::Observer,
            priority: HookPriority::default(),
            matcher: Some("echo".to_string()),
            actions: vec![HookAction::Command {
                command: "echo 'context: pre-hook fired'".to_string(),
            }],
            plugin_name: "test".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
        },
        HookConfig {
            event: HookEvent::AfterToolCall,
            kind: HookKind::Observer,
            priority: HookPriority::default(),
            matcher: Some("echo".to_string()),
            actions: vec![HookAction::Command {
                command: "echo 'context: post-hook fired'".to_string(),
            }],
            plugin_name: "test".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
        },
    ];

    let mut registry = LoopToolRegistry::new();
    registry.register(Box::new(EchoTool));
    let registry = Arc::new(registry);
    let cancel = CancellationToken::new();

    let pipeline = ToolPipeline::new(
        Arc::new(HookExecutor::new(hooks)),
        Arc::new(permissive_guard()),
        "test-session",
    );

    let outcome = pipeline.execute("c1", "echo", &json!({"data": "test"}), &registry, &cancel).await;

    // Tool executed successfully
    assert!(!outcome.outcome.is_error);
    assert!(outcome.outcome.output_text.contains("test"));

    // Both hooks injected contexts
    assert!(outcome.additional_contexts.contains(&"pre-hook fired".to_string()));
    assert!(outcome.additional_contexts.contains(&"post-hook fired".to_string()));
}
```

- [ ] **Step 2: Write integration test for update_input**

```rust
#[tokio::test]
async fn pipeline_update_input_modifies_arguments() {
    let hooks = vec![HookConfig {
        event: HookEvent::BeforeToolCall,
        kind: HookKind::Observer,
        priority: HookPriority::default(),
        matcher: None,
        actions: vec![HookAction::Command {
            command: r#"echo 'update_input: {"injected": true}'"#.to_string(),
        }],
        plugin_name: "test".to_string(),
        plugin_root: PathBuf::from("/tmp"),
        handler: None,
    }];

    let mut registry = LoopToolRegistry::new();
    registry.register(Box::new(EchoTool));
    let registry = Arc::new(registry);
    let cancel = CancellationToken::new();

    let pipeline = ToolPipeline::new(
        Arc::new(HookExecutor::new(hooks)),
        Arc::new(permissive_guard()),
        "test-session",
    );

    let outcome = pipeline.execute("c1", "echo", &json!({"original": true}), &registry, &cancel).await;

    // Echo tool returns its input — should be the modified input
    assert!(!outcome.outcome.is_error);
    assert!(outcome.outcome.output_text.contains("injected"));
}
```

- [ ] **Step 3: Run integration tests**

Run: `cargo test -p alephcore --lib pipeline_full_round_trip pipeline_update_input`
Expected: ALL PASS

- [ ] **Step 4: Run complete test suite**

Run: `cargo test -p alephcore --lib`
Expected: ALL PASS

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/tool_pipeline.rs
git commit -m "test: add integration tests for full hook pipeline round trip"
```

---

### Task 11: Cleanup — remove dead code

**Files:**
- Modify: `src/agent_loop/tool_orchestrator.rs`
- Modify: `src/agent_loop/streaming_bridge.rs`

- [ ] **Step 1: Run cargo clippy to find dead code**

Run: `cargo clippy -p alephcore -- -D warnings`

- [ ] **Step 2: Remove any unused imports, functions, or types**

After Tasks 7-8, the following should be dead:
- `execute_single_tool` in `streaming_bridge.rs` (replaced by ToolPipeline)
- `execute_one` in `tool_orchestrator.rs` (replaced by ToolPipeline)
- Duplicate `value_to_text` functions (now only in `tool_pipeline.rs`)
- Unused `SafetyGuard` imports where replaced by ToolPipeline

- [ ] **Step 3: Run tests and clippy**

Run: `cargo test -p alephcore --lib && cargo clippy -p alephcore -- -D warnings`
Expected: ALL PASS, no warnings

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor: remove dead code after ToolPipeline migration"
```
