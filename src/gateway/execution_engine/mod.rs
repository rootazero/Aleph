//! Execution Engine
//!
//! Bridges the Gateway with the `AgentLoop`.
//! Manages run lifecycle, emits events, and handles cancellation.
//!
//! # Module structure
//!
//! - `engine` - Full `ExecutionEngine<P,R>` with `AgentLoop` integration
//! - `simple` - `SimpleExecutionEngine` for when providers/tools are not available

mod adapter;
mod agent_trace_emit_sink;
mod callback;
mod concurrency;
pub(crate) mod concurrency_handle;
mod deadline;
mod engine;
pub(crate) mod event_drain;
mod execute;
mod fast_path;
mod gate;
mod goal_continuation;
pub mod goal_wait;
pub mod helpers;
mod history;
pub mod markdown_skill_refresh;
mod orchestrator;
mod persistence;
mod run_loop;
mod scratchpad_progress_sink;
mod session_run_registry;
mod simple;
mod slash_command;
mod steering;
mod tool_refresh;
mod tool_service_builder;
pub(crate) mod topic;
mod trace_sink_adapter;
mod turn_permissions;
mod turn_thinking;
mod unattended_redacting_sink;

#[cfg(test)]
mod tests;

#[allow(unused_imports)] // wired into run_loop.rs in this commit
pub(crate) use agent_trace_emit_sink::AgentTraceEmitSink;
pub use concurrency::ConcurrencySnapshot;
pub use engine::{ContinuationDeps, ExecutionEngine};
#[allow(unused_imports)] // wired into run_loop.rs in this commit
pub(crate) use scratchpad_progress_sink::ScratchpadProgressSink;
pub use simple::SimpleExecutionEngine;
pub(crate) use slash_command::{is_continuation_driven_slash, is_shorthand_alias};
#[allow(unused_imports)] // wired into run_loop.rs in this commit
pub(crate) use tool_service_builder::build_request_tool_service;
pub use tool_service_builder::set_config_approval_requester;
pub use tool_service_builder::set_confirmation_requester;
pub use tool_service_builder::set_mcp_tool_registry;
#[allow(unused_imports)] // wired into run_loop.rs in this commit
pub(crate) use trace_sink_adapter::GatewayTraceSink;
#[allow(unused_imports)] // wired into run_loop.rs in this commit
pub(crate) use unattended_redacting_sink::UnattendedRedactingSink;

use crate::gateway::media::PendingMedia;
use crate::sync_primitives::{AtomicU32, AtomicU64, Ordering};
use std::collections::HashMap;
use std::path::PathBuf;

use tokio::sync::mpsc;

use super::router::SessionKey;

/// Configuration for the execution engine
#[derive(Debug, Clone)]
pub struct ExecutionEngineConfig {
    /// Global cap on concurrently-executing runs across all sessions/agents.
    /// Held for the run's lifetime by `ConcurrencyLimiter` (audit 1.4).
    pub max_runs_global: usize,
    /// Per-agent sub-cap so one busy agent can't monopolize all global slots
    /// (audit C4). per-session is hard-capped at 1 by `SessionRunRegistry`.
    pub max_runs_per_agent: usize,
    /// Default timeout for runs (seconds)
    pub default_timeout_secs: u64,
    /// Enable detailed tracing
    pub enable_tracing: bool,
    /// Mid-loop steering: when a message arrives for a session whose loop is
    /// already running, inject it into the live event log (the running loop
    /// consumes it at the next turn boundary) instead of rejecting with
    /// `AgentBusy`. Disable to restore the legacy busy/retry behaviour.
    pub mid_turn_steering: bool,
    /// R5 progress push: when a run is bound to a user channel, mirror
    /// scratchpad progress + watchdog-boundary events to that channel so
    /// headless / background runs aren't a black box. Pure I/O side-channel
    /// (see `scratchpad_progress_sink`). Default off — opt in via
    /// `[execution] progress_push`.
    pub scratchpad_progress_push: bool,
    /// Tools kept at full schema (progressive tool disclosure). Sourced from
    /// `[tools] core`. `["*"]`/empty disables collapsing (escape hatch).
    pub core_tools: Vec<String>,
    /// Mirror of `[tools] truncate_tool_descriptions`.
    pub truncate_tool_descriptions: bool,
    /// Mirror of `[tools] defer_mcp_tools`. Gates the deferred exposure tier +
    /// `tool_search` registration at the per-request seam.
    pub defer_mcp_tools: bool,
}

