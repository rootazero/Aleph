# Tool Calling 2.0 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable parallel tool execution and Strict Mode in Aleph's agent loop, upgrading from single-tool-per-turn to batch execution.

**Architecture:** Compiler-driven refactoring — change `Decision` enum first, fix all match sites via `cargo check`, then wire parallel execution and Strict Mode. TDD where feasible; mechanical replacement where not.

**Tech Stack:** Rust, tokio (JoinSet), serde_json, schemars

**Design doc:** `docs/plans/2026-03-11-tool-calling-2.0-design.md`

---

## Task 1: New Core Types (ToolCallRecord, ToolCallRequest, ToolCallResult)

**Files:**
- Modify: `src/agent_loop/decision.rs:1-30` (add types before Decision enum)

**Step 1: Add the three new structs**

Add before the `Decision` enum (line 28):

```rust
/// A single tool call from LLM response, with provider-assigned ID.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallRecord {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: Value,
}

/// A single tool call ready for execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: Value,
}

/// Result of a single tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub call_id: String,
    pub tool_name: String,
    pub result: SingleToolResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SingleToolResult {
    Success { output: Value, duration_ms: u64 },
    Error { error: String, retryable: bool },
}

impl ToolCallResult {
    pub fn is_success(&self) -> bool {
        matches!(self.result, SingleToolResult::Success { .. })
    }

    pub fn is_error(&self) -> bool {
        matches!(self.result, SingleToolResult::Error { .. })
    }
}
```

**Step 2: Verify it compiles**

Run: `cargo check -p alephcore`
Expected: PASS (new types, no consumers yet)

**Step 3: Commit**

```bash
git add src/agent_loop/decision.rs
git commit -m "core: add ToolCallRecord, ToolCallRequest, ToolCallResult types"
```

---

## Task 2: Replace `Decision::UseTool` with `Decision::UseTools`

**Files:**
- Modify: `src/agent_loop/decision.rs:28-100` (Decision enum)
- Modify: `src/agent_loop/decision.rs:334-389` (LlmAction enum + From impl)

**Step 1: Change the Decision enum**

Replace the `UseTool` variant (lines 30-33):

```rust
// OLD:
UseTool {
    tool_name: String,
    arguments: Value,
},

// NEW:
UseTools(Vec<ToolCallRecord>),
```

**Step 2: Add the deprecated adapter**

Add an `impl Decision` block:

```rust
impl Decision {
    #[deprecated(note = "Migrate to handle UseTools batch directly")]
    pub fn as_single_tool(&self) -> Option<(&str, &Value)> {
        match self {
            Decision::UseTools(calls) if !calls.is_empty() => {
                Some((&calls[0].tool_name, &calls[0].arguments))
            }
            _ => None,
        }
    }
}
```

**Step 3: Update LlmAction enum**

Replace `LlmAction::UseTool` variant (around line 334) with:

```rust
UseTools(Vec<ToolCallRecord>),
```

**Step 4: Update `From<LlmAction> for Decision`**

In the From impl (line 366), replace the UseTool arm:

```rust
// OLD:
LlmAction::UseTool { tool_name, arguments } => Decision::UseTool { tool_name, arguments },

// NEW:
LlmAction::UseTools(records) => Decision::UseTools(records),
```

**Step 5: Run cargo check to find all broken match sites**

Run: `cargo check -p alephcore 2>&1 | head -100`
Expected: FAIL — many match exhaustiveness errors. This is the "error chain" that guides the rest of the refactoring.

**Step 6: Do NOT fix yet — just commit the enum change**

```bash
git add src/agent_loop/decision.rs
git commit -m "core: replace Decision::UseTool with UseTools(Vec<ToolCallRecord>)"
```

---

## Task 3: Replace `Action::ToolCall` with `Action::ToolCalls`

**Files:**
- Modify: `src/agent_loop/decision.rs:104-136` (Action enum)

**Step 1: Change the Action enum**

Replace `ToolCall` variant:

```rust
// OLD:
ToolCall {
    tool_name: String,
    arguments: Value,
},

// NEW:
ToolCalls(Vec<ToolCallRequest>),
```

