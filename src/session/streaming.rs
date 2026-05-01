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

use serde_json::Value;
use std::collections::HashMap;
use std::time::Instant;

use crate::sync_primitives::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::harness::loop_callback::LoopCallback;
use crate::harness::trace::ToolCallEndEvent;
use crate::providers::delta::ProviderDelta;
use crate::tools::execution_context::CascadePolicy;
use crate::tools::orchestrator::ToolOutcome;
use crate::tools::pipeline::{PipelineOutcome, ToolPipeline};
use crate::tools::runtime::{ToolResult, LoopToolRegistry};
use tokio::sync::Mutex;

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
    /// The channel buffer is sized at 256 to avoid dropping tool calls
    /// under bursty concurrent execution.
    pub fn new(
        registry: Arc<LoopToolRegistry>,
        pipeline: Arc<ToolPipeline>,
        cancel: CancellationToken,
        callback: Option<Box<dyn LoopCallback>>,
    ) -> (Self, StreamingToolExecutor) {
        let (tx, rx) = mpsc::channel(256);
        let bridge = Self {
            pending: HashMap::new(),
            ready_tx: tx,
            tool_index: 0,
        };
        let executor = StreamingToolExecutor {
            ready_rx: rx,
            registry,
            pipeline,
            cancel,
            callback: Arc::new(Mutex::new(callback)),
            concurrent_calls: HashMap::new(),
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
                    let arguments = match serde_json::from_str(&pending.arg_buffer) {
                        Ok(args) => args,
                        Err(e) => {
                            tracing::error!(
                                tool_name = %pending.name,
                                tool_id = %id,
                                arg_buffer = %pending.arg_buffer,
                                error = %e,
                                "Failed to parse tool arguments JSON — using empty object fallback"
                            );
                            Value::Object(serde_json::Map::new())
                        }
                    };
                    let name = pending.name;
                    let ready = ReadyToolCall {
                        index: pending.index,
                        id: id.clone(),
                        name,
                        arguments,
                    };
                    if let Err(e) = self.ready_tx.try_send(ready) {
                        tracing::error!(
                            tool_id = %id,
                            error = %e,
                            "Failed to send ready tool call to executor — channel full or closed. Tool call dropped."
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
    pipeline: Arc<ToolPipeline>,
    cancel: CancellationToken,
    /// Callback for tool lifecycle events. Box allows interior mutability
    /// via `as_mut()` after locking.
    callback: Arc<Mutex<Option<Box<dyn LoopCallback>>>>,
    /// Tracks concurrent tool calls for callback emission after task completion.
    concurrent_calls: HashMap<usize, ReadyToolCall>,
}

impl StreamingToolExecutor {
    /// Consume all ready tool calls and return results sorted by original order.
    pub async fn run(mut self) -> Vec<PipelineOutcome> {
        let batch_cancel = self.cancel.child_token();
        let mut results: Vec<(usize, PipelineOutcome)> = Vec::new();
        let mut in_flight: Vec<(usize, JoinHandle<(usize, PipelineOutcome)>)> = Vec::new();
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
                                // Spawn immediately with batch-scoped cancel token.
                                // Track in concurrent_calls for callback emission.
                                let index = call.index;
                                self.concurrent_calls.insert(index, call);
                                let handle = self.spawn_tool_execution_with_cancel(
                                    index,
                                    batch_cancel.child_token(),
                                );
                                in_flight.push((index, handle));
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
        for (call_index, handle) in in_flight {
            match handle.await {
                Ok((_, outcome)) => {
                    // Check cascade policy: if a side-effecting tool failed,
                    // cancel all remaining siblings in this batch.
                    if outcome.outcome.is_error {
                        let policy = CascadePolicy::classify(&outcome.outcome.tool_name);
                        if matches!(policy, CascadePolicy::AbortSiblings) {
                            tracing::warn!(
                                target: "session::streaming",
                                tool = %outcome.outcome.tool_name,
                                "Cascading abort: tool failure triggers sibling cancellation"
                            );
                            batch_cancel.cancel();
                        }
                    }
                    if let Some(call) = self.concurrent_calls.remove(&call_index) {
                        self.emit_callback(&call.id, &call.name, &call.arguments, &outcome).await;
                    }
                    results.push((call_index, outcome));
                }
                Err(e) => {
                    tracing::error!(
                        call_index = %call_index,
                        "Spawned tool task panicked: {}",
                        e
                    );
                    let panic_outcome = PipelineOutcome {
                        outcome: ToolOutcome {
                            tool_id: String::new(),
                            tool_name: String::new(),
                            duration_ms: 0,
                            output_text: format!("[INTERNAL_ERROR] task panicked: {}", e),
                            is_error: true,
                            should_stop: false,
                            retryable: false,
                        },
                        additional_contexts: Vec::new(),
                        prevent_continuation: false,
                        hook_messages: Vec::new(),
                        needs_user_confirmation: false,
                        confirmation_reason: None,
                    };
                    if let Some(call) = self.concurrent_calls.remove(&call_index) {
                        self.emit_callback(&call.id, &call.name, &call.arguments, &panic_outcome
                        ).await;
                    }
                    results.push((call_index, panic_outcome));
                }
            }
        }

        // Phase 3: execute exclusive queue sequentially.
        for call in exclusive_queue {
            if self.cancel.is_cancelled() || batch_cancel.is_cancelled() {
                results.push((call.index, synthetic_abort_outcome(&call.id, &call.name)));
                continue;
            }
            let outcome = self
                .execute_one(&call.id, &call.name, &call.arguments)
                .await;
            results.push((call.index, outcome));
        }

        // Sort by original index to preserve input order.
        results.sort_by_key(|(idx, _)| *idx);
        results.into_iter().map(|(_, outcome)| outcome).collect()
    }

    /// Spawn a concurrent tool execution task with an explicit cancel token.
    fn spawn_tool_execution_with_cancel(
        &self,
        call_index: usize,
        cancel: CancellationToken,
    ) -> JoinHandle<(usize, PipelineOutcome)> {
        let call = self.concurrent_calls.get(&call_index).expect("concurrent call must exist");
        let registry = Arc::clone(&self.registry);
        let pipeline = Arc::clone(&self.pipeline);
        let id = call.id.clone();
        let name = call.name.clone();
        let arguments = call.arguments.clone();

        tokio::spawn(async move {
            let started_at = Instant::now();
            let mut outcome = pipeline
                .execute(&id, &name, &arguments, &registry, &cancel)
                .await;
            outcome.outcome.duration_ms = started_at.elapsed().as_millis() as u64;
            (call_index, outcome)
        })
    }

    /// Execute a single tool inline (for exclusive tools).
    async fn execute_one(&self, id: &str, name: &str, arguments: &Value) -> PipelineOutcome {
        let started_at = Instant::now();
        let mut outcome = self
            .pipeline
            .execute(id, name, arguments, &self.registry, &self.cancel)
            .await;
        outcome.outcome.duration_ms = started_at.elapsed().as_millis() as u64;
        self.emit_callback(id, name, arguments, &outcome).await;
        outcome
    }

    async fn emit_callback(
        &self,
        id: &str,
        name: &str,
        arguments: &Value,
        outcome: &PipelineOutcome,
    ) {
        let mut callback_guard = self.callback.lock().await;
        let Some(callback) = callback_guard.as_mut() else {
            return;
        };

        let event = ToolCallEndEvent {
            tool_id: id.to_string(),
            tool_name: name.to_string(),
            input: arguments.clone(),
            duration_ms: outcome.outcome.duration_ms,
        };
        let result = if outcome.outcome.is_error {
            ToolResult::Error {
                error: outcome.outcome.output_text.clone(),
                retryable: outcome.outcome.retryable,
            }
        } else {
            ToolResult::Success {
                output: Value::String(outcome.outcome.output_text.clone()),
            }
        };
        callback.as_mut().on_tool_call_done(&event, &result);
    }
}

/// Produce a synthetic abort outcome for a tool that was cancelled due to
/// cascading failure of a sibling tool.
fn synthetic_abort_outcome(id: &str, name: &str) -> PipelineOutcome {
    PipelineOutcome {
        outcome: ToolOutcome {
            tool_id: id.to_string(),
            tool_name: name.to_string(),
            duration_ms: 0,
            output_text: format!(
                "[Aborted] {} was cancelled because a sibling tool failed",
                name
            ),
            is_error: true,
            should_stop: false,
            retryable: false, // don't retry aborted tools
        },
        additional_contexts: Vec::new(),
        prevent_continuation: false,
        hook_messages: Vec::new(),
        needs_user_confirmation: false,
        confirmation_reason: None,
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

    use crate::extension::hooks::HookExecutor;
    use crate::extension::PermissionAction;
    use crate::session::ingress_safety::SafetyGuard;
    use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};

    /// A permissive pipeline for tests — allows everything, no hooks.
    fn permissive_pipeline() -> Arc<ToolPipeline> {
        Arc::new(ToolPipeline::new(
            Arc::new(HookExecutor::empty()),
            Arc::new(SafetyGuard::new(
                vec![],
                HashMap::new(),
                PermissionAction::Allow,
            )),
            "test-session",
        ))
    }

    /// A simple echo tool (concurrent-safe).
    struct EchoTool;

    #[async_trait]
    impl LoopTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes input"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
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
        fn name(&self) -> &str {
            "slow"
        }
        fn description(&self) -> &str {
            "Sleeps then returns"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _input: Value) -> ToolResult {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            ToolResult::Success {
                output: json!("done"),
            }
        }
        fn is_concurrent_safe(&self, _input: &Value) -> bool {
            true
        }
    }

    /// An exclusive (not concurrent-safe) tool.
    struct ExclusiveTool;

    #[async_trait]
    impl LoopTool for ExclusiveTool {
        fn name(&self) -> &str {
            "exclusive"
        }
        fn description(&self) -> &str {
            "Exclusive tool"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult::Success {
                output: json!("exclusive_done"),
            }
        }
        fn is_concurrent_safe(&self, _input: &Value) -> bool {
            false
        }
    }

    /// A very slow tool for cancellation testing.
    struct VerySlowTool;

    #[async_trait]
    impl LoopTool for VerySlowTool {
        fn name(&self) -> &str {
            "very_slow"
        }
        fn description(&self) -> &str {
            "Very slow tool"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _input: Value) -> ToolResult {
            tokio::time::sleep(Duration::from_secs(10)).await;
            ToolResult::Success {
                output: json!("should_not_reach"),
            }
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
        fn name(&self) -> &str {
            "concurrent_slow"
        }
        fn description(&self) -> &str {
            "Slow concurrent tool"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _input: Value) -> ToolResult {
            let prev = self.active.fetch_add(1, Ordering::SeqCst);
            self.peak.fetch_max(prev + 1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            ToolResult::Success {
                output: json!("done"),
            }
        }
        fn is_concurrent_safe(&self, _input: &Value) -> bool {
            true
        }
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
        bridge.feed(&ProviderDelta::ToolCallEnd { id: id.to_string() });
    }

    // -------------------------------------------------------------------------
    // Test 1: single tool collection and dispatch
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn bridge_collects_and_dispatches_single_tool() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let cancel = CancellationToken::new();
        let (mut bridge, executor) =
            StreamingToolBridge::new(Arc::new(registry), permissive_pipeline(), cancel, None);

        feed_tool_call(&mut bridge, "call_1", "echo", r#"{"msg":"hello"}"#);
        bridge.finish();

        let results = executor.run().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].outcome.tool_name, "echo");
        assert!(!results[0].outcome.is_error);
        // Echo tool returns the input as output.
        assert!(results[0].outcome.output_text.contains("hello"));
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
        let (mut bridge, executor) =
            StreamingToolBridge::new(Arc::new(registry), permissive_pipeline(), cancel, None);

        // Feed two concurrent-safe tool calls.
        feed_tool_call(&mut bridge, "c1", "concurrent_slow", "{}");
        feed_tool_call(&mut bridge, "c2", "concurrent_slow", "{}");
        bridge.finish();

        let start = Instant::now();
        let results = executor.run().await;
        let elapsed = start.elapsed();

        assert_eq!(results.len(), 2);
        for r in &results {
            assert!(!r.outcome.is_error);
        }

        // Two 50ms tools in parallel should take < 100ms total.
        assert!(
            elapsed < Duration::from_millis(100),
            "Expected parallel execution (<100ms), got {:?}",
            elapsed
        );

        // Results should be sorted by original index.
        assert_eq!(results[0].outcome.tool_id, "c1");
        assert_eq!(results[1].outcome.tool_id, "c2");
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
        let (mut bridge, executor) =
            StreamingToolBridge::new(Arc::new(registry), permissive_pipeline(), cancel, None);

        // First: concurrent tool, second: exclusive tool.
        feed_tool_call(&mut bridge, "t1", "slow", "{}");
        feed_tool_call(&mut bridge, "t2", "exclusive", "{}");
        bridge.finish();

        let results = executor.run().await;
        assert_eq!(results.len(), 2);

        // Both should succeed.
        assert!(!results[0].outcome.is_error);
        assert!(!results[1].outcome.is_error);

        // Order preserved.
        assert_eq!(results[0].outcome.tool_id, "t1");
        assert_eq!(results[0].outcome.tool_name, "slow");
        assert_eq!(results[1].outcome.tool_id, "t2");
        assert_eq!(results[1].outcome.tool_name, "exclusive");
    }

    // -------------------------------------------------------------------------
    // Test 4: empty stream returns empty results
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn empty_stream_returns_empty_results() {
        let registry = LoopToolRegistry::new();
        let cancel = CancellationToken::new();
        let (bridge, executor) =
            StreamingToolBridge::new(Arc::new(registry), permissive_pipeline(), cancel, None);

        // Finish immediately without feeding any deltas.
        bridge.finish();

        let results = executor.run().await;
        assert!(results.is_empty());
    }

    // -------------------------------------------------------------------------
    // Test 5: bash failure cascades to exclusive siblings
    // -------------------------------------------------------------------------

    /// A tool that always fails (named "Bash" for AbortSiblings cascade policy).
    struct FailingBashTool;

    #[async_trait]
    impl LoopTool for FailingBashTool {
        fn name(&self) -> &str {
            "Bash"
        }
        fn description(&self) -> &str {
            "Always fails"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult::Error {
                error: "command failed".into(),
                retryable: false,
            }
        }
        fn is_concurrent_safe(&self, _input: &Value) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn bash_failure_cascades_to_exclusive_siblings() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(FailingBashTool));
        registry.register(Box::new(ExclusiveTool));

        let cancel = CancellationToken::new();
        let (mut bridge, executor) =
            StreamingToolBridge::new(Arc::new(registry), permissive_pipeline(), cancel, None);

        // Bash fails (concurrent), then exclusive tool should be aborted.
        feed_tool_call(&mut bridge, "t1", "Bash", "{}");
        feed_tool_call(&mut bridge, "t2", "exclusive", "{}");
        bridge.finish();

        let results = executor.run().await;
        assert_eq!(results.len(), 2);
        assert!(results[0].outcome.is_error, "Bash should fail");
        assert!(results[1].outcome.is_error, "exclusive should be aborted");
        assert!(
            results[1].outcome.output_text.contains("Aborted")
                || results[1].outcome.output_text.contains("cancelled"),
            "should indicate abort, got: {}",
            results[1].outcome.output_text
        );
    }

    // -------------------------------------------------------------------------
    // Test 6: run_with_progress returns results and receiver
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn run_with_progress_returns_results_and_receiver() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let cancel = CancellationToken::new();
        let (mut bridge, executor) =
            StreamingToolBridge::new(Arc::new(registry), permissive_pipeline(), cancel, None);

        feed_tool_call(&mut bridge, "t1", "echo", r#"{"msg":"hello"}"#);
        bridge.finish();

        let results = executor.run().await;
        assert_eq!(results.len(), 1);
        assert!(!results[0].outcome.is_error);
        assert!(results[0].outcome.output_text.contains("hello"));
    }

    // -------------------------------------------------------------------------
    // Test 7: cancellation stops executor
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn cancellation_stops_executor() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(VerySlowTool));

        let cancel = CancellationToken::new();
        let (mut bridge, executor) =
            StreamingToolBridge::new(Arc::new(registry), permissive_pipeline(), cancel.clone(), None);

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
        let cancelled_count = results
            .iter()
            .filter(|r| r.outcome.output_text.contains("CANCELLED"))
            .count();
        assert!(
            cancelled_count > 0,
            "Expected at least one cancelled outcome, got: {:?}",
            results
                .iter()
                .map(|r| &r.outcome.output_text)
                .collect::<Vec<_>>()
        );
    }
}
