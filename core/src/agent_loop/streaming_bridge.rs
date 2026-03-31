//! Streaming tool bridge — enables tools to start executing as soon as they
//! finish streaming, rather than waiting for all tool calls to arrive.
//!
//! Two components connected by an `mpsc` channel:
//!
//! 1. **StreamingToolBridge** (producer) — fed `ProviderDelta` events during the
//!    collection loop. When it receives `ToolCallEnd`, it parses the accumulated
//!    arguments and sends the ready tool call to the executor via channel.
//!
//! 2. **StreamingToolExecutor** (consumer) — runs as a spawned task. Receives
//!    ready tool calls from the channel. Concurrent-safe tools are spawned
//!    immediately; exclusive tools are queued until all in-flight concurrent
//!    tools complete.

use std::collections::HashMap;
use std::sync::Arc;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::agent_loop::safety::{SafetyError, SafetyGuard, ToolCall as SafetyToolCall};
use crate::agent_loop::tool::{LoopToolRegistry, ToolResult};
use crate::agent_loop::tool_orchestrator::ToolOutcome;
use crate::providers::delta::ProviderDelta;
use crate::tool_output::compressor::compress_tool_output;

// =============================================================================
// ReadyToolCall
// =============================================================================

/// A fully-parsed tool call ready for execution.
struct ReadyToolCall {
    /// Original order index for result sorting.
    index: usize,
    /// Provider-assigned tool call id.
    id: String,
    /// Tool name.
    name: String,
    /// Parsed JSON arguments.
    arguments: Value,
}

// =============================================================================
// PendingToolCall
// =============================================================================

/// A tool call still accumulating argument fragments from the stream.
struct PendingToolCall {
    index: usize,
    name: String,
    arg_buffer: String,
}

// =============================================================================
// StreamingToolBridge (producer)
// =============================================================================

/// Collects streaming deltas and dispatches ready tool calls to the executor.
pub struct StreamingToolBridge {
    pending: HashMap<String, PendingToolCall>,
    ready_tx: mpsc::Sender<ReadyToolCall>,
    tool_index: usize,
}

impl StreamingToolBridge {
    /// Create a bridge/executor pair connected by an mpsc channel.
    ///
    /// The channel buffer is sized at 32 — more than enough for typical
    /// LLM responses which rarely exceed 10 parallel tool calls.
    pub fn new(
        registry: Arc<LoopToolRegistry>,
        safety: Arc<SafetyGuard>,
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
            safety,
            cancel,
        };
        (bridge, executor)
    }

    /// Process one delta event from the provider stream.
    ///
    /// - `ToolCallStart` → register a new pending tool call
    /// - `ToolCallArgDelta` → append to the argument buffer
    /// - `ToolCallEnd` → parse arguments, send to executor
    /// - All other deltas → ignored
    pub fn feed(&mut self, delta: &ProviderDelta) {
        match delta {
            ProviderDelta::ToolCallStart { id, name } => {
                let index = self.tool_index;
                self.tool_index += 1;
                self.pending.insert(
                    id.clone(),
                    PendingToolCall {
                        index,
                        name: name.clone(),
                        arg_buffer: String::new(),
                    },
                );
            }
            ProviderDelta::ToolCallArgDelta { id, delta } => {
                if let Some(pending) = self.pending.get_mut(id) {
                    pending.arg_buffer.push_str(delta);
                }
            }
            ProviderDelta::ToolCallEnd { id } => {
                if let Some(pending) = self.pending.remove(id) {
                    let arguments = serde_json::from_str(&pending.arg_buffer)
                        .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
                    let ready = ReadyToolCall {
                        index: pending.index,
                        id: id.clone(),
                        name: pending.name,
                        arguments,
                    };
                    // try_send: if channel is full (unlikely), log and drop.
                    // This is a bounded buffer — backpressure from slow execution
                    // should not block the delta stream.
                    if let Err(e) = self.ready_tx.try_send(ready) {
                        tracing::warn!(
                            "Failed to send ready tool call to executor: {}",
                            e
                        );
                    }
                }
            }
            _ => {} // ignore non-tool deltas
        }
    }

    /// Signal that no more deltas will arrive. Drops the sender to close the channel.
    pub fn finish(self) {
        drop(self);
    }
}

// =============================================================================
// StreamingToolExecutor (consumer)
// =============================================================================

