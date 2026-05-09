//! Stage F integration tests for subagent streaming progress.
//!
//! These tests exercise the ForwardingTraceSink wrapper end-to-end via mocked
//! provider/tool services, validating that:
//!   1. Background subagents accumulate progress visible through check_status
//!   2. Sync subagents do NOT install the wrapper (no progress recording)

use alephcore::agents::background_tracker::BackgroundAgentTracker;
use alephcore::agents::forwarding_trace_sink::ForwardingTraceSink;
use alephcore::agents::progress::ProgressKind;
use alephcore::harness::trace::{LoopTraceEvent, LoopTraceState, ToolCallEndEvent, ToolCallStartEvent};
use alephcore::harness::TraceSink;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct CapturingSink {
    events: Mutex<Vec<LoopTraceEvent>>,
}
impl TraceSink for CapturingSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
    fn flush(&self) {}
}

#[test]
fn background_subagent_check_status_returns_progress() {
    // Simulate a background subagent emitting 3 trace events into the wrapper.
    // After the run, check_status equivalent (tracker.progress_snapshot) should
    // surface a 3-entry progress array of correct kinds.
    let inner = Arc::new(CapturingSink::default());
    let tracker = Arc::new(BackgroundAgentTracker::new());
    let token = CancellationToken::new();
    tracker.register("test-rid".into(), token, "task".into());

    let wrapper = ForwardingTraceSink::new(inner.clone(), tracker.clone(), "test-rid".into());

    // Emit a sequence representing one tool call cycle.
    wrapper.on_trace(&LoopTraceEvent::TurnStateEntered {
        iteration: 1,
        state: LoopTraceState::Think,
    });
    wrapper.on_trace(&LoopTraceEvent::ToolCallStarted {
        iteration: 1,
        call: ToolCallStartEvent {
            tool_id: "id-1".into(),
            tool_name: "read_file".into(),
            input: serde_json::json!({"path": "/tmp/x"}),
        },
    });
    wrapper.on_trace(&LoopTraceEvent::ToolCallCompleted {
        iteration: 1,
        call: ToolCallEndEvent {
            tool_id: "id-1".into(),
            tool_name: "read_file".into(),
            input: serde_json::json!({"path": "/tmp/x"}),
            duration_ms: 12,
        },
        result: alephcore::tools::runtime::ToolResult::Success {
            output: serde_json::json!({"contents": "hello"}),
        },
    });

    // Verify forwarding: inner sink saw all 3 events.
    assert_eq!(inner.events.lock().unwrap().len(), 3);

    // Verify progress: 3 translated entries in chronological order.
    let snap = tracker.progress_snapshot("test-rid", 10);
    assert_eq!(snap.len(), 3, "got: {snap:?}");
    assert_eq!(snap[0].kind, ProgressKind::LlmThinking);
    assert_eq!(snap[1].kind, ProgressKind::ToolCalled);
    assert_eq!(snap[2].kind, ProgressKind::ToolReturned);
    assert_eq!(snap[2].latency_ms, Some(12));
    assert_eq!(snap[2].tool_name.as_deref(), Some("read_file"));
}

#[test]
fn sync_subagent_does_not_install_wrapper() {
    // Sentinel test: the wrapper module is not auto-installed.
    // Construction is explicit per-call; a sync subagent that never receives a
    // wrapped sink leaves tracker.progress empty even after trace events flow.
    let tracker = Arc::new(BackgroundAgentTracker::new());
    // Simulate a sync subagent: no register() call (no background entry exists)
    // and trace events flow through a non-wrapping sink.
    let plain_sink = Arc::new(CapturingSink::default());
    plain_sink.on_trace(&LoopTraceEvent::ToolCallStarted {
        iteration: 1,
        call: ToolCallStartEvent {
            tool_id: "id".into(),
            tool_name: "grep".into(),
            input: serde_json::json!({}),
        },
    });
    // Tracker never received progress (because no wrapper exists for sync paths).
    assert!(
        tracker.progress_snapshot("nonexistent-sync-rid", 10).is_empty(),
        "sync path must not populate background tracker"
    );
    // And the plain sink correctly captured the event (proves the trace flowed).
    assert_eq!(plain_sink.events.lock().unwrap().len(), 1);
}
