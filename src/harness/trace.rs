//! Trace events for the agent loop
//!
//! Provides structured tracing events emitted during agent loop execution
//! for debugging, logging, and event bus distribution.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Event emitted during agent loop execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LoopTraceEvent {
    /// Text was emitted by the model
    TextEmitted {
        iteration: usize,
        stream: LoopTraceTextKind,
        text: String,
    },
    /// Tool call started
    ToolCallStarted {
        iteration: usize,
        call: ToolCallStartEvent,
    },
    /// Tool call completed
    ToolCallCompleted {
        iteration: usize,
        call: ToolCallEndEvent,
        result: crate::tools::runtime::ToolResult,
    },
    /// Tool summary generated
    ToolSummary { iteration: usize, summary: String },
    /// Turn started
    TurnStarted { iteration: usize },
    /// Turn state entered
    TurnStateEntered {
        iteration: usize,
        state: LoopTraceState,
    },
    /// Turn completed
    TurnCompleted {
        iteration: usize,
        outcome: LoopTraceTurnOutcome,
        metrics: LoopTraceTurnMetrics,
    },
    /// Session completed
    SessionCompleted {
        outcome: LoopTraceSessionOutcome,
        iterations: usize,
        tool_calls_made: usize,
        total_tokens: usize,
        hit_limit: bool,
        final_text: Option<String>,
    },
}

/// Kind of text stream
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopTraceTextKind {
    /// Final text (no more coming)
    Final,
    /// Intermediate text (more to come)
    Intermediate,
}

/// State entered during turn execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopTraceState {
    Prepare,
    Think,
    Resolve,
    Act,
    Finalize,
}

/// Outcome of a turn
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopTraceTurnOutcome {
    Continue,
    Stop,
    HitLimit,
    Cancelled,
}

/// Outcome of a session
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopTraceSessionOutcome {
    Completed,
    HitLimit,
    Cancelled,
}

/// Metrics captured at the end of a turn
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopTraceTurnMetrics {
    pub requested_tool_calls: usize,
    pub executed_tool_calls: usize,
    pub productive: bool,
    pub consecutive_errors: usize,
    pub total_tokens: usize,
}

/// Tool call start event data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallStartEvent {
    pub tool_id: String,
    pub tool_name: String,
    pub input: Value,
}

/// Tool call end event data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallEndEvent {
    pub tool_id: String,
    pub tool_name: String,
    pub input: Value,
    pub duration_ms: u64,
}

impl From<LoopTraceEvent> for aleph_protocol::AgentTraceEvent {
    fn from(event: LoopTraceEvent) -> Self {
        match event {
            LoopTraceEvent::TextEmitted {
                iteration,
                stream,
                text,
            } => aleph_protocol::AgentTraceEvent::TextEmitted {
                iteration,
                stream: stream.into(),
                text,
            },
            LoopTraceEvent::ToolCallStarted { iteration, call } => {
                aleph_protocol::AgentTraceEvent::ToolCallStarted {
                    iteration,
                    call: aleph_protocol::AgentTraceToolCallStart {
                        tool_id: call.tool_id,
                        tool_name: call.tool_name,
                        input: call.input,
                    },
                }
            }
            LoopTraceEvent::ToolCallCompleted {
                iteration,
                call,
                result,
            } => aleph_protocol::AgentTraceEvent::ToolCallCompleted {
                iteration,
                call: aleph_protocol::AgentTraceToolCallEnd {
                    tool_id: call.tool_id,
                    tool_name: call.tool_name,
                    input: call.input,
                    duration_ms: call.duration_ms,
                },
                result: match result {
                    crate::tools::runtime::ToolResult::Success { output } => {
                        aleph_protocol::AgentTraceToolResult::Success { output }
                    }
                    crate::tools::runtime::ToolResult::Error { error, retryable } => {
                        aleph_protocol::AgentTraceToolResult::Error { error, retryable }
                    }
                    crate::tools::runtime::ToolResult::SuccessAndStopLoop { output } => {
                        aleph_protocol::AgentTraceToolResult::SuccessAndStopLoop { output }
                    }
                },
            },
            LoopTraceEvent::ToolSummary { iteration, summary } => {
                aleph_protocol::AgentTraceEvent::ToolSummary { iteration, summary }
            }
            LoopTraceEvent::TurnStarted { iteration } => {
                aleph_protocol::AgentTraceEvent::TurnStarted { iteration }
            }
            LoopTraceEvent::TurnStateEntered { iteration, state } => {
                aleph_protocol::AgentTraceEvent::TurnStateEntered {
                    iteration,
                    state: state.into(),
                }
            }
            LoopTraceEvent::TurnCompleted {
                iteration,
                outcome,
                metrics,
            } => aleph_protocol::AgentTraceEvent::TurnCompleted {
                iteration,
                outcome: outcome.into(),
                metrics: metrics.into(),
            },
            LoopTraceEvent::SessionCompleted {
                outcome,
                iterations,
                tool_calls_made,
                total_tokens,
                hit_limit,
                final_text,
            } => aleph_protocol::AgentTraceEvent::SessionCompleted {
                outcome: outcome.into(),
                iterations,
                tool_calls_made,
                total_tokens,
                hit_limit,
                final_text,
            },
        }
    }
}

