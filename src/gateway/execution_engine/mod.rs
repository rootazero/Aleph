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
pub mod markdown_skill_tools;
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
mod turn_memory;
mod turn_mode;
mod turn_model;
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
pub(crate) use steering::wake_lane_if_burst_drained;
#[allow(unused_imports)] // wired into run_loop.rs in this commit
pub(crate) use tool_service_builder::build_request_tool_service;
pub use tool_service_builder::set_config_approval_requester;
pub use tool_service_builder::set_confirmation_requester;
pub use tool_service_builder::set_mcp_tool_registry;
#[allow(unused_imports)] // wired into run_loop.rs in this commit
pub(crate) use trace_sink_adapter::GatewayTraceSink;
#[allow(unused_imports)] // wired into run_loop.rs in this commit
pub(crate) use unattended_redacting_sink::UnattendedRedactingSink;

use crate::gateway::i18n::{Locale, Msg, ReceiptKind};
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
    /// Backpressure bound on un-consumed steering messages a single run may
    /// accumulate (`[execution] max_pending_steering`). Past the cap an
    /// injection is refused so the busy wait lane redelivers once the burst
    /// drains — backpressure, never a drop.
    pub max_pending_steering: usize,
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
            max_pending_steering: steering::MAX_PENDING_STEERING,
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

/// Metadata key carrying the human who wrote THIS turn's message
/// (`users.user_id`), stamped by `handlers::agent::build_run_request` from the
/// authenticated caller.
///
/// Distinct from the scope stamp (`scope::OWNER_META_KEY`) on purpose, and a
/// project room is exactly where the two diverge: every run in a room carries
/// the ROOM's attribution — that is what puts each member's memory writes in
/// the shared partition — so the scope owner names the room's owner, not
/// whoever is typing. Reading the author off the scope stamp would make every
/// member of a room look like its owner.
///
/// Two consumers, and both read it out of this map rather than re-deriving it:
/// the shared-room busy-lane rule ([`BusyInputMode::for_shared_room`]), and the
/// speaker label the prompt renders for a multi-human room —
/// `scope::room_author_from_metadata` for the engines that hold the request,
/// `run_loop::with_request_scope` → `scope::ambient_room_author` for the main
/// path's session seeder, which holds neither the request nor `CALLER_USER`.
pub const AUTHOR_USER_KEY: &str = "author_user_id";

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

    /// Downgrade to [`Queue`] when this turn would disturb SOMEBODY ELSE's
    /// in-flight run in a shared project room (P2, spec §10).
    ///
    /// `Steer` and `Interrupt` are authority over your own turn: one injects
    /// into a loop you started, the other cancels it. Neither is authority over
    /// a room-mate's turn — in a room, applying them across authors would let
    /// any member silently redirect or kill another member's work, and the knob
    /// that grants it is a personal preference nobody else consented to.
    ///
    /// Deliberately narrow, and each condition earns its place:
    ///
    /// - **Only in a project scope.** A personal session has one human by
    ///   construction, so the rule can never fire there; a room is the only
    ///   place two authors share one transcript.
    /// - **Only when the authors differ.** One person sending two messages in a
    ///   row keeps `Steer` — that is the coalescing the queue auto-drain
    ///   depends on, and breaking it here would look exactly like the
    ///   `mark_admitted` bug (`Steer` silently degrading to `Queue`) while
    ///   having a completely different cause.
    /// - **Unknown author reads as "not the same person".** An unstamped
    ///   incoming turn against a stamped running one queues, rather than
    ///   assuming they match.
    #[must_use]
    pub fn for_shared_room(
        self,
        incoming: &HashMap<String, String>,
        running: &HashMap<String, String>,
    ) -> Self {
        let in_a_room = incoming
            .get(crate::scope::SCOPE_META_KEY)
            .and_then(|s| crate::scope::ScopeId::parse(s))
            .is_some_and(|s| matches!(s, crate::scope::ScopeId::Project(_)));
        if !in_a_room {
            return self;
        }
        let same_author = match (incoming.get(AUTHOR_USER_KEY), running.get(AUTHOR_USER_KEY)) {
            (Some(a), Some(b)) => a == b,
            // Neither turn names an author: an unrestricted/internal producer
            // on both sides, which is the pre-P2 single-writer world.
            (None, None) => true,
            _ => false,
        };
        if same_author {
            self
        } else {
            Self::Queue
        }
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
    /// The same moment as [`Self::started_at`], on the monotonic clock.
    ///
    /// Not redundant: `started_at` is a wall-clock value that exists to be
    /// *reported* (`RunStatus`, traces, the Panel), and the busy-input gate has
    /// to *compare* it against `busy_queue::waiting_since` to decide whether an
    /// interrupt-mode message is superseding a run it ever saw. Comparing wall
    /// clocks across a clock step would flip that decision silently, and the
    /// wrong answer is "cancel a run this message never knew about".
    pub(crate) admitted_at: std::time::Instant,
    pub(crate) completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) steps_completed: u32,
    pub(crate) current_tool: Option<String>,
    pub(crate) cancel_tx: Option<mpsc::Sender<()>>,
    pub(crate) seq_counter: AtomicU64,
    pub(crate) chunk_counter: AtomicU32,
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
    /// **Single source of truth** for user-facing error presentation. The
    /// in-engine `RunError` emit ([`execute`]), the gateway RPC handlers
    /// (`agent.run` / `chat.send` in the aleph-server bin) and the inbound
    /// router's channel error reply all route through it, so the flattened
    /// internal chain (e.g. `"Execution failed: flow: internal dispatch error:
    /// harness: llm error: Rate limit error: Anthropic API rate limited
    /// (429)..."`) never reaches a user surface. `ExecutionError` is `pub` and
    /// this method must stay `pub` because one call site lives in the bin
    /// crate. The typed error is unchanged for internal callers; only the
    /// presentation string is derived.
    ///
    /// Classification and wording both live in [`crate::gateway::i18n`]
    /// ([`ReceiptKind`] + [`Msg::ErrReceipt`]) so the text is localized — this
    /// method previously hardcoded Chinese and carried a *second*, subtly
    /// different classifier next to `i18n::format_execution_error`.
    #[must_use]
    pub fn user_receipt(&self, locale: Locale) -> (&'static str, String) {
        let kind = self.receipt_kind();
        (
            kind.code(),
            crate::gateway::i18n::t(Msg::ErrReceipt(kind), locale),
        )
    }

    /// The user-facing bucket this error falls into. Split out from
    /// [`Self::user_receipt`] so callers that only need the machine code (or
    /// want to branch on the bucket) don't render a string.
    #[must_use]
    pub fn receipt_kind(&self) -> ReceiptKind {
        match self {
            Self::Timeout => ReceiptKind::Timeout,
            Self::Cancelled => ReceiptKind::Cancelled,
            Self::AgentBusy(_) => ReceiptKind::AgentBusy,
            // The only string-carrying variant; classify by signature so a
            // provider rate-limit / auth failure / network outage is reported
            // as such instead of a useless "please retry".
            Self::Failed(msg) => crate::gateway::i18n::classify_error_text(msg),
            // Routing / lookup errors the user cannot act on — keep generic and
            // never echo the raw internal string.
            Self::RunNotFound(_)
            | Self::RunNotActive(_)
            | Self::Fallthrough { .. }
            | Self::Orchestrator(_) => ReceiptKind::Failed,
        }
    }
}

