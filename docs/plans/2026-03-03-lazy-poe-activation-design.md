# Lazy POE Activation Design

## Goal

Integrate POE (Principle-Operation-Evaluation) validation into the normal channel message execution flow, so that complex tasks (tool-using) get structured validation, retry, and anti-hallucination checking — without adding overhead to simple conversations.

## Problem

Currently, channel messages (Telegram, Discord, iMessage) execute through `AgentLoop` without any POE involvement:
- No success criteria defined
- No validation of tool usage
- No retry on poor results
- Agent can hallucinate (claim "PDF ready" without calling `pdf_generate`)

The full POE system exists (`core/src/poe/`, 59 files) but is only accessible via explicit RPC calls (`poe.run`). Normal channel messages completely bypass it.

## Architecture: Lazy POE Activation

### Key Insight

Not all messages need POE. "你好" doesn't need validation. But "帮我写一份比特币交易报告" does. The distinction is simple: **if the agent decides to use tools, it needs POE validation**.

### Activation Trigger

POE activates lazily — only when the agent's first `Decision::UseTool` occurs:

```
Message → AgentLoop.run()
  Think → Decision::Complete("你好！")  → Return directly. Zero POE overhead.
  Think → Decision::UseTool("search")  → ACTIVATE POE
           ↓
    LazyPoeEvaluator.activate(original_query, tool_name)
    → Create LightManifest
    → Enable step tracking
    → Enable completion validation
```

### LightManifest (Rule-Based, No LLM)

Unlike full `SuccessManifest` which requires LLM generation, `LightManifest` is constructed from rules:

```rust
pub struct LightManifest {
    /// Original user request text
    original_query: String,
    /// Tools that have been invoked during execution
    tools_invoked: Vec<ToolInvocation>,
    /// Whether tool results contained actual data
    tool_results_valid: Vec<bool>,
    /// Maximum retry attempts (default: 2)
    max_retries: u8,
    /// Current retry count
    retry_count: u8,
    /// Whether POE mode is active
    active: bool,
}

pub struct ToolInvocation {
    pub tool_name: String,
    pub had_result: bool,
    pub result_non_empty: bool,
}
```

## Validation Rules

### Per-Step Validation (on_step_evaluate)

After each tool execution:

| Rule | Check | Action on Failure |
|------|-------|-------------------|
| ToolResultNonEmpty | Tool returned non-empty, non-error result | ContinueWithHint: "Tool returned empty, try different parameters" |
| ToolExecutionSuccess | Tool didn't error out | ContinueWithHint: "Tool execution failed, consider alternative approach" |

### Completion Validation (on_complete)

When agent issues `Decision::Complete`:

| Rule | Check | Action on Failure |
|------|-------|-------------------|
| ToolActuallyUsed | If POE is active, at least one tool must have been called | Retry with hint: "You claimed results but didn't use any tools" |
| NoHallucination | Agent's claims match tool invocation record | Retry with hint: "Response references data not obtained via tools" |
| QueryRelevance | Final response addresses original query (keyword overlap) | Retry with hint: "Response doesn't address the user's question" |

### Hallucination Detection Heuristics (No LLM needed)

```
Agent says "PDF已生成" BUT pdf_generate not in tools_invoked → HALLUCINATION
Agent gives specific data/numbers BUT no search tool called → HALLUCINATION
Agent references URLs BUT no web_search/browse called → SUSPICIOUS
```

## Retry Mechanism

On completion validation failure:

1. Increment `retry_count`
2. If `retry_count <= max_retries`:
   - Inject failure reason into `state.poe_hint`
   - Re-enter agent loop (don't emit Complete yet)
   - Agent sees hint in next Think and adjusts behavior
3. If `retry_count > max_retries`:
   - Accept current result (best effort)
   - Log warning for observability

## Code Changes

### Modified Files

#### 1. `poe/lazy_evaluator.rs` (NEW)

Core lazy POE evaluator with LightManifest, validation rules, and retry logic.

#### 2. `gateway/loop_callback_adapter.rs`

Add `LazyPoeEvaluator` to `EventEmittingCallback`:

```rust
pub struct EventEmittingCallback {
    // ... existing fields ...
    /// Lazy POE evaluator (activates on first UseTool)
    lazy_poe: LazyPoeEvaluator,
}
```

Modify `on_step_evaluate()`:
```rust
async fn on_step_evaluate(&self, step: &LoopStep, state: &LoopState) -> StepDirective {
    self.lazy_poe.evaluate_step(step, state).await
}
```

#### 3. `agent_loop/agent_loop.rs`

Add activation hook when `Decision::UseTool` is detected:
```rust
Decision::UseTool { tool_name, .. } => {
    callback.on_tool_decided(&tool_name, &state.original_input()).await;
    // ... existing tool execution ...
}
```

#### 4. `gateway/execution_engine/engine.rs`

Pass original user query to callback construction so LazyPoeEvaluator has context.

### Files NOT Modified

- `InboundRouter` — execution path unchanged
- `ExecutionAdapter` trait — interface unchanged
- `Thinker / PromptPipeline` — system prompt unchanged
- `PoeManager` — independent system, not affected
- `SuccessManifest` / `ManifestBuilder` — not used by lazy POE

## Integration with Existing POE

LazyPoeEvaluator is **complementary** to the full POE system:

```
┌─────────────────────────────────────────┐
│ Channel Messages (Telegram, Discord)    │
│   → LazyPoeEvaluator (lightweight)      │  ← THIS DESIGN
│   → Rule-based validation               │
│   → Auto-retry with hints               │
└─────────────────────────────────────────┘

┌─────────────────────────────────────────┐
│ Explicit POE RPC (poe.run, poe.sign)    │
│   → Full PoeManager                     │  ← EXISTING SYSTEM
│   → ManifestBuilder + SuccessManifest   │
│   → CompositeValidator (hard + semantic)│
│   → Budget tracking + crystallization   │
└─────────────────────────────────────────┘
```

Both use the same `StepDirective` mechanism and `on_step_evaluate` callback hook.

## Future Evolution

Phase 2 (optional): If `LazyPoeEvaluator` detects a truly complex task (multi-tool, multi-step), it can escalate to full PoeManager mid-execution:

```
LazyPoeEvaluator detects complexity > threshold
  → Generate SuccessManifest via ManifestBuilder
  → Hand off to PoeManager for remaining execution
```

This keeps the door open for full POE integration without requiring it now.

## Success Criteria

1. Simple messages ("你好") have ZERO additional overhead
2. Tool-using tasks get validated (no hallucinated results)
3. Failed validations trigger automatic retry with feedback (max 2)
4. Agent tool usage is tracked and verified against claims
5. No breaking changes to existing execution flow
