# Tool Pipeline Enhancement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Strengthen Aleph's ToolPipeline with input validation, hook deny semantics, output modification, tracing, and dead code cleanup.

**Architecture:** All changes are confined to two files: `tool_pipeline.rs` (pipeline stages) and `hooks/mod.rs` (HookResult + command output parsing). No new dependencies. The pipeline expands from 6 to 7 stages with schema validation inserted before hooks.

**Tech Stack:** Rust, serde_json, tracing

**Spec:** `docs/superpowers/specs/2026-04-01-tool-pipeline-enhancement-design.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `src/extension/hooks/mod.rs` | Modify | HookResult new fields, parse_command_output extensions, delete InterceptorResult |
| `src/agent_loop/tool_pipeline.rs` | Modify | 7-stage pipeline, validate_input_fast, deny handling, output modification, tracing, delete run_from_safety |

---

### Task 1: Delete Dead Code

**Files:**
- Modify: `src/agent_loop/tool_pipeline.rs:347-446` (delete `run_from_safety`)
- Modify: `src/extension/hooks/mod.rs:193-242` (delete `InterceptorResult`)

- [ ] **Step 1: Delete `run_from_safety` method**

In `src/agent_loop/tool_pipeline.rs`, delete the entire `run_from_safety` method (the `#[allow(clippy::too_many_arguments)]` annotation through the closing brace of the method, lines 347-446).

- [ ] **Step 2: Delete `InterceptorResult` struct and impl**

In `src/extension/hooks/mod.rs`, delete the `InterceptorResult` struct and its `impl` block (lines 193-242):

```rust
// DELETE everything from this line:
/// Result from an interceptor hook
#[derive(Debug, Clone, Default)]
pub struct InterceptorResult {
// ... through the closing brace of impl InterceptorResult
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: no errors (these items have zero references)

- [ ] **Step 4: Run existing tests**

Run: `cargo test -p alephcore --lib -- tool_pipeline`
Expected: all existing tests pass

Run: `cargo test -p alephcore --lib -- extension::hooks`
Expected: all existing tests pass

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/tool_pipeline.rs src/extension/hooks/mod.rs
git commit -m "refactor: remove unused run_from_safety and InterceptorResult"
```

---

### Task 2: Extend HookResult with deny + updated_output fields

**Files:**
- Modify: `src/extension/hooks/mod.rs`

- [ ] **Step 1: Write failing tests for new parse_command_output prefixes**

In `src/extension/hooks/mod.rs`, add these tests inside the existing `#[cfg(test)] mod tests` block:

```rust
#[test]
fn test_parse_command_output_deny() {
    let mut result = HookResult::default();
    parse_command_output("deny: policy violation", &mut result);
    assert!(result.denied);
    assert_eq!(result.deny_reason, Some("policy violation".to_string()));
    // deny should NOT set blocked
    assert!(!result.blocked);
}

#[test]
fn test_parse_command_output_deny_and_block_coexist() {
    let mut result = HookResult::default();
    parse_command_output("block: temp issue\ndeny: permanent ban", &mut result);
    // deny takes precedence when both present
    assert!(result.denied);
    assert_eq!(result.deny_reason, Some("permanent ban".to_string()));
    assert!(result.blocked);
}

#[test]
fn test_parse_command_output_update_output() {
    let mut result = HookResult::default();
    parse_command_output("update_output: [REDACTED]", &mut result);
    assert_eq!(result.updated_output, Some("[REDACTED]".to_string()));
}

#[test]
fn test_parse_command_output_update_output_last_writer_wins() {
    let mut result = HookResult::default();
    parse_command_output("update_output: first\nupdate_output: second", &mut result);
    assert_eq!(result.updated_output, Some("second".to_string()));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib -- extension::hooks::tests::test_parse_command_output_deny`
Expected: FAIL — `denied` field does not exist on `HookResult`

- [ ] **Step 3: Add new fields to HookResult**

In `src/extension/hooks/mod.rs`, add three new fields to the `HookResult` struct, after the existing `prevent_continuation` field:

```rust
pub struct HookResult {
    // ... existing fields ...
    /// If true, agent loop should stop even if the tool succeeded.
    pub prevent_continuation: bool,
    /// If true, tool call is denied by policy (not retryable). Takes precedence over blocked.
    pub denied: bool,
    /// Reason for denial (if denied).
    pub deny_reason: Option<String>,
    /// Replacement for tool output text (last-writer-wins). Only effective in AfterToolCall/AfterToolCallFailure.
    pub updated_output: Option<String>,
}
```

These fields are all `Default`-compatible (bool defaults to false, Option to None), so the existing `#[derive(Default)]` works without changes.

