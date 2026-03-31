//! Tool orchestrator — partitions tool calls by concurrency safety and executes them.
//!
//! Concurrent-safe tools run in parallel via `futures::future::join_all`;
//! exclusive tools run one at a time. Original order is always preserved.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::agent_loop::{LoopCallback, LoopToolRegistry, SafetyError, SafetyGuard, ToolResult};
use crate::providers::adapter::NativeToolCall;
use crate::tool_output::compressor::compress_tool_output;

// =============================================================================
// ToolOutcome
// =============================================================================

/// The result of executing a single tool call, ready for the conversation history.
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub tool_id: String,
    pub tool_name: String,
    pub output_text: String,
    pub is_error: bool,
    pub should_stop: bool,
    /// Whether the error is transient and should not count toward consecutive error limits.
    pub retryable: bool,
}

// =============================================================================
// Batch (internal)
// =============================================================================

/// A group of tool calls that share the same execution strategy.
enum Batch {
    /// Multiple concurrent-safe tools — run in parallel.
    Concurrent(Vec<usize>),
    /// A single exclusive tool — run alone.
    Exclusive(usize),
}

// =============================================================================
// partition_tool_calls
// =============================================================================

/// Partition tool calls into concurrent and exclusive batches, preserving order.
fn partition_tool_calls(tool_calls: &[NativeToolCall], registry: &LoopToolRegistry) -> Vec<Batch> {
    let mut batches: Vec<Batch> = Vec::new();

    for (i, tc) in tool_calls.iter().enumerate() {
        let is_safe = registry
            .get(&tc.name)
            .map(|t| t.is_concurrent_safe(&tc.arguments))
            .unwrap_or(false);

        if is_safe {
            // Try to extend the current concurrent batch.
            if let Some(Batch::Concurrent(ref mut indices)) = batches.last_mut() {
                indices.push(i);
            } else {
                batches.push(Batch::Concurrent(vec![i]));
            }
        } else {
            batches.push(Batch::Exclusive(i));
        }
    }

    batches
}

// =============================================================================
// execute_one
// =============================================================================

/// Execute a single tool call with safety check and cancellation support.
async fn execute_one(
    tc: &NativeToolCall,
    registry: &LoopToolRegistry,
    safety: &SafetyGuard,
    cancel: &CancellationToken,
) -> ToolOutcome {
    // Safety check first.
    let safety_call = crate::agent_loop::SafetyToolCall {
        name: tc.name.clone(),
        input: tc.arguments.clone(),
    };
    if let Err(e) = safety.check(&safety_call) {
        let msg = match &e {
            SafetyError::Blocked { tool, pattern } => {
                format!(
                    "[BLOCKED] Tool '{}' blocked by safety pattern '{}'",
                    tool, pattern
                )
            }
            SafetyError::NeedsConfirmation { tool } => {
                format!(
                    "[NEEDS_CONFIRMATION] Tool '{}' requires user confirmation",
                    tool
                )
            }
            SafetyError::PolicyDenied { tool } => {
                format!("[DENIED] Tool '{}' denied by policy", tool)
            }
        };
        return ToolOutcome {
            tool_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            output_text: msg,
            is_error: true,
            should_stop: false,
            retryable: false,
        };
    }

    // Execute with cancellation.
    let result = tokio::select! {
        r = registry.execute(&tc.name, tc.arguments.clone()) => r,
        _ = cancel.cancelled() => {
            return ToolOutcome {
                tool_id: tc.id.clone(),
                tool_name: tc.name.clone(),
                output_text: "[CANCELLED] Tool execution was cancelled".to_string(),
                is_error: true,
                should_stop: false,
                retryable: false,
            };
        }
    };

    // Map ToolResult to ToolOutcome.
    match result {
        ToolResult::Success { output } => {
            let raw = value_to_text(&output);
            let compressed = compress_tool_output(&tc.name, &raw);
            ToolOutcome {
                tool_id: tc.id.clone(),
                tool_name: tc.name.clone(),
                output_text: compressed,
                is_error: false,
                should_stop: false,
                retryable: false,
            }
        }
        ToolResult::Error { error, retryable } => ToolOutcome {
            tool_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            output_text: error,
            is_error: true,
            should_stop: false,
            retryable,
        },
        ToolResult::SuccessAndStopLoop { output } => {
            let raw = value_to_text(&output);
            let compressed = compress_tool_output(&tc.name, &raw);
            ToolOutcome {
                tool_id: tc.id.clone(),
                tool_name: tc.name.clone(),
                output_text: compressed,
                is_error: false,
                should_stop: true,
                retryable: false,
            }
        }
    }
}

