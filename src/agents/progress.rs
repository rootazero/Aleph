//! SubagentProgress — domain types for tracking background subagent activity.
//!
//! Per P2 Stage F design (§3.2): structured progress events live in the agent
//! layer (not LoopTraceEvent). Translated from child harness LoopTraceEvent
//! emissions by ForwardingTraceSink (forwarding_trace_sink.rs) and stored in
//! BackgroundAgentTracker.progress (capped FIFO 50).

use std::time::SystemTime;

/// One step in a background subagent's run, surfaced to parent via check_status.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SubagentProgress {
    /// Child harness iteration index (matches LoopTraceEvent.iteration).
    pub step: usize,
    /// Wall-clock timestamp at translation time. Used for "is it stuck?" diagnostics.
    pub timestamp: SystemTime,
    /// Categorical signal of what the child is doing.
    pub kind: ProgressKind,
    /// Tool being called (Some for ToolCalled/Returned; None otherwise).
    pub tool_name: Option<String>,
    /// Tool execution duration in milliseconds (Some for ToolReturned).
    pub latency_ms: Option<u64>,
    /// First 200 chars of the tool's output preview (Some for ToolReturned).
    pub preview: Option<String>,
}

/// Categorical kind of subagent progress event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressKind {
    /// Child started invoking a tool.
    ToolCalled,
    /// Child received a tool result.
    ToolReturned,
    /// Child entered the LLM "Think" turn state (waiting on model).
    LlmThinking,
    /// Child's session was cancelled.
    Cancelled,
}
