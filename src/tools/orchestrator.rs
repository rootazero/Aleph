// TODO(Phase 5): move to Orchestrator. See docs/superpowers/specs/2026-04-18-managed-agents-refactor-roadmap.md §8 Phase 5.
// This file is the embryonic Orchestrator: `execute_tool_batch` + `partition_tool_calls` are the
// primary migration targets. In Phase 5, `LoopToolRegistry` is retired and all tool calls route
// through `ToolService`; this batch-scheduling logic moves into the new `Orchestrator` struct.

//! Tool orchestrator — partitions tool calls by concurrency safety and executes them.
//!
//! Concurrent-safe tools run in parallel via `futures::future::join_all`;
//! exclusive tools run one at a time. Original order is always preserved.

use std::time::Instant;

use crate::sync_primitives::Arc;

use tokio_util::sync::CancellationToken;

use crate::tools::runtime::LoopToolRegistry;
use crate::tools::pipeline::{PipelineOutcome, ToolPipeline};
use crate::harness::LoopCallback;
use crate::harness::trace::{LoopTraceEvent, ToolCallEndEvent, ToolCallStartEvent};
use crate::tools::runtime::ToolResult;
use crate::providers::adapter::NativeToolCall;
use crate::session::ingress_safety::ToolCall as SafetyToolCall;
use serde_json::Value;

// =============================================================================
// ToolOutcome
// =============================================================================