**Step 2: Run cargo check**

Run: `cargo check -p alephcore 2>&1 | head -100`
Expected: More match errors (accumulating with Task 2 errors). Note them.

**Step 3: Commit**

```bash
git add src/agent_loop/decision.rs
git commit -m "core: replace Action::ToolCall with ToolCalls(Vec<ToolCallRequest>)"
```

---

## Task 4: Replace `ActionResult::ToolSuccess/ToolError` with `ActionResult::ToolResults`

**Files:**
- Modify: `src/agent_loop/decision.rs:230-255` (ActionResult enum)

**Step 1: Change the ActionResult enum**

Replace `ToolSuccess` and `ToolError` variants:

```rust
// OLD:
ToolSuccess {
    output: Value,
    duration_ms: u64,
},
ToolError {
    error: String,
    retryable: bool,
},

// NEW:
ToolResults(Vec<ToolCallResult>),
```

**Step 2: Run cargo check**

Run: `cargo check -p alephcore 2>&1 | head -100`
Expected: Maximum error count. All three enum changes are now in place.

**Step 3: Commit**

```bash
git add src/agent_loop/decision.rs
git commit -m "core: replace ActionResult::ToolSuccess/ToolError with ToolResults(Vec)"
```

---

## Task 5: Update Thinking struct (remove tool_call_id)

**Files:**
- Modify: `src/agent_loop/state.rs:189-204` (Thinking struct)

**Step 1: Remove `tool_call_id` field**

Remove line 203:

```rust
// REMOVE:
#[serde(default, skip_serializing_if = "Option::is_none")]
pub tool_call_id: Option<String>,
```

**Step 2: Run cargo check (accumulating)**

Run: `cargo check -p alephcore 2>&1 | grep "tool_call_id" | head -20`
Expected: Errors where `thinking.tool_call_id` is referenced. Note these locations.

**Step 3: Commit**

```bash
git add src/agent_loop/state.rs
git commit -m "core: remove Thinking.tool_call_id (IDs now in ToolCallRecord)"
```

---

## Task 6: Add `total_tool_calls` counter to LoopState

**Files:**
- Modify: `src/agent_loop/state.rs:16-42` (LoopState struct)

**Step 1: Add the counter field**

Add to the LoopState struct:

```rust
pub total_tool_calls: usize,
```

Initialize it to `0` in the constructor / `Default` impl.

**Step 2: Run cargo check**

Run: `cargo check -p alephcore 2>&1 | head -50`
Expected: May need to update constructor calls. Fix any `LoopState { ... }` literal that's now missing the field.

**Step 3: Commit**

```bash
git add src/agent_loop/state.rs
git commit -m "core: add LoopState.total_tool_calls counter"
```

---

## Task 7: Fix all match sites in agent_loop.rs (the big mechanical fix)

**Files:**
- Modify: `src/agent_loop/agent_loop.rs` (multiple match sites)

This is the largest single task. Use `cargo check` errors as guide.

**Step 1: Catalog all errors**

Run: `cargo check -p alephcore 2>&1 | grep "agent_loop.rs" | head -50`

**Step 2: Fix Decision::UseTool matches**

Find every `Decision::UseTool { tool_name, arguments }` pattern (around line 741). Replace with:

```rust
Decision::UseTools(ref records) => {
    // For now, sequential execution (parallel comes in Task 10)
    let mut tool_results = Vec::new();
    for record in records {
        let action = Action::ToolCalls(vec![ToolCallRequest {
            call_id: record.call_id.clone(),
            tool_name: record.tool_name.clone(),
            arguments: record.arguments.clone(),
        }]);
        let result = executor.execute(&action, &identity).await;
        if let ActionResult::ToolResults(mut results) = result {
            tool_results.append(&mut results);
        }
    }
    // Combine into batch result
    let batch_result = ActionResult::ToolResults(tool_results);
    // ... rest of step recording
}
```

**Step 3: Fix ActionResult::ToolSuccess/ToolError matches**

Replace all `ActionResult::ToolSuccess { output, duration_ms }` and `ActionResult::ToolError { error, retryable }` with `ActionResult::ToolResults(ref results)` and process the vec.

