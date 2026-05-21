# Agent Loop Core Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add concurrent tool execution, cancellation propagation, and LLM retry to the agent loop.

**Architecture:** Extract tool execution into `ToolOrchestrator` (partition + parallel), thread `CancellationToken` through the loop, wrap LLM calls with retry logic. All changes contained within `src/agent_loop/`.

**Tech Stack:** Rust, tokio, tokio-util (CancellationToken), futures (join_all)

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `Cargo.toml` | Modify | Add `sync` feature to tokio-util |
| `src/agent_loop/tool.rs` | Modify | Add `is_concurrent_safe()` default method to `LoopTool` |
| `src/agent_loop/retry.rs` | Create | LLM retry with error classification + exponential backoff |
| `src/agent_loop/tool_orchestrator.rs` | Create | Partition tool calls + concurrent execution + safety checks |
| `src/agent_loop/loop_core.rs` | Modify | Integrate orchestrator, cancel token, retry; remove inlined tool execution |
| `src/agent_loop/mod.rs` | Modify | Export new modules |
| `src/agent_loop/adapters/registry_adapter.rs` | Modify | Override `is_concurrent_safe()` for exclusive tools |
| `src/gateway/execution_engine/engine.rs` | Modify | Bridge cancel_tx to CancellationToken |

---

### Task 1: Add tokio-util sync feature

**Files:**
- Modify: `Cargo.toml:30`

- [ ] **Step 1: Add sync feature to tokio-util**

In `Cargo.toml`, change:

```toml
tokio-util = { version = "0.7", features = ["rt", "compat"] }
```

to:

```toml
tokio-util = { version = "0.7", features = ["rt", "compat", "sync"] }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: `Finished` with no errors

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "chore: add sync feature to tokio-util for CancellationToken"
```

---

### Task 2: Extend LoopTool trait with `is_concurrent_safe()`

**Files:**
- Modify: `src/agent_loop/tool.rs:46-58`
- Test: `src/agent_loop/tool.rs` (existing test module)

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` at the bottom of `tool.rs`:

```rust
#[test]
fn test_default_concurrent_safe() {
    // EchoTool doesn't override is_concurrent_safe — should default to true
    let tool = EchoTool;
    assert!(tool.is_concurrent_safe(&json!({})));
}

/// A tool that declares itself exclusive (not concurrent-safe).
struct ExclusiveTool;

#[async_trait]
impl LoopTool for ExclusiveTool {
    fn name(&self) -> &str { "exclusive" }
    fn description(&self) -> &str { "Exclusive tool" }
    fn schema(&self) -> Value { json!({"type": "object", "properties": {}}) }
    async fn execute(&self, _input: Value) -> ToolResult {
        ToolResult::Success { output: json!("done") }
    }
    fn is_concurrent_safe(&self, _input: &Value) -> bool { false }
}

