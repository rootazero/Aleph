# Tool Pipeline Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify SafetyGuard with ToolSafetyPolicy keyword inference, implement confirmation flow via LoopCallback, add structured Pipeline tracing, and fix Hook-Safety permission collision.

**Architecture:** SafetyGuard gains a `ToolSafetyPolicy` field for keyword-based risk inference (replacing hardcoded tool names). LoopCallback gets an async `on_confirmation_needed()` method enabling Channel-autonomous confirmation UX. Pipeline stages get structured `tracing` spans. Hook Ask and Safety Ask converge to the same confirmation path.

**Tech Stack:** Rust, `tracing` crate (existing), `serde_json` (existing), `regex` (existing)

**Spec:** `docs/superpowers/specs/2026-04-07-tool-pipeline-hardening-design.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `src/agent_loop/safety.rs` | Modify | Add `ToolSafetyPolicy` field, `infer_permission()`, delete hardcoded tools |
| `src/agent_loop/tool_pipeline.rs` | Modify | Structured tracing spans, Safety NeedsConfirmation → confirmation flow, collision fix |
| `src/agent_loop/loop_core.rs` | Modify | `LoopCallback::on_confirmation_needed()`, consume `needs_user_confirmation` |
| `src/agent_loop/mod.rs` | Modify | Re-export new types if needed |
| `src/gateway/execution_engine/run_loop.rs` | Modify | Update `from_permissions()` call sites (add policy arg) |

---

## Phase 1: Internal Cleanup (Tasks 1-5)

### Task 1: SafetyGuard — Add ToolSafetyPolicy and infer_permission()

**Files:**
- Modify: `src/agent_loop/safety.rs:66-98` (SafetyGuard struct + new constructor)
- Modify: `src/agent_loop/safety.rs:233-241` (delete `default_high_risk_permissions`)

- [ ] **Step 1: Write failing tests for keyword inference**

Add these tests at the bottom of the `#[cfg(test)] mod tests` block in `src/agent_loop/safety.rs`:

```rust
#[test]
fn test_infer_permission_high_risk_keyword() {
    let policy = ToolSafetyPolicy::default();
    let guard = SafetyGuard::with_policy(
        vec![],
        HashMap::new(),
        PermissionAction::Allow,
        Some(policy),
    );
    // "file_delete" contains "delete" which is a high_risk keyword
    let call = ToolCall {
        name: "file_delete".to_string(),
        input: json!({}),
    };
    let err = guard.check(&call).unwrap_err();
    assert!(matches!(err, SafetyError::NeedsConfirmation { .. }));
}

#[test]
fn test_infer_permission_readonly_keyword() {
    let policy = ToolSafetyPolicy::default();
    let guard = SafetyGuard::with_policy(
        vec![],
        HashMap::new(),
        PermissionAction::Allow,
        Some(policy),
    );
    // "memory_search" contains "search" which is a readonly keyword
    let call = ToolCall {
        name: "memory_search".to_string(),
        input: json!({}),
    };
    assert!(guard.check(&call).is_ok());
}

#[test]
fn test_infer_permission_low_risk_keyword() {
    let policy = ToolSafetyPolicy::default();
    let guard = SafetyGuard::with_policy(
        vec![],
        HashMap::new(),
        PermissionAction::Allow,
        Some(policy),
    );
    // "email_send" contains "send" which is a low_risk keyword → Ask
    let call = ToolCall {
        name: "email_send".to_string(),
        input: json!({}),
    };
    let err = guard.check(&call).unwrap_err();
    assert!(matches!(err, SafetyError::NeedsConfirmation { .. }));
}

#[test]
fn test_infer_permission_unknown_tool_uses_default() {
    let policy = ToolSafetyPolicy::default();
    let guard = SafetyGuard::with_policy(
        vec![],
        HashMap::new(),
        PermissionAction::Allow,
        Some(policy),
    );
    // "foobar" matches no keywords → use default (Allow)
    let call = ToolCall {
        name: "foobar".to_string(),
        input: json!({}),
    };
    assert!(guard.check(&call).is_ok());
}

#[test]
fn test_explicit_override_beats_inference() {
    let policy = ToolSafetyPolicy::default();
    // Explicitly allow "file_delete" even though it has high_risk keyword
    let perms = [("file_delete".to_string(), PermissionAction::Allow)]
        .into_iter()
        .collect();
    let guard = SafetyGuard::with_policy(vec![], perms, PermissionAction::Allow, Some(policy));
    let call = ToolCall {
        name: "file_delete".to_string(),
        input: json!({}),
    };
    assert!(guard.check(&call).is_ok());
}

#[test]
fn test_no_policy_falls_back_to_default() {
    // No policy → same behavior as before (default permission for unknown tools)
    let guard = SafetyGuard::with_policy(
        vec![],
        HashMap::new(),
        PermissionAction::Allow,
        None,
    );
    let call = ToolCall {
        name: "file_delete".to_string(),
        input: json!({}),
    };
    // No policy, no override → default Allow
    assert!(guard.check(&call).is_ok());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib safety -- test_infer_permission 2>&1 | tail -20`

