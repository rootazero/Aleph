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
mod btw_promote;
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
mod settle;
mod simple;
mod slash_command;
mod steering;
mod tool_refresh;
pub(crate) mod tool_service_builder;
pub(crate) mod topic;
mod trace_sink_adapter;
mod turn_memory;
mod turn_mode;
mod turn_model;
mod turn_permissions;
mod turn_thinking;
mod unattended_redacting_sink;

#[cfg(test)]
mod btw_wire_tests;
#[cfg(test)]
mod tests;

#[allow(unused_imports)] // wired into run_loop.rs in this commit
pub(crate) use agent_trace_emit_sink::AgentTraceEmitSink;
pub use concurrency::{AgentSlotUsage, ConcurrencySnapshot};
pub use engine::{ContinuationDeps, ExecutionEngine};
#[allow(unused_imports)] // wired into run_loop.rs in this commit
pub(crate) use scratchpad_progress_sink::ScratchpadProgressSink;
pub use simple::SimpleExecutionEngine;
pub(crate) use slash_command::{is_continuation_driven_slash, is_shorthand_alias, stamp_btw};
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

/// Metadata key carrying the RAW, un-normalized id of the channel sender who
/// woke this run tree — the approval-originator gate's identity.
///
/// Distinct from both siblings it sits next to in the same metadata map:
/// unlike `sender_id` (normalized for session/routing lookups), this one must
/// stay exactly as the channel delivered it, because the channel
/// button-approval callback compares a clicker's raw id against it byte for
/// byte; and unlike [`AUTHOR_USER_KEY`] (this TURN's speaker, re-derived on
/// every message in a room), this is the id that opened the run tree and does
/// not change as the tree spawns children.
///
/// Two producers, writing ids from DIFFERENT namespaces: `teams::broadcast`
/// stamps an Aleph `u-*` id (from `scope::current_room_author()`), while
/// `inbound_router::executor` stamps the raw platform sender id straight off
/// the channel message — see the doc comment on
/// `gateway::handlers::exec_approvals::originator_narrows_within_room` for how
/// the approval bridge reconciles the two. One reader: `run_loop` seeds the
/// `TURN_ORIGINATOR` task-local from this key. A missing value degrades the
/// approval-originator gate to the prior any-paired-user rule — a fail-open
/// degradation indistinguishable at runtime from a run that legitimately has
/// no originator (e.g. cron/heartbeat).
pub const ORIGINATOR_USER_KEY: &str = "originator_user_id";

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
    ///
    /// `in_a_room` is a parameter rather than something this reads out of
    /// `incoming` — it used to parse `SCOPE_META_KEY` itself, which is the
    /// producer's RAW stamp. The six producers that need
    /// `run_loop::request_scope`'s correction (channel inbound router, cron,
    /// heartbeat, teams dispatcher, `session_send`, A2A) stamp
    /// `personal:<speaker>` on a session key a room has already claimed, so on
    /// exactly those paths this rule read "not a room" and let one member's
    /// message steer or cancel another member's in-flight run. The room question
    /// now has one answer in the process (`run_loop::request_is_in_a_room`) and
    /// this method keeps only its real subject: whether the two turns have the
    /// same author.
    #[must_use]
    pub fn for_shared_room(
        self,
        in_a_room: bool,
        incoming: &HashMap<String, String>,
        running: &HashMap<String, String>,
    ) -> Self {
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

/// `metadata` key: an execution-tier **ceiling** for this run.
///
/// Written only by [`crate::gateway::resume_coordinator`], carrying the tier
/// the crashed run was executing under. It is deliberately NOT
/// [`crate::config::types::policies::EXEC_TIER_SESSION_KEY`], for two reasons
/// that are the same reason twice:
///
/// 1. that key is the *request* rung, which outranks the session and the
///    global value — so replaying a `full` snapshot through it would let a
///    crash recovery RAISE a conversation the operator has since tightened.
///    This one composes through `ExecTier::most_restrictive` **after** the
///    three rungs resolve, so it can only tighten, whatever they said; and
/// 2. the request rung stamps itself onto the session
///    (`resolve_turn_permissions`), and a resume must not rewrite the knobs
///    the user changed *after* the crash to tame the run.
pub const RESUME_TIER_CEILING_KEY: &str = "resume_tier_ceiling";

impl RunRequest {
    /// True when this request re-drives an existing session log rather than
    /// seeding a new user message: the boot/on-demand resume
    /// ([`crate::gateway::resume_coordinator`]) and the post-run steering
    /// rescue (`steering::build_steering_rescue_request`) both set it.
    ///
    /// The one reader of `metadata["resume"]`. It had three hand-written
    /// comparisons against that literal, and the fourth thing that needed to
    /// ask — "may this turn stamp its knobs onto the session?" — is exactly
    /// the kind of question that gets a fourth copy.
    #[must_use]
    pub fn is_resume(&self) -> bool {
        self.metadata.get("resume").map(String::as_str) == Some("true")
    }
}

/// The knob value a turn should stamp onto its session, if any.
///
/// The one derivation behind four faces (`turn_permissions`, `turn_mode`,
/// `turn_thinking`, `turn_memory`). It was four inline copies of
/// `requested.filter(|v| stored != Some(*v))`, and the fifth thing they all
/// had to learn — that a **resume** carries an envelope rather than a user's
/// choice — is exactly the kind of rule that gets learned by three of four.
///
/// A resume replays the crashed run's settings. Stamping them would overwrite
/// whatever the user changed *after* the crash, which is most likely the
/// change they made to tame the run that is now coming back (④-D8).
#[must_use]
pub(super) fn knob_to_stamp<T: PartialEq + Copy>(
    requested: Option<T>,
    stored: Option<T>,
    is_resume: bool,
) -> Option<T> {
    requested.filter(|v| stored != Some(*v) && !is_resume)
}

#[cfg(test)]
mod knob_stamp_tests {
    use super::knob_to_stamp;

    #[test]
    fn a_fresh_request_carrying_a_new_value_is_stamped() {
        assert_eq!(knob_to_stamp(Some(2), Some(1), false), Some(2));
        assert_eq!(knob_to_stamp(Some(2), None, false), Some(2));
    }

    #[test]
    fn a_value_the_session_already_holds_is_not_rewritten() {
        assert_eq!(knob_to_stamp(Some(1), Some(1), false), None);
    }

    /// ④ The one this exists for: a resume replays an envelope, so it must
    /// leave the session row exactly as the user left it after the crash.
    #[test]
    fn a_resume_stamps_nothing_however_far_the_snapshot_differs() {
        assert_eq!(knob_to_stamp(Some(2), Some(1), true), None);
        assert_eq!(knob_to_stamp(Some(2), None, true), None);
    }
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

    /// The run's principal ([`crate::spend::principal_from_metadata`]) is
    /// over its spend ceiling for the period — the admission arm's denial,
    /// raised by `run_loop::deny_if_over_spend` before either engine claims
    /// any run resource. See [`crate::spend`] for the floor arm this is the
    /// run-admission sibling of.
    ///
    /// `reset_ms` is the period boundary [`crate::spend::check`] itself
    /// computed for this denial — carried on `Verdict::Denied`'s own
    /// `Spent::period_end_ms` — captured here at denial time rather than
    /// recomputed later at `receipt_kind()` time. Recomputing would not be
    /// "the same answer, asked again a few instructions later": (1) if
    /// rendering happens to cross a period boundary, or the run was parked
    /// and retried later, a fresh computation names the *next* window's
    /// end, not the one the caller actually hit; (2) `spend::current_policy`
    /// is hot-swappable (`spend::update_policy`, and this task's own plan
    /// makes `[policies.spend]` apply live), so "moments later, under the
    /// same policy" is not a premise a fresh read can rely on. Carrying the
    /// value sidesteps both.
    #[error("spend ceiling reached")]
    SpendExhausted {
        limit: crate::spend::Limit,
        reset_ms: i64,
    },
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
            // `reset_ms` is carried on the error (see `Self::SpendExhausted`'s
            // doc) rather than recomputed here — this arm must not read
            // `Utc::now()` or `spend::current_policy()`.
            Self::SpendExhausted { limit, reset_ms } => ReceiptKind::SpendExhausted {
                limit: *limit,
                reset_ms: *reset_ms,
            },
        }
    }
}

