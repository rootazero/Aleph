//! Trace events for the agent loop
//!
//! Provides structured tracing events emitted during agent loop execution
//! for debugging, logging, and event bus distribution.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Event emitted during agent loop execution.
///
/// `#[non_exhaustive]`: trace events grow over time as new observability
/// hooks are added (e.g. round 2 didn't add any but historically the enum
/// has grown from 6 → 14 variants). Downstream trace-sink consumers in
/// other crates must therefore include a wildcard arm; this annotation
/// makes that requirement compile-time enforced.
///
/// Rust 优势 over TypeScript: claude-code's equivalent event shapes are
/// open-ended structural records that silently accept new fields. The
/// `#[non_exhaustive]` annotation gives Aleph compile-time forward-compat
/// on the trace ABI without paying any runtime cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
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
        /// Precise loop-exit cause. `None` for trace blobs written by older
        /// Aleph versions; current emitter always populates it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminate_reason: Option<crate::orchestrator::dispatch::TerminateReason>,
        /// Wall-clock harness duration. `None` on legacy trace blobs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        /// Per-component token breakdown. `None` on legacy trace blobs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_breakdown: Option<crate::orchestrator::dispatch::TokenBreakdown>,
        /// Tool invocation timeline. Empty by default so the JSON shape is
        /// stable across producer versions.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_timeline: Vec<crate::orchestrator::dispatch::ToolInvocation>,
    },
    /// Subagent worktree isolation primitive created (P3 Stage H).
    WorktreeCreated { path: std::path::PathBuf },
    /// Subagent worktree cleaned up; `leaked = true` means cleanup was via
    /// Drop safety-net rather than explicit `cleanup()` (P3 Stage H).
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
    /// Per-agent MCP scope cleaned up; `leaked = true` means cleanup was via
    /// Drop safety-net rather than explicit `shutdown()` (P3 Stage I).
    McpScopeCleaned { agent_id: String, leaked: bool },
    /// Per-call provider usage (Stage J-pre cache observability).
    /// `agent_id` is "root" for the top-level harness or the subagent_id
    /// when emitted from within a spawned subagent.
    ProviderUsage {
        agent_id: String,
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: Option<u32>,
        cache_creation_tokens: Option<u32>,
        thinking_tokens: Option<u32>,
    },
    /// Harness attempted reactive compaction in response to a provider
    /// `prompt_too_long` / 413 error and retried the LLM call once with a
    /// summarised history. `token_gap` is the reported overflow when the
    /// provider error string carried one (often `None` for non-Anthropic
    /// providers). `succeeded` is `true` when the retry produced a usable
    /// response; `false` when the compactor was not wired, the rescue
    /// attempt cap was hit, or the retried call still errored. Pairs with
    /// [`crate::orchestrator::dispatch::TerminateReason::ReactiveCompactExhausted`].
    ReactiveCompactionAttempted {
        token_gap: Option<usize>,
        succeeded: bool,
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

/// State entered during turn execution.
///
/// `#[non_exhaustive]`: pre-emptive forward-compat for new turn states
/// (e.g. future `Compact`, `Verify`, `Recover` sub-phases). External
/// trace consumers must add wildcard arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LoopTraceState {
    Prepare,
    Think,
    Resolve,
    Act,
    Finalize,
}

/// Outcome of a turn.
///
/// `#[non_exhaustive]`: see [`LoopTraceState`] for rationale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LoopTraceTurnOutcome {
    Continue,
    Stop,
    HitLimit,
    Cancelled,
}