#[test]
fn test_exclusive_tool_not_concurrent_safe() {
    let tool = ExclusiveTool;
    assert!(!tool.is_concurrent_safe(&json!({})));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib tool::tests::test_default_concurrent_safe 2>&1 | tail -5`
Expected: FAIL — `is_concurrent_safe` method not found

- [ ] **Step 3: Add the default method to LoopTool trait**

In `tool.rs`, add the method to the `LoopTool` trait (after `execute`):

```rust
#[async_trait]
pub trait LoopTool: Send + Sync {
    /// Unique tool name used in function calls.
    fn name(&self) -> &str;

    /// Human-readable description for LLM.
    fn description(&self) -> &str;

    /// JSON Schema for input parameters.
    fn schema(&self) -> Value;

    /// Execute the tool with the given input.
    async fn execute(&self, input: Value) -> ToolResult;

    /// Whether this tool can safely run concurrently with other concurrent-safe tools.
    ///
    /// Returns `true` by default. Override to `false` for tools that mutate shared
    /// state (file writes, shell execution, agent management, etc.).
    fn is_concurrent_safe(&self, _input: &Value) -> bool {
        true
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib tool::tests 2>&1 | tail -10`
Expected: all tests PASS including the two new ones

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/tool.rs
git commit -m "feat(agent_loop): add is_concurrent_safe() default method to LoopTool trait"
```

---

### Task 3: Annotate exclusive tools in adapters

**Files:**
- Modify: `src/agent_loop/adapters/registry_adapter.rs:31-43`

- [ ] **Step 1: Write the failing test**

Add to the existing test module in `registry_adapter.rs`:

```rust
#[test]
fn test_exclusive_tools_not_concurrent_safe() {
    use serde_json::json;

    // These tools should NOT be concurrent-safe
    let exclusive_names = ["bash", "code_exec", "file_ops", "self_manage",
                           "secret_vault", "cron_manage", "agent_create",
                           "agent_delete", "team_create", "team_delegate"];

    for name in &exclusive_names {
        assert!(
            super::EXCLUSIVE_TOOLS.contains(name),
            "{} should be in EXCLUSIVE_TOOLS",
            name
        );
    }
}

#[test]
fn test_readonly_tools_are_concurrent_safe() {
    // These tools SHOULD be concurrent-safe (not in exclusive list)
    let safe_names = ["web_fetch", "search", "web_search", "read_skill",
                      "memory_query", "memory_search"];

    for name in &safe_names {
        assert!(
            !super::EXCLUSIVE_TOOLS.contains(name),
            "{} should NOT be in EXCLUSIVE_TOOLS",
            name
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib registry_adapter::tests::test_exclusive 2>&1 | tail -5`
Expected: FAIL — `EXCLUSIVE_TOOLS` not found

- [ ] **Step 3: Add EXCLUSIVE_TOOLS constant and override in RegistryToolAdapter**

At the top of `registry_adapter.rs` (after imports), add:

```rust
/// Tools that mutate state and must NOT run concurrently.
///
/// These execute exclusively — no other tools run in parallel with them.
/// All other tools default to concurrent-safe via the LoopTool trait default.
pub(crate) const EXCLUSIVE_TOOLS: &[&str] = &[
    "bash",
    "code_exec",
    "file_ops",
    "self_manage",
    "secret_vault",
    "cron_manage",
    "agent_create",
    "agent_delete",
    "team_create",
    "team_delegate",
    "heartbeat_create",
    "heartbeat_delete",
    "task_create",
    "task_update",
    "channel_manage",
];
```

Then in the `impl<R: ToolRegistry + 'static> LoopTool for RegistryToolAdapter<R>` block, add after the `execute` method:

```rust
fn is_concurrent_safe(&self, _input: &Value) -> bool {
    !EXCLUSIVE_TOOLS.contains(&self.name.as_str())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib registry_adapter::tests 2>&1 | tail -10`
Expected: all PASS

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/adapters/registry_adapter.rs
git commit -m "feat(agent_loop): annotate exclusive tools for concurrency partitioning"
```

---

### Task 4: Create retry.rs module

**Files:**
- Create: `src/agent_loop/retry.rs`

- [ ] **Step 1: Write the test file inline (tests at bottom of retry.rs)**

Create `src/agent_loop/retry.rs` with tests first:

```rust
//! LLM call retry with error classification and exponential backoff.
//!
//! Wraps `LoopProvider::stream()` to transparently retry transient errors
//! (rate limits, overload, connection resets) with cancellation awareness.

use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// How to handle a failed LLM call.
#[derive(Debug, Clone, PartialEq)]
pub enum RetryVerdict {
    /// Retry after the given delay.
    Retry { delay: Duration },
    /// Do not retry — error is permanent.
    Fatal,
}

/// Classify an error to decide whether to retry.
///
/// Inspects the error message string for known transient patterns.
/// This is intentionally simple — provider errors surface as `anyhow::Error`
/// and we pattern-match on the Display string.
pub fn classify_error(err: &anyhow::Error) -> RetryVerdict {
    let msg = err.to_string().to_lowercase();

    if msg.contains("429") || msg.contains("rate limit") {
        return RetryVerdict::Retry {
            delay: Duration::from_millis(500),
        };
    }

    if msg.contains("529") || msg.contains("overloaded") {
        return RetryVerdict::Retry {
            delay: Duration::from_millis(500),
        };
    }

    if msg.contains("connection")
        || msg.contains("reset")
        || msg.contains("timeout")
        || msg.contains("eof")
        || msg.contains("broken pipe")
    {
        return RetryVerdict::Retry {
            delay: Duration::from_millis(300),
        };
    }

    RetryVerdict::Fatal
}

/// Compute exponential backoff delay for a given attempt.
///
/// Formula: base * 2^attempt, capped at max_delay.
pub fn backoff_delay(attempt: usize, base: Duration, max_delay: Duration) -> Duration {
    let factor = 2u32.saturating_pow(attempt as u32);
    let delay = base.saturating_mul(factor);
    delay.min(max_delay)
}

/// Retry a fallible async factory with exponential backoff and cancellation.
///
/// Calls `make_stream` up to `max_retries + 1` times. On transient errors,
/// waits with exponential backoff. The wait is cancellation-aware via `select!`.
///
/// Returns the first successful result or the last fatal/exhausted error.
pub async fn retry_async<F, Fut, T>(
    make_stream: F,
    cancel: &CancellationToken,
    max_retries: usize,
) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let base_delay = Duration::from_millis(500);
    let max_delay = Duration::from_secs(10);
    let mut attempt = 0;

    loop {
        match make_stream().await {
            Ok(value) => return Ok(value),
            Err(e) => {
                let verdict = classify_error(&e);
                match verdict {
                    RetryVerdict::Fatal => return Err(e),
                    RetryVerdict::Retry { delay: hint_delay } if attempt < max_retries => {
                        let delay = backoff_delay(attempt, hint_delay.max(base_delay), max_delay);
                        tracing::warn!(
                            attempt = attempt + 1,
                            max_retries,
                            delay_ms = delay.as_millis() as u64,
                            error = %e,
                            "LLM call failed, retrying"
                        );
                        attempt += 1;
                        tokio::select! {
                            _ = tokio::time::sleep(delay) => continue,
                            _ = cancel.cancelled() => {
                                return Err(anyhow::anyhow!("Cancelled during retry backoff"));
                            }
                        }
                    }
                    _ => return Err(e),
                }
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_classify_rate_limit() {
        let err = anyhow::anyhow!("HTTP 429: rate limit exceeded");
        assert!(matches!(classify_error(&err), RetryVerdict::Retry { .. }));
    }

    #[test]
    fn test_classify_overloaded() {
        let err = anyhow::anyhow!("HTTP 529: API overloaded");
        assert!(matches!(classify_error(&err), RetryVerdict::Retry { .. }));
    }

    #[test]
    fn test_classify_connection_error() {
        let err = anyhow::anyhow!("connection reset by peer");
        assert!(matches!(classify_error(&err), RetryVerdict::Retry { .. }));
    }

    #[test]
    fn test_classify_fatal() {
        let err = anyhow::anyhow!("HTTP 401: invalid API key");
        assert_eq!(classify_error(&err), RetryVerdict::Fatal);
    }

    #[test]
    fn test_classify_unknown_fatal() {
        let err = anyhow::anyhow!("something completely unexpected");
        assert_eq!(classify_error(&err), RetryVerdict::Fatal);
    }

    #[test]
    fn test_backoff_delay() {
        let base = Duration::from_millis(500);
        let max = Duration::from_secs(10);

        assert_eq!(backoff_delay(0, base, max), Duration::from_millis(500));
        assert_eq!(backoff_delay(1, base, max), Duration::from_millis(1000));
        assert_eq!(backoff_delay(2, base, max), Duration::from_millis(2000));
        assert_eq!(backoff_delay(3, base, max), Duration::from_millis(4000));
        assert_eq!(backoff_delay(4, base, max), Duration::from_millis(8000));
        // Capped at max
        assert_eq!(backoff_delay(5, base, max), Duration::from_secs(10));
        assert_eq!(backoff_delay(10, base, max), Duration::from_secs(10));
    }

    #[tokio::test]
    async fn test_retry_succeeds_first_try() {
        let token = CancellationToken::new();
        let result = retry_async(|| async { Ok::<_, anyhow::Error>(42) }, &token, 3).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_transient_failure() {
        let token = CancellationToken::new();
        let attempt = AtomicUsize::new(0);

        let result = retry_async(
            || {
                let n = attempt.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n < 2 {
                        Err(anyhow::anyhow!("HTTP 429: rate limit"))
                    } else {
                        Ok(42)
                    }
                }
            },
            &token,
            3,
        )
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempt.load(Ordering::SeqCst), 3); // 2 failures + 1 success
    }

    #[tokio::test]
    async fn test_retry_fatal_no_retry() {
        let token = CancellationToken::new();
        let attempt = AtomicUsize::new(0);

        let result = retry_async(
            || {
                attempt.fetch_add(1, Ordering::SeqCst);
                async { Err::<i32, _>(anyhow::anyhow!("HTTP 401: invalid key")) }
            },
            &token,
            3,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(attempt.load(Ordering::SeqCst), 1); // No retries for fatal
    }

    #[tokio::test]
    async fn test_retry_cancelled_during_backoff() {
        let token = CancellationToken::new();
        let attempt = AtomicUsize::new(0);

        // Cancel after a short delay
        let cancel_token = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_token.cancel();
        });

        let result = retry_async(
            || {
                attempt.fetch_add(1, Ordering::SeqCst);
                async { Err::<i32, _>(anyhow::anyhow!("HTTP 529: overloaded")) }
            },
            &token,
            10, // many retries allowed
        )
        .await;

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Cancelled"), "Expected cancellation error, got: {}", msg);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let token = CancellationToken::new();
        let attempt = AtomicUsize::new(0);

        let result = retry_async(
            || {
                attempt.fetch_add(1, Ordering::SeqCst);
                async { Err::<i32, _>(anyhow::anyhow!("HTTP 429: rate limit")) }
            },
            &token,
            2, // max 2 retries
        )
        .await;

        assert!(result.is_err());
        assert_eq!(attempt.load(Ordering::SeqCst), 3); // 1 initial + 2 retries
    }
}
```

- [ ] **Step 2: Add module declaration to mod.rs**

In `src/agent_loop/mod.rs`, add:

```rust
pub mod retry;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib agent_loop::retry::tests 2>&1 | tail -15`
Expected: all 9 tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/agent_loop/retry.rs src/agent_loop/mod.rs
git commit -m "feat(agent_loop): add retry module with error classification and backoff"
```

---

### Task 5: Create tool_orchestrator.rs module

**Files:**
- Create: `src/agent_loop/tool_orchestrator.rs`

- [ ] **Step 1: Create the module with implementation and tests**

Create `src/agent_loop/tool_orchestrator.rs`:

```rust
//! Tool orchestration — partitions tool calls by concurrency safety,
//! executes concurrent batches in parallel and exclusive batches serially.
//!
//! Extracted from `loop_core.rs` to keep the main loop lean and enable
//! independent testing of the execution strategy.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::safety::{SafetyGuard, SafetyError, ToolCall as SafetyToolCall};
use super::tool::{LoopTool, LoopToolRegistry, ToolResult};
use super::loop_core::LoopCallback;
use crate::providers::adapter::NativeToolCall;

// =============================================================================
// ToolOutcome
// =============================================================================

/// Result of executing a single tool call through the orchestrator.
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub tool_id: String,
    pub tool_name: String,
    pub output_text: String,
    pub is_error: bool,
    pub should_stop: bool,
}

// =============================================================================
// Partitioning
// =============================================================================

/// A batch of tool calls that share the same concurrency mode.
#[derive(Debug)]
enum Batch {
    /// All tools in this batch can run concurrently.
    Concurrent(Vec<usize>),
    /// This tool must run exclusively (index into the original slice).
    Exclusive(usize),
}

/// Partition tool calls into concurrent and exclusive batches.
///
/// Consecutive concurrent-safe tools are grouped together.
/// Each exclusive tool forms its own single-item batch.
/// Original ordering is preserved across batches.
fn partition_tool_calls(
    tool_calls: &[NativeToolCall],
    registry: &LoopToolRegistry,
) -> Vec<Batch> {
    let mut batches: Vec<Batch> = Vec::new();
    let mut concurrent_group: Vec<usize> = Vec::new();

    for (i, tc) in tool_calls.iter().enumerate() {
        let is_safe = registry
            .get(&tc.name)
            .map(|tool| tool.is_concurrent_safe(&tc.arguments))
            .unwrap_or(false); // Unknown tools are exclusive for safety

        if is_safe {
            concurrent_group.push(i);
        } else {
            // Flush any accumulated concurrent group
            if !concurrent_group.is_empty() {
                batches.push(Batch::Concurrent(std::mem::take(&mut concurrent_group)));
            }
            batches.push(Batch::Exclusive(i));
        }
    }

    // Flush remaining concurrent group
    if !concurrent_group.is_empty() {
        batches.push(Batch::Concurrent(concurrent_group));
    }

    batches
}

// =============================================================================
// Single tool execution
// =============================================================================

/// Execute a single tool call with safety check, compression, and cancellation.
async fn execute_one(
    tc: &NativeToolCall,
    registry: &LoopToolRegistry,
    safety: &SafetyGuard,
    cancel: &CancellationToken,
) -> ToolOutcome {
    // Safety check
    let safety_call = SafetyToolCall {
        name: tc.name.clone(),
        input: tc.arguments.clone(),
    };

    if let Err(safety_err) = safety.check(&safety_call) {
        let output_text = match &safety_err {
            SafetyError::Blocked { tool, pattern } => {
                format!("BLOCKED: tool '{}' blocked by safety pattern '{}'", tool, pattern)
            }
            SafetyError::NeedsConfirmation { tool } => {
                format!("NEEDS_CONFIRMATION: tool '{}' requires user approval (auto-denied for now)", tool)
            }
            SafetyError::PolicyDenied { tool } => {
                format!("DENIED: tool '{}' is not allowed by permission policy", tool)
            }
        };
        return ToolOutcome {
            tool_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            output_text,
            is_error: true,
            should_stop: false,
        };
    }

    // Execute with cancellation
    let result = tokio::select! {
        r = registry.execute(&tc.name, tc.arguments.clone()) => r,
        _ = cancel.cancelled() => {
            ToolResult::Error {
                error: "Cancelled".into(),
                retryable: false,
            }
        }
    };

    // Process result
    let (output_text, is_error, should_stop) = match &result {
        ToolResult::Success { output } | ToolResult::SuccessAndStopLoop { output } => {
            let stop = matches!(&result, ToolResult::SuccessAndStopLoop { .. });
            let text = serde_json::to_string(output).unwrap_or_else(|e| {
                format!("[serialization error: {}]", e)
            });
            (text, false, stop)
        }
        ToolResult::Error { error, .. } => {
            (error.clone(), true, false)
        }
    };

    // Compress verbose tool outputs
    let output_text = if !is_error {
        crate::tool_output::compressor::compress_tool_output(&tc.name, &output_text)
    } else {
        output_text
    };

    ToolOutcome {
        tool_id: tc.id.clone(),
        tool_name: tc.name.clone(),
        output_text,
        is_error,
        should_stop,
    }
}

// =============================================================================
// Public API
// =============================================================================

/// Execute a batch of tool calls with concurrency partitioning.
///
/// Tools are partitioned into concurrent and exclusive batches based on
/// `LoopTool::is_concurrent_safe()`. Concurrent batches run in parallel
/// via `futures::future::join_all`. Exclusive batches run one at a time.
///
/// Results are returned in the original tool call order.
pub async fn execute_tool_batch(
    tool_calls: &[NativeToolCall],
    registry: &LoopToolRegistry,
    safety: &SafetyGuard,
    cancel: &CancellationToken,
    callback: &mut dyn LoopCallback,
) -> Vec<ToolOutcome> {
    if tool_calls.is_empty() {
        return Vec::new();
    }

    let batches = partition_tool_calls(tool_calls, registry);
    let mut results: Vec<(usize, ToolOutcome)> = Vec::with_capacity(tool_calls.len());

    for batch in batches {
        // Early exit on cancellation between batches
        if cancel.is_cancelled() {
            break;
        }

        match batch {
            Batch::Concurrent(indices) => {
                // Fire callbacks for all tools in the batch
                for &i in &indices {
                    callback.on_tool_start(&tool_calls[i].name, &tool_calls[i].arguments);
                }

                // Execute all concurrently
                let futures: Vec<_> = indices
                    .iter()
                    .map(|&i| execute_one(&tool_calls[i], registry, safety, cancel))
                    .collect();

                let outcomes = futures::future::join_all(futures).await;

                for (&i, outcome) in indices.iter().zip(outcomes) {
                    tracing::info!(
                        tool = %outcome.tool_name,
                        is_error = outcome.is_error,
                        "Tool execution complete (concurrent)"
                    );
                    // Find the ToolResult variant for callback
                    let tool_result = if outcome.is_error {
                        ToolResult::Error { error: outcome.output_text.clone(), retryable: false }
                    } else if outcome.should_stop {
                        ToolResult::SuccessAndStopLoop {
                            output: serde_json::Value::String(outcome.output_text.clone()),
                        }
                    } else {
                        ToolResult::Success {
                            output: serde_json::Value::String(outcome.output_text.clone()),
                        }
                    };
                    callback.on_tool_done(&outcome.tool_name, &tool_result);
                    results.push((i, outcome));
                }
            }
            Batch::Exclusive(i) => {
                callback.on_tool_start(&tool_calls[i].name, &tool_calls[i].arguments);

                let outcome = execute_one(&tool_calls[i], registry, safety, cancel).await;

                tracing::info!(
                    tool = %outcome.tool_name,
                    is_error = outcome.is_error,
                    "Tool execution complete (exclusive)"
                );
                let tool_result = if outcome.is_error {
                    ToolResult::Error { error: outcome.output_text.clone(), retryable: false }
                } else if outcome.should_stop {
                    ToolResult::SuccessAndStopLoop {
                        output: serde_json::Value::String(outcome.output_text.clone()),
                    }
                } else {
                    ToolResult::Success {
                        output: serde_json::Value::String(outcome.output_text.clone()),
                    }
                };
                callback.on_tool_done(&outcome.tool_name, &tool_result);
                results.push((i, outcome));
            }
        }
    }

    // Sort by original index to preserve tool call order
    results.sort_by_key(|(i, _)| *i);
    results.into_iter().map(|(_, outcome)| outcome).collect()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::tool::LoopTool;
    use crate::agent_loop::loop_core::LoopCallback;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    // --- Test tools ---

    struct ReadTool {
        delay_ms: u64,
        call_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LoopTool for ReadTool {
        fn name(&self) -> &str { "read_tool" }
        fn description(&self) -> &str { "Read-only" }
        fn schema(&self) -> Value { json!({"type": "object", "properties": {}}) }
        async fn execute(&self, _input: Value) -> ToolResult {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            ToolResult::Success { output: json!("read_done") }
        }
        // Default: is_concurrent_safe = true
    }

    struct WriteTool {
        call_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LoopTool for WriteTool {
        fn name(&self) -> &str { "write_tool" }
        fn description(&self) -> &str { "Write-only" }
        fn schema(&self) -> Value { json!({"type": "object", "properties": {}}) }
        async fn execute(&self, _input: Value) -> ToolResult {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            ToolResult::Success { output: json!("write_done") }
        }
        fn is_concurrent_safe(&self, _input: &Value) -> bool { false }
    }

    struct NoopCb;
    impl LoopCallback for NoopCb {}

    fn make_tc(id: &str, name: &str) -> NativeToolCall {
        NativeToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: json!({}),
        }
    }

    // --- Partitioning tests ---

    #[test]
    fn test_partition_all_concurrent() {
        let mut reg = LoopToolRegistry::new();
        let count = Arc::new(AtomicUsize::new(0));
        reg.register(Box::new(ReadTool { delay_ms: 0, call_count: count }));

        let calls = vec![
            make_tc("1", "read_tool"),
            make_tc("2", "read_tool"),
            make_tc("3", "read_tool"),
        ];

        let batches = partition_tool_calls(&calls, &reg);
        assert_eq!(batches.len(), 1);
        assert!(matches!(&batches[0], Batch::Concurrent(v) if v.len() == 3));
    }

    #[test]
    fn test_partition_mixed() {
        let mut reg = LoopToolRegistry::new();
        let count = Arc::new(AtomicUsize::new(0));
        reg.register(Box::new(ReadTool { delay_ms: 0, call_count: count.clone() }));
        reg.register(Box::new(WriteTool { call_count: count }));

        let calls = vec![
            make_tc("1", "read_tool"),
            make_tc("2", "read_tool"),
            make_tc("3", "write_tool"),
            make_tc("4", "read_tool"),
        ];

        let batches = partition_tool_calls(&calls, &reg);
        assert_eq!(batches.len(), 3);
        assert!(matches!(&batches[0], Batch::Concurrent(v) if v.len() == 2));
        assert!(matches!(&batches[1], Batch::Exclusive(2)));
        assert!(matches!(&batches[2], Batch::Concurrent(v) if v.len() == 1));
    }

    #[test]
    fn test_partition_unknown_tool_is_exclusive() {
        let reg = LoopToolRegistry::new(); // empty
        let calls = vec![make_tc("1", "unknown")];

        let batches = partition_tool_calls(&calls, &reg);
        assert_eq!(batches.len(), 1);
        assert!(matches!(&batches[0], Batch::Exclusive(0)));
    }

    // --- Execution tests ---

    #[tokio::test]
    async fn test_concurrent_batch_runs_in_parallel() {
        let mut reg = LoopToolRegistry::new();
        let count = Arc::new(AtomicUsize::new(0));
        reg.register(Box::new(ReadTool { delay_ms: 100, call_count: count.clone() }));

        let calls = vec![
            make_tc("1", "read_tool"),
            make_tc("2", "read_tool"),
            make_tc("3", "read_tool"),
        ];

        let safety = SafetyGuard::permissive();
        let token = CancellationToken::new();
        let mut cb = NoopCb;

        let start = Instant::now();
        let outcomes = execute_tool_batch(&calls, &reg, &safety, &token, &mut cb).await;
        let elapsed = start.elapsed();

        assert_eq!(outcomes.len(), 3);
        assert!(outcomes.iter().all(|o| !o.is_error));
        assert_eq!(count.load(Ordering::SeqCst), 3);

        // If truly parallel, 3 x 100ms tools should complete in ~100ms, not ~300ms
        assert!(elapsed.as_millis() < 200, "Expected parallel execution, took {}ms", elapsed.as_millis());
    }

    #[tokio::test]
    async fn test_results_preserve_original_order() {
        let mut reg = LoopToolRegistry::new();
        let count = Arc::new(AtomicUsize::new(0));
        reg.register(Box::new(ReadTool { delay_ms: 0, call_count: count.clone() }));
        reg.register(Box::new(WriteTool { call_count: count }));

        let calls = vec![
            make_tc("a", "read_tool"),
            make_tc("b", "write_tool"),
            make_tc("c", "read_tool"),
        ];

        let safety = SafetyGuard::permissive();
        let token = CancellationToken::new();
        let mut cb = NoopCb;

        let outcomes = execute_tool_batch(&calls, &reg, &safety, &token, &mut cb).await;

        assert_eq!(outcomes.len(), 3);
        assert_eq!(outcomes[0].tool_id, "a");
        assert_eq!(outcomes[1].tool_id, "b");
        assert_eq!(outcomes[2].tool_id, "c");
    }

    #[tokio::test]
    async fn test_cancellation_stops_execution() {
        let mut reg = LoopToolRegistry::new();
        let count = Arc::new(AtomicUsize::new(0));
        reg.register(Box::new(ReadTool { delay_ms: 500, call_count: count.clone() }));

        let calls = vec![
            make_tc("1", "read_tool"),
            make_tc("2", "read_tool"),
        ];

        let safety = SafetyGuard::permissive();
        let token = CancellationToken::new();
        let mut cb = NoopCb;

        // Cancel after 50ms
        let cancel_token = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            cancel_token.cancel();
        });

        let outcomes = execute_tool_batch(&calls, &reg, &safety, &token, &mut cb).await;

        // Should have results (cancelled tools return error)
        assert!(outcomes.iter().any(|o| o.is_error && o.output_text.contains("Cancelled")));
    }
}
```

- [ ] **Step 2: Add module declaration to mod.rs**

In `src/agent_loop/mod.rs`, add:

```rust
pub mod tool_orchestrator;
```

- [ ] **Step 3: Check if SafetyGuard::permissive() exists, add if needed**

Check `src/agent_loop/safety.rs` for a `permissive()` constructor. If it doesn't exist, add to `SafetyGuard`:

```rust
/// Create a permissive guard that allows all tool calls (for testing).
#[cfg(test)]
pub fn permissive() -> Self {
    Self {
        blocked_patterns: Vec::new(),
        permissions: HashMap::new(),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib agent_loop::tool_orchestrator::tests 2>&1 | tail -20`
Expected: all 6 tests PASS (partition x3, execution x3)

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/tool_orchestrator.rs src/agent_loop/mod.rs src/agent_loop/safety.rs
git commit -m "feat(agent_loop): add tool_orchestrator with concurrent batch execution"
```

---

### Task 6: Add CancellationToken to AgentLoop and integrate orchestrator + retry

**Files:**
- Modify: `src/agent_loop/loop_core.rs:262-706`

This is the core integration task. It modifies `AgentLoop` to accept a `CancellationToken`, replaces the inline tool execution with `execute_tool_batch`, and wraps the LLM streaming call with `retry_async`.

- [ ] **Step 1: Add CancellationToken field and constructor changes**

In `loop_core.rs`, add the import at the top:

```rust
use tokio_util::sync::CancellationToken;
```

Modify the `AgentLoop` struct to add the cancel token:

```rust
pub struct AgentLoop<P: LoopProvider> {
    provider: P,
    tool_registry: LoopToolRegistry,
    prompt_builder: PromptBuilder,
    safety_guard: SafetyGuard,
    config: LoopConfig,
    tool_compactor_config: Option<ToolCompactorConfig>,
    delta_sink: Box<dyn DeltaSink>,
    cancel_token: CancellationToken,
}
```

Update `new()` to accept and store the token:

```rust
pub fn new(
    provider: P,
    tool_registry: LoopToolRegistry,
    prompt_builder: PromptBuilder,
    safety_guard: SafetyGuard,
    config: LoopConfig,
    cancel_token: CancellationToken,
) -> Self {
    Self {
        provider,
        tool_registry,
        prompt_builder,
        safety_guard,
        config,
        tool_compactor_config: None,
        delta_sink: Box::new(NoopSink),
        cancel_token,
    }
}
```

Add `cancelled` field to `LoopRunResult`:

```rust
pub struct LoopRunResult {
    pub final_text: Option<String>,
    pub iterations: usize,
    pub tool_calls_made: usize,
    pub total_tokens: usize,
    pub hit_limit: bool,
    pub cancelled: bool,
}
```

- [ ] **Step 2: Replace LLM streaming with retry + select!**

In the `run()` method, replace:

```rust
let delta_stream = self
    .provider
    .stream(&messages, &system_prompt, &tool_defs)
    .await?;
let mut collector = DeltaCollector::new();
futures::pin_mut!(delta_stream);
while let Some(delta) = delta_stream.next().await {
    let delta = delta?;
    self.delta_sink.on_delta(&delta).await;
    collector.push(delta);
}
let response = collector.finish();
```

with:

```rust
// Think: stream deltas with retry and cancellation
let delta_stream = super::retry::retry_async(
    || self.provider.stream(&messages, &system_prompt, &tool_defs),
    &self.cancel_token,
    3,
).await?;

let mut collector = DeltaCollector::new();
futures::pin_mut!(delta_stream);
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
            return Ok(LoopRunResult {
                final_text: None,
                iterations,
                tool_calls_made,
                total_tokens,
                hit_limit: false,
                cancelled: true,
            });
        }
    }
}
let response = collector.finish();
```

- [ ] **Step 3: Replace inline tool execution with orchestrator**

Replace the entire `// Act: process each tool call` block (lines 548-671) with:

```rust
// Act: execute tool calls via orchestrator (concurrent where safe)
let outcomes = super::tool_orchestrator::execute_tool_batch(
    &response.tool_calls,
    &self.tool_registry,
    &self.safety_guard,
    &self.cancel_token,
    callback,
).await;

for outcome in &outcomes {
    if outcome.is_error {
        // Check if retryable by looking at the error
        let is_retryable = outcome.output_text.starts_with("BLOCKED:")
            || outcome.output_text.starts_with("NEEDS_CONFIRMATION:")
            || outcome.output_text.starts_with("DENIED:");
        if !is_retryable {
            consecutive_errors += 1;
        }
    } else {
        consecutive_errors = 0;
    }

    messages.push(UnifiedMessage::tool_result(
        outcome.tool_id.clone(),
        outcome.tool_name.clone(),
        outcome.output_text.clone(),
        outcome.is_error,
    ));

    if outcome.should_stop {
        if final_text.is_none() {
            final_text = Some(outcome.output_text.clone());
        }
        stop_requested = true;
    }
}

tool_calls_made += outcomes.len();
```

- [ ] **Step 4: Update the final return to include cancelled**

Update the `Ok(LoopRunResult { ... })` at the end of `run()`:

```rust
Ok(LoopRunResult {
    final_text,
    iterations,
    tool_calls_made,
    total_tokens,
    hit_limit,
    cancelled: false,
})
```