/// The result of executing a single tool call, ready for the conversation history.
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub tool_id: String,
    pub tool_name: String,
    /// End-to-end execution time for this tool call in milliseconds.
    pub duration_ms: u64,
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
    registry: &Arc<LoopToolRegistry>,
    pipeline: &ToolPipeline,
    cancel: &CancellationToken,
    callback: &mut dyn LoopCallback,
) -> Vec<PipelineOutcome> {
    let batches = partition_tool_calls(tool_calls, registry);
    let mut results: Vec<(usize, PipelineOutcome)> = Vec::with_capacity(tool_calls.len());

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
                    let safety_call = SafetyToolCall {
                        name: tc.name.clone(),
                        input: tc.arguments.clone(),
                    };
                    if pipeline.safety().check(&safety_call).is_ok() {
                        callback.on_trace(&LoopTraceEvent::ToolCallStarted {
                            iteration: 0,
                            call: ToolCallStartEvent {
                                tool_id: tc.id.clone(),
                                tool_name: tc.name.clone(),
                                input: tc.arguments.clone(),
                            },
                        });
                        safe_indices.push(idx);
                    } else {
                        // Safety-blocked: produce outcome directly, skip execution.
                        let po = pipeline
                            .execute(&tc.id, &tc.name, &tc.arguments, registry, cancel)
                            .await;
                        results.push((idx, po));
                    }
                }

                // Execute only safety-cleared tools concurrently.
                if !safe_indices.is_empty() {
                    let futures: Vec<_> = safe_indices
                        .iter()
                        .map(|&idx| {
                            let tc = &tool_calls[idx];
                            async move {
                                let started_at = Instant::now();
                                let mut outcome = pipeline
                                    .execute(&tc.id, &tc.name, &tc.arguments, registry, cancel)
                                    .await;
                                outcome.outcome.duration_ms =
                                    started_at.elapsed().as_millis() as u64;
                                outcome
                            }
                        })
                        .collect();

                    let outcomes = futures::future::join_all(futures).await;

                    for (i, po) in outcomes.into_iter().enumerate() {
                        let idx = safe_indices[i];
                        let tool_result = outcome_to_tool_result(&po.outcome);
                        callback.on_trace(&LoopTraceEvent::ToolCallCompleted {
                            iteration: 0,
                            call: ToolCallEndEvent {
                                tool_id: tool_calls[idx].id.clone(),
                                tool_name: tool_calls[idx].name.clone(),
                                input: tool_calls[idx].arguments.clone(),
                                duration_ms: po.outcome.duration_ms,
                            },
                            result: tool_result,
                        });
                        results.push((idx, po));
                    }
                }
            }
            Batch::Exclusive(idx) => {
                let idx = *idx;
                let tc = &tool_calls[idx];

                // Pre-check safety to fire on_tool_start before execution.
                let safety_call = SafetyToolCall {
                    name: tc.name.clone(),
                    input: tc.arguments.clone(),
                };
                let is_safe = pipeline.safety().check(&safety_call).is_ok();
                if is_safe {
                    callback.on_trace(&LoopTraceEvent::ToolCallStarted {
                        iteration: 0,
                        call: ToolCallStartEvent {
                            tool_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            input: tc.arguments.clone(),
                        },
                    });
                }

                let started_at = Instant::now();
                let mut po = pipeline
                    .execute(&tc.id, &tc.name, &tc.arguments, registry, cancel)
                    .await;
                po.outcome.duration_ms = started_at.elapsed().as_millis() as u64;

                if is_safe {
                    let tool_result = outcome_to_tool_result(&po.outcome);
                    callback.on_trace(&LoopTraceEvent::ToolCallCompleted {
                        iteration: 0,
                        call: ToolCallEndEvent {
                            tool_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            input: tc.arguments.clone(),
                            duration_ms: po.outcome.duration_ms,
                        },
                        result: tool_result,
                    });
                }
                results.push((idx, po));
            }
        }
    }

    // Sort by original index to preserve input order.
    results.sort_by_key(|(idx, _)| *idx);
    results.into_iter().map(|(_, po)| po).collect()
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

    use crate::harness::NoopCallback;
    use crate::tools::runtime::{LoopTool, LoopToolRegistry};
    use crate::extension::hooks::HookExecutor;
    use crate::extension::PermissionAction;
    use crate::session::ingress_safety::SafetyGuard;

    fn permissive_pipeline() -> ToolPipeline {
        ToolPipeline::new(
            Arc::new(HookExecutor::empty()),
            Arc::new(SafetyGuard::new(
                vec![],
                HashMap::new(),
                PermissionAction::Allow,
            )),
            "test-session",
        )
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
        let registry = Arc::new(registry);

        let calls = vec![
            make_tc_with_args("1", "slow_read", json!({ "ms": 100 })),
            make_tc_with_args("2", "slow_read", json!({ "ms": 100 })),
            make_tc_with_args("3", "slow_read", json!({ "ms": 100 })),
        ];

        let pipeline = permissive_pipeline();
        let cancel = CancellationToken::new();
        let mut callback = NoopCallback;

        let start = Instant::now();
        let outcomes =
            execute_tool_batch(&calls, &registry, &pipeline, &cancel, &mut callback).await;
        let elapsed = start.elapsed();

        assert_eq!(outcomes.len(), 3);
        for o in &outcomes {
            assert!(!o.outcome.is_error);
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
        let registry = Arc::new(registry);

        // Mixed: [slow_read, write, read, slow_read]
        let calls = vec![
            make_tc_with_args("a", "slow_read", json!({ "ms": 50 })),
            make_tc("b", "write"),
            make_tc("c", "read"),
            make_tc_with_args("d", "slow_read", json!({ "ms": 50 })),
        ];

        let pipeline = permissive_pipeline();
        let cancel = CancellationToken::new();
        let mut callback = NoopCallback;

        let outcomes =
            execute_tool_batch(&calls, &registry, &pipeline, &cancel, &mut callback).await;

        // Verify order matches input.
        assert_eq!(outcomes.len(), 4);
        assert_eq!(outcomes[0].outcome.tool_id, "a");
        assert_eq!(outcomes[0].outcome.tool_name, "slow_read");
        assert_eq!(outcomes[1].outcome.tool_id, "b");
        assert_eq!(outcomes[1].outcome.tool_name, "write");
        assert_eq!(outcomes[2].outcome.tool_id, "c");
        assert_eq!(outcomes[2].outcome.tool_name, "read");
        assert_eq!(outcomes[3].outcome.tool_id, "d");
        assert_eq!(outcomes[3].outcome.tool_name, "slow_read");

        // All should succeed.
        for o in &outcomes {
            assert!(
                !o.outcome.is_error,
                "tool {} should not be an error",
                o.outcome.tool_name
            );
        }
    }

    #[tokio::test]
    async fn test_cancellation_stops_execution() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(SlowReadTool::new(active, peak)));
        let registry = Arc::new(registry);

        // Two batches: first sleeps 200ms, second sleeps 200ms.
        // Cancel after 50ms — first batch should get cancelled, second should not start.
        let calls = vec![
            make_tc_with_args("1", "slow_read", json!({ "ms": 200 })),
            make_tc_with_args("2", "slow_read", json!({ "ms": 200 })),
        ];

        let pipeline = permissive_pipeline();
        let cancel = CancellationToken::new();
        let mut callback = NoopCallback;

        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        let outcomes =
            execute_tool_batch(&calls, &registry, &pipeline, &cancel, &mut callback).await;

        // Should have outcomes (the first batch starts before cancel fires).
        // At least one outcome should be cancelled.
        let cancelled_count = outcomes
            .iter()
            .filter(|o| o.outcome.output_text.contains("CANCELLED"))
            .count();

        assert!(
            cancelled_count > 0,
            "Expected at least one cancelled outcome, got none. Outcomes: {:?}",
            outcomes
                .iter()
                .map(|o| &o.outcome.output_text)
                .collect::<Vec<_>>()
        );
    }
}
