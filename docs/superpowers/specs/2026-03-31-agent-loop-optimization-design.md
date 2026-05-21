# Agent Loop Core Optimization Design

**Date:** 2026-03-31
**Scope:** Option A — Core Agent Loop (loop_core.rs)
**Approach:** Option 2 — Extract ToolOrchestrator

## Problem

Aleph's agent loop has three critical performance and reliability gaps compared to state-of-the-art agent architectures:

1. **Sequential tool execution** — All tool calls run one-by-one (`for tc in tool_calls`), even when multiple read-only tools could safely run in parallel.
2. **No cancellation propagation** — `cancel_tx` exists at `ExecutionEngine` level but does not penetrate into `AgentLoop`. The loop runs to completion or limits with no way to interrupt mid-iteration.
3. **No LLM call retry** — A single transient API error (429/529/connection reset) kills the entire agent run.

## Design

### 1. ToolOrchestrator Module

**File:** `src/agent_loop/tool_orchestrator.rs`

Extracts ~120 lines of tool execution logic from `loop_core.rs` into a focused module with added concurrency support.

#### LoopTool Trait Extension

```rust
// tool.rs — add default method
#[async_trait]
pub trait LoopTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> Value;
    async fn execute(&self, input: Value) -> ToolResult;

    /// Whether this tool can safely run concurrently with other concurrent-safe tools.
    /// Default: true (most tools are read-only).
    /// Override to false for tools that mutate state (file writes, shell exec, etc.).
    fn is_concurrent_safe(&self, _input: &Value) -> bool { true }
}
```

Only ~10-15 write tools need to override: `shell_exec`, `file_write`, `agent_manage`, etc.

#### Partitioning Algorithm

Input: ordered list of tool calls from LLM response.

```
[read_file, search, write_file, read_file, shell_exec]
→ Batch 1: [read_file, search]   — concurrent (join_all)
  Batch 2: [write_file]           — exclusive
  Batch 3: [read_file]            — concurrent
  Batch 4: [shell_exec]           — exclusive
```

Rule: consecutive `is_concurrent_safe() == true` tools form a concurrent batch. A `false` tool forms a single-item exclusive batch. Original order preserved across batches.

#### Interface

```rust
pub struct ToolOutcome {
    pub tool_id: String,
    pub tool_name: String,
    pub output_text: String,
    pub is_error: bool,
    pub should_stop: bool,
}

pub struct ToolOrchestrator<'a> {
    registry: &'a LoopToolRegistry,
    safety: &'a SafetyGuard,
    cancel_token: CancellationToken,
    callback: &'a dyn LoopCallback,
}

impl<'a> ToolOrchestrator<'a> {
    pub async fn execute_batch(&self, tool_calls: &[ToolCall]) -> Vec<ToolOutcome>;
}
```

#### Loop Integration

```rust
// loop_core.rs — replaces ~120 lines
let outcomes = orchestrator.execute_batch(&response.tool_calls).await;
for outcome in outcomes {
    consecutive_errors = if outcome.is_error { consecutive_errors + 1 } else { 0 };
    messages.push(UnifiedMessage::tool_result(
        outcome.tool_id, outcome.tool_name, outcome.output_text, outcome.is_error,
    ));
    if outcome.should_stop { stop_requested = true; }
}
```

### 2. CancellationToken Propagation

**Dependency:** `tokio_util::sync::CancellationToken`

#### Hierarchy

```
ExecutionEngine (root token, bridged from cancel_tx)
  └─ AgentLoop (child token)
       ├─ LLM streaming (select! on child token)
       ├─ ToolOrchestrator (same child token)
       │    ├─ concurrent batch: each tool gets grandchild token
       │    └─ exclusive batch: uses child token directly
       └─ SubagentTool (future: child-of-child token)
```

#### AgentLoop Changes

```rust
pub struct AgentLoop {
    // ... existing fields ...
    cancel_token: CancellationToken,
}
```