Expected: compilation error — `SafetyGuard::with_policy` does not exist.

- [ ] **Step 3: Add `use` import and `ToolSafetyPolicy` field**

At the top of `src/agent_loop/safety.rs`, add the import:

```rust
use crate::config::types::policies::ToolSafetyPolicy;
```

Modify the `SafetyGuard` struct (line 66-70) to:

```rust
pub struct SafetyGuard {
    blocked_patterns: Vec<Regex>,
    tool_permissions: HashMap<String, PermissionAction>,
    default_permission: PermissionAction,
    safety_policy: Option<ToolSafetyPolicy>,
}
```

- [ ] **Step 4: Update existing `new()` to pass `safety_policy: None`**

Modify the `Self { ... }` return in `SafetyGuard::new()` (line 93-97):

```rust
Self {
    blocked_patterns,
    tool_permissions,
    default_permission,
    safety_policy: None,
}
```

- [ ] **Step 5: Add `with_policy()` constructor and `infer_permission()` method**

Add after `SafetyGuard::new()` (after line 98):

```rust
/// Create a new guard with keyword-based safety inference.
///
/// When a tool is not in `tool_permissions`, the `ToolSafetyPolicy` keywords
/// are used to infer its permission level. If no policy is provided, falls
/// through to `default_permission`.
pub fn with_policy(
    blocked: Vec<String>,
    tool_permissions: HashMap<String, PermissionAction>,
    default_permission: PermissionAction,
    safety_policy: Option<ToolSafetyPolicy>,
) -> Self {
    let blocked_patterns: Vec<Regex> = blocked
        .into_iter()
        .filter_map(|p| match Regex::new(&p) {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!(pattern = %p, error = %e, "Failed to compile safety regex — pattern skipped");
                None
            }
        })
        .collect();
    Self {
        blocked_patterns,
        tool_permissions,
        default_permission,
        safety_policy,
    }
}

/// Infer permission for a tool not explicitly listed in `tool_permissions`.
///
/// Uses `ToolSafetyPolicy` keyword matching with priority:
/// high_risk → Ask, low_risk → Ask, readonly → Allow, reversible → Allow.
/// Falls back to `default_permission` if no keywords match or no policy is set.
fn infer_permission(&self, tool_name: &str) -> PermissionAction {
    if let Some(ref policy) = self.safety_policy {
        if policy.is_high_risk(tool_name) {
            return PermissionAction::Ask;
        }
        if policy.is_low_risk(tool_name) {
            return PermissionAction::Ask;
        }
        if policy.is_readonly(tool_name) {
            return PermissionAction::Allow;
        }
        if policy.is_reversible(tool_name) {
            return PermissionAction::Allow;
        }
    }
    self.default_permission
}
```

- [ ] **Step 6: Update `check()` to use `infer_permission()`**

In `SafetyGuard::check()`, replace the permission lookup block (lines 156-162):

