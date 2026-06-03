//! ForwardingTraceSink — decorator over a parent's TraceSink that translates
//! select child LoopTraceEvent emissions into SubagentProgress entries on a
//! BackgroundAgentTracker, while always forwarding the original event through
//! to the inner sink.
//!
//! Per P2 Stage F design (§3.3): installed ONLY on the background subagent
//! spawn path. Sync subagents share the parent's trace_sink directly (Stage A
//! inheritance); no wrapper is needed or installed there.

use crate::sync_primitives::Arc;

use crate::agents::background_tracker::BackgroundAgentTracker;
use crate::agents::progress::{ProgressKind, SubagentProgress};
use crate::harness::trace::{LoopTraceEvent, LoopTraceSessionOutcome, LoopTraceState};
use crate::harness::TraceSink;

/// Decorator over a parent's TraceSink that translates select child events
/// into SubagentProgress entries on a BackgroundAgentTracker, while always
/// forwarding the original event through to the inner sink.
///
/// Installed only on background subagent paths (see SubagentTool's background
/// branch). Sync subagents share the parent's trace_sink directly.
pub struct ForwardingTraceSink {
    inner: Arc<dyn TraceSink>,
    tracker: Arc<BackgroundAgentTracker>,
    request_id: String,
}

impl ForwardingTraceSink {
    pub fn new(
        inner: Arc<dyn TraceSink>,
        tracker: Arc<BackgroundAgentTracker>,
        request_id: String,
    ) -> Self {
        Self {
            inner,
            tracker,
            request_id,
        }
    }

    fn translate(&self, event: &LoopTraceEvent) -> Option<SubagentProgress> {
        use std::time::SystemTime;

        match event {
            LoopTraceEvent::ToolCallStarted { iteration, call } => Some(SubagentProgress {
                step: *iteration,
                timestamp: SystemTime::now(),
                kind: ProgressKind::ToolCalled,
                tool_name: Some(call.tool_name.clone()),
                latency_ms: None,
                preview: None,
            }),
            LoopTraceEvent::ToolCallCompleted {
                iteration,
                call,
                result,
            } => {
                let preview = render_tool_result_preview(result);
                Some(SubagentProgress {
                    step: *iteration,
                    timestamp: SystemTime::now(),
                    kind: ProgressKind::ToolReturned,
                    tool_name: Some(call.tool_name.clone()),
                    latency_ms: Some(call.duration_ms),
                    preview,
                })
            }
            LoopTraceEvent::TurnStateEntered {
                iteration,
                state: LoopTraceState::Think,
            } => Some(SubagentProgress {
                step: *iteration,
                timestamp: std::time::SystemTime::now(),
                kind: ProgressKind::LlmThinking,
                tool_name: None,
                latency_ms: None,
                preview: None,
            }),
            LoopTraceEvent::SessionCompleted {
                outcome: LoopTraceSessionOutcome::Cancelled,
                iterations,
                ..
            } => Some(SubagentProgress {
                step: *iterations,
                timestamp: std::time::SystemTime::now(),
                kind: ProgressKind::Cancelled,
                tool_name: None,
                latency_ms: None,
                preview: None,
            }),
            _ => None,
        }
    }
}

/// Render a 200-char preview of a tool result for SubagentProgress.preview.
fn render_tool_result_preview(result: &crate::tools::runtime::ToolResult) -> Option<String> {
    let raw = match result {
        crate::tools::runtime::ToolResult::Success { output } => {
            serde_json::to_string(output).ok()?
        }
        crate::tools::runtime::ToolResult::Error { error, .. } => error.clone(),
    };
    let mut s: String = raw.chars().take(200).collect();
    if raw.chars().count() > 200 {
        s.push('\u{2026}');
    }
    Some(s)
}

