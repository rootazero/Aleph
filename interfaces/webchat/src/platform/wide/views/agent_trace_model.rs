//! Pure model layer for agent trace rendering.
//!
//! Converts `AgentTraceEvent` into `TraceNode` for display. No Leptos
//! dependency — this module is testable in plain `cargo test`.

use aleph_protocol::{
    present_agent_trace_event_with_labels_and_preset, AgentTraceEvent, AgentTracePresentation,
    AgentTracePresentationLabels, AgentTracePresentationPreset, AgentTracePresentationStatus,
};

use crate::models::{TraceNode, TraceNodeType, TraceStatus};

// ---------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------

/// Wrapper around [`AgentTracePresentationLabels`] with a convenient default.
pub struct TraceLabels {
    pub inner: AgentTracePresentationLabels,
}

impl Default for TraceLabels {
    fn default() -> Self {
        Self {
            inner: AgentTracePresentationLabels::english(),
        }
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Map presentation status to the local `TraceStatus` enum.
const fn map_status(status: AgentTracePresentationStatus) -> TraceStatus {
    match status {
        AgentTracePresentationStatus::InProgress => TraceStatus::InProgress,
        AgentTracePresentationStatus::Success => TraceStatus::Success,
        AgentTracePresentationStatus::Failed => TraceStatus::Failed,
        // No dedicated warning tint in the Panel trace tree — a degraded-but-
        // running condition maps to the same neutral pending marker as Info.
        AgentTracePresentationStatus::Info | AgentTracePresentationStatus::Warning => {
            TraceStatus::Pending
        }
    }
}

/// Map the presentation kind string to a `TraceNodeType`.
fn map_node_type(kind: &str) -> TraceNodeType {
    match kind {
        "tool_call_started" | "tool_call_completed" => TraceNodeType::ToolCall,
        "tool_summary" => TraceNodeType::ToolResult,
        "turn_state_entered" => TraceNodeType::Thinking,
        "text_emitted" => TraceNodeType::Observation,
        "turn_started" | "turn_completed" | "session_completed" => TraceNodeType::Decision,
        _ => TraceNodeType::Observation,
    }
}

/// Convert a single `AgentTraceEvent` into a `TraceNode`.
///
/// Returns `None` when the presentation layer filters out the event.
#[must_use]
pub fn trace_node_from_event(
    event: &AgentTraceEvent,
    step: u64,
    labels: &TraceLabels,
) -> Option<TraceNode> {
    let presentation: AgentTracePresentation = present_agent_trace_event_with_labels_and_preset(
        event,
        &labels.inner,
        AgentTracePresentationPreset::PanelTrace,
    )?;

    Some(TraceNode {
        id: format!("step-{step}"),
        node_type: map_node_type(&presentation.kind),
        timestamp: 0.0,
        duration_ms: presentation.duration_ms,
        content: presentation.content,
        status: map_status(presentation.status),
        children: Vec::new(),
    })
}

/// Convert a full replay entry list into `TraceNode`s.
#[must_use]
pub fn trace_nodes_from_replay(
    entries: &[(u64, AgentTraceEvent)],
    labels: &TraceLabels,
) -> Vec<TraceNode> {
    entries
        .iter()
        .filter_map(|(step, event)| trace_node_from_event(event, *step, labels))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::{AgentTraceEvent, AgentTraceToolCallStart};
    use serde_json::json;

    #[test]
    fn converts_tool_call_started_to_trace_node() {
        let labels = TraceLabels::default();
        let event = AgentTraceEvent::ToolCallStarted {
            iteration: 1,
            call: AgentTraceToolCallStart {
                tool_id: "t1".into(),
                tool_name: "read_file".into(),
                input: json!({"path": "/tmp/x"}),
            },
        };

        let node = trace_node_from_event(&event, 0, &labels);
        assert!(node.is_some(), "ToolCallStarted should produce a node");
        let node = node.unwrap();
        assert_eq!(node.id, "step-0");
        assert_eq!(node.node_type, TraceNodeType::ToolCall);
        assert_eq!(node.status, TraceStatus::InProgress);
        assert!(node.content.contains("read_file"));
    }

    #[test]
    fn converts_turn_started_to_trace_node() {
        let labels = TraceLabels::default();
        let event = AgentTraceEvent::TurnStarted { iteration: 3 };

        let node = trace_node_from_event(&event, 5, &labels).unwrap();
        assert_eq!(node.id, "step-5");
        assert!(node.content.contains("iteration 3"));
    }

    #[test]
    fn trace_nodes_from_replay_filters_and_collects() {
        let labels = TraceLabels::default();
        let entries: Vec<(u64, AgentTraceEvent)> = vec![
            (0, AgentTraceEvent::TurnStarted { iteration: 1 }),
            (
                1,
                AgentTraceEvent::ToolCallStarted {
                    iteration: 1,
                    call: AgentTraceToolCallStart {
                        tool_id: "t1".into(),
                        tool_name: "bash".into(),
                        input: json!({}),
                    },
                },
            ),
        ];

        let nodes = trace_nodes_from_replay(&entries, &labels);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].id, "step-0");
        assert_eq!(nodes[1].id, "step-1");
    }
}