/// The one untyped hop in an otherwise typed attribution chain: past this
/// const everything is compiler-checked (`turn_context::with_originator` →
/// `current_originator()` → `ExecApprovalRecord.originator_user_id`), but the
/// `HashMap` key itself is a bare string with nothing stopping a producer or
/// the reader from re-spelling it. Three layers, none subsuming the others —
/// the same division `run_loop::flow_scope_census` uses for the scope keys,
/// scaled down to this key's single reader and two producers:
///
/// 1. A round trip through the ONE producer cheap to call directly
///    (`teams::broadcast::member_run_metadata`, already exercised by
///    `broadcast::tests::member_run_metadata_carries_originator_for_approval_gate`
///    and its siblings, which read the value back via this const rather than
///    the literal).
/// 2. A source-level census below, over the other producer
///    (`inbound_router::executor`) and the reader (`run_loop`), both too
///    heavy to drive end to end from a unit test (they need a wired
///    `agent_registry` / `execution_adapter`, or a full `Agent` + provider):
///    the const's IDENTIFIER must appear in each file's production code
///    ([`crate::utils::source_scan::code_text`]), and the bare literal must
///    NOT appear as a quoted payload
///    ([`crate::utils::source_scan::code_keeping_literals`]) — a test that
///    re-typed the literal to check this would be the same defect moved one
///    layer out.
/// 3. A value pin: nothing but this test ties `ORIGINATOR_USER_KEY`'s VALUE
///    to `"originator_user_id"` — layer 2 only proves every site uses the
///    IDENTIFIER, which stays true even if the value drifts, since every
///    site would drift together. Renaming the const's value with no other
///    change turns this assertion red.
#[cfg(test)]
mod originator_key_tests {
    use super::ORIGINATOR_USER_KEY;
    use crate::utils::source_scan::{code_keeping_literals, code_text};