#[cfg(test)]
mod shared_room_lane_tests {
    use super::{BusyInputMode, AUTHOR_USER_KEY};
    use std::collections::HashMap;

    fn turn(scope: Option<&str>, author: Option<&str>) -> HashMap<String, String> {
        let mut m = HashMap::new();
        if let Some(s) = scope {
            m.insert(crate::scope::SCOPE_META_KEY.to_string(), s.to_string());
        }
        if let Some(a) = author {
            m.insert(AUTHOR_USER_KEY.to_string(), a.to_string());
        }
        m
    }

    /// The rule itself: another member's run is not yours to steer or kill.
    #[test]
    fn a_room_mates_run_forces_queue_whatever_the_knob_says() {
        let incoming = turn(Some("project:p-1"), Some("u-bob"));
        let running = turn(Some("project:p-1"), Some("u-alice"));
        for knob in [BusyInputMode::Steer, BusyInputMode::Interrupt] {
            assert_eq!(
                knob.for_shared_room(&incoming, &running),
                BusyInputMode::Queue,
                "{knob:?} must not reach across authors"
            );
        }
    }

    /// The separation the plan asked for explicitly: this rule downgrades on
    /// AUTHORSHIP. A person sending two messages in a row still steers — the
    /// coalescing the queue auto-drain depends on. If this ever fails, the bug
    /// is here and NOT the `busy_queue::mark_admitted` bug that presents with
    /// the identical symptom.
    #[test]
    fn the_same_person_speaking_twice_in_a_room_still_steers() {
        let alice = turn(Some("project:p-1"), Some("u-alice"));
        assert_eq!(
            BusyInputMode::Steer.for_shared_room(&alice, &alice),
            BusyInputMode::Steer
        );
        assert_eq!(
            BusyInputMode::Interrupt.for_shared_room(&alice, &alice),
            BusyInputMode::Interrupt
        );
    }