impl Default for ExecutionEngineConfig {
    fn default() -> Self {
        Self {
            max_runs_global: 8,
            max_runs_per_agent: 3,
            default_timeout_secs: 172_800,
            enable_tracing: true,
            mid_turn_steering: true,
            scratchpad_progress_push: false,
            core_tools: crate::config::types::tools::default_core_tools(),
            truncate_tool_descriptions: false,
            defer_mcp_tools: false,
        }
    }
}

/// Busy-input policy: what to do when a message arrives for a session whose
/// Think→Act loop is already running. Selected **explicitly** per channel
/// (R7 — never inferred from message content), defaulting to
/// [`BusyInputMode::Steer`] so every existing path stays byte-identical until an
/// operator opts a channel in.
///
/// This is the policy knob the reference harnesses all expose (hermes
/// `HERMES_GATEWAY_BUSY_INPUT_MODE`, openclaw `QueueMode`, Pi `streamingBehavior`);
/// Aleph previously hardcoded `Steer`. Pure scaffolding — the decision is a
/// mechanical metadata lookup, the harness loop is untouched (R10, Future-Proof ✓).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BusyInputMode {
    /// Inject the new message into the live event log; the running loop consumes
    /// it at its next turn boundary and course-corrects without losing progress.
    /// The original behaviour, and the safe default.
    #[default]
    Steer,
    /// Cancel the running sibling on this session, then let the inbound router's
    /// FIFO busy queue restart the message as a fresh run once the slot frees.
    /// The new message supersedes the in-flight task, picking up its full
    /// (interrupted) context from the session log — the cancelled loop's
    /// `RunFinished{Cancelled}` marker is replayed by the prompt builder as an
    /// interruption note, so the successor run knows the prior task was cut
    /// short rather than completed. Reuses [`ExecutionEngine::cancel`] and the
    /// `AgentBusy` delivery path; no new dispatch machinery.
    Interrupt,
    /// Never disturb the running task: no mid-loop injection, no cancellation.
    /// The message waits in the inbound router's per-session FIFO busy queue and
    /// is delivered as a fresh run once the current one finishes — the
    /// follow-up lane every reference harness exposes (openclaw `followup`,
    /// hermes `queue`, Pi `followUp`, `OpenSquilla` `followup`). Bounded wait:
    /// past the queue deadline the user is notified instead of the message
    /// being silently dropped.
    Queue,
}

/// Metadata key carrying the per-run [`BusyInputMode`] wire string
/// (`"steer"` / `"interrupt"` / `"queue"`). Stamped by the inbound router from
/// the channel's `ChannelPolicyConfig`; absent on Panel/CLI paths (which
/// default to `Steer`).
pub const BUSY_INPUT_MODE_KEY: &str = "busy_input_mode";

/// Metadata key carrying the originating channel's tool permission override as
/// a JSON-serialized `ToolPermissionsConfig`. Stamped by the inbound router
/// from the channel's `ChannelPolicyConfig` (`tool_permissions` block); absent
/// for unconfigured channels and Panel/CLI/cron paths. `run_loop` merges it as
/// the third (most specific) layer over global + agent permissions.
pub const CHANNEL_TOOL_PERMISSIONS_KEY: &str = "channel_tool_permissions";

/// Metadata key marking a run that no human is waiting on and whose approval
/// prompts nobody can answer.
///
/// Two consumers, both in `run_loop::inner`: the per-run `ScopedToolService` is
/// built `.with_unattended(true)` — confirm-gated tools then fail CLOSED
/// (immediate deny with an actionable hint, `tools/scoped/dispatch.rs`) instead
/// of parking on an approval card until the 120 s timeout — and the trace sink
/// is wrapped in `UnattendedRedactingSink`.
///
/// Stamped by every producer of a headless run: goal/loop continuations
/// (`execute::spawn_continuation_run`), heartbeat, A2A delegations, and cron
/// jobs with no origin channel. A run that CAN reach a human (a channel-bound
/// cron, a team member run resolving to the Panel operator) must NOT carry it —
/// the marker would auto-deny a working human-in-the-loop path.
pub const UNATTENDED_KEY: &str = "unattended";