**Step 4: Fix Action::ToolCall matches**

Replace `Action::ToolCall { tool_name, arguments }` with `Action::ToolCalls(ref requests)`.

**Step 5: Fix thinking.tool_call_id references**

Remove or adapt any code that reads `thinking.tool_call_id`. The call IDs are now in `thinking.decision` (inside `UseTools` records).

**Step 6: Run cargo check**

Run: `cargo check -p alephcore 2>&1 | grep "agent_loop.rs"`
Expected: Zero errors from this file.

**Step 7: Commit**

```bash
git add src/agent_loop/agent_loop.rs
git commit -m "core: update agent_loop to use UseTools/ToolCalls/ToolResults"
```

---

## Task 8: Fix remaining match sites across codebase

**Files:**
- Multiple files identified by `cargo check`

**Step 1: Run cargo check and catalog remaining errors**

Run: `cargo check -p alephcore 2>&1`

Expected errors in:
- `src/thinker/mod.rs` (tool_call_id, Decision construction)
- `src/thinker/decision_parser.rs` (Decision::UseTool construction)
- `src/agent_loop/traits.rs` (ActionExecutor if it references old types)
- Any other file that pattern-matches on Decision/Action/ActionResult

**Step 2: Fix each file**

For each error, apply the mechanical transformation:
- `Decision::UseTool { tool_name, arguments }` → `Decision::UseTools(vec![ToolCallRecord { call_id: generate_id(), tool_name, arguments }])`
- `Action::ToolCall { .. }` → `Action::ToolCalls(vec![ToolCallRequest { .. }])`
- `ActionResult::ToolSuccess { .. }` → `ActionResult::ToolResults(vec![ToolCallResult { .. }])`
- `thinking.tool_call_id = Some(id)` → remove (ID already in ToolCallRecord)

**Step 3: Run cargo check until clean**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib`
Expected: PASS (or known pre-existing failures only)

**Step 5: Commit**

```bash
git add -A
git commit -m "core: fix all remaining match sites for Tool Calling 2.0 types"
```

---

## Task 9: Rewrite Thinker mapping with terminal defense

**Files:**
- Modify: `src/thinker/mod.rs:426-518` (mapping functions)
- Modify: `src/thinker/virtual_tools.rs:14-16` (add priority constant)

**Step 1: Add terminal priority constant**

In `src/thinker/virtual_tools.rs`, add:

```rust
/// Priority order for conflicting terminal actions: fail > ask > complete
pub const TERMINAL_PRIORITY: &[&str] = &[VIRTUAL_FAIL, VIRTUAL_ASK_USER, VIRTUAL_COMPLETE];
```

**Step 2: Split `map_native_tool_call_to_decision` into two functions**

In `src/thinker/mod.rs`, replace the single function (lines 426-471) with:

```rust
/// Maps virtual tool calls to terminal decisions.
fn map_virtual_tool_to_decision(
    &self,
    tc: &crate::providers::adapter::NativeToolCall,
) -> Decision {
    match tc.name.as_str() {
        VIRTUAL_COMPLETE => {
            let summary = tc.arguments.get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Decision::Complete { summary }
        }
        VIRTUAL_ASK_USER => {
            let question = tc.arguments.get("question")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Decision::AskUser { question, options: None }
        }
        VIRTUAL_FAIL => {
            let reason = tc.arguments.get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Decision::Fail { reason }
        }
        _ => Decision::Silent, // should not reach here
    }
}