    #[test]
    fn the_value_is_pinned() {
        assert_eq!(ORIGINATOR_USER_KEY, "originator_user_id");
    }

    #[test]
    fn the_reader_and_the_inbound_router_producer_spell_the_key_by_const() {
        let reader = include_str!("run_loop/mod.rs");
        assert!(
            code_text(reader).contains("ORIGINATOR_USER_KEY"),
            "run_loop/mod.rs must read the key via ORIGINATOR_USER_KEY, not a bare literal"
        );
        assert!(
            !code_keeping_literals(reader).contains("\"originator_user_id\""),
            "run_loop/mod.rs must not re-spell the key as a bare string literal"
        );

        let producer = include_str!("../inbound_router/executor.rs");
        assert!(
            code_text(producer).contains("ORIGINATOR_USER_KEY"),
            "inbound_router::executor must stamp the key via ORIGINATOR_USER_KEY, not a bare literal"
        );
        assert!(
            !code_keeping_literals(producer).contains("\"originator_user_id\""),
            "inbound_router::executor must not re-spell the key as a bare string literal"
        );
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

    /// A conversation-shaped key, the shape the channel inbound router mints.
    fn key(peer: &str) -> crate::routing::session_key::SessionKey {
        crate::routing::session_key::SessionKey::group(
            "main",
            "telegram",
            crate::routing::session_key::PeerKind::Group,
            peer,
        )
    }

    /// The rule itself: another member's run is not yours to steer or kill.
    #[test]
    fn a_room_mates_run_forces_queue_whatever_the_knob_says() {
        let incoming = turn(Some("project:p-1"), Some("u-bob"));
        let running = turn(Some("project:p-1"), Some("u-alice"));
        for knob in [BusyInputMode::Steer, BusyInputMode::Interrupt] {
            assert_eq!(
                knob.for_shared_room(true, &incoming, &running),
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
            BusyInputMode::Steer.for_shared_room(true, &alice, &alice),
            BusyInputMode::Steer
        );
        assert_eq!(
            BusyInputMode::Interrupt.for_shared_room(true, &alice, &alice),
            BusyInputMode::Interrupt
        );
    }

    /// The door the stamped half cannot see: a channel conversation BOUND to a
    /// room. The producer stamps `personal:<speaker>` — the room correction
    /// happens later, in `run_loop::request_scope`, and this runs on the
    /// admission path — so before the claim was consulted this guard
    /// early-returned and a room-mate's message folded into the running turn.
    ///
    /// Bob is deliberately NOT added to the roster. `request_scope`'s arm-2
    /// gate refuses to give an off-roster speaker the room's DATA scope; this
    /// is a different question, and an off-roster speaker steering a member's
    /// turn is worse rather than better. If someone later reuses that gate
    /// here, this test is what goes red.
    ///
    /// # Why nothing here is torn down
    ///
    /// The project and the binding stay in `ProjectStore::shared()` for the
    /// rest of the test binary, and that is a decision rather than an
    /// oversight. Both identifiers are unique by construction, and nothing
    /// reads that store in a way this leftover can move: no test in the crate
    /// enumerates it (`projects::store`'s counting tests all build a
    /// `fresh_store()`, and there are zero `ProjectStore::shared()` calls in
    /// that module), and the only two production `list()` consumers
    /// (`extension`) `filter_map` on `workspace_path`, which `create(…, None)`
    /// leaves unset. A teardown would be a SECOND write to a global that
    /// sibling cases read without holding this test's guard — the failure mode
    /// 「一个进程全局的表被第二个实例写」 names, and this branch already
    /// carries one flaky test. Leaving it is the smaller risk, and what makes
    /// that true is the distinctness of these two names: keep them distinct.
    #[test]
    fn a_room_mate_in_a_bound_channel_conversation_still_cannot_steer() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = crate::projects::ProjectStore::shared();
        let room = store
            .create("busy-lane-bound-room", Some("u-alice"), None)
            .unwrap();
        store
            .bind_conversation(
                &room.id,
                "telegram",
                aleph_protocol::projects::BindingPeerKind::Group,
                "C-busy-lane",
                Some("u-alice"),
                None,
            )
            .unwrap();

        // Exactly what the channel inbound router stamps: the SPEAKER.
        let incoming = turn(Some("personal:u-bob"), Some("u-bob"));
        let running = turn(Some("personal:u-alice"), Some("u-alice"));

        // Premise: on the stamped half alone this pair is invisible — both
        // turns read `personal:`. If this ever queues, the assertion below
        // stops proving that the room CLAIM is what saw through it.
        let unbound = super::tests::gate_test_request(&key("C-unbound"), "run-unbound");
        assert!(
            !super::run_loop::request_is_in_a_room(&unbound),
            "premise: `C-unbound` must be a key no room has claimed"
        );
        assert_eq!(
            BusyInputMode::Steer.for_shared_room(false, &incoming, &running),
            BusyInputMode::Steer,
        );

        // The claim half: the binding is what sees through the personal stamp,
        // and it does so WITHOUT the arm-2 roster gate — bob is deliberately
        // off-roster, and an off-roster speaker steering a member's turn is
        // worse rather than better. If someone routes this predicate through
        // `request_scope`, this assertion is what goes red.
        let mut bound = super::tests::gate_test_request(&key("C-busy-lane"), "run-bound");
        bound.metadata = incoming.clone();
        assert!(
            super::run_loop::request_is_in_a_room(&bound),
            "the room claim must see through the producer's `personal:` stamp"
        );

        for knob in [BusyInputMode::Steer, BusyInputMode::Interrupt] {
            assert_eq!(
                knob.for_shared_room(true, &incoming, &running),
                BusyInputMode::Queue,
                "{knob:?} must not reach across authors in a room reached through \
                 a bound channel conversation either — the binding is what makes \
                 two humans share one transcript there"
            );
        }
    }

    /// A personal session has one human by construction, so the rule may never
    /// fire there — not even when the stamps somehow disagree.
    #[test]
    fn a_personal_session_is_untouched() {
        let incoming = turn(Some("personal:u-alice"), Some("u-bob"));
        let running = turn(Some("personal:u-alice"), Some("u-alice"));
        assert_eq!(
            BusyInputMode::Steer.for_shared_room(false, &incoming, &running),
            BusyInputMode::Steer
        );
        // An unstamped (pre-P1) pair likewise.
        let bare = turn(None, None);
        assert_eq!(
            BusyInputMode::Interrupt.for_shared_room(false, &bare, &bare),
            BusyInputMode::Interrupt
        );
    }

    /// One side unstamped reads as "not the same person" — queue, do not guess.
    #[test]
    fn an_unknown_author_in_a_room_queues_rather_than_assuming_a_match() {
        let anonymous = turn(Some("project:p-1"), None);
        let alice = turn(Some("project:p-1"), Some("u-alice"));
        assert_eq!(
            BusyInputMode::Steer.for_shared_room(true, &anonymous, &alice),
            BusyInputMode::Queue
        );
        assert_eq!(
            BusyInputMode::Steer.for_shared_room(true, &alice, &anonymous),
            BusyInputMode::Queue
        );
    }

    /// The room verdict comes from the caller, never from the metadata.
    ///
    /// This rule used to parse `SCOPE_META_KEY` out of `incoming` itself. That
    /// is the producer's RAW stamp, and the six producers that go through
    /// `run_loop::request_scope`'s correction (channel inbound router, cron,
    /// heartbeat, teams dispatcher, `session_send`, A2A) write
    /// `personal:<speaker>` onto a session key a room has already claimed — so
    /// on exactly those paths the rule read "not a room" and did nothing, while
    /// two members' turns steered and cancelled each other. Both directions are
    /// pinned: a stamp that says personal must not disarm the rule, and a stamp
    /// that says project must not arm it on its own.
    #[test]
    fn the_room_verdict_comes_from_the_caller_not_from_the_stamp() {
        let bob = turn(Some("personal:u-bob"), Some("u-bob"));
        let alice = turn(Some("personal:u-bob"), Some("u-alice"));
        assert_eq!(
            BusyInputMode::Steer.for_shared_room(true, &bob, &alice),
            BusyInputMode::Queue,
            "a room turn carrying an uncorrected personal stamp must still queue"
        );

        let bob_p = turn(Some("project:p-1"), Some("u-bob"));
        let alice_p = turn(Some("project:p-1"), Some("u-alice"));
        assert_eq!(
            BusyInputMode::Steer.for_shared_room(false, &bob_p, &alice_p),
            BusyInputMode::Steer,
            "a project stamp alone must not arm the rule — the corrector decides"
        );
    }
}

#[cfg(test)]
mod user_receipt_tests {
    use super::{ExecutionError, Locale, ReceiptKind};
    use crate::tasks::shared::retry_hint::classify;

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