impl BusyInputMode {
    /// Wire string stored in run metadata. Inverse of [`BusyInputMode::from_wire`].
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Steer => "steer",
            Self::Interrupt => "interrupt",
            Self::Queue => "queue",
        }
    }

    /// Parse from the optional metadata wire string. Any unknown / absent value
    /// falls back to the safe [`BusyInputMode::Steer`] default.
    #[must_use]
    pub fn from_wire(s: Option<&str>) -> Self {
        match s {
            Some("interrupt") => Self::Interrupt,
            Some("queue") => Self::Queue,
            _ => Self::Steer,
        }
    }

    /// Resolve the mode from a run's metadata map.
    #[must_use]
    pub fn from_metadata(metadata: &HashMap<String, String>) -> Self {
        Self::from_wire(metadata.get(BUSY_INPUT_MODE_KEY).map(String::as_str))
    }
}

/// A run request
#[derive(Clone)]
pub struct RunRequest {
    /// Unique run ID
    pub run_id: String,
    /// Input message
    pub input: String,
    /// Session key for context
    pub session_key: SessionKey,
    /// Optional timeout override
    pub timeout_secs: Option<u64>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
    /// Attachments from inbound message (images, audio, documents)
    pub attachments: Vec<crate::gateway::channel::Attachment>,
    /// Shared pending media buffer (for media attachment delivery)
    pub pending_media: PendingMedia,
    /// G2 — per-run sandbox override. `None` defers to the orchestrator's
    /// sandbox factory (production default); `Some(sandbox)` short-circuits
    /// the factory and is used by the team dispatcher to wrap each member
    /// task in an isolated git worktree.
    pub sandbox_override: Option<std::sync::Arc<dyn crate::sandbox::Sandbox>>,
    /// Per-run workspace override (project mode). When `Some`, this path
    /// replaces `agent.workspace()` as the effective working directory
    /// for the run — used for `ToolContext`, the default cwd of shell
    /// tools, and project-local file/skill discovery
    /// (`<root>/AGENTS.md`, `<root>/CLAUDE.md`, `<root>/.claude/skills`,
    /// `<root>/.aleph/skills`).
    ///
    /// `None` keeps the legacy behaviour of running inside
    /// `~/.aleph/workspaces/{agent_id}/`. The path is **not** validated by
    /// the engine; the gateway handler that constructs `RunRequest` is
    /// responsible for trust + existence checks.
    pub workspace_override: Option<PathBuf>,
    /// D2: per-run Think→Act iteration cap override. When `Some(n>0)`, this
    /// wins over both `FlowOverrides.max_iterations` and the boot-time
    /// `[execution] max_iterations` default. Cron-driven runs set this from
    /// `CronConfig::default_max_iterations` so a single misbehaving job
    /// can't burn the much-larger global cap (default 1000) before the
    /// wall-clock timeout fires. `None` falls through to the legacy
    /// resolution chain.
    pub max_iterations_override: Option<u32>,
    /// Chat-window picker — per-turn model override. When `Some`, the
    /// `run_loop` short-circuits `provider_registry.resolve_with_fallback`
    /// and pins the requested (provider, model) pair (Qualified) or the
    /// requested model with auto-resolved provider (Raw). `None` keeps the
    /// agent's configured default + fallback chain.
    pub model_override: Option<crate::gateway::model_override::ModelOverride>,
}

impl std::fmt::Debug for RunRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunRequest")
            .field("run_id", &self.run_id)
            .field("input", &self.input)
            .field("session_key", &self.session_key)
            .field("timeout_secs", &self.timeout_secs)
            .field("metadata", &self.metadata)
            .field("attachments", &self.attachments)
            .field(
                "sandbox_override",
                &self.sandbox_override.as_ref().map(|_| "<dyn Sandbox>"),
            )
            .field("workspace_override", &self.workspace_override)
            .finish()
    }
}