/// Maps a real tool call to a ToolCallRecord.
fn map_to_record(tc: &crate::providers::adapter::NativeToolCall) -> ToolCallRecord {
    ToolCallRecord {
        call_id: tc.id.clone(),
        tool_name: tc.name.clone(),
        arguments: tc.arguments.clone(),
    }
}
```

**Step 3: Rewrite `build_thinking_from_native_response`**

Replace lines 473-518 with the two-phase logic:

```rust
fn build_thinking_from_native_response(
    &self,
    response: ProviderResponse,
) -> Result<Thinking> {
    let reasoning = response.thinking.clone().or(response.text.clone());
    let tokens_used = response.usage.as_ref().map(|u| u.total());

    if !response.has_tool_calls() {
        // No tool calls — fallback to text parsing
        let text = response.text.as_deref().unwrap_or("");
        let decision = self.decision_parser.parse(text)?;
        return Ok(Thinking {
            reasoning,
            decision,
            structured: None,
            tokens_used,
        });
    }

    // Phase 1: Partition into virtual and real calls
    let (virtual_calls, real_calls): (Vec<_>, Vec<_>) = response
        .tool_calls
        .iter()
        .partition(|tc| is_virtual_tool(&tc.name));

    // Phase 2: Terminal defense — virtual tools take priority
    if !virtual_calls.is_empty() {
        let terminal = pick_terminal(&virtual_calls);
        let decision = self.map_virtual_tool_to_decision(terminal);
        return Ok(Thinking {
            reasoning,
            decision,
            structured: None,
            tokens_used,
        });
    }

    // Phase 3: Batch all real tool calls
    let records: Vec<ToolCallRecord> = real_calls
        .iter()
        .map(|tc| Self::map_to_record(tc))
        .collect();

    Ok(Thinking {
        reasoning,
        decision: Decision::UseTools(records),
        structured: None,
        tokens_used,
    })
}

/// Pick highest-priority terminal action from conflicting virtuals.
fn pick_terminal<'a>(
    virtuals: &[&'a crate::providers::adapter::NativeToolCall],
) -> &'a crate::providers::adapter::NativeToolCall {
    for name in TERMINAL_PRIORITY {
        if let Some(tc) = virtuals.iter().find(|v| v.name == *name) {
            return tc;
        }
    }
    virtuals[0]
}
```

**Step 4: Run cargo check**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 5: Run tests**

Run: `cargo test -p alephcore --lib`
Expected: PASS

**Step 6: Commit**

```bash
git add src/thinker/mod.rs src/thinker/virtual_tools.rs
git commit -m "thinker: rewrite native response mapping with parallel collection and terminal defense"
```

---

## Task 10: Wire parallel execution via JoinSet

**Files:**
- Modify: `src/agent_loop/traits.rs:70-80` (ActionExecutor trait)
- Modify: `src/agent_loop/agent_loop.rs` (execution block from Task 7)

**Step 1: Extend ActionExecutor trait**

Add methods to the trait (line 70):

```rust
#[async_trait]
pub trait ActionExecutor: Send + Sync {
    async fn execute(&self, action: &Action, identity: &IdentityContext) -> ActionResult;