    /// A denied run's receipt carries the wire code and, by shape, the
    /// caller's own numbers.
    #[test]
    fn spend_exhausted_carries_the_wire_code_and_the_callers_own_numbers() {
        let e = ExecutionError::SpendExhausted {
            limit: crate::spend::Limit::PerUser {
                spent: 42.0,
                limit: 40.0,
            },
            reset_ms: 1_000,
        };
        let (code, message) = e.user_receipt(Locale::En);
        assert_eq!(code, "SPEND_EXHAUSTED");
        assert!(
            message.contains("42.0") && message.contains("40.0"),
            "{message}"
        );
    }

    /// `Limit::Total` never renders a dollar figure — there is no actor at
    /// render time to ask whether this caller may see the machine total.
    #[test]
    fn spend_exhausted_total_never_leaks_a_number() {
        let e = ExecutionError::SpendExhausted {
            limit: crate::spend::Limit::Total,
            reset_ms: 1_000,
        };
        let (code, message) = e.user_receipt(Locale::En);
        assert_eq!(code, "SPEND_EXHAUSTED");
        assert!(!message.contains('$'), "{message}");
    }

    /// G11 — a spend denial is terminal, never one of the two provider-hiccup
    /// buckets a caller of `receipt_kind()` might park-and-retry.
    #[test]
    fn spend_exhausted_receipt_kind_is_not_transient() {
        let e = ExecutionError::SpendExhausted {
            limit: crate::spend::Limit::Total,
            reset_ms: 1_000,
        };
        assert!(!e.receipt_kind().is_transient());
    }