- [ ] **Step 4: Extend parse_command_output with deny: and update_output: prefixes**

In `src/extension/hooks/mod.rs`, in the `parse_command_output` function, add two new branches inside the if-else chain, after the `block:` branch and before the `update_input:` branch:

```rust
pub fn parse_command_output(output: &str, result: &mut HookResult) {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(reason) = trimmed.strip_prefix("block:") {
            result.blocked = true;
            result.block_reason = Some(reason.trim().to_string());
        } else if let Some(reason) = trimmed.strip_prefix("deny:") {
            result.denied = true;
            result.deny_reason = Some(reason.trim().to_string());
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
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib -- extension::hooks`
Expected: ALL tests pass including the 4 new ones

- [ ] **Step 6: Commit**

```bash
git add src/extension/hooks/mod.rs
git commit -m "feat(hooks): add deny semantics and update_output to HookResult"
```

---

### Task 3: Add input schema validation to pipeline (Stage 2)

**Files:**
- Modify: `src/agent_loop/tool_pipeline.rs`

- [ ] **Step 1: Write failing test for schema validation**

In `src/agent_loop/tool_pipeline.rs`, add these tests inside the existing `#[cfg(test)] mod tests` block:

```rust
#[tokio::test]
async fn pipeline_rejects_missing_required_field() {
    struct StrictTool;

    #[async_trait]
    impl LoopTool for StrictTool {
        fn name(&self) -> &str {
            "strict"
        }
        fn description(&self) -> &str {
            "Requires 'path' field"
        }
        fn schema(&self) -> Value {
            json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string" }
                }
            })
        }
        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult::Success {
                output: json!("ok"),
            }
        }
    }

    let mut registry = LoopToolRegistry::new();
    registry.register(Box::new(StrictTool));
    let registry = Arc::new(registry);
    let cancel = CancellationToken::new();
    let pipeline = empty_pipeline();

    // Missing required field 'path'
    let outcome = pipeline
        .execute("c1", "strict", &json!({"other": "value"}), &registry, &cancel)
        .await;

    assert!(outcome.outcome.is_error);
    assert!(
        outcome.outcome.output_text.contains("missing required field"),
        "expected validation error, got: {}",
        outcome.outcome.output_text
    );
}

#[tokio::test]
async fn pipeline_passes_validation_when_no_required() {
    let mut registry = LoopToolRegistry::new();
    registry.register(Box::new(EchoTool));
    let registry = Arc::new(registry);
    let cancel = CancellationToken::new();
    let pipeline = empty_pipeline();

    // EchoTool schema is {"type": "object"} — no required fields
    let outcome = pipeline
        .execute("c1", "echo", &json!({}), &registry, &cancel)
        .await;

    assert!(!outcome.outcome.is_error);
}

#[tokio::test]
async fn pipeline_rejects_non_object_input() {
    let mut registry = LoopToolRegistry::new();
    registry.register(Box::new(EchoTool));
    let registry = Arc::new(registry);
    let cancel = CancellationToken::new();
    let pipeline = empty_pipeline();

    let outcome = pipeline
        .execute("c1", "echo", &json!("not an object"), &registry, &cancel)
        .await;

    assert!(outcome.outcome.is_error);
    assert!(outcome.outcome.output_text.contains("expected JSON object"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib -- tool_pipeline::tests::pipeline_rejects_missing_required_field`
Expected: FAIL — no validation in pipeline, tool executes with missing field

- [ ] **Step 3: Add validate_input_fast function**

In `src/agent_loop/tool_pipeline.rs`, add this function in the helpers section (after the existing `value_to_text` function, before the `#[cfg(test)]` block):

```rust
/// Fast-fail input validation against tool schema.
///
/// Checks:
/// 1. Input is a JSON object
/// 2. All `required` fields from schema are present
///
/// This is a lightweight pre-check; full validation happens inside tool.call()
/// via serde deserialization.
fn validate_input_fast(schema: &Value, input: &Value) -> Result<(), String> {
    if !input.is_object() {
        return Err("expected JSON object".into());
    }
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        let obj = input.as_object().unwrap();
        for field in required {
            if let Some(name) = field.as_str() {
                if !obj.contains_key(name) {
                    return Err(format!("missing required field: {name}"));
                }
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Insert Stage 2 into execute()**

In `src/agent_loop/tool_pipeline.rs`, in the `execute` method, insert validation between Stage 1 (build context) and Stage 3 (pre-hooks). Add this block after the `let base_ctx = ...` line:

```rust
        // -----------------------------------------------------------------
        // Stage 2: Input schema validation (fast-fail before hooks)
        // -----------------------------------------------------------------
        if let Some(tool) = registry.get(name) {
            let schema = tool.schema();
            if let Err(msg) = validate_input_fast(&schema, arguments) {
                return PipelineOutcome {
                    outcome: ToolOutcome {
                        tool_id: id.to_string(),
                        tool_name: name.to_string(),
                        output_text: format!("[VALIDATION_ERROR] {}", msg),
                        is_error: true,
                        should_stop: false,
                        retryable: true,
                    },
                    additional_contexts: Vec::new(),
                    prevent_continuation: false,
                    hook_messages: Vec::new(),
                };
            }
        }