    async fn execute_single_tool(
        &self,
        req: &ToolCallRequest,
        identity: &IdentityContext,
    ) -> ToolCallResult {
        // Default: delegate to execute() and unwrap
        let action = Action::ToolCalls(vec![req.clone()]);
        let result = self.execute(&action, identity).await;
        match result {
            ActionResult::ToolResults(mut results) if !results.is_empty() => results.remove(0),
            _ => ToolCallResult {
                call_id: req.call_id.clone(),
                tool_name: req.tool_name.clone(),
                result: SingleToolResult::Error {
                    error: "Unexpected result type".into(),
                    retryable: false,
                },
            },
        }
    }
}
```

**Step 2: Update agent_loop.rs execution block**

Replace the sequential loop from Task 7 with parallel JoinSet execution:

```rust
Decision::UseTools(ref records) => {
    let requests: Vec<ToolCallRequest> = records.iter().map(|r| ToolCallRequest {
        call_id: r.call_id.clone(),
        tool_name: r.tool_name.clone(),
        arguments: r.arguments.clone(),
    }).collect();

    // Doom loop check per call
    for req in &requests {
        self.check_doom_loop(req, &state)?;
    }

    // Batch confirmation
    let needs_confirm: Vec<&ToolCallRequest> = requests.iter()
        .filter(|r| self.requires_confirmation(&r.tool_name))
        .collect();
    if !needs_confirm.is_empty() {
        if !self.request_batch_confirmation(&needs_confirm).await? {
            // User denied — record denial and continue loop
            // ... build denial result ...
            continue;
        }
    }

    let results = if requests.len() == 1 {
        // N=1 fast path
        vec![executor.execute_single_tool(&requests[0], &identity).await]
    } else {
        // Parallel path
        let mut join_set = tokio::task::JoinSet::new();
        for req in requests.clone() {
            let exec = executor.clone();
            let id = identity.clone();
            join_set.spawn(async move {
                exec.execute_single_tool(&req, &id).await
            });
        }

        let mut results = Vec::with_capacity(requests.len());
        while let Some(join_result) = join_set.join_next().await {
            match join_result {
                Ok(tool_result) => results.push(tool_result),
                Err(join_err) => results.push(ToolCallResult {
                    call_id: "unknown".into(),
                    tool_name: "unknown".into(),
                    result: SingleToolResult::Error {
                        error: format!("Task panicked: {join_err}"),
                        retryable: false,
                    },
                }),
            }
        }

        // Restore request order
        results.sort_by_key(|r| {
            requests.iter().position(|req| req.call_id == r.call_id).unwrap_or(usize::MAX)
        });
        results
    };

    // Update counter
    state.total_tool_calls += results.len();

    let batch_result = ActionResult::ToolResults(results);
    // ... record step and build feedback messages ...
}
```

**Step 3: Ensure executor is Clone**

Check that the concrete `ActionExecutor` impl is `Clone` (needed for `JoinSet::spawn`). If not, wrap in `Arc`:

```rust
let exec = Arc::new(executor);
// In spawn: let exec = Arc::clone(&exec);
```

**Step 4: Run cargo check**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 5: Commit**

```bash
git add src/agent_loop/traits.rs src/agent_loop/agent_loop.rs
git commit -m "core: wire parallel tool execution via JoinSet"
```

---

## Task 11: Update feedback message generation

**Files:**
- Modify: `src/thinker/prompt_builder/messages.rs:57-63` (native_tool_result method)
- Modify: `src/agent_loop/agent_loop.rs` (feedback section)

**Step 1: Add batch message builder**

In `messages.rs`, add a new method:

```rust
/// Build tool result messages for a batch of results.
pub fn native_tool_results(results: &[ToolCallResult]) -> Vec<Self> {
    results.iter().map(|r| {
        let content = match &r.result {
            SingleToolResult::Success { output, .. } => {
                format!("[{}]\n{}", r.tool_name, serde_json::to_string(output).unwrap_or_default())
            }
            SingleToolResult::Error { error, .. } => {
                format!("[{}]\nError: {}", r.tool_name, error)
            }
        };
        Self {
            role: MessageRole::Tool,
            content,
            tool_call_id: Some(r.call_id.clone()),
        }
    }).collect()
}
```

**Step 2: Update agent_loop.rs feedback section**

Where tool results are added to conversation history, replace single-message logic with:

```rust
if let ActionResult::ToolResults(ref results) = batch_result {
    let messages = Message::native_tool_results(results);
    for msg in messages {
        state.add_message(msg);
    }
}
```

**Step 3: Run cargo check + tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: PASS

**Step 4: Commit**

```bash
git add src/thinker/prompt_builder/messages.rs src/agent_loop/agent_loop.rs
git commit -m "core: batch tool result feedback messages with call_id correlation"
```

---

## Task 12: Adapt DecisionParser for JSON-in-text path

**Files:**
- Modify: `src/thinker/decision_parser.rs` (wherever Decision::UseTool is constructed)

**Step 1: Find all Decision::UseTool constructions**

Run: `grep -n "Decision::UseTool" src/thinker/decision_parser.rs`

**Step 2: Replace each with UseTools(vec![...])**

```rust
// OLD:
Decision::UseTool { tool_name: name, arguments: args }