/// Receives ready tool calls from the bridge and executes them with
/// concurrency control: concurrent-safe tools run in parallel, exclusive
/// tools wait for all in-flight work to complete first.
pub struct StreamingToolExecutor {
    ready_rx: mpsc::Receiver<ReadyToolCall>,
    registry: Arc<LoopToolRegistry>,
    safety: Arc<SafetyGuard>,
    cancel: CancellationToken,
}

impl StreamingToolExecutor {
    /// Consume all ready tool calls and return results sorted by original order.
    pub async fn run(mut self) -> Vec<ToolOutcome> {
        let mut results: Vec<(usize, ToolOutcome)> = Vec::new();
        let mut in_flight: Vec<(usize, JoinHandle<ToolOutcome>)> = Vec::new();
        let mut exclusive_queue: Vec<ReadyToolCall> = Vec::new();

        // Phase 1: receive tool calls from the channel.
        loop {
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => {
                    break;
                }
                maybe_call = self.ready_rx.recv() => {
                    match maybe_call {
                        None => break, // channel closed
                        Some(call) => {
                            let is_concurrent = self.registry
                                .get(&call.name)
                                .map(|t| t.is_concurrent_safe(&call.arguments))
                                .unwrap_or(false);

                            if is_concurrent && exclusive_queue.is_empty() {
                                // Spawn immediately.
                                let handle = self.spawn_tool_execution(call.id.clone(), call.name.clone(), call.arguments.clone());
                                in_flight.push((call.index, handle));
                            } else {
                                // Queue for sequential execution.
                                exclusive_queue.push(call);
                            }
                        }
                    }
                }
            }
        }

        // Phase 2: await all in-flight concurrent tasks.
        for (idx, handle) in in_flight {
            match handle.await {
                Ok(outcome) => results.push((idx, outcome)),
                Err(e) => {
                    tracing::error!("Spawned tool task panicked: {}", e);
                    results.push((idx, ToolOutcome {
                        tool_id: String::new(),
                        tool_name: String::new(),
                        output_text: format!("[INTERNAL_ERROR] task panicked: {}", e),
                        is_error: true,
                        should_stop: false,
                        retryable: false,
                    }));
                }
            }
        }

        // Phase 3: execute exclusive queue sequentially.
        for call in exclusive_queue {
            if self.cancel.is_cancelled() {
                break;
            }
            let outcome = self.execute_one(&call.id, &call.name, &call.arguments).await;
            results.push((call.index, outcome));
        }

        // Sort by original index to preserve input order.
        results.sort_by_key(|(idx, _)| *idx);
        results.into_iter().map(|(_, outcome)| outcome).collect()
    }

    /// Spawn a concurrent tool execution task.
    fn spawn_tool_execution(
        &self,
        id: String,
        name: String,
        arguments: Value,
    ) -> JoinHandle<ToolOutcome> {
        let registry = Arc::clone(&self.registry);
        let safety = Arc::clone(&self.safety);
        let cancel = self.cancel.clone();

        tokio::spawn(async move {
            execute_single_tool(&id, &name, &arguments, &registry, &safety, &cancel).await
        })
    }

    /// Execute a single tool inline (for exclusive tools).
    async fn execute_one(&self, id: &str, name: &str, arguments: &Value) -> ToolOutcome {
        execute_single_tool(id, name, arguments, &self.registry, &self.safety, &self.cancel).await
    }
}

// =============================================================================
// execute_single_tool (shared logic)
// =============================================================================

