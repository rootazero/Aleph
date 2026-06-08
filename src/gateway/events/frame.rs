//! Typed Gateway Event Frame
//!
//! Replaces `broadcast::Sender<String>` in `GatewayEventBus` with a typed enum.
//! Subscribers receive `GatewayEventFrame` directly — no String deserialization.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::exec::socket::ApprovalDecisionType;
use crate::gateway::event_emitter::{RunSummary, StreamEvent, ToolResult};
use crate::gateway::{ChannelId, ChannelStatus};

/// Typed event frame for the gateway event bus.
///
/// This enum replaces `TopicEvent { topic: String, data: Value }` with
/// compile-time type safety. Each variant carries its own typed payload.
///
/// Serializes with `#[serde(tag = "type")]` to produce the same JSON-RPC
/// wire format that WebSocket clients expect:
/// `{"type": "AgentRunStarted", "run_id": "...", ...}`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GatewayEventFrame {
    RunAccepted {
        run_id: String,
        session_key: String,
        accepted_at: String,
    },
    Reasoning {
        run_id: String,
        seq: u64,
        content: String,
        is_complete: bool,
    },
    ToolStart {
        run_id: String,
        seq: u64,
        tool_name: String,
        tool_id: String,
        params: Value,
    },
    ToolUpdate {
        run_id: String,
        seq: u64,
        tool_id: String,
        progress: String,
    },
    ToolEnd {
        run_id: String,
        seq: u64,
        tool_id: String,
        result: ToolResult,
        duration_ms: u64,
    },
    AgentTrace {
        run_id: String,
        seq: u64,
        event: aleph_protocol::AgentTraceEvent,
    },
    ResponseChunk {
        run_id: String,
        seq: u64,
        delta: String,
        full_text: String,
        content: String,
        chunk_index: u32,
        is_final: bool,
        #[serde(default)]
        is_intermediate: bool,
    },
    RunComplete {
        run_id: String,
        seq: u64,
        summary: RunSummary,
        total_duration_ms: u64,
    },
    RunError {
        run_id: String,
        seq: u64,
        error: String,
        error_code: Option<String>,
    },
    AskUser {
        run_id: String,
        seq: u64,
        question: String,
        options: Vec<String>,
    },
    ReasoningBlock {
        run_id: String,
        seq: u64,
        step_type: crate::gateway::event_emitter::ReasoningStepType,
        label: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        confidence: Option<crate::gateway::event_emitter::ConfidenceLevel>,
        is_final: bool,
    },
    UncertaintySignal {
        run_id: String,
        seq: u64,
        uncertainty: String,
        suggested_action: crate::gateway::event_emitter::UncertaintyAction,
    },
    ModelResolved {
        run_id: String,
        model_info: crate::providers::health::ModelInfo,
    },
    SessionUpdated {
        session_key: String,
    },
    ChannelMessage {
        channel_id: ChannelId,
        conversation_id: crate::gateway::channel::ConversationId,
        message: InboundMessagePayload,
    },
    ChannelTyping {
        channel_id: ChannelId,
        conversation_id: crate::gateway::channel::ConversationId,
    },
    ChannelStatusChanged {
        channel_id: ChannelId,
        status: ChannelStatus,
    },
    ChannelError {
        channel_id: ChannelId,
        error: String,
    },
    ConfigChanged {
        section: Option<String>,
        value: Value,
    },
    PairingRequested {
        device_name: String,
    },
    PairingCompleted {
        device_id: String,
    },
    ApprovalRequested {
        approval_id: String,
        session_key: String,
        channel_id: String,
        conversation_id: String,
    },
    ApprovalResolved {
        approval_id: String,
        session_key: String,
        decision: ApprovalDecisionType,
        resolved_by: Option<String>,
    },
    ApprovalExpired {
        approval_id: String,
        session_key: String,
    },
    SessionLifecycleChanged {
        session_key: String,
        old_state: Option<String>,
        new_state: String,
        reason: Option<String>,
    },
    /// Fired whenever the ACP session pool changes (created / removed /
    /// respawned). Payload-free on purpose: panels re-fetch via
    /// `acp.sessions.list` so the truth source stays single.
    AcpSessionsChanged,
    /// Emitted whenever a cron job is mutated server-side (created / updated
    /// / deleted / enabled / disabled / forced-run / state-changed by a
    /// scheduler tick). The panel subscribes to `cron.job.changed` so it can
    /// drop polling. Payload is intentionally minimal — clients fetch the
    /// full job via `cron.get` if they need fresh data.
    CronJobChanged {
        job_id: String,
        change: ChangeKind,
    },
    /// Heartbeat-task analogue of `CronJobChanged`. Topic: `heartbeat.task.changed`.
    HeartbeatTaskChanged {
        task_id: String,
        change: ChangeKind,
    },
    /// Core-decided R5 interrupt addressed to one or more delivery surfaces.
    /// Unlike the raw agent-lifecycle frames, the "is this worth interrupting
    /// the user" policy has already been applied by the core R5 router; the
    /// shell only focus-gates and renders. `audience` lists the `SurfaceKind`
    /// wire strings (e.g. `["desktop"]`) the gateway forward-filter routes to.
    SurfaceNotify {
        audience: Vec<String>,
        title: String,
        body: String,
        /// Originating topic (e.g. `agent.run.complete`) — diagnostics only.
        source_topic: String,
    },
    /// Core-decided approval banner addressed to one or more delivery surfaces.
    /// The raw `approval.requested` frame stays operator-gated and drives the
    /// Panel card; this is the *banner* leg, routed by the R5 router so the
    /// shell renders it through the same unified path as `SurfaceNotify`. The
    /// payload is intentionally sparse — approval detail lives in the Panel
    /// card (via `exec.approvals.pending`); the banner only needs to get
    /// attention. Gated operator-only by `event_scope` (`surface.approval`).
    SurfaceApproval {
        audience: Vec<String>,
        approval_id: String,
        title: String,
        body: String,
    },
}