impl From<LoopTraceTextKind> for aleph_protocol::AgentTraceTextKind {
    fn from(kind: LoopTraceTextKind) -> Self {
        match kind {
            LoopTraceTextKind::Final => aleph_protocol::AgentTraceTextKind::Final,
            LoopTraceTextKind::Intermediate => aleph_protocol::AgentTraceTextKind::Intermediate,
        }
    }
}

impl From<LoopTraceState> for aleph_protocol::AgentTraceState {
    fn from(state: LoopTraceState) -> Self {
        match state {
            LoopTraceState::Prepare => aleph_protocol::AgentTraceState::Prepare,
            LoopTraceState::Think => aleph_protocol::AgentTraceState::Think,
            LoopTraceState::Resolve => aleph_protocol::AgentTraceState::Resolve,
            LoopTraceState::Act => aleph_protocol::AgentTraceState::Act,
            LoopTraceState::Finalize => aleph_protocol::AgentTraceState::Finalize,
        }
    }
}

impl From<LoopTraceTurnOutcome> for aleph_protocol::AgentTraceTurnOutcome {
    fn from(outcome: LoopTraceTurnOutcome) -> Self {
        match outcome {
            LoopTraceTurnOutcome::Continue => aleph_protocol::AgentTraceTurnOutcome::Continue,
            LoopTraceTurnOutcome::Stop => aleph_protocol::AgentTraceTurnOutcome::Stop,
            LoopTraceTurnOutcome::HitLimit => aleph_protocol::AgentTraceTurnOutcome::HitLimit,
            LoopTraceTurnOutcome::Cancelled => aleph_protocol::AgentTraceTurnOutcome::Cancelled,
        }
    }
}

impl From<LoopTraceSessionOutcome> for aleph_protocol::AgentTraceSessionOutcome {
    fn from(outcome: LoopTraceSessionOutcome) -> Self {
        match outcome {
            LoopTraceSessionOutcome::Completed => {
                aleph_protocol::AgentTraceSessionOutcome::Completed
            }
            LoopTraceSessionOutcome::HitLimit => aleph_protocol::AgentTraceSessionOutcome::HitLimit,
            LoopTraceSessionOutcome::Cancelled => {
                aleph_protocol::AgentTraceSessionOutcome::Cancelled
            }
        }
    }
}

impl From<LoopTraceTurnMetrics> for aleph_protocol::AgentTraceTurnMetrics {
    fn from(metrics: LoopTraceTurnMetrics) -> Self {
        aleph_protocol::AgentTraceTurnMetrics {
            requested_tool_calls: metrics.requested_tool_calls,
            executed_tool_calls: metrics.executed_tool_calls,
            productive: metrics.productive,
            consecutive_errors: metrics.consecutive_errors,
            total_tokens: metrics.total_tokens,
        }
    }
}