```

Note: `retryable: true` because the LLM can fix its input and retry.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib -- tool_pipeline`
Expected: ALL tests pass including the 3 new ones

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/tool_pipeline.rs
git commit -m "feat(pipeline): add input schema validation as stage 2"
```

---

### Task 4: Integrate deny + update_output into pipeline

**Files:**
- Modify: `src/agent_loop/tool_pipeline.rs`

- [ ] **Step 1: Write failing test for deny in pipeline**

In `src/agent_loop/tool_pipeline.rs`, add these tests inside the existing `#[cfg(test)] mod tests` block:

```rust
#[tokio::test]
async fn pipeline_deny_produces_non_retryable_error() {
    let hooks = vec![HookConfig {
        event: HookEvent::BeforeToolCall,
        kind: HookKind::Interceptor,
        priority: HookPriority::default(),
        matcher: None,
        actions: vec![HookAction::Command {
            command: "echo 'deny: policy forbids this tool'".to_string(),
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

    let outcome = pipeline
        .execute("c1", "echo", &json!({}), &registry, &cancel)
        .await;

    assert!(outcome.outcome.is_error);
    assert!(
        outcome.outcome.output_text.contains("[HOOK_DENIED]"),
        "expected HOOK_DENIED, got: {}",
        outcome.outcome.output_text
    );
    assert!(
        !outcome.outcome.retryable,
        "deny should not be retryable"
    );
}

#[tokio::test]
async fn pipeline_post_hook_updates_output() {
    let hooks = vec![HookConfig {
        event: HookEvent::AfterToolCall,
        kind: HookKind::Observer,
        priority: HookPriority::default(),
        matcher: None,
        actions: vec![HookAction::Command {
            command: "echo 'update_output: [REDACTED]'".to_string(),
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

    let outcome = pipeline
        .execute("c1", "echo", &json!({"secret": "key"}), &registry, &cancel)
        .await;

    assert!(!outcome.outcome.is_error);
    assert_eq!(
        outcome.outcome.output_text, "[REDACTED]",
        "post-hook should have replaced output"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib -- tool_pipeline::tests::pipeline_deny_produces_non_retryable_error`
Expected: FAIL — pipeline treats deny: same as a plain message (no special handling)

- [ ] **Step 3: Add deny check to Stage 3 (pre-hooks)**

In `src/agent_loop/tool_pipeline.rs`, in the `execute` method, inside the `if self.has_hooks()` block for Stage 3 (pre-hooks), add a deny check after the existing `if interceptor_result.blocked` check:

```rust
            if interceptor_result.blocked {
                let reason = interceptor_result.block_reason.unwrap_or_default();
                let msg = format!("[HOOK_BLOCKED] {}", reason);
                return self.blocked_outcome(id, name, msg);
            }

            // Deny check — policy refusal, not retryable
            if interceptor_result.denied {
                let reason = interceptor_result.deny_reason.unwrap_or_default();
                return PipelineOutcome {
                    outcome: ToolOutcome {
                        tool_id: id.to_string(),
                        tool_name: name.to_string(),
                        output_text: format!("[HOOK_DENIED] {}", reason),
                        is_error: true,
                        should_stop: false,
                        retryable: false,
                    },
                    additional_contexts: Vec::new(),
                    prevent_continuation: false,
                    hook_messages: Vec::new(),
                };
            }
```

- [ ] **Step 4: Add update_output application in Stages 6 & 7**

In `src/agent_loop/tool_pipeline.rs`, in the `execute` method, in Stage 6 (AfterToolCall), after collecting `additional_contexts` and `prevent_continuation` from `post_result`, add:

```rust
                Ok(post_result) => {
                    hook_messages.extend(post_result.messages);
                    additional_contexts.extend(post_result.additional_contexts);
                    if post_result.prevent_continuation {
                        prevent_continuation = true;
                    }
                    // Apply output modification (last-writer-wins)
                    if let Some(new_output) = post_result.updated_output {
                        outcome.output_text = new_output;
                    }
                }
```

Apply the same pattern in Stage 7 (AfterToolCallFailure), inside the `Ok(fail_result)` arm:

```rust
                    Ok(fail_result) => {
                        hook_messages.extend(fail_result.messages);
                        additional_contexts.extend(fail_result.additional_contexts);
                        if fail_result.prevent_continuation {
                            prevent_continuation = true;
                        }
                        if let Some(new_output) = fail_result.updated_output {
                            outcome.output_text = new_output;
                        }
                    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib -- tool_pipeline`
Expected: ALL tests pass including the 2 new ones

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/tool_pipeline.rs
git commit -m "feat(pipeline): integrate hook deny semantics and post-hook output modification"
```

---

### Task 5: Add tracing spans to pipeline

**Files:**
- Modify: `src/agent_loop/tool_pipeline.rs`

- [ ] **Step 1: Add tracing instrument to execute()**

In `src/agent_loop/tool_pipeline.rs`, add the `#[tracing::instrument]` attribute to the `execute` method:

```rust
    #[tracing::instrument(
        name = "tool_pipeline",
        skip(self, arguments, registry, cancel),
        fields(tool_name = %name, tool_id = %id)
    )]
    pub async fn execute(
        &self,
        id: &str,
        name: &str,
        arguments: &Value,
        registry: &Arc<LoopToolRegistry>,
        cancel: &CancellationToken,
    ) -> PipelineOutcome {
```

- [ ] **Step 2: Add debug_span to each stage**

Wrap each stage with a debug_span. Add these spans at the start of each stage section inside `execute()`:

Stage 2 (validation):
```rust
        // Stage 2: Input schema validation
        {
            let _span = tracing::debug_span!("pipeline_validate").entered();
            // ... existing validation code ...
        }
```

Stage 3 (pre-hooks):
```rust
        // Stage 3: Pre-hooks
        let effective_args = {
            let _span = tracing::debug_span!("pipeline_pre_hooks").entered();
            // ... existing pre-hook code ...
        };
```

Stage 4 (safety):
```rust
        // Stage 4: Safety check
        {
            let _span = tracing::debug_span!("pipeline_safety").entered();
            // ... existing safety code ...
        }
```

Stage 5 (execute):
```rust
        // Stage 5: Execute
        let result = {
            let _span = tracing::debug_span!("pipeline_execute").entered();
            // ... existing execute code ...
        };
```

Stages 6-7 (post-hooks):
```rust
        // Stages 6-7: Post-hooks
        if self.has_hooks() {
            let _span = tracing::debug_span!("pipeline_post_hooks").entered();
            // ... existing post-hook code ...
        }
```

- [ ] **Step 3: Add debug events for stage outcomes**

After each stage's logic, add a `tracing::debug!` event:

After validation:
```rust
tracing::debug!("input validation passed");
```

After pre-hooks:
```rust
tracing::debug!(hooks_fired = interceptor_result.hooks_executed, "pre-hooks complete");
```

After safety:
```rust
tracing::debug!("safety check passed");
```

After execution:
```rust
tracing::debug!(is_error = outcome.is_error, "tool execution complete");
```

- [ ] **Step 4: Verify compilation and tests**

Run: `cargo check -p alephcore`
Expected: no errors

Run: `cargo test -p alephcore --lib -- tool_pipeline`
Expected: ALL tests pass (tracing doesn't affect behavior)

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/tool_pipeline.rs
git commit -m "feat(pipeline): add structured tracing spans to all pipeline stages"
```

---

### Task 6: Update module doc comments

**Files:**
- Modify: `src/agent_loop/tool_pipeline.rs` (top-of-file doc comment)

- [ ] **Step 1: Update the module doc to reflect 7-stage pipeline**

Replace the existing module doc comment at the top of `src/agent_loop/tool_pipeline.rs`:

```rust
//! ToolPipeline — 7-stage hook-integrated tool execution pipeline.
//!
//! Stages:
//! 1. Build HookContext from tool call metadata
//! 2. Input schema validation (fast-fail before hooks)
//! 3. Pre-hooks (interceptors): block, deny, or modify arguments before execution
//! 4. Safety check: blocked patterns and permission policy
//! 5. Execute tool with cancellation support
//! 6. Post-hooks (observers): inject additional context or modify output after success
//! 7. Failure hooks (observers): fire on error outcomes
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: no errors

- [ ] **Step 3: Commit**

```bash
git add src/agent_loop/tool_pipeline.rs
git commit -m "docs(pipeline): update module comment to reflect 7-stage pipeline"
```

---

### Task 7: Final verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: ALL tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: no warnings

- [ ] **Step 3: Verify no dead code warnings**

Check that removing `InterceptorResult` and `run_from_safety` didn't break any re-exports:

Run: `cargo check -p alephcore 2>&1 | grep -i "unused\|dead_code"`
Expected: no matches related to our changes