/// Convert a JSON Value to a display string.
fn value_to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// =============================================================================
// execute_tool_batch (public)
// =============================================================================

/// Execute a slice of tool calls respecting concurrency safety.
///
/// - Concurrent-safe tools within a batch run in parallel via `join_all`.
/// - Exclusive tools run one at a time.
/// - Cancellation is checked between batches for early exit.
/// - Results are returned in the same order as the input `tool_calls`.
pub async fn execute_tool_batch(
    tool_calls: &[NativeToolCall],
    registry: &LoopToolRegistry,
    safety: &SafetyGuard,
    cancel: &CancellationToken,
    callback: &mut dyn LoopCallback,
) -> Vec<ToolOutcome> {
    let batches = partition_tool_calls(tool_calls, registry);
    let mut results: Vec<(usize, ToolOutcome)> = Vec::with_capacity(tool_calls.len());

    for batch in &batches {
        // Early exit on cancellation between batches.
        if cancel.is_cancelled() {
            break;
        }

        match batch {
            Batch::Concurrent(indices) => {
                // Pre-check safety and fire on_tool_start BEFORE execution begins,
                // so streaming consumers see the "tool started" signal immediately.
                let mut safe_indices: Vec<usize> = Vec::new();
                for &idx in indices {
                    let tc = &tool_calls[idx];
                    let safety_call = crate::agent_loop::SafetyToolCall {
                        name: tc.name.clone(),
                        input: tc.arguments.clone(),
                    };
                    if safety.check(&safety_call).is_ok() {
                        callback.on_tool_start(&tc.name, &tc.arguments);
                        safe_indices.push(idx);
                    } else {
                        // Safety-blocked: produce outcome directly, skip execution.
                        let outcome = execute_one(tc, registry, safety, cancel).await;
                        results.push((idx, outcome));
                    }
                }

                // Execute only safety-cleared tools concurrently.
                if !safe_indices.is_empty() {
                    let futures: Vec<_> = safe_indices
                        .iter()
                        .map(|&idx| {
                            let tc = &tool_calls[idx];
                            execute_one(tc, registry, safety, cancel)
                        })
                        .collect();

                    let outcomes = futures::future::join_all(futures).await;

                    for (i, outcome) in outcomes.into_iter().enumerate() {
                        let idx = safe_indices[i];
                        let tool_result = outcome_to_tool_result(&outcome);
                        callback.on_tool_done(&tool_calls[idx].name, &tool_result);
                        results.push((idx, outcome));
                    }
                }
            }
            Batch::Exclusive(idx) => {
                let idx = *idx;
                let tc = &tool_calls[idx];

                // Pre-check safety to fire on_tool_start before execution.
                let safety_call = crate::agent_loop::SafetyToolCall {
                    name: tc.name.clone(),
                    input: tc.arguments.clone(),
                };
                let is_safe = safety.check(&safety_call).is_ok();
                if is_safe {
                    callback.on_tool_start(&tc.name, &tc.arguments);
                }

                let outcome = execute_one(tc, registry, safety, cancel).await;

                if is_safe {
                    let tool_result = outcome_to_tool_result(&outcome);
                    callback.on_tool_done(&tc.name, &tool_result);
                }
                results.push((idx, outcome));
            }
        }
    }

    // Sort by original index to preserve input order.
    results.sort_by_key(|(idx, _)| *idx);
    results.into_iter().map(|(_, outcome)| outcome).collect()
}