/// Execute a single tool call with safety check and cancellation.
///
/// Callbacks (on_tool_start, on_tool_done, on_safety_block) are NOT fired here.
/// The main loop fires them when processing `ToolOutcome`s, which is the single
/// point where callback ordering is deterministic.
async fn execute_single_tool(
    id: &str,
    name: &str,
    arguments: &Value,
    registry: &LoopToolRegistry,
    safety: &SafetyGuard,
    cancel: &CancellationToken,
) -> ToolOutcome {
    // Safety check.
    let safety_call = SafetyToolCall {
        name: name.to_string(),
        input: arguments.clone(),
    };
    if let Err(e) = safety.check(&safety_call) {
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
        return ToolOutcome {
            tool_id: id.to_string(),
            tool_name: name.to_string(),
            output_text: msg,
            is_error: true,
            should_stop: false,
            retryable: false,
        };
    }

    // Execute with cancellation.
    let result = tokio::select! {
        r = registry.execute(name, arguments.clone()) => r,
        _ = cancel.cancelled() => {
            return ToolOutcome {
                tool_id: id.to_string(),
                tool_name: name.to_string(),
                output_text: "[CANCELLED] Tool execution was cancelled".to_string(),
                is_error: true,
                should_stop: false,
                retryable: false,
            };
        }
    };

    // Map ToolResult to ToolOutcome.
    match &result {
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

/// Convert a JSON Value to a display string.
fn value_to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
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
    use std::time::{Duration, Instant};

    use crate::agent_loop::tool::LoopTool;
    use crate::extension::PermissionAction;

    /// A permissive safety guard for tests — allows everything.
    fn permissive_guard() -> SafetyGuard {
        SafetyGuard::new(vec![], HashMap::new(), PermissionAction::Allow)
    }

    /// A simple echo tool (concurrent-safe).
    struct EchoTool;

    #[async_trait]
    impl LoopTool for EchoTool {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "Echoes input" }
        fn schema(&self) -> Value { json!({ "type": "object", "properties": {} }) }
        async fn execute(&self, input: Value) -> ToolResult {
            ToolResult::Success { output: input }
        }
    }

    /// A slow tool for timing tests (concurrent-safe).
    struct SlowTool {
        delay_ms: u64,
    }

    #[async_trait]
    impl LoopTool for SlowTool {
        fn name(&self) -> &str { "slow" }
        fn description(&self) -> &str { "Sleeps then returns" }
        fn schema(&self) -> Value { json!({ "type": "object", "properties": {} }) }
        async fn execute(&self, _input: Value) -> ToolResult {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            ToolResult::Success { output: json!("done") }
        }
        fn is_concurrent_safe(&self, _input: &Value) -> bool { true }
    }

    /// An exclusive (not concurrent-safe) tool.
    struct ExclusiveTool;

    #[async_trait]
    impl LoopTool for ExclusiveTool {
        fn name(&self) -> &str { "exclusive" }
        fn description(&self) -> &str { "Exclusive tool" }
        fn schema(&self) -> Value { json!({ "type": "object", "properties": {} }) }
        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult::Success { output: json!("exclusive_done") }
        }
        fn is_concurrent_safe(&self, _input: &Value) -> bool { false }
    }

    /// A very slow tool for cancellation testing.
    struct VerySlowTool;

    #[async_trait]
    impl LoopTool for VerySlowTool {
        fn name(&self) -> &str { "very_slow" }
        fn description(&self) -> &str { "Very slow tool" }
        fn schema(&self) -> Value { json!({ "type": "object", "properties": {} }) }
        async fn execute(&self, _input: Value) -> ToolResult {
            tokio::time::sleep(Duration::from_secs(10)).await;
            ToolResult::Success { output: json!("should_not_reach") }
        }
    }

    /// A concurrent-safe slow tool that tracks peak concurrency.
    struct ConcurrentSlowTool {
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        delay_ms: u64,
    }

    #[async_trait]
    impl LoopTool for ConcurrentSlowTool {
        fn name(&self) -> &str { "concurrent_slow" }
        fn description(&self) -> &str { "Slow concurrent tool" }
        fn schema(&self) -> Value { json!({ "type": "object", "properties": {} }) }
        async fn execute(&self, _input: Value) -> ToolResult {
            let prev = self.active.fetch_add(1, Ordering::SeqCst);
            self.peak.fetch_max(prev + 1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            ToolResult::Success { output: json!("done") }
        }
        fn is_concurrent_safe(&self, _input: &Value) -> bool { true }
    }

    /// Feed a complete tool call (start + args + end) into the bridge.
    fn feed_tool_call(bridge: &mut StreamingToolBridge, id: &str, name: &str, args: &str) {
        bridge.feed(&ProviderDelta::ToolCallStart {
            id: id.to_string(),
            name: name.to_string(),
        });
        bridge.feed(&ProviderDelta::ToolCallArgDelta {
            id: id.to_string(),
            delta: args.to_string(),
        });
        bridge.feed(&ProviderDelta::ToolCallEnd {
            id: id.to_string(),
        });
    }

    // -------------------------------------------------------------------------
    // Test 1: single tool collection and dispatch
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn bridge_collects_and_dispatches_single_tool() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let cancel = CancellationToken::new();
        let (mut bridge, executor) = StreamingToolBridge::new(
            Arc::new(registry),
            Arc::new(permissive_guard()),
            cancel,
        );

        feed_tool_call(&mut bridge, "call_1", "echo", r#"{"msg":"hello"}"#);
        bridge.finish();

        let results = executor.run().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_name, "echo");
        assert!(!results[0].is_error);
        // Echo tool returns the input as output.
        assert!(results[0].output_text.contains("hello"));
    }

    // -------------------------------------------------------------------------
    // Test 2: concurrent tools execute in parallel
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn concurrent_tools_execute_in_parallel() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(ConcurrentSlowTool {
            active: active.clone(),
            peak: peak.clone(),
            delay_ms: 50,
        }));

        let cancel = CancellationToken::new();
        let (mut bridge, executor) = StreamingToolBridge::new(
            Arc::new(registry),
            Arc::new(permissive_guard()),
            cancel,
        );

        // Feed two concurrent-safe tool calls.
        feed_tool_call(&mut bridge, "c1", "concurrent_slow", "{}");
        feed_tool_call(&mut bridge, "c2", "concurrent_slow", "{}");
        bridge.finish();

        let start = Instant::now();
        let results = executor.run().await;
        let elapsed = start.elapsed();

        assert_eq!(results.len(), 2);
        for r in &results {
            assert!(!r.is_error);
        }

        // Two 50ms tools in parallel should take < 100ms total.
        assert!(
            elapsed < Duration::from_millis(100),
            "Expected parallel execution (<100ms), got {:?}",
            elapsed
        );

        // Results should be sorted by original index.
        assert_eq!(results[0].tool_id, "c1");
        assert_eq!(results[1].tool_id, "c2");
    }

    // -------------------------------------------------------------------------
    // Test 3: exclusive tool waits for concurrent
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn exclusive_tool_waits_for_concurrent() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(SlowTool { delay_ms: 20 }));
        registry.register(Box::new(ExclusiveTool));

        let cancel = CancellationToken::new();
        let (mut bridge, executor) = StreamingToolBridge::new(
            Arc::new(registry),
            Arc::new(permissive_guard()),
            cancel,
        );

        // First: concurrent tool, second: exclusive tool.
        feed_tool_call(&mut bridge, "t1", "slow", "{}");
        feed_tool_call(&mut bridge, "t2", "exclusive", "{}");
        bridge.finish();

        let results = executor.run().await;
        assert_eq!(results.len(), 2);

        // Both should succeed.
        assert!(!results[0].is_error);
        assert!(!results[1].is_error);

        // Order preserved.
        assert_eq!(results[0].tool_id, "t1");
        assert_eq!(results[0].tool_name, "slow");
        assert_eq!(results[1].tool_id, "t2");
        assert_eq!(results[1].tool_name, "exclusive");
    }

    // -------------------------------------------------------------------------
    // Test 4: empty stream returns empty results
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn empty_stream_returns_empty_results() {
        let registry = LoopToolRegistry::new();
        let cancel = CancellationToken::new();
        let (bridge, executor) = StreamingToolBridge::new(
            Arc::new(registry),
            Arc::new(permissive_guard()),
            cancel,
        );

        // Finish immediately without feeding any deltas.
        bridge.finish();

        let results = executor.run().await;
        assert!(results.is_empty());
    }

    // -------------------------------------------------------------------------
    // Test 5: cancellation stops executor
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn cancellation_stops_executor() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(VerySlowTool));

        let cancel = CancellationToken::new();
        let (mut bridge, executor) = StreamingToolBridge::new(
            Arc::new(registry),
            Arc::new(permissive_guard()),
            cancel.clone(),
        );

        feed_tool_call(&mut bridge, "s1", "very_slow", "{}");
        bridge.finish();

        // Cancel after 50ms.
        let cancel_handle = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_handle.cancel();
        });

        let start = Instant::now();
        let results = executor.run().await;
        let elapsed = start.elapsed();

        // Should return quickly (< 1s), not wait the full 10s.
        assert!(
            elapsed < Duration::from_secs(1),
            "Expected quick return after cancellation, got {:?}",
            elapsed
        );

        // The spawned tool should have been cancelled.
        assert!(!results.is_empty());
        let cancelled_count = results.iter().filter(|r| {
            r.output_text.contains("CANCELLED")
        }).count();
        assert!(
            cancelled_count > 0,
            "Expected at least one cancelled outcome, got: {:?}",
            results.iter().map(|r| &r.output_text).collect::<Vec<_>>()
        );
    }
}