- [ ] **Step 5: Fix all existing tests to pass CancellationToken**

In the test module, update all `AgentLoop::new(...)` calls to include `CancellationToken::new()`:

Add import at top of test module:

```rust
use tokio_util::sync::CancellationToken;
```

Every `AgentLoop::new(provider, registry, prompt_builder, safety, config)` becomes:

```rust
AgentLoop::new(provider, registry, prompt_builder, safety, config, CancellationToken::new())
```

Also update any `LoopRunResult` assertions to account for the new `cancelled` field.

- [ ] **Step 6: Run all agent_loop tests**

Run: `cargo test -p alephcore --lib agent_loop 2>&1 | tail -20`
Expected: all tests PASS

- [ ] **Step 7: Commit**

```bash
git add src/agent_loop/loop_core.rs
git commit -m "feat(agent_loop): integrate orchestrator, cancellation, and retry into main loop"
```

---

### Task 7: Bridge ExecutionEngine cancel_tx to CancellationToken

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs:34-97`
- Modify: `src/gateway/execution_engine/engine.rs:170-214`

- [ ] **Step 1: Update run_agent_loop to create and pass CancellationToken**

In `run_loop.rs`, in the `run_agent_loop` method, add the import:

```rust
use tokio_util::sync::CancellationToken;
```

After the existing setup code (around line 47), create a `CancellationToken` and pass it to `AgentLoop::new`:

Find the `AgentLoop::new(...)` call in this file. Add `cancel_token.clone()` as the last argument.

Create the token at the start of the method:

```rust
let cancel_token = CancellationToken::new();
```

- [ ] **Step 2: Accept CancellationToken as parameter in run_agent_loop**

Change the method signature to accept a `CancellationToken`:

```rust
pub(super) async fn run_agent_loop<E: EventEmitter + Send + Sync + 'static>(
    &self,
    run_id: &str,
    request: &RunRequest,
    agent: Arc<AgentInstance>,
    emitter: Arc<E>,
    deadline: Arc<tokio::sync::Mutex<tokio::time::Instant>>,
    cancel_token: CancellationToken,
) -> Result<String, ExecutionError> {
```

- [ ] **Step 3: Create CancellationToken in engine.rs and bridge from cancel_tx**

In `engine.rs`, where the `execute()` method creates `cancel_tx`:

```rust
let (cancel_tx, mut cancel_rx) = mpsc::channel::<()>(1);
```

Add after it:

```rust
let cancel_token = CancellationToken::new();
let bridge_token = cancel_token.clone();
tokio::spawn(async move {
    if cancel_rx.recv().await.is_some() {
        bridge_token.cancel();
    }
});
```

Then pass `cancel_token` to the `run_agent_loop` call.

- [ ] **Step 4: Handle cancelled result in engine.rs**

Where the result of `run_agent_loop` is processed, check for cancellation in the `LoopRunResult` and map to `RunState::Cancelled` if `result.cancelled`.

- [ ] **Step 5: Run cargo check**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: compiles with no errors

- [ ] **Step 6: Commit**

```bash
git add src/gateway/execution_engine/run_loop.rs src/gateway/execution_engine/engine.rs
git commit -m "feat(gateway): bridge cancel_tx to CancellationToken for agent loop cancellation"
```

---

### Task 8: Update mod.rs exports and final verification

**Files:**
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Ensure mod.rs exports are complete**

Verify `src/agent_loop/mod.rs` contains:

```rust
pub mod adapters;
pub mod factory;
mod loop_core;
pub mod model_behaviors;
mod prompt_builder;
pub mod provider_bridge;
pub mod retry;
mod safety;
pub mod subagent_tool;
mod tool;
pub mod tool_orchestrator;

#[cfg(test)]
mod integration_probe;

pub use factory::LoopFactory;
pub use loop_core::{
    AgentLoop, LoopCallback, LoopConfig, LoopProvider, LoopRunResult, ToolCompactorConfig,
};
pub use prompt_builder::{PromptBuilder, ToolInfo};
pub use provider_bridge::AiProviderBridge;
pub use safety::{SafetyError, SafetyGuard, ToolCall as SafetyToolCall};
pub use tool::{LoopTool, LoopToolRegistry, ToolDefinition, ToolResult};
```

- [ ] **Step 2: Run full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`
Expected: all tests PASS

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore 2>&1 | tail -20`
Expected: no warnings

- [ ] **Step 4: Compile check (release)**

Run: `cargo check -p alephcore --release 2>&1 | tail -5`
Expected: `Finished`

- [ ] **Step 5: Final commit**

```bash
git add src/agent_loop/mod.rs
git commit -m "feat(agent_loop): complete concurrent tool execution + cancellation + retry"
```