impl TraceSink for ForwardingTraceSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        if let Some(progress) = self.translate(event) {
            self.tracker.push_progress(&self.request_id, progress);
        }
        self.inner.on_trace(event);
    }

    fn flush(&self) {
        self.inner.flush();
    }

    fn on_init_seam(&self, stage: &'static str, seam: &'static str, configured: bool) {
        self.inner.on_init_seam(stage, seam, configured);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::background_tracker::BackgroundAgentTracker;
    use crate::agents::progress::ProgressKind;
    use crate::harness::trace::{
        LoopTraceEvent, LoopTraceSessionOutcome, LoopTraceState, ToolCallEndEvent,
        ToolCallStartEvent,
    };
    use crate::harness::TraceSink;
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;

    /// Test sink that records all events it receives.
    #[derive(Default)]
    struct CapturingSink {
        events: Mutex<Vec<LoopTraceEvent>>,
    }
    impl TraceSink for CapturingSink {
        fn on_trace(&self, event: &LoopTraceEvent) {
            self.events
                .lock()
                .expect("test sink lock")
                .push(event.clone());
        }
        fn flush(&self) {}
    }

    fn setup() -> (
        Arc<CapturingSink>,
        Arc<BackgroundAgentTracker>,
        ForwardingTraceSink,
    ) {
        let inner: Arc<CapturingSink> = Arc::new(CapturingSink::default());
        let tracker = Arc::new(BackgroundAgentTracker::new());
        tracker.register("rid".into(), CancellationToken::new(), "task".into());
        let inner_dyn: Arc<dyn TraceSink> = inner.clone();
        let wrapper = ForwardingTraceSink::new(inner_dyn, tracker.clone(), "rid".into());
        (inner, tracker, wrapper)
    }

    #[test]
    fn forwarding_translates_tool_call_started_to_tool_called() {
        let (_inner, tracker, wrapper) = setup();
        wrapper.on_trace(&LoopTraceEvent::ToolCallStarted {
            iteration: 3,
            call: ToolCallStartEvent {
                tool_id: "id".into(),
                tool_name: "read_file".into(),
                input: serde_json::json!({}),
            },
        });
        let snap = tracker.progress_snapshot("rid", 10);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].step, 3);
        assert_eq!(snap[0].kind, ProgressKind::ToolCalled);
        assert_eq!(snap[0].tool_name.as_deref(), Some("read_file"));
        assert!(snap[0].latency_ms.is_none());
    }

    #[test]
    fn forwarding_pairs_started_completed_for_latency() {
        let (_inner, tracker, wrapper) = setup();
        wrapper.on_trace(&LoopTraceEvent::ToolCallStarted {
            iteration: 1,
            call: ToolCallStartEvent {
                tool_id: "id".into(),
                tool_name: "grep".into(),
                input: serde_json::json!({}),
            },
        });
        wrapper.on_trace(&LoopTraceEvent::ToolCallCompleted {
            iteration: 1,
            call: ToolCallEndEvent {
                tool_id: "id".into(),
                tool_name: "grep".into(),
                input: serde_json::json!({}),
                duration_ms: 42,
            },
            result: crate::tools::runtime::ToolResult::Success {
                output: serde_json::json!({"hits": 3}),
            },
        });
        let snap = tracker.progress_snapshot("rid", 10);
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[1].kind, ProgressKind::ToolReturned);
        assert_eq!(snap[1].latency_ms, Some(42));
        assert!(snap[1].preview.is_some());
    }

    #[test]
    fn forwarding_forwards_unrelated_events_unchanged() {
        let (inner, tracker, wrapper) = setup();
        wrapper.on_trace(&LoopTraceEvent::TextEmitted {
            iteration: 1,
            stream: crate::harness::trace::LoopTraceTextKind::Final,
            text: "hello".into(),
        });
        // Inner sink received the event…
        assert_eq!(inner.events.lock().expect("test sink lock").len(), 1);
        // …but tracker.progress unchanged (TextEmitted is not translated).
        assert!(tracker.progress_snapshot("rid", 10).is_empty());
    }

    #[test]
    fn forwarding_translates_think_state_to_llm_thinking() {
        let (_inner, tracker, wrapper) = setup();
        wrapper.on_trace(&LoopTraceEvent::TurnStateEntered {
            iteration: 5,
            state: LoopTraceState::Think,
        });
        let snap = tracker.progress_snapshot("rid", 10);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].kind, ProgressKind::LlmThinking);
    }

    #[test]
    fn forwarding_translates_cancelled_session_to_cancelled() {
        let (_inner, tracker, wrapper) = setup();
        wrapper.on_trace(&LoopTraceEvent::SessionCompleted {
            outcome: LoopTraceSessionOutcome::Cancelled,
            iterations: 7,
            tool_calls_made: 2,
            total_tokens: 1000,
            hit_limit: false,
            final_text: None,
            terminate_reason: None,
            duration_ms: None,
            token_breakdown: None,
            tool_timeline: Vec::new(),
        });
        let snap = tracker.progress_snapshot("rid", 10);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].kind, ProgressKind::Cancelled);
        assert_eq!(snap[0].step, 7);
    }

    #[test]
    fn forwarding_other_turn_states_not_translated() {
        let (_inner, tracker, wrapper) = setup();
        for state in [
            LoopTraceState::Prepare,
            LoopTraceState::Resolve,
            LoopTraceState::Act,
            LoopTraceState::Finalize,
        ] {
            wrapper.on_trace(&LoopTraceEvent::TurnStateEntered {
                iteration: 1,
                state,
            });
        }
        // Only Think translates; others are forwarded but not stored.
        assert!(tracker.progress_snapshot("rid", 10).is_empty());
    }
}