```rust
// Permission lookup
let permission = self
    .tool_permissions
    .get(&call.name)
    .copied()
    .unwrap_or(self.default_permission);
```

with:

```rust
// Permission lookup: explicit override > keyword inference > default
let permission = self
    .tool_permissions
    .get(&call.name)
    .copied()
    .unwrap_or_else(|| self.infer_permission(&call.name));
```

- [ ] **Step 7: Update `check_permissions_only()` likewise**

In `check_permissions_only()`, replace the same pattern (lines 180-184):

```rust
let permission = self
    .tool_permissions
    .get(&call.name)
    .copied()
    .unwrap_or(self.default_permission);
```

with:

```rust
let permission = self
    .tool_permissions
    .get(&call.name)
    .copied()
    .unwrap_or_else(|| self.infer_permission(&call.name));
```

- [ ] **Step 8: Update `is_high_risk()` to use inference**

Replace the `is_high_risk()` method (line 126-131):

```rust
pub fn is_high_risk(&self, tool_name: &str) -> bool {
    match self.tool_permissions.get(tool_name) {
        Some(p) => *p == PermissionAction::Ask,
        None => self.infer_permission(tool_name) == PermissionAction::Ask,
    }
}
```

- [ ] **Step 9: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib safety -- test_infer_permission -v 2>&1 | tail -20`

Expected: all 6 new tests PASS.

- [ ] **Step 10: Commit**

```bash
git add src/agent_loop/safety.rs
git commit -m "safety: add ToolSafetyPolicy keyword inference to SafetyGuard"
```

---

### Task 2: SafetyGuard — Delete hardcoded defaults, update constructors

**Files:**
- Modify: `src/agent_loop/safety.rs:106-119` (`default_guard`, `from_permissions`)
- Delete: `src/agent_loop/safety.rs:233-241` (`default_high_risk_permissions`)

- [ ] **Step 1: Write failing test for updated `default_guard()`**

Add to test module in `src/agent_loop/safety.rs`:

```rust
#[test]
fn test_default_guard_uses_policy_inference() {
    let guard = SafetyGuard::default_guard();

    // "file_delete" is inferred as high-risk via keyword "delete" (not hardcoded)
    let call = ToolCall {
        name: "file_delete".to_string(),
        input: json!({}),
    };
    assert!(matches!(
        guard.check(&call),
        Err(SafetyError::NeedsConfirmation { .. })
    ));

    // "custom_destroy_tool" is also high-risk via keyword "destroy"
    let call = ToolCall {
        name: "custom_destroy_tool".to_string(),
        input: json!({}),
    };
    assert!(matches!(
        guard.check(&call),
        Err(SafetyError::NeedsConfirmation { .. })
    ));

    // "list_files" is readonly via keyword "list"
    let call = ToolCall {
        name: "list_files".to_string(),
        input: json!({}),
    };
    assert!(guard.check(&call).is_ok());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib safety -- test_default_guard_uses_policy 2>&1 | tail -10`

Expected: FAIL — `custom_destroy_tool` is not in hardcoded list, so it gets `Allow`.

- [ ] **Step 3: Update `default_guard()` to use ToolSafetyPolicy**

Replace `default_guard()` (lines 106-110):

```rust
pub fn default_guard() -> Self {
    let blocked = default_blocked_patterns();
    Self::with_policy(
        blocked,
        HashMap::new(),
        PermissionAction::Allow,
        Some(ToolSafetyPolicy::default()),
    )
}
```

- [ ] **Step 4: Update `from_permissions()` to accept ToolSafetyPolicy**

Replace `from_permissions()` (lines 116-120):

```rust
pub fn from_permissions(
    global: &ToolPermissionsConfig,
    agent: &ToolPermissionsConfig,
    safety_policy: Option<ToolSafetyPolicy>,
) -> Self {
    let merged = ToolPermissionsConfig::merge(global, agent);
    let blocked = default_blocked_patterns();
    Self::with_policy(blocked, merged.overrides, merged.default, safety_policy)
}
```

- [ ] **Step 5: Delete `default_high_risk_permissions()` function**

Delete the entire function `default_high_risk_permissions()` (lines 230-241).

- [ ] **Step 6: Fix existing tests that call `from_permissions()` without policy arg**

In the `test_from_permissions` test, update the call:

```rust
let guard = SafetyGuard::from_permissions(&global, &agent, None);
```

- [ ] **Step 7: Fix callers of `from_permissions()` in `run_loop.rs`**

Two callers exist in `src/gateway/execution_engine/run_loop.rs`:

At line 285, update:

```rust
move || SafetyGuard::from_permissions(&global_perms, &agent_perms_clone, Some(ToolSafetyPolicy::default()))
```

At line 510, update:

```rust
let safety = SafetyGuard::from_permissions(&self.global_tool_permissions, &agent_perms, Some(ToolSafetyPolicy::default()));
```

Add the import at the top of `run_loop.rs`:

```rust
use crate::config::types::policies::ToolSafetyPolicy;
```

Verify compilation: `cargo check -p alephcore 2>&1 | tail -10`

- [ ] **Step 8: Update the existing `test_default_guard_has_sensible_defaults` test**

The existing test checks for hardcoded tool names like `bash_exec` and `code_exec`. These should still work via keyword inference (`bash` and `exec` are high-risk keywords). Verify:

Run: `cargo test -p alephcore --lib safety -- test_default_guard 2>&1 | tail -20`

Expected: PASS — keyword inference covers the same tools the hardcoded list did.

- [ ] **Step 9: Run full safety test suite**

Run: `cargo test -p alephcore --lib safety 2>&1 | tail -20`

Expected: all tests PASS.

- [ ] **Step 10: Commit**

```bash
git add src/agent_loop/safety.rs
git commit -m "safety: replace hardcoded high-risk tools with ToolSafetyPolicy inference"
```

---

### Task 3: Hook-Permission Collision Fix (Part D)

**Files:**
- Modify: `src/agent_loop/tool_pipeline.rs:265-297` (Stage 4 safety check)

- [ ] **Step 1: Write failing test for collision scenario**

Add to the `#[cfg(test)] mod tests` block in `src/agent_loop/tool_pipeline.rs`:

```rust
#[tokio::test]
async fn test_hook_ask_plus_safety_ask_converge_to_confirmation() {
    // Setup: SafetyGuard classifies "shell" as Ask
    let perms = [("shell".to_string(), PermissionAction::Ask)]
        .into_iter()
        .collect();
    let safety = Arc::new(SafetyGuard::new(vec![], perms, PermissionAction::Allow));

    // Setup: Hook executor that emits Ask decision
    let hooks = vec![HookConfig {
        event: HookEvent::BeforeToolCall,
        kind: HookKind::Interceptor,
        priority: HookPriority::default(),
        matcher: None,
        actions: vec![HookAction::Command {
            command: "echo 'ask: confirm dangerous operation'".to_string(),
        }],
        plugin_name: "test".to_string(),
        plugin_root: std::path::PathBuf::from("/tmp"),
        handler: None,
    }];
    let hook_exec = Arc::new(HookExecutor::new(hooks));

    let pipeline = ToolPipeline::new(hook_exec, safety, "test-session");
    let cancel = CancellationToken::new();

    let mut registry = LoopToolRegistry::new();
    registry.register(Box::new(EchoTool));
    let registry = Arc::new(registry);

    let outcome = pipeline
        .execute("t1", "shell", &json!({"command": "echo hi"}), &registry, &cancel)
        .await;

    // Key assertion: needs_user_confirmation is true, NOT an error
    assert!(outcome.needs_user_confirmation);
    assert!(outcome.confirmation_reason.is_some());
    // The tool should NOT have been blocked — it awaits confirmation
    assert!(!outcome.outcome.output_text.starts_with("[DENIED]"));
    assert!(!outcome.outcome.output_text.starts_with("[BLOCKED]"));
}
```

Note: `EchoTool` is a simple test tool — if it doesn't already exist in the test module, add:

```rust
struct EchoTool;

#[async_trait]
impl LoopTool for EchoTool {
    fn name(&self) -> &str { "shell" }
    fn description(&self) -> &str { "echo tool" }
    fn schema(&self) -> Value { json!({"type": "object", "properties": {}}) }
    async fn execute(&self, _input: Value) -> ToolResult {
        ToolResult::Success { output: json!("ok") }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib tool_pipeline -- test_hook_ask_plus_safety 2>&1 | tail -20`

Expected: FAIL — currently Safety Ask returns error immediately, overriding Hook Ask.

- [ ] **Step 3: Fix Stage 4 to handle NeedsConfirmation as confirmation**

In `src/agent_loop/tool_pipeline.rs`, in the Stage 4 safety check block (around line 277), replace:

```rust
if let Err(e) = safety_result {
    let msg = map_safety_error(&e);
    return PipelineOutcome {
        outcome: ToolOutcome {
            tool_id: id.to_string(),
            tool_name: name.to_string(),
            duration_ms: 0,
            output_text: msg,
            is_error: true,
            should_stop: false,
            retryable: false,
        },
        additional_contexts,
        prevent_continuation,
        hook_messages,
        needs_user_confirmation: false,
        confirmation_reason: None,
    };
}
```

with:

```rust
if let Err(e) = safety_result {
    match e {
        SafetyError::NeedsConfirmation { ref tool } => {
            // Safety agrees tool needs confirmation.
            // Don't return error — route through confirmation flow.
            if !needs_user_confirmation {
                needs_user_confirmation = true;
                confirmation_reason =
                    Some(format!("Tool '{}' is classified as high-risk", tool));
            }
            tracing::debug!(
                tool = name,
                "safety NeedsConfirmation routed to confirmation flow"
            );
        }
        _ => {
            // Blocked or PolicyDenied — hard stop
            let msg = map_safety_error(&e);
            return PipelineOutcome {
                outcome: ToolOutcome {
                    tool_id: id.to_string(),
                    tool_name: name.to_string(),
                    duration_ms: 0,
                    output_text: msg,
                    is_error: true,
                    should_stop: false,
                    retryable: false,
                },
                additional_contexts,
                prevent_continuation,
                hook_messages,
                needs_user_confirmation: false,
                confirmation_reason: None,
            };
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib tool_pipeline -- test_hook_ask_plus_safety 2>&1 | tail -10`

Expected: PASS

- [ ] **Step 5: Run full pipeline test suite**

Run: `cargo test -p alephcore --lib tool_pipeline 2>&1 | tail -20`

Expected: all tests PASS.

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/tool_pipeline.rs
git commit -m "pipeline: route NeedsConfirmation to confirmation flow instead of hard deny"
```

---

### Task 4: Structured Pipeline Tracing (Part C)

**Files:**
- Modify: `src/agent_loop/tool_pipeline.rs:117-408` (all stages)

- [ ] **Step 1: Refactor Stage 2 (validation) to use structured span**

Replace the validation span (around line 140-141):

```rust
{
    let _span = tracing::info_span!("pipeline_validate").entered();
```

with:

```rust
{
    let _span = tracing::info_span!("pipeline.validate",
        tool = name, tool_id = id
    ).entered();
```

- [ ] **Step 2: Refactor Stage 3 (pre-hooks) to use structured span**

Wrap the pre-hooks block (around line 170) with a span. Replace:

```rust
tracing::debug!("pipeline_pre_hooks: start");
```

with a span at the beginning of the `if self.has_hooks()` block:

```rust
let _hooks_span = tracing::info_span!("pipeline.pre_hooks",
    tool = name, hooks_count = tracing::field::Empty
).entered();
```

Remove the manual timing:

```rust
// DELETE these lines:
// let hook_start = Instant::now();
// ...
// tracing::info!(
//     tool = name,
//     elapsed_ms = hook_start.elapsed().as_millis() as u64,
//     "pre-hooks completed"
// );
```

The span auto-records duration.

- [ ] **Step 3: Hoist hook decision variable and add Stage 4 decision trace**

First, before the `if self.has_hooks()` block in Stage 3, declare:

```rust
let mut hook_decision: Option<&str> = None;
```

Inside the `match decision` block (Stage 3), after each arm, record the decision:

```rust
Some(PermissionDecision::Deny { .. }) => { hook_decision = Some("deny"); /* ... */ }
Some(PermissionDecision::Block { .. }) => { hook_decision = Some("block"); /* ... */ }
Some(PermissionDecision::Ask { .. }) => { hook_decision = Some("ask"); /* ... */ }
Some(PermissionDecision::Allow) => { hook_decision = Some("allow"); /* ... */ }
None => {}
```

Then after Stage 4's safety check completes, add the structured decision log:

```rust
let final_action = if needs_user_confirmation {
    "confirm"
} else {
    "execute"
};
tracing::info!(
    tool = name,
    hook_decision = hook_decision.unwrap_or("none"),
    safety_passed = true,
    final_action = final_action,
    "permission resolved"
);
```

- [ ] **Step 4: Refactor Stage 5 (execute) span**

Replace:

```rust
tracing::debug!("pipeline_execute: start");
let exec_start = Instant::now();
```

with:

```rust
let _exec_span = tracing::info_span!("pipeline.execute",
    tool = name, tool_id = id
).entered();
```

Keep `Instant::now()` only for populating `outcome.duration_ms` (this is a data field, not a log).

- [ ] **Step 5: Refactor Stages 6-7 (post-hooks) span**

Replace:

```rust
tracing::debug!("pipeline_post_hooks: start");
```

and the manual timing block with:

```rust
let _post_span = tracing::info_span!("pipeline.post_hooks",
    tool = name, is_error = outcome.is_error
).entered();
```

Remove the manual post-hook timing `tracing::info!(... elapsed_ms ...)`.

- [ ] **Step 6: Verify compilation and run tests**

Run: `cargo test -p alephcore --lib tool_pipeline 2>&1 | tail -20`

Expected: all tests PASS. Tracing changes are behavioral no-ops for tests.

- [ ] **Step 7: Commit**

```bash
git add src/agent_loop/tool_pipeline.rs
git commit -m "pipeline: structured tracing spans for all stages"
```

---

### Task 5: Phase 1 integration test

**Files:**
- Existing test infrastructure in `src/agent_loop/safety.rs` and `src/agent_loop/tool_pipeline.rs`

- [ ] **Step 1: Run full agent_loop test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`

Expected: all tests PASS.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -20`

Expected: no warnings.

- [ ] **Step 3: Commit Phase 1 completion marker (if any fixes needed)**

```bash
git add -A
git commit -m "chore: Phase 1 cleanup — fix clippy warnings"
```

---

## Phase 2: Capability Addition (Tasks 6-7)

### Task 6: LoopCallback — Add `on_confirmation_needed()`

**Files:**
- Modify: `src/agent_loop/loop_core.rs:606-641` (LoopCallback trait)

- [ ] **Step 1: Add `on_confirmation_needed()` to LoopCallback trait**

In `src/agent_loop/loop_core.rs`, add after `on_stop_hook_error` (line 639):

```rust
/// Request user confirmation for a high-risk tool call.
///
/// Called when SafetyGuard or a Hook classifies a tool as needing
/// user confirmation before execution. Default returns `false` (reject),
/// preserving backward compatibility with all Channel implementations.
///
/// Channels override this to implement their own confirmation UX:
/// CLI → stdin prompt, Telegram → inline keyboard, API → webhook.
fn on_confirmation_needed(
    &mut self,
    _tool_name: &str,
    _tool_input: &Value,
    _reason: &str,
) -> bool {
    false
}
```

Note: This is a synchronous method (not async) because `LoopCallback` is `Send` but not `Async`. The Channel implementations that need async (like Telegram) should use internal synchronization (e.g., a `tokio::sync::oneshot` channel stored beforehand).

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | tail -10`

Expected: compiles. Default method means no existing impls break.

- [ ] **Step 3: Update test mock callback**

In the test callback struct (around line 2425), add:

```rust
fn on_confirmation_needed(
    &mut self,
    tool_name: &str,
    _tool_input: &Value,
    reason: &str,
) -> bool {
    // Test callback records the request but rejects by default
    self.safety_blocks.push(format!("confirmation_needed: {} ({})", tool_name, reason));
    false
}
```

- [ ] **Step 4: Commit**

```bash
git add src/agent_loop/loop_core.rs
git commit -m "loop: add on_confirmation_needed() to LoopCallback trait"
```

---

### Task 7: Consume `needs_user_confirmation` in loop_core

**Files:**
- Modify: `src/agent_loop/loop_core.rs:1535-1615` (outcome processing)

- [ ] **Step 1: Write failing test for confirmation flow**

Add to the test module in `src/agent_loop/loop_core.rs`:

```rust
#[test]
fn test_confirmation_needed_calls_callback() {
    // This test verifies that when PipelineOutcome has needs_user_confirmation,
    // the callback's on_confirmation_needed is called instead of treating as error.
    // Exact integration test depends on the loop harness — for now, verify
    // the callback method exists and can be called.
    let mut callback = TestCallback::new();
    let result = callback.on_confirmation_needed("shell", &json!({"command": "rm -rf temp"}), "high-risk tool");
    assert!(!result); // default rejects
    assert!(callback.safety_blocks.iter().any(|s| s.contains("confirmation_needed")));
}
```

- [ ] **Step 2: Run test to verify it passes** (callback method was added in Task 6)

Run: `cargo test -p alephcore --lib loop_core -- test_confirmation_needed 2>&1 | tail -10`

Expected: PASS.

- [ ] **Step 3: Modify outcome processing to check `needs_user_confirmation`**

In `src/agent_loop/loop_core.rs`, in the outcome processing loop (around line 1535), add a check BEFORE the `is_safety_denial` block. After `let o = &outcome.outcome;` (line 1536), add:

```rust
// Confirmation flow: if pipeline flagged confirmation needed,
// ask the channel before treating as error.
if outcome.needs_user_confirmation {
    let reason = outcome
        .confirmation_reason
        .as_deref()
        .unwrap_or("Tool requires confirmation");
    let confirmed = callback.on_confirmation_needed(
        &o.tool_name,
        &tool_args_by_id
            .get(&o.tool_id)
            .cloned()
            .unwrap_or(json!({})),
        reason,
    );
    if confirmed {
        // User confirmed — the tool already executed (pipeline ran it),
        // proceed normally with the result.
        // Skip the safety_denial classification below.
    } else {
        // User rejected — override output to denial message.
        let denial_msg = format!(
            "[DENIED] Tool '{}' requires user confirmation. Confirmation was rejected.",
            o.tool_name
        );
        messages.push(UnifiedMessage::tool_result(
            o.tool_id.clone(),
            o.tool_name.clone(),
            denial_msg,
            true,
        ));
        callback.on_safety_block(&SafetyError::NeedsConfirmation {
            tool: o.tool_name.clone(),
        });
        continue; // skip normal outcome processing
    }
}
```

- [ ] **Step 4: Remove `[NEEDS_CONFIRMATION]` from `is_safety_denial` check**

In the `is_safety_denial` check (line 1538-1541), remove the `NEEDS_CONFIRMATION` branch since it's now handled above:

```rust
let is_safety_denial = o.is_error
    && (o.output_text.starts_with("[BLOCKED]")
        || o.output_text.starts_with("[DENIED]"));
```

- [ ] **Step 5: Run full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`

Expected: all tests PASS.

- [ ] **Step 6: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -20`

Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add src/agent_loop/loop_core.rs
git commit -m "loop: consume needs_user_confirmation via LoopCallback instead of silent denial"
```

---

## Final Verification

- [ ] **Run full project build**: `cargo build -p alephcore 2>&1 | tail -10`
- [ ] **Run full test suite**: `cargo test -p alephcore 2>&1 | tail -30`
- [ ] **Run clippy**: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -20`