// NEW:
Decision::UseTools(vec![ToolCallRecord {
    call_id: format!("synth_{}", uuid::Uuid::new_v4()),
    tool_name: name,
    arguments: args,
}])
```

If `uuid` is not available, use a simpler ID generator:

```rust
call_id: format!("synth_{}", std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos()),
```

**Step 3: Run cargo check + tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: PASS

**Step 4: Commit**

```bash
git add src/thinker/decision_parser.rs
git commit -m "thinker: adapt DecisionParser to emit UseTools with synthetic call_ids"
```

---

## Task 13: Add `strict` field to ToolDefinition

**Files:**
- Modify: `src/dispatcher/types/definition.rs:24-44` (ToolDefinition struct)
- Modify: `src/dispatcher/types/definition.rs:97-115` (serialization methods)

**Step 1: Add strict field**

```rust
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub requires_confirmation: bool,
    pub category: ToolCategory,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_context: Option<String>,
    #[serde(default)]
    pub strict: bool,  // NEW
}
```

**Step 2: Update constructors**

Find all `ToolDefinition { ... }` literals and add `strict: true` (or `false` for special cases). Use `cargo check` to find them.

**Step 3: Update to_openai_function**

```rust
pub fn to_openai_function(&self) -> Value {
    let mut func = serde_json::json!({
        "type": "function",
        "function": {
            "name": self.name,
            "description": self.description,
            "parameters": self.parameters
        }
    });
    if self.strict {
        func["function"]["strict"] = serde_json::json!(true);
    }
    func
}
```

`to_anthropic_tool` unchanged (Anthropic has no explicit strict flag).

**Step 4: Run cargo check + tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add src/dispatcher/types/definition.rs
git commit -m "core: add strict field to ToolDefinition with OpenAI serialization"
```

---

## Task 14: Add `strict_schema()` to AlephTool trait

**Files:**
- Modify: `src/tools/traits.rs:64-155` (AlephTool trait)

**Step 1: Add default method**

```rust
/// Whether this tool's schema is strict-mode compatible.
/// Default: true. Override to false for tools with dynamic schemas.
fn strict_schema(&self) -> bool { true }
```

**Step 2: Update `definition()` method**

In the default `definition()` implementation, set `strict` from `self.strict_schema()`:

```rust
fn definition(&self) -> ToolDefinition {
    // ... existing schema generation ...
    ToolDefinition {
        // ... existing fields ...
        strict: self.strict_schema(),
    }
}
```

**Step 3: Run cargo check + tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: PASS

**Step 4: Commit**

```bash
git add src/tools/traits.rs
git commit -m "core: add strict_schema() to AlephTool trait (default true)"
```

---

## Task 15: Create schema_strictify module

**Files:**
- Create: `src/tools/schema_strictify.rs`
- Modify: `src/tools/mod.rs` (add module declaration)

**Step 1: Write tests first**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_strictify_adds_required_and_no_additional() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "integer" }
            }
        });
        strictify_schema(&mut schema);
        assert_eq!(schema["additionalProperties"], json!(false));
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("name")));
        assert!(required.contains(&json!("age")));
    }

    #[test]
    fn test_strictify_recurses_into_nested_objects() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "config": {
                    "type": "object",
                    "properties": {
                        "key": { "type": "string" }
                    }
                }
            }
        });
        strictify_schema(&mut schema);
        assert_eq!(schema["properties"]["config"]["additionalProperties"], json!(false));
        let nested_required = schema["properties"]["config"]["required"].as_array().unwrap();
        assert!(nested_required.contains(&json!("key")));
    }

    #[test]
    fn test_strictify_non_object_is_noop() {
        let mut schema = json!({ "type": "string" });
        let original = schema.clone();
        strictify_schema(&mut schema);
        assert_eq!(schema, original);
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib schema_strictify`
Expected: FAIL (module doesn't exist yet)

**Step 3: Implement the module**

```rust
//! Transform schemars-generated JSON Schema into strict-mode compatible format.

use serde_json::Value;

/// Recursively transform a JSON Schema for strict mode compatibility.
/// - Sets `additionalProperties: false` on all object types
/// - Makes all properties required
pub fn strictify_schema(schema: &mut Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };

    if obj.get("type").and_then(|v| v.as_str()) == Some("object") {
        obj.insert("additionalProperties".into(), Value::Bool(false));

        if let Some(properties) = obj.get("properties").cloned() {
            if let Some(props) = properties.as_object() {
                let all_keys: Vec<Value> = props.keys()
                    .map(|k| Value::String(k.clone()))
                    .collect();
                obj.insert("required".into(), Value::Array(all_keys));
            }
        }
    }

    // Recurse into nested schemas
    for key in &["properties", "items", "definitions", "$defs"] {
        if let Some(nested) = obj.get_mut(*key) {
            strictify_nested(nested);
        }
    }

    for key in &["allOf", "anyOf", "oneOf"] {
        if let Some(arr) = obj.get_mut(*key) {
            if let Some(items) = arr.as_array_mut() {
                for item in items {
                    strictify_schema(item);
                }
            }
        }
    }
}

