//! Streaming Event Types
//!
//! Event types for real-time agent feedback via WebSocket.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::thinking::{ConfidenceLevel, ReasoningStepType};

/// Streaming event types for real-time agent feedback
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Agent run has been accepted and started
    RunAccepted {
        run_id: String,
        session_key: String,
        accepted_at: String,
    },

    /// Reasoning/thinking process update
    Reasoning {
        run_id: String,
        seq: u64,
        content: String,
        is_complete: bool,
    },

    /// Tool execution started
    ToolStart {
        run_id: String,
        seq: u64,
        tool_name: String,
        tool_id: String,
        params: Value,
    },

    /// Tool execution progress update
    ToolUpdate {
        run_id: String,
        seq: u64,
        tool_id: String,
        progress: String,
    },

    /// Tool execution completed
    ToolEnd {
        run_id: String,
        seq: u64,
        tool_id: String,
        result: ToolResult,
        duration_ms: u64,
    },

    /// Structured execution trace emitted directly by the agent loop.
    AgentTrace {
        run_id: String,
        seq: u64,
        event: AgentTraceEvent,
    },

    /// Response text chunk (streaming output)
    ResponseChunk {
        run_id: String,
        seq: u64,
        content: String,
        chunk_index: u32,
        is_final: bool,
        #[serde(default)]
        is_intermediate: bool,
    },

    /// Agent run completed successfully
    RunComplete {
        run_id: String,
        seq: u64,
        summary: RunSummary,
        total_duration_ms: u64,
    },

    /// Agent run failed with error
    RunError {
        run_id: String,
        seq: u64,
        error: String,
        error_code: Option<String>,
    },

    /// Agent is asking the user a question
    AskUser {
        run_id: String,
        seq: u64,
        /// Clarification registry key the answer must be posted back against
        /// (`clarification.resolve`). Replies route by session, not by run, so a
        /// client that only kept `run_id` cannot answer — this is the field the
        /// TUI/CLI need to unblock the parked `ask_user`. Mirrors the core
        /// `GatewayEventFrame::AskUser` the wire carries.
        #[serde(default)]
        session_key: String,
        question: String,
        options: Vec<String>,
    },

    /// Structured reasoning block with semantic type
    ReasoningBlock {
        run_id: String,
        seq: u64,
        /// Semantic step type (observation, analysis, planning, etc.)
        step_type: ReasoningStepType,
        /// Human-readable label for this block
        label: String,
        /// Content of this reasoning block
        content: String,
        /// Confidence level if determinable
        #[serde(skip_serializing_if = "Option::is_none")]
        confidence: Option<ConfidenceLevel>,
        /// Is this the final block before action?
        is_final: bool,
    },

    /// Uncertainty signal from the AI
    UncertaintySignal {
        run_id: String,
        seq: u64,
        /// What the AI is uncertain about
        uncertainty: String,
        /// Suggested action for handling the uncertainty
        suggested_action: UncertaintyAction,
    },

    /// Provider chain failed transiently; the run is about to retry.
    ///
    /// Mirrors the gateway's `stream.run_retrying` frame so terminal clients
    /// (CLI `ask`, TUI chat) can show "provider unreachable, retrying"
    /// instead of a silent thinking indicator while the retry ladder burns
    /// minutes. Field names/types must stay aligned with
    /// `GatewayEventFrame::RunRetrying` (`src/gateway/events/frame.rs`).
    RunRetrying {
        run_id: String,
        seq: u64,
        /// Provider that just failed.
        provider: String,
        /// 1-based attempt about to run (`2..=max_attempts`).
        attempt: u32,
        /// Total dispatch attempts before the run gives up.
        max_attempts: u32,
        /// Short human-readable failure reason (truncated at emit site).
        reason: String,
    },

    /// The provider router settled on a concrete model for this run.
    ///
    /// Mirrors the gateway's `stream.model_resolved` frame (no `seq` —
    /// `GatewayEventFrame::ModelResolved` carries only `run_id` +
    /// `model_info`). Lets terminal clients flag fallback selections the
    /// way the Panel does, instead of silently answering with a different
    /// model than the user asked for.
    ModelResolved {
        run_id: String,
        model_info: ModelInfo,
    },

    /// Live context-window occupancy after one LLM call (mid-run).
    ///
    /// Mirrors the gateway's `stream.context_gauge` frame — already broadcast
    /// to every subscriber (the webchat Panel renders it live) but invisible to
    /// the *typed* terminal clients until this variant existed to parse it.
    /// Same precedent as [`Self::RunRetrying`] / [`Self::ModelResolved`]. Field
    /// names/types must stay aligned with `StreamEvent::ContextGauge` in
    /// `src/gateway/event_emitter/types.rs`: `context_tokens` = current
    /// occupancy, `context_window` = server-authoritative per-model
    /// denominator, `total_tokens` = run-cumulative tally.
    ContextGauge {
        run_id: String,
        seq: u64,
        context_tokens: u32,
        context_window: u32,
        total_tokens: u64,
    },
}

