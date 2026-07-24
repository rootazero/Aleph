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
/// Rust advantage over TypeScript: claude-code's equivalent event shapes are
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
    /// Per-call provider usage (Stage J-pre cache observability). `agent_id` is
    /// whoever spent the tokens: the run's `spec.agent`, or the `subagent_id`
    /// from within a spawned subagent — never the literal "root".
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
    /// Structural goal-loop watchdog vetoed the model's attempt to stop because
    /// its scratchpad still lists unchecked `- [ ]` items; the harness forced
    /// another iteration. `reason` is the pending-item list the verifier built.
    /// Pure structural signal — no LLM judgment (R7/R10). Mirrors the synthetic
    /// `[verifier veto]` message injected for the model, but surfaced to the
    /// user stream so the interception reason is not a black box.
    VerifierVeto { iteration: usize, reason: String },
    /// MoA advisor consultation result — one per advisor per fan-out
    /// (cache-MISS iterations only). Emitted by the `MoaProvider` facade
    /// through the run's TraceSink (MeteringProvider pattern — zero harness
    /// logic; this enum is the carrier, not the brain).
    /// `count == 0` is the activation-failure form: MoA could not engage for
    /// this run (preset missing / slot unresolvable) and `error` says why.
    MoaAdvisor {
        index: usize,
        count: usize,
        /// `provider:model` of the advisor slot.
        label: String,
        text: String,
        /// Structural failure reason (timeout / provider error); `None` on
        /// success. The guidance-block `[failed:]` note is the display twin.
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
    /// Summed advisor spend for one fan-out. Priced per-advisor at each
    /// advisor's OWN model rate; kept out of `ProviderResponse.usage` so the
    /// context gauge stays honest (spec §8). `advisor_count` = advisors
    /// consulted this fan-out; `billed_count` = advisors that returned usage
    /// (denominator for cost-per-advisor math).
    MoaAdvisorSpend {
        advisor_count: usize,
        billed_count: usize,
        input_tokens: u32,
        output_tokens: u32,
        cost_usd: Option<f64>,
    },
    /// Full advisor I/O snapshot for one fan-out. Heavy; emitted only when
    /// `[moa] save_traces = true`; persisted-only (never whitelisted onto
    /// the wire). Opaque JSON keeps the payload shape free to evolve.
    MoaTurnTrace { preset: String, payload: Value },
}

/// Kind of text stream. Live incremental text travels through the
/// `HarnessCallback::on_delta` seam, not the trace — so the loop only ever
/// emits authoritative `Final` text here. (The protocol-side
/// `AgentTraceTextKind::Intermediate` survives for stored legacy blobs.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopTraceTextKind {
    /// Final text (no more coming)
    Final,
}

/// State entered during turn execution. Only the two phases the loop
/// actually has — Think → Act (R10 dumb loop). The protocol-side
/// `AgentTraceState` keeps its wider set for stored legacy blobs.
///
/// `#[non_exhaustive]`: pre-emptive forward-compat for new turn states
/// (e.g. future `Compact`, `Verify`, `Recover` sub-phases). External
/// trace consumers must add wildcard arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LoopTraceState {
    Think,
    Act,
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