#### LLM Streaming — select! wrapping

```rust
loop {
    tokio::select! {
        maybe_delta = delta_stream.next() => {
            match maybe_delta {
                Some(Ok(delta)) => {
                    self.delta_sink.on_delta(&delta).await;
                    collector.push(delta);
                }
                Some(Err(e)) => return Err(e.into()),
                None => break,
            }
        }
        _ = self.cancel_token.cancelled() => {
            return Ok(LoopRunResult { cancelled: true, .. });
        }
    }
}
```

#### Tool Execution — select! in orchestrator

```rust
tokio::select! {
    result = registry.execute(&name, input) => result,
    _ = cancel_token.cancelled() => {
        ToolResult::Error { error: "Cancelled".into(), retryable: false }
    }
}
```

#### ExecutionEngine Bridge

```rust
// engine.rs — bridge existing mpsc cancel to token
let cancel_token = CancellationToken::new();
let bridge_token = cancel_token.clone();
tokio::spawn(async move {
    cancel_rx.recv().await;
    bridge_token.cancel();
});
```

#### LoopRunResult Extension

```rust
pub struct LoopRunResult {
    pub text: Option<String>,
    pub hit_limit: bool,
    pub cancelled: bool, // NEW
    // ...
}
```

### 3. LLM Call Retry

**File:** `src/agent_loop/retry.rs`

#### Error Classification

```rust
pub enum RetryVerdict {
    Retry { delay: Duration },
    Fatal,
}

pub fn classify_error(err: &LoopError) -> RetryVerdict;
```

Classification:
- **Rate limited (429)** → Retry with exponential backoff
- **Overloaded (529)** → Retry with exponential backoff
- **Connection error (reset/timeout)** → Retry with 500ms delay
- **Everything else** → Fatal

#### Backoff

- Base: 500ms, factor: 2x, cap: 10s
- Max retries: 3
- Cancellation-aware: `select!` on sleep + cancel_token

#### Integration

```rust
// loop_core.rs — wraps provider.stream()
let delta_stream = retry::stream_with_retry(
    || self.provider.stream(&messages, &system_prompt, &tool_defs),
    &self.cancel_token,
    3,
).await?;
```

#### LoopError Extension

```rust
impl LoopError {
    pub fn is_rate_limited(&self) -> bool;
    pub fn is_overloaded(&self) -> bool;
    pub fn is_connection_error(&self) -> bool;
}
```

## Files Changed

| File | Action | Description |
|------|--------|-------------|
| `src/agent_loop/tool_orchestrator.rs` | NEW | Partition + concurrent execution + safety checks |
| `src/agent_loop/retry.rs` | NEW | LLM retry + error classification + backoff |
| `src/agent_loop/loop_core.rs` | REFACTOR | Extract tool logic, add cancel_token + select!, add retry |
| `src/agent_loop/tool.rs` | EXTEND | `is_concurrent_safe()` default method on LoopTool |
| `src/agent_loop/mod.rs` | EXTEND | Export new modules |
| `src/gateway/execution_engine/engine.rs` | BRIDGE | cancel_tx → CancellationToken bridge |
| ~10-15 write tool impls | ANNOTATE | Override `is_concurrent_safe() → false` |

## Not In Scope

- Model fallback (requires multi-model LoopProvider)
- Streaming tool execution (start tools while LLM still streaming)
- SubagentTool upgrade (context sharing, streaming back)
- Hook system (pre/post tool hooks)
- ToolResult enrichment (newMessages, contextModifier)
- Multi-layer context compression pipeline

## Expected Impact

- **Concurrent tools:** 3 read-only tools: latency drops from t1+t2+t3 to max(t1,t2,t3)
- **Cancel responsiveness:** From "wait for iteration to complete" to "millisecond-level interrupt"
- **Transient error recovery:** 429/529/connection errors auto-retry, user unaware
- **Code quality:** loop_core.rs shrinks ~120 lines; new modules independently testable
