# Hook Pipeline Design: Tool Execution Runtime Policy Layer

**Date:** 2026-04-01
**Status:** Approved
**Scope:** Hook system integration into tool execution and session lifecycle

---

## Problem

Aleph's `HookExecutor` is fully implemented (3 execution kinds, regex caching, priority sorting, timeout control) but **not wired into the tool execution pipeline**. `execute_tool_batch` in `tool_orchestrator.rs` runs `safety check -> execute -> callback` with zero hook involvement. This means plugins/extensions cannot intercept, modify, or observe tool executions at runtime.

## Design Decisions

1. **Two-layer integration**: tool_orchestrator handles BeforeToolCall/AfterToolCall; run_loop handles SessionStart/MessageReceived/etc.
2. **Capability level B**: hooks can block + inject messages + inject additional_contexts + modify tool input (`updated_input`). No permission semantics (allow/ask/deny) — permission stays with SafetyGuard + LLM (R8 LLM Sovereignty).
3. **Injection via parameter**: new `&HookExecutor` parameter (wrapped in `ToolPipeline`), not via LoopCallback.

## Architecture

### 1. HookResult Enhancement

Add 3 fields to `HookResult`:

```rust
pub struct HookResult {
    // Existing fields preserved
    pub blocked: bool,
    pub block_reason: Option<String>,
    pub messages: Vec<String>,
    pub agents_to_invoke: Vec<String>,
    pub action_results: Vec<ActionResult>,
    pub hooks_executed: usize,

    // New fields
    pub updated_input: Option<serde_json::Value>,
    pub additional_contexts: Vec<String>,
    pub prevent_continuation: bool,
}
```

### 2. HookContext Extension

Add tool output fields for post-execution hooks:

```rust
pub struct HookContext {
    // Existing fields preserved
    pub session_id: String,
    pub tool_name: Option<String>,
    pub arguments: Option<String>,
    pub tool_input: Option<String>,
    pub file_path: Option<PathBuf>,
    pub working_dir: Option<PathBuf>,
    pub env: HashMap<String, String>,

    // New fields
    pub tool_output: Option<String>,
    pub tool_error: Option<bool>,
}
```

### 3. New HookEvent Variant

```rust
pub enum HookEvent {
    // Existing variants preserved
    BeforeToolCall, AfterToolCall,
    // ...
    AfterToolCallFailure,  // NEW
}
```

### 4. Command Output Parse Protocol

Line-prefix protocol for shell hook stdout:

```
block: <reason>                  -> blocked=true, block_reason
update_input: <json>             -> updated_input (invalid JSON = warn + ignore)
context: <text>                  -> additional_contexts.push(text)
prevent_continuation             -> prevent_continuation=true
<no prefix>                      -> messages.push(text)
```

Extracted to standalone `parse_command_output(&str, &mut HookResult)` function. Prompt hooks do NOT use prefix parsing (output is always a message).

### 5. ToolPipeline

New file: `src/agent_loop/tool_pipeline.rs`

```rust
pub struct ToolPipeline<'a> {
    hooks: &'a HookExecutor,
    safety: &'a SafetyGuard,
    session_id: String,
    working_dir: Option<PathBuf>,
}

pub struct PipelineOutcome {
    pub outcome: ToolOutcome,
    pub additional_contexts: Vec<String>,
    pub prevent_continuation: bool,
    pub hook_messages: Vec<String>,
}
```

Pipeline stages:
1. Build HookContext from NativeToolCall
2. Pre-hooks (interceptors) — can block or modify input
3. Safety check (SafetyGuard) — uses potentially-modified arguments
4. Execute tool
5. Post-hooks (observers) — fire-and-forget, collect additional_contexts
6. Failure hooks (if error) — observers only

### 6. execute_tool_batch Signature Change

```rust
// Old
pub async fn execute_tool_batch(
    tool_calls: &[NativeToolCall],
    registry: &LoopToolRegistry,
    safety: &SafetyGuard,
    cancel: &CancellationToken,
    callback: &mut dyn LoopCallback,
) -> Vec<ToolOutcome>

// New
pub async fn execute_tool_batch(
    tool_calls: &[NativeToolCall],
    registry: &LoopToolRegistry,
    pipeline: &ToolPipeline<'_>,
    cancel: &CancellationToken,
    callback: &mut dyn LoopCallback,
) -> Vec<PipelineOutcome>
```

Callback (on_tool_start/on_tool_done) stays at batch layer. Safety check moves inside pipeline.

### 7. run_loop Session Lifecycle Hooks

| HookEvent | Location | Constraint |
|-----------|----------|------------|
| SessionStart | Loop entry, before first LLM call | Observers only |
| SessionEnd | Loop exit | Observers only |
| MessageReceived | After user message, before prompt build | Interceptors (can inject additional_contexts) |
| MessageSent | After LLM response sent | Observers only |
| BeforeCompaction | Before context compression | Can inject compaction hints |
| AfterCompaction | After context compression | Observers only |

HookExecutor passed via `LoopDependencies`. additional_contexts from MessageReceived hooks injected via existing `session_guidance` prompt section.

## File Impact

### New (1)
- `src/agent_loop/tool_pipeline.rs`

### Modified (5)
- `src/extension/hooks/mod.rs` — HookResult +3 fields, HookContext +2 fields, AfterToolCallFailure event, parse_command_output fn
- `src/extension/hooks/executor.rs` — Command output parsing uses parse_command_output
- `src/agent_loop/tool_orchestrator.rs` — Signature change, use pipeline.execute, remove execute_one
- `src/agent_loop/mod.rs` — Export tool_pipeline module
- `src/gateway/execution_engine/run_loop.rs` — Build ToolPipeline, consume PipelineOutcome, session hook callsites

### Deleted Code
- `execute_one` function in tool_orchestrator.rs (replaced by ToolPipeline::execute)
- Inline `block:` prefix parsing in executor.rs (replaced by parse_command_output)

### Estimated Size
~350 lines added, ~80 deleted, ~50 modified. Net +220 lines.

## Backward Compatibility

- Empty HookExecutor (hook_count == 0): pipeline stages 2/5/6 skip entirely, zero overhead
- Existing `block:` prefix behavior preserved (parse_command_output is a superset)
- ToolOutcome type unchanged (wrapped inside PipelineOutcome)
- LoopCallback interface unchanged
- SafetyGuard unchanged (consumed by pipeline, not modified)
