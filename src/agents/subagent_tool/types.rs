//! Input argument types parsed out of the subagent tool JSON payload.

/// A2 — default cap on concurrently-running subagent spawns per top-level
/// agent run. Matches the deleted `Lane::Subagent` default.
pub(super) const DEFAULT_MAX_CONCURRENT_SUBAGENTS: usize = 4;

/// Default bounded window for the `wait` action (seconds). When it elapses with
/// the child still running, `wait` returns `still_running` and the parent model
/// may re-issue `wait` to keep blocking — a longer wait is expressed as
/// repeated bounded waits, never one unbounded block.
pub(super) const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 120;

/// Ceiling on a single `wait` window (seconds). Kept well under the subagent
/// tool's 1_800_000ms wall-clock budget so one `wait` can never hang the turn;
/// the model chunks a longer wait into repeated bounded calls.
pub(super) const MAX_WAIT_TIMEOUT_SECS: u64 = 600;

/// Default wall-clock ceiling for one `run` (seconds) when the caller omits
/// `timeout_secs`. Must stay in sync with the schema's advertised default.
pub(super) const DEFAULT_RUN_TIMEOUT_SECS: u64 = 120;

/// Round-8 — how many finished sub-agents the `list` action renders, newest
/// first. The full result text is one `check_status` away — `list` is a
/// directory, not a payload. Lives in `types.rs` next to the timeout
/// constants so a single module owns the sub-agent tool's shape
/// (entropic-reduction; the old inline const in `loop_tool.rs` was the
/// only Knob in the file not co-located with its peers).
pub(super) const MAX_LISTED_COMPLETED: usize = 20;

/// Round-8 — characters of a finished sub-agent's result kept on a `list`
/// directory row AND on `SubagentNode.result_preview` (Panel tree view).
/// Single source so the tree row and the `list` row read as the same
/// preview width.
pub(super) const LIST_RESULT_PREVIEW_CHARS: usize = 200;

/// Gap between a child's own wall-clock timeout and the `subagent` tool's
/// advertised budget, so the CHILD's `tokio::time::timeout` is what fires. The
/// model then reads "Sub-agent timed out after Ns" (actionable: it knows which
/// child, and can retry with a smaller task) instead of an opaque tool-budget
/// overrun on the whole call. Same clock-ordering invariant the `ask_user` row
/// in `tools::budget` documents, applied in the other direction.
const RUN_TIMEOUT_HEADROOM_SECS: u64 = 300;

/// Ceiling used when `subagent` is somehow absent from the builtin budget table.
const FALLBACK_MAX_RUN_TIMEOUT_SECS: u64 = 1_500;

/// Ceiling on one `run` (and on each `batch_tasks` entry), derived from the
/// tool's own budget so the two never drift apart.
///
/// A model-supplied `timeout_secs` is clamped into `[1, this]`: `0` would make
/// `tokio::time::timeout` elapse immediately ("timed out after 0s" before the
/// child ever thinks), and anything above this lets the harness budget fire
/// first, discarding a child that was still working.
pub(super) fn max_run_timeout_secs() -> u64 {
    crate::tools::budget::builtin_tool_budget_ms("subagent")
        .map(|ms| (ms / 1000).saturating_sub(RUN_TIMEOUT_HEADROOM_SECS))
        .filter(|secs| *secs > 0)
        .unwrap_or(FALLBACK_MAX_RUN_TIMEOUT_SECS)
}

/// Every top-level argument key the tool accepts.
///
/// This is the parser's half of the tool contract; the schema in `loop_tool.rs`
/// is the model-facing half, and `subagent_schema_properties_match_accepted_keys`
/// (tests) pins the two together — the schema is hand-written, so nothing else
/// stops them from drifting.
///
/// `name` is listed even though the schema does NOT advertise it: it has a
/// dedicated "sub-agents are not addressable teammates" rejection further down
/// the parser, and that specific message is far more useful than a generic
/// unknown-key error.
pub(super) const ACCEPTED_ARG_KEYS: &[&str] = &[
    "action",
    "agent_type",
    "aggregator_model",
    "batch_tasks",
    "context_summary",
    "model",
    "name",
    "proposer_models",
    "request_id",
    "request_ids",
    "run_in_background",
    "synthesis_instruction",
    "synthesize",
    "task",
    "team_name",
    "text",
    "timeout_secs",
    "to",
];

/// Parsed arguments for the subagent tool.
#[derive(Debug)]
pub(super) enum SubagentAction {
    /// Run a new sub-agent task.
    Run(RunArgs),
    /// Check status of a background sub-agent.
    CheckStatus(String),
    /// Park until a background sub-agent finishes or the bounded window elapses
    /// (event-driven, no per-check LLM turn). One id → wait for that agent;
    /// many ids → wait for whichever finishes first (fan-out first-completion).
    /// Mirrors `check_status`'s output shape on completion; returns
    /// `still_running` on timeout.
    Wait {
        request_ids: Vec<String>,
        timeout_secs: u64,
    },
    /// Cancel a still-running background sub-agent by `request_id`. Hits the
    /// shared `CancellationToken`; the running task observes it on its next
    /// await point and unwinds cleanly. Idempotent — cancelling an unknown
    /// or already-completed request returns an error rather than panicking.
    Cancel(String),
    /// Send a message to a named teammate.
    SendMessage {
        to: String,
        text: String,
        team_name: String,
    },
    /// Read inbox messages.
    ReadInbox { team_name: String },
    /// Enumerate every background sub-agent (running + recently-completed)
    /// so the parent can recover `request_ids` it no longer holds.
    List,
}

/// A single task within a batch execution.
#[derive(Debug, Clone)]
pub(super) struct BatchTask {
    pub(super) task: String,
    pub(super) agent_type: Option<String>,
    pub(super) model: Option<String>,
    pub(super) timeout_secs: Option<u64>,
}

#[derive(Debug)]
pub(super) struct RunArgs {
    pub(super) task: String,
    pub(super) agent_type: Option<String>,
    pub(super) model: Option<String>,
    pub(super) timeout_secs: u64,
    pub(super) run_in_background: bool,
    pub(super) context_summary: Option<String>,
    /// Batch tasks for parallel execution. When provided, all tasks run in
    /// background automatically and a list of `request_ids` is returned.
    pub(super) batch_tasks: Option<Vec<BatchTask>>,
    /// Mixture-of-Agents convenience: replicate the top-level `task` across
    /// these models as parallel proposers. `["claude-opus-4-8", "gpt-5"]`
    /// expands to one batch entry per model running the same prompt — the
    /// classic MoA shape. Ignored when explicit `batch_tasks` is supplied.
    pub(super) proposer_models: Option<Vec<String>>,
    /// When true, after a synchronous batch fans out and every proposer
    /// returns, run ONE aggregator sub-agent that synthesizes the proposals
    /// into a single answer (Mixture-of-Agents reduce step). Requires a
    /// foreground batch (`run_in_background = false`).
    pub(super) synthesize: bool,
    /// Model for the aggregator run. Falls back to the top-level `model`,
    /// then the agent's default. Use a strong model here for best synthesis.
    pub(super) aggregator_model: Option<String>,
    /// Optional extra guidance handed to the aggregator on top of the default
    /// "merge the strongest parts, resolve conflicts" instruction.
    pub(super) synthesis_instruction: Option<String>,
}