    /// The reset instant is *carried* on the error, not recomputed at
    /// `receipt_kind()` time — see `Self::SpendExhausted`'s doc for the two
    /// ways recomputing could drift (a period boundary crossed between
    /// denial and render; a live policy reload, which this plan's own
    /// Task 10 makes possible). Pin `reset_ms` to an instant nowhere near
    /// "now" (2000-01-01T00:00:00Z) so a fresh `period_end_ms(Utc::now(),
    /// ..)` call could never coincidentally reproduce it: if `receipt_kind()`
    /// ever regressed to recomputing instead of reading the field, this
    /// assertion would fail rather than passing by accident.
    #[test]
    fn spend_exhausted_receipt_names_the_carried_reset_instant_not_a_recomputed_one() {
        let reset_ms: i64 = 946_684_800_000; // 2000-01-01T00:00:00Z
        let e = ExecutionError::SpendExhausted {
            limit: crate::spend::Limit::Total,
            reset_ms,
        };
        let (_, message) = e.user_receipt(Locale::En);
        assert!(
            message.contains("2000-01-01"),
            "receipt must name the carried reset instant verbatim, got: {message}"
        );

        // Same property through `receipt_kind()` directly, without going
        // through rendering.
        match e.receipt_kind() {
            ReceiptKind::SpendExhausted { reset_ms: got, .. } => {
                assert_eq!(got, reset_ms);
            }
            other => panic!("expected ReceiptKind::SpendExhausted, got {other:?}"),
        }
    }