/// Resolved model metadata attached to [`StreamEvent::ModelResolved`].
///
/// Field names/types must stay aligned with
/// `crate::providers::health::ModelInfo` on the gateway side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model identifier actually used.
    pub model: String,
    /// Provider name serving the model.
    pub provider: String,
    /// Whether this was a fallback selection (not the user's first choice).
    #[serde(default)]
    pub is_fallback: bool,
    /// Original model requested, if different from `model`.
    #[serde(default)]
    pub original_model: Option<String>,
}

/// Suggested action for handling AI uncertainty
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UncertaintyAction {
    /// Proceed despite uncertainty
    ProceedWithCaution,
    /// Ask user for clarification before proceeding
    AskForClarification,
    /// Use a safer/more conservative approach
    UseSaferApproach,
    /// Stop and wait for user input
    WaitForUser,
}

impl UncertaintyAction {
    /// Get human-readable description
    #[must_use]
    pub const fn description(&self) -> &'static str {
        match self {
            Self::ProceedWithCaution => "Proceeding with caution despite uncertainty",
            Self::AskForClarification => "Asking user for clarification",
            Self::UseSaferApproach => "Using a safer, more conservative approach",
            Self::WaitForUser => "Waiting for user guidance",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentTraceState {
    Prepare,
    Think,
    Resolve,
    Act,
    Finalize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentTraceTextKind {
    Final,
    Intermediate,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentTraceTurnOutcome {
    #[default]
    Continue,
    Stop,
    HitLimit,
    Cancelled,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTraceTurnMetrics {
    pub requested_tool_calls: usize,
    pub executed_tool_calls: usize,
    pub productive: bool,
    pub total_tokens: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentTraceSessionOutcome {
    Completed,
    HitLimit,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTraceToolCallStart {
    pub tool_id: String,
    pub tool_name: String,
    pub input: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTraceToolCallEnd {
    pub tool_id: String,
    pub tool_name: String,
    pub input: Value,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentTraceToolResult {
    Success { output: Value },
    Error { error: String, retryable: bool },
}

impl AgentTraceToolResult {
    #[must_use]
    pub const fn is_success(&self) -> bool {
        !matches!(self, Self::Error { .. })
    }

    #[must_use]
    pub fn error_text(&self) -> Option<&str> {
        match self {
            Self::Error { error, .. } => Some(error),
            Self::Success { .. } => None,
        }
    }
}

// `Eq` dropped: `MoaAdvisorSpend.cost_usd` is `Option<f64>`, and `f64` has no
// `Eq` impl (NaN). `PartialEq` alone covers every actual use (`assert_eq!` in
// tests); nothing in the codebase requires `AgentTraceEvent: Eq`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentTraceEvent {
    TurnStarted {
        iteration: usize,
    },
    TurnStateEntered {
        iteration: usize,
        state: AgentTraceState,
    },
    TextEmitted {
        iteration: usize,
        stream: AgentTraceTextKind,
        text: String,
    },
    ToolCallStarted {
        iteration: usize,
        call: AgentTraceToolCallStart,
    },
    ToolCallCompleted {
        iteration: usize,
        call: AgentTraceToolCallEnd,
        result: AgentTraceToolResult,
    },
    ToolSummary {
        iteration: usize,
        summary: String,
    },
    TurnCompleted {
        iteration: usize,
        outcome: AgentTraceTurnOutcome,
        metrics: AgentTraceTurnMetrics,
    },
    SessionCompleted {
        outcome: AgentTraceSessionOutcome,
        iterations: usize,
        tool_calls_made: usize,
        total_tokens: usize,
        hit_limit: bool,
        final_text: Option<String>,
        /// Precise loop-exit cause. Encoded as opaque JSON so the protocol
        /// crate stays free of `alephcore` types. `None` on legacy blobs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminate_reason: Option<Value>,
        /// Wall-clock harness duration. `None` on legacy blobs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        /// Per-component token breakdown encoded as opaque JSON.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_breakdown: Option<Value>,
        /// Tool invocation timeline encoded as opaque JSON entries.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_timeline: Vec<Value>,
    },
    /// Subagent worktree isolation primitive created (P3 Stage H).
    WorktreeCreated {
        path: std::path::PathBuf,
    },
    /// Subagent worktree cleaned up (P3 Stage H).
    WorktreeCleanedUp {
        path: std::path::PathBuf,
        leaked: bool,
    },
    /// Per-agent MCP scope attached (P3 Stage I).
    McpScopeAttached {
        agent_id: String,
        references: Vec<String>,
        inline_count: usize,
    },
    /// Per-agent MCP scope cleaned up (P3 Stage I).
    McpScopeCleaned {
        agent_id: String,
        leaked: bool,
    },
    /// Per-call provider usage (Stage J-pre cache observability).
    ProviderUsage {
        agent_id: String,
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: Option<u32>,
        cache_creation_tokens: Option<u32>,
        thinking_tokens: Option<u32>,
    },
    /// Reactive-compaction rescue fired in response to a provider
    /// `prompt_too_long` / 413 error. `token_gap` is the reported overflow
    /// when the error carried one; `succeeded` is `true` only if the
    /// post-compaction retry produced a usable response. See
    /// `harness::agent::think::try_reactive_compact_and_retry`.
    ReactiveCompactionAttempted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_gap: Option<usize>,
        succeeded: bool,
    },
    /// Structural goal-loop watchdog vetoed the model's attempt to stop: its
    /// self-maintained scratchpad still has unchecked `- [ ]` items, so the
    /// harness forced another iteration. `reason` lists the pending items.
    /// Purely mechanical — no LLM judgment (R7/R10). Surfacing it lets the user
    /// see *why* the run kept going instead of an unexplained extra turn. See
    /// `verification::scratchpad_goal_verifier`.
    VerifierVeto {
        iteration: usize,
        reason: String,
    },
    /// MoA advisor consultation result — one per advisor per fan-out
    /// (cache-MISS iterations only). Mirrors `harness::trace::LoopTraceEvent::MoaAdvisor`.
    /// `count == 0` is the activation-failure form: MoA could not engage for
    /// this run (preset missing / slot unresolvable) and `error` says why.
    MoaAdvisor {
        index: usize,
        count: usize,
        /// `provider:model` of the advisor slot.
        label: String,
        text: String,
        /// Structural failure reason (timeout / provider error); `None` on
        /// success.
        error: Option<String>,
    },
    /// MoA fan-out complete (or reused); the aggregator (acting model) is
    /// being called. `cached: true` = this iteration reuses the previous
    /// fan-out's advice (no advisor re-run, no new spend).
    MoaAggregating {
        aggregator: String,
        advisor_count: usize,
        cached: bool,
    },
    /// Summed advisor spend for one fan-out, priced per-advisor at each
    /// advisor's OWN model rate (kept out of `ProviderResponse.usage` so the
    /// context gauge stays honest, spec §8). `advisor_count` = advisors
    /// consulted this fan-out; `billed_count` = advisors that returned usage.
    MoaAdvisorSpend {
        advisor_count: usize,
        billed_count: usize,
        input_tokens: u32,
        output_tokens: u32,
        cost_usd: Option<f64>,
    },
    /// Full advisor I/O snapshot for one fan-out. Heavy; persisted-only
    /// (never whitelisted onto the wire). Opaque JSON keeps the payload
    /// shape free to evolve.
    MoaTurnTrace {
        preset: String,
        payload: Value,
    },
}

impl AgentTraceEvent {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::TurnStarted { .. } => "turn_started",
            Self::TurnStateEntered { .. } => "turn_state_entered",
            Self::TextEmitted { .. } => "text_emitted",
            Self::ToolCallStarted { .. } => "tool_call_started",
            Self::ToolCallCompleted { .. } => "tool_call_completed",
            Self::ToolSummary { .. } => "tool_summary",
            Self::TurnCompleted { .. } => "turn_completed",
            Self::SessionCompleted { .. } => "session_completed",
            Self::WorktreeCreated { .. } => "worktree_created",
            Self::WorktreeCleanedUp { .. } => "worktree_cleaned_up",
            Self::McpScopeAttached { .. } => "mcp_scope_attached",
            Self::McpScopeCleaned { .. } => "mcp_scope_cleaned",
            Self::ProviderUsage { .. } => "provider_usage",
            Self::ReactiveCompactionAttempted { .. } => "reactive_compaction_attempted",
            Self::VerifierVeto { .. } => "verifier_veto",
            Self::MoaAdvisor { .. } => "moa_advisor",
            Self::MoaAggregating { .. } => "moa_aggregating",
            Self::MoaAdvisorSpend { .. } => "moa_advisor_spend",
            Self::MoaTurnTrace { .. } => "moa_turn_trace",
        }
    }
}

impl StreamEvent {
    /// Create a new `ReasoningBlock` event
    pub fn reasoning_block(
        run_id: impl Into<String>,
        seq: u64,
        step_type: ReasoningStepType,
        label: impl Into<String>,
        content: impl Into<String>,
        is_final: bool,
    ) -> Self {
        Self::ReasoningBlock {
            run_id: run_id.into(),
            seq,
            step_type,
            label: label.into(),
            content: content.into(),
            confidence: None,
            is_final,
        }
    }

    /// Create a new `ReasoningBlock` event with confidence
    pub fn reasoning_block_with_confidence(
        run_id: impl Into<String>,
        seq: u64,
        step_type: ReasoningStepType,
        label: impl Into<String>,
        content: impl Into<String>,
        confidence: ConfidenceLevel,
        is_final: bool,
    ) -> Self {
        Self::ReasoningBlock {
            run_id: run_id.into(),
            seq,
            step_type,
            label: label.into(),
            content: content.into(),
            confidence: Some(confidence),
            is_final,
        }
    }

    /// Create a new `UncertaintySignal` event
    pub fn uncertainty_signal(
        run_id: impl Into<String>,
        seq: u64,
        uncertainty: impl Into<String>,
        suggested_action: UncertaintyAction,
    ) -> Self {
        Self::UncertaintySignal {
            run_id: run_id.into(),
            seq,
            uncertainty: uncertainty.into(),
            suggested_action,
        }
    }

    /// Create a new structured agent trace event
    pub fn agent_trace(run_id: impl Into<String>, seq: u64, event: AgentTraceEvent) -> Self {
        Self::AgentTrace {
            run_id: run_id.into(),
            seq,
            event,
        }
    }

    /// Get the `run_id` from any event variant
    #[must_use]
    pub fn run_id(&self) -> &str {
        match self {
            Self::RunAccepted { run_id, .. }
            | Self::Reasoning { run_id, .. }
            | Self::ToolStart { run_id, .. }
            | Self::ToolUpdate { run_id, .. }
            | Self::ToolEnd { run_id, .. }
            | Self::AgentTrace { run_id, .. }
            | Self::ResponseChunk { run_id, .. }
            | Self::RunComplete { run_id, .. }
            | Self::RunError { run_id, .. }
            | Self::AskUser { run_id, .. }
            | Self::ReasoningBlock { run_id, .. }
            | Self::UncertaintySignal { run_id, .. }
            | Self::RunRetrying { run_id, .. }
            | Self::ModelResolved { run_id, .. }
            | Self::ContextGauge { run_id, .. } => run_id,
        }
    }
}

/// Result of a tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: Option<String>,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl ToolResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: Some(output.into()),
            error: None,
            metadata: None,
        }
    }

    pub fn error(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: None,
            error: Some(error.into()),
            metadata: None,
        }
    }
}

/// Summary of a completed agent run.
///
/// The first four fields are the legacy shape — preserved verbatim so older
/// channels and persisted blobs keep deserializing. Every new field carries
/// `#[serde(default)]` plus a `skip_serializing_if` so legacy producers
/// (which omit them) round-trip cleanly and minimal blobs stay minimal.
///
/// Hermes-agent surfaces these signals on every turn; Aleph used to swallow
/// them. The harness already records every one (token breakdown via
/// `LoopTraceEvent::ProviderUsage`, per-tool duration via
/// `ToolCallCompleted`, terminate cause via `TerminateReason`) — this struct
/// is the renderable seam.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunSummary {
    pub total_tokens: u64,
    pub tool_calls: u32,
    pub loops: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_response: Option<String>,
    /// Wall-clock harness duration. Often duplicated with
    /// `RunComplete.total_duration_ms`; channels typically prefer the
    /// outer field but the summary carries it so persistence stays
    /// self-contained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Stable string form of `TerminateReason::as_static_str()` —
    /// `"completed"`, `"hit_max_iterations"`, … Channels that want richer
    /// formatting reconstruct the enum on the alephcore side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate_reason: Option<String>,
    /// Granular cap label that lives inside an umbrella variant.
    ///
    /// `terminate_reason` carries the static enum tag, which collapses
    /// every escalated budget exit into the single umbrella token
    /// `"budget_exhausted_partial_result"`. This field carries the
    /// underlying cap label (`"hit_max_iterations"` /
    /// `"context_budget_exhausted"` / `"max_output_tokens_exhausted"`)
    /// so cron carry-over / dashboards can show *which* budget was hit
    /// rather than just "some budget". `None` for every terminate
    /// reason where the static label is already granular.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate_detail: Option<String>,
    /// Per-component provider token usage. Granular complement to
    /// `total_tokens`; surfaces cache hit ratio + reasoning spend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_breakdown: Option<TokenBreakdownView>,
    /// Best-effort USD estimate. `None` when the pricing module had no
    /// rate for this provider/model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost_usd: Option<f64>,
    /// Confidence band on `estimated_cost_usd` — `"complete"`,
    /// `"partial_missing_price"`, `"unknown"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_status: Option<String>,
    /// One entry per tool invocation in run order. Empty when no tool ran
    /// or when the executor did not record a timeline.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_summaries: Vec<ToolSummaryItem>,
    /// Errors that surfaced during the run; one per failed tool call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ToolErrorItem>,
    /// Terminal state of the session's execution list (todo/plan), when one
    /// was active.
    ///
    /// Authoritative, for the same reason `tool_summaries` is: the live plan
    /// frames ride `tool_call_completed` on the deliberately-lossy
    /// `agent_trace` mirror (bounded mpsc + `try_send`), so a dropped frame
    /// would otherwise leave a renderer stuck on a stale checklist forever
    /// with no repair path. Consumers reconcile against this at `run_complete`
    /// exactly as they do for tool rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<crate::plan::PlanSnapshot>,
}