/// Run state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    /// Run is executing
    Running,
    /// Run completed successfully
    Completed,
    /// Run was cancelled
    Cancelled,
    /// Run failed
    Failed { error: String },
}

/// Run status information
#[derive(Debug, Clone)]
pub struct RunStatus {
    pub run_id: String,
    pub state: RunState,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub steps_completed: u32,
    pub current_tool: Option<String>,
}

/// Internal run tracking
pub(crate) struct ActiveRun {
    pub(crate) request: RunRequest,
    pub(crate) state: RunState,
    pub(crate) started_at: chrono::DateTime<chrono::Utc>,
    pub(crate) completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) steps_completed: u32,
    pub(crate) current_tool: Option<String>,
    pub(crate) cancel_tx: Option<mpsc::Sender<()>>,
    pub(crate) seq_counter: AtomicU64,
    pub(crate) chunk_counter: AtomicU32,
    /// Sticky Interrupt-demotion marker (`gate.rs`): once an `Interrupt` was
    /// demoted to the follow-up queue to protect this run's sub-agent fan-out,
    /// every later poll of that queued message keeps demoting — even after the
    /// last child exits — so the run's synthesis turn survives. Atomic so the
    /// gate can stamp it under the `active_runs` READ lock; dies with the run.
    pub(crate) demote_protected: std::sync::atomic::AtomicBool,
}

impl ActiveRun {
    pub(crate) fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }

    pub(crate) fn next_chunk(&self) -> u32 {
        self.chunk_counter.fetch_add(1, Ordering::SeqCst)
    }
}

/// Execution errors
#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("Agent is busy: {0}")]
    AgentBusy(String),

    #[error("Run not found: {0}")]
    RunNotFound(String),

    #[error("Run is not active: {0}")]
    RunNotActive(String),

    #[error("Run was cancelled")]
    Cancelled,

    #[error("Run timed out")]
    Timeout,

    #[error("Execution failed: {0}")]
    Failed(String),

    #[error("Command requires LLM processing: {reason}")]
    Fallthrough { reason: String },

    #[error("orchestrator: {0}")]
    Orchestrator(String),
}

impl ExecutionError {
    /// Map this error into a user-facing receipt: a stable machine `code` plus a
    /// short, non-leaky message telling the user *whether retrying is worthwhile*
    /// (rate-limited / unreachable → yes, soon) without exposing the raw internal
    /// error chain.
    ///
    /// **Single source of truth** for user-channel error presentation. Both the
    /// in-engine `RunError` emit ([`execute`]) and the gateway RPC handlers
    /// (`agent.run` / `chat.send` in the aleph-server bin) route through it, so
    /// the flattened internal chain (e.g. `"Execution failed: flow: internal
    /// dispatch error: harness: llm error: Rate limit error: Anthropic API rate
    /// limited (429)..."`) never reaches the Panel. `ExecutionError` is `pub` and
    /// this method must stay `pub` because the leak site lives in the bin crate,
    /// which cannot reach the (private) presentation helpers. The typed error is
    /// unchanged for internal callers; only the presentation string is derived.
    #[must_use]
    pub fn user_receipt(&self) -> (&'static str, String) {
        match self {
            Self::Timeout => (
                "TIMEOUT",
                "任务超时未完成。请重试，或把任务拆小一些再发给我。".to_string(),
            ),
            Self::Cancelled => ("CANCELLED", "任务已取消。".to_string()),
            Self::AgentBusy(_) => ("AGENT_BUSY", "当前 Agent 正忙，请稍后再试。".to_string()),
            // The only string-carrying variant; classify by signature so a
            // provider rate-limit / network outage reads as transient.
            Self::Failed(msg) => classify_failed(msg),
            // Routing / lookup errors the user cannot act on — keep generic and
            // never echo the raw internal string.
            Self::RunNotFound(_)
            | Self::RunNotActive(_)
            | Self::Fallthrough { .. }
            | Self::Orchestrator(_) => ("FAILED", "任务执行失败，请重试。".to_string()),
        }
    }
}