/// Convert a ToolOutcome back to a ToolResult for the callback.
fn outcome_to_tool_result(outcome: &ToolOutcome) -> ToolResult {
    if outcome.is_error {
        ToolResult::Error {
            error: outcome.output_text.clone(),
            retryable: false,
        }
    } else if outcome.should_stop {
        ToolResult::SuccessAndStopLoop {
            output: Value::String(outcome.output_text.clone()),
        }
    } else {
        ToolResult::Success {
            output: Value::String(outcome.output_text.clone()),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::agent_loop::tool::{LoopTool, LoopToolRegistry};
    use crate::agent_loop::NoopCallback;
    use crate::extension::PermissionAction;

    /// A permissive safety guard for tests — allows everything.
    fn permissive_guard() -> SafetyGuard {
        SafetyGuard::new(vec![], HashMap::new(), PermissionAction::Allow)
    }

    /// A concurrent-safe tool that sleeps for a given duration, tracking peak concurrency.
    struct SlowReadTool {
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }

    impl SlowReadTool {
        fn new(active: Arc<AtomicUsize>, peak: Arc<AtomicUsize>) -> Self {
            Self { active, peak }
        }
    }

    #[async_trait]
    impl LoopTool for SlowReadTool {
        fn name(&self) -> &str {
            "slow_read"
        }
        fn description(&self) -> &str {
            "A slow read tool"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, input: Value) -> ToolResult {
            let ms = input.get("ms").and_then(|v| v.as_u64()).unwrap_or(100);
            let prev = self.active.fetch_add(1, Ordering::SeqCst);
            let current = prev + 1;
            // Update peak.
            self.peak.fetch_max(current, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(ms)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            ToolResult::Success {
                output: json!({ "slept_ms": ms }),
            }
        }
        fn is_concurrent_safe(&self, _input: &Value) -> bool {
            true
        }
    }

    /// An exclusive tool (not concurrent-safe).
    struct WriteTool;

    #[async_trait]
    impl LoopTool for WriteTool {
        fn name(&self) -> &str {
            "write"
        }
        fn description(&self) -> &str {
            "An exclusive write tool"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult::Success {
                output: json!("written"),
            }
        }
        fn is_concurrent_safe(&self, _input: &Value) -> bool {
            false
        }
    }

    /// A simple read tool (concurrent-safe, instant).
    struct ReadTool;

    #[async_trait]
    impl LoopTool for ReadTool {
        fn name(&self) -> &str {
            "read"
        }
        fn description(&self) -> &str {
            "A read tool"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult::Success {
                output: json!("read_ok"),
            }
        }
        fn is_concurrent_safe(&self, _input: &Value) -> bool {
            true
        }
    }

    fn make_tc(id: &str, name: &str) -> NativeToolCall {
        NativeToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: json!({}),
        }
    }

    fn make_tc_with_args(id: &str, name: &str, args: Value) -> NativeToolCall {
        NativeToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: args,
        }
    }

    // -------------------------------------------------------------------------
    // Partitioning tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_partition_all_concurrent() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(ReadTool));

        let calls = vec![
            make_tc("1", "read"),
            make_tc("2", "read"),
            make_tc("3", "read"),
        ];

        let batches = partition_tool_calls(&calls, &registry);
        assert_eq!(batches.len(), 1);
        match &batches[0] {
            Batch::Concurrent(indices) => assert_eq!(indices, &[0, 1, 2]),
            Batch::Exclusive(_) => panic!("expected Concurrent batch"),
        }
    }

    #[test]
    fn test_partition_mixed() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(ReadTool));
        registry.register(Box::new(WriteTool));

        // [read, read, write, read] → 3 batches: Concurrent([0,1]), Exclusive(2), Concurrent([3])
        let calls = vec![
            make_tc("1", "read"),
            make_tc("2", "read"),
            make_tc("3", "write"),
            make_tc("4", "read"),
        ];

        let batches = partition_tool_calls(&calls, &registry);
        assert_eq!(batches.len(), 3);

        match &batches[0] {
            Batch::Concurrent(indices) => assert_eq!(indices, &[0, 1]),
            Batch::Exclusive(_) => panic!("expected Concurrent"),
        }
        match &batches[1] {
            Batch::Exclusive(idx) => assert_eq!(*idx, 2),
            Batch::Concurrent(_) => panic!("expected Exclusive"),
        }
        match &batches[2] {
            Batch::Concurrent(indices) => assert_eq!(indices, &[3]),
            Batch::Exclusive(_) => panic!("expected Concurrent"),
        }
    }

    #[test]
    fn test_partition_unknown_tool_is_exclusive() {
        let registry = LoopToolRegistry::new(); // empty

        let calls = vec![
            make_tc("1", "unknown_tool"),
            make_tc("2", "another_unknown"),
        ];

        let batches = partition_tool_calls(&calls, &registry);
        assert_eq!(batches.len(), 2);
        assert!(matches!(&batches[0], Batch::Exclusive(0)));
        assert!(matches!(&batches[1], Batch::Exclusive(1)));
    }

    // -------------------------------------------------------------------------
    // Execution tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_concurrent_batch_runs_in_parallel() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(SlowReadTool::new(active.clone(), peak.clone())));

        let calls = vec![
            make_tc_with_args("1", "slow_read", json!({ "ms": 100 })),
            make_tc_with_args("2", "slow_read", json!({ "ms": 100 })),
            make_tc_with_args("3", "slow_read", json!({ "ms": 100 })),
        ];

        let safety = permissive_guard();
        let cancel = CancellationToken::new();
        let mut callback = NoopCallback;

        let start = Instant::now();
        let outcomes = execute_tool_batch(&calls, &registry, &safety, &cancel, &mut callback).await;
        let elapsed = start.elapsed();

        assert_eq!(outcomes.len(), 3);
        for o in &outcomes {
            assert!(!o.is_error);
        }

        // 3 tools each sleeping 100ms — if parallel, total < 200ms.
        assert!(
            elapsed < Duration::from_millis(250),
            "Expected parallel execution (<250ms), got {:?}",
            elapsed
        );

        // Peak concurrency should be >= 2 (ideally 3).
        assert!(
            peak.load(Ordering::SeqCst) >= 2,
            "Expected peak concurrency >= 2, got {}",
            peak.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn test_results_preserve_original_order() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(SlowReadTool::new(active, peak)));
        registry.register(Box::new(WriteTool));
        registry.register(Box::new(ReadTool));

        // Mixed: [slow_read, write, read, slow_read]
        let calls = vec![
            make_tc_with_args("a", "slow_read", json!({ "ms": 50 })),
            make_tc("b", "write"),
            make_tc("c", "read"),
            make_tc_with_args("d", "slow_read", json!({ "ms": 50 })),
        ];

        let safety = permissive_guard();
        let cancel = CancellationToken::new();
        let mut callback = NoopCallback;

        let outcomes = execute_tool_batch(&calls, &registry, &safety, &cancel, &mut callback).await;

        // Verify order matches input.
        assert_eq!(outcomes.len(), 4);
        assert_eq!(outcomes[0].tool_id, "a");
        assert_eq!(outcomes[0].tool_name, "slow_read");
        assert_eq!(outcomes[1].tool_id, "b");
        assert_eq!(outcomes[1].tool_name, "write");
        assert_eq!(outcomes[2].tool_id, "c");
        assert_eq!(outcomes[2].tool_name, "read");
        assert_eq!(outcomes[3].tool_id, "d");
        assert_eq!(outcomes[3].tool_name, "slow_read");

        // All should succeed.
        for o in &outcomes {
            assert!(!o.is_error, "tool {} should not be an error", o.tool_name);
        }
    }

    #[tokio::test]
    async fn test_cancellation_stops_execution() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(SlowReadTool::new(active, peak)));

        // Two batches: first sleeps 200ms, second sleeps 200ms.
        // Cancel after 50ms — first batch should get cancelled, second should not start.
        let calls = vec![
            make_tc_with_args("1", "slow_read", json!({ "ms": 200 })),
            make_tc_with_args("2", "slow_read", json!({ "ms": 200 })),
        ];

        let safety = permissive_guard();
        let cancel = CancellationToken::new();
        let mut callback = NoopCallback;

        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        let outcomes = execute_tool_batch(&calls, &registry, &safety, &cancel, &mut callback).await;

        // Should have outcomes (the first batch starts before cancel fires).
        // At least one outcome should be cancelled.
        let cancelled_count = outcomes
            .iter()
            .filter(|o| o.output_text.contains("CANCELLED"))
            .count();

        assert!(
            cancelled_count > 0,
            "Expected at least one cancelled outcome, got none. Outcomes: {:?}",
            outcomes.iter().map(|o| &o.output_text).collect::<Vec<_>>()
        );
    }
}