fn strictify_nested(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for v in map.values_mut() {
                strictify_schema(v);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                strictify_schema(v);
            }
        }
        _ => {}
    }
}
```

**Step 4: Add module to mod.rs**

In `src/tools/mod.rs`, add:

```rust
pub mod schema_strictify;
```

**Step 5: Run tests**

Run: `cargo test -p alephcore --lib schema_strictify`
Expected: PASS (3 tests)

**Step 6: Commit**

```bash
git add src/tools/schema_strictify.rs src/tools/mod.rs
git commit -m "core: add schema_strictify module for strict mode schema transformation"
```

---

## Task 16: Wire strictification into Thinker

**Files:**
- Modify: `src/thinker/mod.rs:523-537` (collect_native_tool_defs)

**Step 1: Apply strictification in collect_native_tool_defs**

```rust
fn collect_native_tool_defs(&self, filtered_tools: &[ToolInfo]) -> Vec<ToolDefinition> {
    let mut defs: Vec<ToolDefinition> = filtered_tools
        .iter()
        .map(|tool| {
            let mut def = tool.definition.clone();
            if def.strict {
                crate::tools::schema_strictify::strictify_schema(&mut def.parameters);
            }
            def
        })
        .collect();

    // Virtual tools: always strict
    let mut virtuals = virtual_tool_definitions();
    for v in &mut virtuals {
        v.strict = true;
        crate::tools::schema_strictify::strictify_schema(&mut v.parameters);
    }
    defs.extend(virtuals);

    defs
}
```

**Step 2: Run cargo check + tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: PASS

**Step 3: Commit**

```bash
git add src/thinker/mod.rs
git commit -m "thinker: wire schema strictification into native tool def collection"
```

---

## Task 17: Final integration test and cleanup

**Files:**
- All modified files

**Step 1: Full compilation check**

Run: `cargo check -p alephcore`
Expected: PASS, zero warnings about deprecated `as_single_tool` (no callers yet)

**Step 2: Run all tests**

Run: `cargo test -p alephcore --lib`
Expected: PASS (or only known pre-existing failures: `tools::markdown_skill::loader::tests`)

**Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -50`
Fix any new clippy warnings.

**Step 4: Remove deprecated adapter if no callers remain**

Check: `grep -r "as_single_tool" src/`
If no callers, remove the `#[deprecated]` method from Task 2.

**Step 5: Final commit**

```bash
git add -A
git commit -m "core: Tool Calling 2.0 — parallel execution and strict mode complete"
```

---

## Summary

| Task | Description | Risk |
|------|-------------|------|
| 1 | New core types | Low |
| 2 | Decision::UseTools | Low (compile errors expected) |
| 3 | Action::ToolCalls | Low (accumulating errors) |
| 4 | ActionResult::ToolResults | Low (accumulating errors) |
| 5 | Remove Thinking.tool_call_id | Low |
| 6 | Add total_tool_calls counter | Low |
| 7 | Fix agent_loop.rs match sites | **Medium** (largest single change) |
| 8 | Fix remaining match sites | **Medium** (cross-codebase) |
| 9 | Thinker parallel mapping + terminal defense | **Medium** (core logic rewrite) |
| 10 | JoinSet parallel execution | **High** (concurrency) |
| 11 | Batch feedback messages | Low |
| 12 | DecisionParser adaptation | Low |
| 13 | ToolDefinition strict field | Low |
| 14 | AlephTool strict_schema() | Low |
| 15 | schema_strictify module | Low (TDD) |
| 16 | Wire strictification | Low |
| 17 | Integration test + cleanup | Low |
