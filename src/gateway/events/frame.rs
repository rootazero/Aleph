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
        event: crate::agent_loop::LoopTraceEvent,
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
        }
        .to_string()
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