/// Classify a [`ExecutionError::Failed`] payload. The message originates from the
/// gateway dispatch loop after failover/retries are exhausted, so a rate-limit or
/// network signature here means *every* provider/model in the chain was tried and
/// the failure is genuinely transient — worth telling the user to retry.
fn classify_failed(msg: &str) -> (&'static str, String) {
    if is_rate_limited(msg) {
        (
            "RATE_LIMITED",
            "模型服务商当前限流（请求过于频繁），且备用通道也已用尽。稍等片刻后重试即可。"
                .to_string(),
        )
    } else if is_unreachable(msg) {
        (
            "PROVIDERS_UNREACHABLE",
            "无法连接到模型 / 搜索服务（网络不可达或服务暂时中断），所有可用通道均已尝试。请检查网络后重试。"
                .to_string(),
        )
    } else {
        ("FAILED", "任务执行失败，请重试。".to_string())
    }
}

fn is_rate_limited(msg: &str) -> bool {
    msg.contains("429")
        || msg.contains("rate limit")
        || msg.contains("Rate limit")
        || msg.contains("rate_limit")
        || msg.contains("receiving too many requests")
}

fn is_unreachable(msg: &str) -> bool {
    msg.contains("Network error")
        || msg.contains("error sending request")
        || msg.contains("connection")
        || msg.contains("Connection")
        || msg.contains("dns")
        || msg.contains("timed out")
        || msg.contains("502")
        || msg.contains("503")
}

#[cfg(test)]
mod user_receipt_tests {
    use super::ExecutionError;

    #[test]
    fn rate_limit_failure_reads_as_retryable() {
        let e = ExecutionError::Failed(
            "flow: internal dispatch error: harness: llm error: Rate limit error: \
             Anthropic API rate limited (429): receiving too many requests"
                .to_string(),
        );
        let (code, message) = e.user_receipt();
        assert_eq!(code, "RATE_LIMITED");
        assert!(message.contains("限流"));
        // The raw internal chain must not leak to the user.
        assert!(!message.contains("dispatch error"));
    }

    #[test]
    fn network_outage_reads_as_unreachable() {
        let e = ExecutionError::Failed(
            "provider kimi-for-coding transient: Network error: error sending request \
             for url (https://api.kimi.com/coding/v1/messages)"
                .to_string(),
        );
        let (code, message) = e.user_receipt();
        assert_eq!(code, "PROVIDERS_UNREACHABLE");
        assert!(message.contains("网络"));
    }

    #[test]
    fn rate_limit_takes_precedence_over_network() {
        // A 429 that also mentions a url should still classify as rate-limited.
        let e = ExecutionError::Failed(
            "429 rate limit on error sending request for url (x)".to_string(),
        );
        assert_eq!(e.user_receipt().0, "RATE_LIMITED");
    }

    #[test]
    fn unclassified_failed_stays_generic() {
        let e = ExecutionError::Failed("some opaque internal failure".to_string());
        assert_eq!(e.user_receipt().0, "FAILED");
    }

    #[test]
    fn timeout_and_cancel_keep_their_codes() {
        assert_eq!(ExecutionError::Timeout.user_receipt().0, "TIMEOUT");
        assert_eq!(ExecutionError::Cancelled.user_receipt().0, "CANCELLED");
    }

    #[test]
    fn agent_busy_and_routing_errors_never_leak_raw() {
        // AgentBusy is the admit_run early-return path surfaced by the bin-crate
        // RPC handlers; routing errors carry internal ids. Neither may echo raw.
        let busy = ExecutionError::AgentBusy("run 7f3a already active".to_string());
        let (code, message) = busy.user_receipt();
        assert_eq!(code, "AGENT_BUSY");
        assert!(!message.contains("7f3a"));

        let routing = ExecutionError::RunNotFound("internal-run-id-42".to_string());
        let (code, message) = routing.user_receipt();
        assert_eq!(code, "FAILED");
        assert!(!message.contains("internal-run-id-42"));
    }
}