    /// **CRITICAL INVARIANT**: `ExecutionError::SpendExhausted` must be
    /// classified as permanent (not retryable) by **both**:
    ///
    /// 1. The typed path: `receipt_kind().is_transient()` in this file
    /// 2. The string path: `cron/executor.rs`'s `classify(&err.to_string())`
    ///    for fallback classification when a typed `ReceiptKind` is unavailable
    ///
    /// # Why this guard exists
    ///
    /// `SpendExhausted`'s Display is `#[error("spend ceiling reached")]`, a
    /// terse string chosen for brevity, not to dodge the regex patterns in
    /// `retry_hint::classify`. If that string is ever made more informative
    /// (e.g., "spend quota exhausted", "rate limit for this period"), the
    /// absence of this test would let it silently start matching one of the
    /// `classify` regexes, flipping the error to retryable. The cron executor
    /// would then retry-storm a permanent condition while suppressing alerts,
    /// and the user would be neither served nor told.
    ///
    /// This test asserts the invariant is enforced, not left to chance.
    #[test]
    fn cron_string_classifier_agrees_spend_denial_is_permanent() {
        // Test both variants: per-principal and fleet-wide.
        let per_user = ExecutionError::SpendExhausted {
            limit: crate::spend::Limit::PerUser {
                spent: 100.0,
                limit: 100.0,
            },
            reset_ms: 1_000,
        };
        let total = ExecutionError::SpendExhausted {
            limit: crate::spend::Limit::Total,
            reset_ms: 1_000,
        };

        // Both variants must agree with the typed path.
        for err in &[&per_user, &total] {
            let err_text = err.to_string();

            // String classifier must say "not retryable".
            let hint = classify(&err_text);
            assert!(
                !hint.retryable,
                "spend denial '{}' must not be retryable by string classifier, got: {:?}",
                err_text, hint
            );

            // Typed path must also say "not transient".
            let is_transient = err.receipt_kind().is_transient();
            assert!(
                !is_transient,
                "spend denial must not be transient by receipt_kind"
            );

            // Both mechanisms must agree.
            assert_eq!(
                hint.retryable, is_transient,
                "string classifier and typed path must agree: \
                 string says retryable={}, typed says transient={}",
                hint.retryable, is_transient
            );
        }
    }
}