/// Tagging value for the panel so it can pick the right local action
/// (re-fetch list vs drop row) without re-issuing a list query.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Created,
    Updated,
    Deleted,
    /// Runtime-state-only change (e.g. `last_run_at_ms`, `consecutive_errors`).
    /// Cheaper than `Updated` so clients can skip re-rendering the whole form
    /// while still refreshing the timeline indicators.
    StateChanged,
}

impl From<StreamEvent> for GatewayEventFrame {
    fn from(event: StreamEvent) -> Self {
        match event {
            StreamEvent::RunAccepted {
                run_id,
                session_key,
                accepted_at,
            } => GatewayEventFrame::RunAccepted {
                run_id,
                session_key,
                accepted_at,
            },
            StreamEvent::Reasoning {
                run_id,
                seq,
                content,
                is_complete,
            } => GatewayEventFrame::Reasoning {
                run_id,
                seq,
                content,
                is_complete,
            },
            StreamEvent::ToolStart {
                run_id,
                seq,
                tool_name,
                tool_id,
                params,
            } => GatewayEventFrame::ToolStart {
                run_id,
                seq,
                tool_name,
                tool_id,
                params,
            },
            StreamEvent::ToolUpdate {
                run_id,
                seq,
                tool_id,
                progress,
            } => GatewayEventFrame::ToolUpdate {
                run_id,
                seq,
                tool_id,
                progress,
            },
            StreamEvent::ToolEnd {
                run_id,
                seq,
                tool_id,
                result,
                duration_ms,
            } => GatewayEventFrame::ToolEnd {
                run_id,
                seq,
                tool_id,
                result,
                duration_ms,
            },
            StreamEvent::AgentTrace { run_id, seq, event } => {
                GatewayEventFrame::AgentTrace { run_id, seq, event }
            }
            StreamEvent::ResponseChunk {
                run_id,
                seq,
                delta,
                full_text,
                content,
                chunk_index,
                is_final,
                is_intermediate,
            } => GatewayEventFrame::ResponseChunk {
                run_id,
                seq,
                delta,
                full_text,
                content,
                chunk_index,
                is_final,
                is_intermediate,
            },
            StreamEvent::RunComplete {
                run_id,
                seq,
                summary,
                total_duration_ms,
            } => GatewayEventFrame::RunComplete {
                run_id,
                seq,
                summary,
                total_duration_ms,
            },
            StreamEvent::RunError {
                run_id,
                seq,
                error,
                error_code,
            } => GatewayEventFrame::RunError {
                run_id,
                seq,
                error,
                error_code,
            },
            StreamEvent::AskUser {
                run_id,
                seq,
                question,
                options,
            } => GatewayEventFrame::AskUser {
                run_id,
                seq,
                question,
                options,
            },
            StreamEvent::ReasoningBlock {
                run_id,
                seq,
                step_type,
                label,
                content,
                confidence,
                is_final,
            } => GatewayEventFrame::ReasoningBlock {
                run_id,
                seq,
                step_type,
                label,
                content,
                confidence,
                is_final,
            },
            StreamEvent::UncertaintySignal {
                run_id,
                seq,
                uncertainty,
                suggested_action,
            } => GatewayEventFrame::UncertaintySignal {
                run_id,
                seq,
                uncertainty,
                suggested_action,
            },
            StreamEvent::ModelResolved { run_id, model_info } => {
                GatewayEventFrame::ModelResolved { run_id, model_info }
            }
            StreamEvent::SessionUpdated { session_key } => {
                GatewayEventFrame::SessionUpdated { session_key }
            }
        }
    }
}