    /// A personal session has one human by construction, so the rule may never
    /// fire there — not even when the stamps somehow disagree.
    #[test]
    fn a_personal_session_is_untouched() {
        let incoming = turn(Some("personal:u-alice"), Some("u-bob"));
        let running = turn(Some("personal:u-alice"), Some("u-alice"));
        assert_eq!(
            BusyInputMode::Steer.for_shared_room(&incoming, &running),
            BusyInputMode::Steer
        );
        // An unstamped (pre-P1) pair likewise.
        let bare = turn(None, None);
        assert_eq!(
            BusyInputMode::Interrupt.for_shared_room(&bare, &bare),
            BusyInputMode::Interrupt
        );
    }

    /// One side unstamped reads as "not the same person" — queue, do not guess.
    #[test]
    fn an_unknown_author_in_a_room_queues_rather_than_assuming_a_match() {
        let anonymous = turn(Some("project:p-1"), None);
        let alice = turn(Some("project:p-1"), Some("u-alice"));
        assert_eq!(
            BusyInputMode::Steer.for_shared_room(&anonymous, &alice),
            BusyInputMode::Queue
        );
        assert_eq!(
            BusyInputMode::Steer.for_shared_room(&alice, &anonymous),
            BusyInputMode::Queue
        );
    }
}

#[cfg(test)]
mod user_receipt_tests {
    use super::{ExecutionError, Locale};

    #[test]
    fn rate_limit_failure_reads_as_retryable() {
        let e = ExecutionError::Failed(
            "flow: internal dispatch error: harness: llm error: Rate limit error: \
             Anthropic API rate limited (429): receiving too many requests"
                .to_string(),
        );
        let (code, message) = e.user_receipt(Locale::Zh);
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
        let (code, message) = e.user_receipt(Locale::Zh);
        assert_eq!(code, "PROVIDERS_UNREACHABLE");
        assert!(message.contains("网络"));
    }

    #[test]
    fn rate_limit_takes_precedence_over_network() {
        // A 429 that also mentions a url should still classify as rate-limited.
        let e = ExecutionError::Failed(
            "429 rate limit on error sending request for url (x)".to_string(),
        );
        assert_eq!(e.user_receipt(Locale::Zh).0, "RATE_LIMITED");
    }

    #[test]
    fn unclassified_failed_stays_generic() {
        let e = ExecutionError::Failed("some opaque internal failure".to_string());
        assert_eq!(e.user_receipt(Locale::Zh).0, "FAILED");
    }

    #[test]
    fn timeout_and_cancel_keep_their_codes() {
        assert_eq!(
            ExecutionError::Timeout.user_receipt(Locale::Zh).0,
            "TIMEOUT"
        );
        assert_eq!(
            ExecutionError::Cancelled.user_receipt(Locale::Zh).0,
            "CANCELLED"
        );
    }

    #[test]
    fn agent_busy_and_routing_errors_never_leak_raw() {
        // AgentBusy is the admit_run early-return path surfaced by the bin-crate
        // RPC handlers; routing errors carry internal ids. Neither may echo raw.
        let busy = ExecutionError::AgentBusy("run 7f3a already active".to_string());
        let (code, message) = busy.user_receipt(Locale::Zh);
        assert_eq!(code, "AGENT_BUSY");
        assert!(!message.contains("7f3a"));

        let routing = ExecutionError::RunNotFound("internal-run-id-42".to_string());
        let (code, message) = routing.user_receipt(Locale::Zh);
        assert_eq!(code, "FAILED");
        assert!(!message.contains("internal-run-id-42"));
    }

    /// An expired API key used to be reported as a bare "please retry" — the
    /// engine's own classifier had no auth bucket at all, while the i18n one
    /// did. Unifying them gave this path actionable advice.
    #[test]
    fn auth_failure_says_check_the_key_not_just_retry() {
        let e = ExecutionError::Failed(
            "flow: llm error: 401 Unauthorized: invalid api key".to_string(),
        );
        let (code, message) = e.user_receipt(Locale::Zh);
        assert_eq!(code, "AUTH");
        assert!(message.contains("API Key"), "{message}");
    }

    /// The receipt follows the deployment's locale. It used to be hardcoded
    /// Chinese, so an `[general] language = "en"` Panel showed Chinese errors.
    #[test]
    fn receipt_follows_the_locale() {
        let (_, zh) = ExecutionError::Timeout.user_receipt(Locale::Zh);
        let (_, en) = ExecutionError::Timeout.user_receipt(Locale::En);
        assert_ne!(zh, en);
        assert!(!en.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)));
    }
}