/// Outcome of a session.
///
/// `#[non_exhaustive]`: see [`LoopTraceState`] for rationale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
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
                terminate_reason,
                duration_ms,
                token_breakdown,
                tool_timeline,
            } => aleph_protocol::AgentTraceEvent::SessionCompleted {
                outcome: outcome.into(),
                iterations,
                tool_calls_made,
                total_tokens,
                hit_limit,
                final_text,
                // Re-serialize through JSON so the protocol crate stays
                // independent of `alephcore` types — keeps the crate-edge
                // boundary clean (R3 core minimalism).
                terminate_reason: terminate_reason
                    .as_ref()
                    .and_then(|r| serde_json::to_value(r).ok()),
                duration_ms,
                token_breakdown: token_breakdown
                    .as_ref()
                    .and_then(|b| serde_json::to_value(b).ok()),
                tool_timeline: tool_timeline
                    .iter()
                    .filter_map(|inv| serde_json::to_value(inv).ok())
                    .collect(),
            },
            LoopTraceEvent::WorktreeCreated { path } => {
                aleph_protocol::AgentTraceEvent::WorktreeCreated { path }
            }
            LoopTraceEvent::WorktreeCleanedUp { path, leaked } => {
                aleph_protocol::AgentTraceEvent::WorktreeCleanedUp { path, leaked }
            }
            LoopTraceEvent::McpScopeAttached {
                agent_id,
                references,
                inline_count,
            } => aleph_protocol::AgentTraceEvent::McpScopeAttached {
                agent_id,
                references,
                inline_count,
            },
            LoopTraceEvent::McpScopeCleaned { agent_id, leaked } => {
                aleph_protocol::AgentTraceEvent::McpScopeCleaned { agent_id, leaked }
            }
            LoopTraceEvent::ProviderUsage {
                agent_id,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                thinking_tokens,
            } => aleph_protocol::AgentTraceEvent::ProviderUsage {
                agent_id,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                thinking_tokens,
            },
            LoopTraceEvent::ReactiveCompactionAttempted {
                token_gap,
                succeeded,
            } => aleph_protocol::AgentTraceEvent::ReactiveCompactionAttempted {
                token_gap,
                succeeded,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_usage_serializes_with_agent_id_and_token_split() {
        let event = LoopTraceEvent::ProviderUsage {
            agent_id: "subagent-foo".into(),
            input_tokens: 250,
            output_tokens: 75,
            cache_read_tokens: Some(100),
            cache_creation_tokens: Some(30),
            thinking_tokens: None,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains(r#""type":"provider_usage""#));
        assert!(json.contains(r#""agent_id":"subagent-foo""#));
        assert!(json.contains(r#""cache_creation_tokens":30"#));
        assert!(json.contains(r#""cache_read_tokens":100"#));
    }

    #[test]
    fn mcp_scope_attached_serializes_with_agent_id_and_counts() {
        let event = LoopTraceEvent::McpScopeAttached {
            agent_id: "git-research".into(),
            references: vec!["github".into()],
            inline_count: 2,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains(r#""type":"mcp_scope_attached""#));
        assert!(json.contains(r#""agent_id":"git-research""#));
        assert!(json.contains(r#""inline_count":2"#));
    }

    #[test]
    fn mcp_scope_cleaned_serializes_with_leaked_flag() {
        let event = LoopTraceEvent::McpScopeCleaned {
            agent_id: "git-research".into(),
            leaked: true,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains(r#""type":"mcp_scope_cleaned""#));
        assert!(json.contains(r#""leaked":true"#));
    }
}

#[cfg(test)]
mod p3_stage_h_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn worktree_created_serializes_with_path() {
        let event = LoopTraceEvent::WorktreeCreated {
            path: PathBuf::from("/tmp/aleph-subagent-x"),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains(r#""type":"worktree_created""#));
        assert!(json.contains(r#""path":"/tmp/aleph-subagent-x""#));
    }

    #[test]
    fn worktree_cleaned_up_serializes_with_leaked_flag() {
        let event = LoopTraceEvent::WorktreeCleanedUp {
            path: PathBuf::from("/tmp/aleph-subagent-y"),
            leaked: true,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains(r#""type":"worktree_cleaned_up""#));
        assert!(json.contains(r#""leaked":true"#));
    }
}