impl GatewayEventFrame {
    pub fn topic_name(&self) -> String {
        match self {
            GatewayEventFrame::RunAccepted { .. } => "run.accepted",
            GatewayEventFrame::Reasoning { .. } => "agent.reasoning",
            GatewayEventFrame::ToolStart { .. } => "agent.tool.start",
            GatewayEventFrame::ToolUpdate { .. } => "agent.tool.update",
            GatewayEventFrame::ToolEnd { .. } => "agent.tool.end",
            GatewayEventFrame::AgentTrace { .. } => "agent.trace",
            GatewayEventFrame::ResponseChunk { .. } => "agent.response.chunk",
            GatewayEventFrame::RunComplete { .. } => "agent.run.complete",
            GatewayEventFrame::RunError { .. } => "agent.run.error",
            GatewayEventFrame::AskUser { .. } => "agent.ask.user",
            GatewayEventFrame::ReasoningBlock { .. } => "agent.reasoning.block",
            GatewayEventFrame::UncertaintySignal { .. } => "agent.uncertainty",
            GatewayEventFrame::ModelResolved { .. } => "agent.model.resolved",
            GatewayEventFrame::SessionUpdated { .. } => "session.updated",
            GatewayEventFrame::ChannelMessage { .. } => "channel.message",
            GatewayEventFrame::ChannelTyping { .. } => "channel.typing",
            GatewayEventFrame::ChannelStatusChanged { .. } => "channel.status",
            GatewayEventFrame::ChannelError { .. } => "channel.error",
            GatewayEventFrame::ConfigChanged { .. } => "config.changed",
            GatewayEventFrame::PairingRequested { .. } => "pairing.requested",
            GatewayEventFrame::PairingCompleted { .. } => "pairing.completed",
            GatewayEventFrame::ApprovalRequested { .. } => "approval.requested",
            GatewayEventFrame::ApprovalResolved { .. } => "approval.resolved",
            GatewayEventFrame::ApprovalExpired { .. } => "approval.expired",
            GatewayEventFrame::SessionLifecycleChanged { .. } => "session.lifecycle.changed",
            GatewayEventFrame::AcpSessionsChanged => "acp.sessions.changed",
            GatewayEventFrame::CronJobChanged { .. } => "cron.job.changed",
            GatewayEventFrame::HeartbeatTaskChanged { .. } => "heartbeat.task.changed",
            GatewayEventFrame::SurfaceNotify { .. } => "surface.notify",
            GatewayEventFrame::SurfaceApproval { .. } => "surface.approval",
        }
        .to_string()
    }