/// Lightweight view of `alephcore::orchestrator::dispatch::TokenBreakdown`
/// that lives in the protocol crate so channels can render without pulling
/// `alephcore`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenBreakdownView {
    pub input: u32,
    pub output: u32,
    pub cache_read: u32,
    pub cache_creation: u32,
    pub reasoning: u32,
}

/// Tool execution summary item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSummaryItem {
    pub tool_id: String,
    pub tool_name: String,
    pub emoji: String,
    pub duration_ms: u64,
    pub success: bool,
}

/// Tool error item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolErrorItem {
    pub tool_name: String,
    pub error: String,
    pub tool_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_result_success() {
        let result = ToolResult::success("output data");
        assert!(result.success);
        assert_eq!(result.output, Some("output data".to_string()));
        assert!(result.error.is_none());
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult::error("something went wrong");
        assert!(!result.success);
        assert!(result.output.is_none());
        assert_eq!(result.error, Some("something went wrong".to_string()));
    }

    #[test]
    fn run_retrying_deserializes_from_gateway_frame_shape() {
        // Wire-compat guard: this is the exact params shape the gateway's
        // `GatewayEventFrame::RunRetrying` serializes (tag = "type",
        // snake_case). If either side drifts, terminal clients go blind to
        // provider retries again.
        let params = serde_json::json!({
            "type": "run_retrying",
            "run_id": "run-9",
            "seq": 4,
            "provider": "anthropic",
            "attempt": 2,
            "max_attempts": 12,
            "reason": "connect timeout",
        });
        let event: StreamEvent = serde_json::from_value(params).unwrap();
        match &event {
            StreamEvent::RunRetrying {
                provider,
                attempt,
                max_attempts,
                ..
            } => {
                assert_eq!(provider, "anthropic");
                assert_eq!(*attempt, 2);
                assert_eq!(*max_attempts, 12);
            }
            other => panic!("expected RunRetrying, got {other:?}"),
        }
        assert_eq!(event.run_id(), "run-9");
    }

    #[test]
    fn model_resolved_deserializes_from_gateway_frame_shape() {
        // Wire-compat guard: exact params shape of the gateway's
        // `GatewayEventFrame::ModelResolved` (tag = "type", snake_case,
        // no seq field; model_info nests providers::health::ModelInfo).
        let params = serde_json::json!({
            "type": "model_resolved",
            "run_id": "run-3",
            "model_info": {
                "model": "claude-haiku-4-5-20251001",
                "provider": "anthropic",
                "is_fallback": true,
                "original_model": "claude-fable-5",
            },
        });
        let event: StreamEvent = serde_json::from_value(params).unwrap();
        match &event {
            StreamEvent::ModelResolved { model_info, .. } => {
                assert_eq!(model_info.model, "claude-haiku-4-5-20251001");
                assert_eq!(model_info.provider, "anthropic");
                assert!(model_info.is_fallback);
                assert_eq!(model_info.original_model.as_deref(), Some("claude-fable-5"));
            }
            other => panic!("expected ModelResolved, got {other:?}"),
        }
        assert_eq!(event.run_id(), "run-3");
    }

    #[test]
    fn context_gauge_deserializes_from_gateway_frame_shape() {
        // Wire-compat guard: exact params shape of the gateway's
        // `StreamEvent::ContextGauge` (src/gateway/event_emitter/types.rs),
        // tag = "type", snake_case. A drift here is what silently blinded the
        // TUI before this variant existed.
        let params = serde_json::json!({
            "type": "context_gauge",
            "run_id": "run-7",
            "seq": 4,
            "context_tokens": 12_345,
            "context_window": 200_000,
            "total_tokens": 15_000,
        });
        let event: StreamEvent = serde_json::from_value(params).unwrap();
        match &event {
            StreamEvent::ContextGauge {
                context_tokens,
                context_window,
                total_tokens,
                ..
            } => {
                assert_eq!(*context_tokens, 12_345);
                assert_eq!(*context_window, 200_000);
                assert_eq!(*total_tokens, 15_000);
            }
            other => panic!("expected ContextGauge, got {other:?}"),
        }
        assert_eq!(event.run_id(), "run-7");
    }

    #[test]
    fn test_reasoning_block_serialization() {
        let event = StreamEvent::reasoning_block(
            "run-123",
            1,
            ReasoningStepType::Analysis,
            "Analyzing options",
            "Comparing Redis vs in-memory cache",
            false,
        );

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("reasoning_block"));
        assert!(json.contains("analysis"));
        assert!(json.contains("Analyzing options"));
    }

    #[test]
    fn test_agent_trace_serialization() {
        let event = StreamEvent::agent_trace(
            "run-123",
            2,
            AgentTraceEvent::ToolCallCompleted {
                iteration: 3,
                call: AgentTraceToolCallEnd {
                    tool_id: "tool-1".to_string(),
                    tool_name: "read_file".to_string(),
                    input: serde_json::json!({"path": "README.md"}),
                    duration_ms: 12,
                },
                result: AgentTraceToolResult::Success {
                    output: serde_json::json!({"ok": true}),
                },
            },
        );

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("agent_trace"));
        assert!(json.contains("tool_call_completed"));
        assert!(json.contains("read_file"));
    }

    #[test]
    fn test_uncertainty_action_description() {
        assert!(UncertaintyAction::ProceedWithCaution
            .description()
            .contains("caution"));
        assert!(UncertaintyAction::AskForClarification
            .description()
            .contains("clarification"));
    }

    #[test]
    fn moa_event_wire_field_names_locked() {
        let advisor = AgentTraceEvent::MoaAdvisor {
            index: 1,
            count: 2,
            label: "p:m".into(),
            text: "t".into(),
            error: Some("boom".into()),
        };
        let v = serde_json::to_value(&advisor).unwrap();
        assert_eq!(v["kind"], "moa_advisor");
        for k in ["index", "count", "label", "text", "error"] {
            assert!(v.get(k).is_some(), "missing {k}");
        }

        let agg = AgentTraceEvent::MoaAggregating {
            aggregator: "p:m".into(),
            advisor_count: 2,
            cached: true,
        };
        let v = serde_json::to_value(&agg).unwrap();
        assert_eq!(v["kind"], "moa_aggregating");
        for k in ["aggregator", "advisor_count", "cached"] {
            assert!(v.get(k).is_some(), "missing {k}");
        }

        let spend = AgentTraceEvent::MoaAdvisorSpend {
            advisor_count: 2,
            billed_count: 1,
            input_tokens: 10,
            output_tokens: 5,
            cost_usd: Some(0.01),
        };
        let v = serde_json::to_value(&spend).unwrap();
        assert_eq!(v["kind"], "moa_advisor_spend");
        for k in [
            "advisor_count",
            "billed_count",
            "input_tokens",
            "output_tokens",
            "cost_usd",
        ] {
            assert!(v.get(k).is_some(), "missing {k}");
        }
    }
}