    /// Returns the `stream.*` method name for agent streaming events,
    /// or `None` for non-streaming events (config, channel, etc.).
    ///
    /// Streaming events are sent over WebSocket as JSON-RPC notifications:
    /// `{"method": "stream.<type>", "params": <frame_data>}`
    ///
    /// The frontend subscribes to `stream.*` and converts the method prefix
    /// from `stream.` to `run.` for internal event dispatch.
    pub fn stream_method(&self) -> Option<&'static str> {
        match self {
            GatewayEventFrame::RunAccepted { .. } => Some("stream.run_accepted"),
            GatewayEventFrame::Reasoning { .. } => Some("stream.reasoning"),
            GatewayEventFrame::ToolStart { .. } => Some("stream.tool_start"),
            GatewayEventFrame::ToolUpdate { .. } => Some("stream.tool_update"),
            GatewayEventFrame::ToolEnd { .. } => Some("stream.tool_end"),
            GatewayEventFrame::AgentTrace { .. } => Some("stream.agent_trace"),
            GatewayEventFrame::ResponseChunk { .. } => Some("stream.response_chunk"),
            GatewayEventFrame::RunComplete { .. } => Some("stream.run_complete"),
            GatewayEventFrame::RunError { .. } => Some("stream.run_error"),
            GatewayEventFrame::AskUser { .. } => Some("stream.ask_user"),
            GatewayEventFrame::ReasoningBlock { .. } => Some("stream.reasoning_block"),
            GatewayEventFrame::UncertaintySignal { .. } => Some("stream.uncertainty_signal"),
            GatewayEventFrame::ModelResolved { .. } => Some("stream.model_resolved"),
            GatewayEventFrame::SessionUpdated { .. } => Some("stream.session_updated"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundMessagePayload {
    pub text: String,
    pub sender: MessageSender,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSender {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

#[cfg(test)]
mod surface_notify_tests {
    use super::*;

    #[test]
    fn surface_notify_topic_and_wire_shape() {
        let f = GatewayEventFrame::SurfaceNotify {
            audience: vec!["desktop".to_string()],
            title: "Aleph finished".to_string(),
            body: "Your turn is complete.".to_string(),
            source_topic: "agent.run.complete".to_string(),
        };
        // Non-streaming → TopicEvent wire shape (topic + data), no stream method.
        assert_eq!(f.topic_name(), "surface.notify");
        assert!(f.stream_method().is_none());

        // serde(tag = "type", rename_all = "snake_case")
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["type"], "surface_notify");
        assert_eq!(v["audience"][0], "desktop");
        assert_eq!(v["title"], "Aleph finished");
        assert_eq!(v["source_topic"], "agent.run.complete");
    }

    #[test]
    fn surface_approval_topic_and_wire_shape() {
        let f = GatewayEventFrame::SurfaceApproval {
            audience: vec!["desktop".to_string()],
            approval_id: "a1".to_string(),
            title: "Aleph needs your approval".to_string(),
            body: "A tool call is waiting for you.".to_string(),
        };
        // Non-streaming → TopicEvent wire shape (topic + data), no stream method.
        assert_eq!(f.topic_name(), "surface.approval");
        assert!(f.stream_method().is_none());

        // serde(tag = "type", rename_all = "snake_case")
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["type"], "surface_approval");
        assert_eq!(v["audience"][0], "desktop");
        assert_eq!(v["approval_id"], "a1");
        assert_eq!(v["title"], "Aleph needs your approval");
        assert_eq!(v["body"], "A tool call is waiting for you.");
    }
}
